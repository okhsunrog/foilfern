//! Port of libs/librepcb/core/library/dev/device.{h,cpp}.

use super::device_check::run_device_checks;
use super::device_pad_signal_map::DevicePadSignalMap;
use super::part::PartList;
use crate::attribute::AttributeList;
use crate::fileio::{FileSystem, TransactionalDirectory};
use crate::geometry::property;
use crate::library::{
    BaseMetadata, ElementMetadata, LibraryBaseElement, LibraryCheckMessage, Result,
    element_accessors, impl_library_element,
};
use crate::serialization::{DeserializeObject, List, SExpression, SerializeObject};
use crate::types::Uuid;

/// An instance of a component with a specific package ("real" component).
///
/// The UUID, the component UUID, the package UUID and the pad-signal-map
/// are the interface of a device and must never be changed.
#[derive(Debug)]
pub struct Device {
    directory: TransactionalDirectory,
    metadata: ElementMetadata,
    component_uuid: Uuid,
    package_uuid: Uuid,
    pad_signal_map: DevicePadSignalMap,
    attributes: AttributeList,
    parts: PartList,
}

static_assertions::assert_impl_all!(Device: Send, Sync);

impl Device {
    /// Creates a new device in a temporary directory.
    pub fn new(metadata: BaseMetadata, component_uuid: Uuid, package_uuid: Uuid) -> Result<Self> {
        Ok(Self {
            directory: TransactionalDirectory::new_temporary()?,
            metadata: ElementMetadata::new(metadata),
            component_uuid,
            package_uuid,
            pad_signal_map: DevicePadSignalMap::new(),
            attributes: AttributeList::new(),
            parts: PartList::new(),
        })
    }

    property!(
        /// Returns the UUID of the component.
        copy component_uuid: Uuid, set_component_uuid
    );
    property!(
        /// Returns the UUID of the package.
        copy package_uuid: Uuid, set_package_uuid
    );

    /// Returns the pad-signal-map.
    pub fn pad_signal_map(&self) -> &DevicePadSignalMap {
        &self.pad_signal_map
    }

    /// Returns the pad-signal-map for modification.
    pub fn pad_signal_map_mut(&mut self) -> &mut DevicePadSignalMap {
        &mut self.pad_signal_map
    }

    /// Returns the attributes.
    pub fn attributes(&self) -> &AttributeList {
        &self.attributes
    }

    /// Returns the attributes for modification.
    pub fn attributes_mut(&mut self) -> &mut AttributeList {
        &mut self.attributes
    }

    /// Returns the orderable parts.
    pub fn parts(&self) -> &PartList {
        &self.parts
    }

    /// Returns the orderable parts for modification.
    pub fn parts_mut(&mut self) -> &mut PartList {
        &mut self.parts
    }

    /// Makes this device a copy of `other` (but keeps the device UUID),
    /// removing all files of this device (upstream `duplicateFrom()`).
    pub fn duplicate_from(&mut self, other: &Device) -> Result<()> {
        self.directory.remove_dir_recursively("")?;
        self.metadata.duplicate_from(&other.metadata);
        self.component_uuid = other.component_uuid;
        self.package_uuid = other.package_uuid;
        self.pad_signal_map = other.pad_signal_map.clone();
        self.attributes = other.attributes.clone();
        self.parts = other.parts.clone();
        Ok(())
    }
}

impl LibraryBaseElement for Device {
    const SHORT_ELEMENT_NAME: &'static str = "dev";
    const LONG_ELEMENT_NAME: &'static str = "device";

    element_accessors!();

    fn load(directory: TransactionalDirectory, root: &SExpression) -> Result<Self> {
        Ok(Self {
            directory,
            metadata: ElementMetadata::deserialize(root)?,
            component_uuid: root.child_value("component/@0")?,
            package_uuid: root.child_value("package/@0")?,
            pad_signal_map: DevicePadSignalMap::deserialize(root)?,
            attributes: AttributeList::deserialize(root)?,
            parts: PartList::deserialize(root)?,
        })
    }

    fn run_checks(&self) -> Result<Vec<LibraryCheckMessage>> {
        let mut msgs = Vec::new();
        run_device_checks(self, &mut msgs);
        Ok(msgs)
    }
}

impl_library_element!(Device);

impl SerializeObject for Device {
    fn serialize(&self, root: &mut List) {
        self.metadata.serialize(root);
        root.ensure_line_break();
        root.append_child("component", &self.component_uuid);
        root.ensure_line_break();
        root.append_child("package", &self.package_uuid);
        root.ensure_line_break();
        self.pad_signal_map.sorted_by_uuid().serialize(root);
        root.ensure_line_break();
        self.attributes.serialize(root);
        root.ensure_line_break();
        self.parts.serialize(root);
        root.ensure_line_break();
        self.metadata.base().serialize_message_approvals(root);
        root.ensure_line_break();
    }
}
