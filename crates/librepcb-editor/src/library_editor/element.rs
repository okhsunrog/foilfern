//! The editable content of library elements: what the undo stack of a
//! [`LibraryElementEditor`](super::LibraryElementEditor) snapshots (no
//! direct upstream counterpart: upstream's undo commands modify the element
//! objects in place and keep the old values themselves).
//!
//! The content of an element is everything except its directory, its grid
//! interval and its message approvals: upstream changes the grid interval
//! and the approvals without going through the undo stack (they are "manual
//! modifications" which only make the element dirty), so undoing must not
//! revert them.

use std::collections::BTreeSet;

use librepcb_core::attribute::AttributeList;
use librepcb_core::geometry::{CircleList, ImageList, PolygonList, TextList};
use librepcb_core::library::cmp::{
    Component, ComponentSignalList, ComponentSymbolVariantList, NormDependentPrefixMap,
};
use librepcb_core::library::dev::{Device, DevicePadSignalMap, PartList};
use librepcb_core::library::pkg::{
    AlternativeName, AssemblyType, FootprintList, Package, PackageModelList, PackagePadList,
};
use librepcb_core::library::sym::{Symbol, SymbolPinList};
use librepcb_core::library::{ElementMetadata, LibraryElement};
use librepcb_core::types::{UnsignedLength, Uuid};

/// A library element which can be edited through a
/// [`LibraryElementEditor`](super::LibraryElementEditor): symbols,
/// packages, components and devices.
pub trait EditableElement: LibraryElement + std::fmt::Debug {
    /// Everything of the element which is edited through the undo stack.
    type Content: Clone + PartialEq + std::fmt::Debug + Send + Sync;

    /// Returns a snapshot of the content.
    fn content(&self) -> Self::Content;

    /// Replaces the content (keeps the directory, grid interval and message
    /// approvals).
    fn set_content(&mut self, content: Self::Content);

    /// Whether `current` breaks the interface of `original` (upstream
    /// `isInterfaceBroken()` of the element tabs): elements using the
    /// element (e.g. components using a symbol) would no longer work.
    fn is_interface_broken(original: &Self::Content, current: &Self::Content) -> bool;

    /// The auxiliary files of the element directory referenced by
    /// `content` (images of symbols, 3D models of packages). When saving,
    /// files which were referenced or written since the last save but are
    /// no longer referenced are removed (upstream: `CmdImageRemove`,
    /// `CmdPackageModelRemove` remove the files).
    fn referenced_files(_content: &Self::Content) -> BTreeSet<String> {
        BTreeSet::new()
    }

    /// Upstream translation context of the element's editor tab (e.g.
    /// `"SymbolTab"`), for texts shared by all element kinds.
    const TAB_CONTEXT: &'static str;
}

/// Replaces the element metadata but keeps the message approvals (they are
/// not part of the undo history).
fn set_metadata<E: LibraryElement>(element: &mut E, mut metadata: ElementMetadata) {
    let approvals = element.metadata().message_approvals().clone();
    metadata.base_mut().set_message_approvals(approvals);
    *element.element_metadata_mut() = metadata;
}

/// The content of a [`Symbol`].
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolContent {
    /// Metadata (the message approvals are ignored on restore).
    pub metadata: ElementMetadata,
    /// Pins.
    pub pins: SymbolPinList,
    /// Polygons.
    pub polygons: PolygonList,
    /// Circles.
    pub circles: CircleList,
    /// Texts.
    pub texts: TextList,
    /// Images.
    pub images: ImageList,
}

impl EditableElement for Symbol {
    type Content = SymbolContent;
    const TAB_CONTEXT: &'static str = "SymbolTab";

    fn content(&self) -> SymbolContent {
        SymbolContent {
            metadata: self.element_metadata().clone(),
            pins: self.pins().clone(),
            polygons: self.polygons().clone(),
            circles: self.circles().clone(),
            texts: self.texts().clone(),
            images: self.images().clone(),
        }
    }

    fn set_content(&mut self, c: SymbolContent) {
        set_metadata(self, c.metadata);
        *self.pins_mut() = c.pins;
        *self.polygons_mut() = c.polygons;
        *self.circles_mut() = c.circles;
        *self.texts_mut() = c.texts;
        *self.images_mut() = c.images;
    }

    fn referenced_files(content: &SymbolContent) -> BTreeSet<String> {
        content
            .images
            .iter()
            .map(|i| i.file_name().as_str().to_owned())
            .collect()
    }

    fn is_interface_broken(original: &SymbolContent, current: &SymbolContent) -> bool {
        // Upstream SymbolTab::refreshUiData().
        original.pins.uuid_set() != current.pins.uuid_set()
    }
}

/// The content of a [`Package`].
#[derive(Debug, Clone, PartialEq)]
pub struct PackageContent {
    /// Metadata (the message approvals are ignored on restore).
    pub metadata: ElementMetadata,
    /// Alternative names.
    pub alternative_names: Vec<AlternativeName>,
    /// Assembly type (as stored in the file).
    pub assembly_type: AssemblyType,
    /// Minimum copper clearance for the checks.
    pub min_copper_clearance: UnsignedLength,
    /// Package pads.
    pub pads: PackagePadList,
    /// 3D models.
    pub models: PackageModelList,
    /// Footprints.
    pub footprints: FootprintList,
}

impl EditableElement for Package {
    type Content = PackageContent;
    const TAB_CONTEXT: &'static str = "PackageTab";

    fn content(&self) -> PackageContent {
        PackageContent {
            metadata: self.element_metadata().clone(),
            alternative_names: self.alternative_names().clone(),
            assembly_type: self.assembly_type(),
            min_copper_clearance: self.min_copper_clearance(),
            pads: self.pads().clone(),
            models: self.models().clone(),
            footprints: self.footprints().clone(),
        }
    }

    fn set_content(&mut self, c: PackageContent) {
        set_metadata(self, c.metadata);
        self.set_alternative_names(c.alternative_names);
        self.set_assembly_type(c.assembly_type);
        self.set_min_copper_clearance(c.min_copper_clearance);
        *self.pads_mut() = c.pads;
        *self.models_mut() = c.models;
        *self.footprints_mut() = c.footprints;
    }

    fn referenced_files(content: &PackageContent) -> BTreeSet<String> {
        content.models.iter().map(|m| m.file_name()).collect()
    }

    fn is_interface_broken(original: &PackageContent, current: &PackageContent) -> bool {
        // Upstream PackageTab::refreshUiData().
        if original.pads.uuid_set() != current.pads.uuid_set() {
            return true;
        }
        original.footprints.iter().any(|orig| {
            current
                .footprints
                .by_uuid(&orig.uuid())
                .is_none_or(|fpt| fpt.pads().uuid_set() != orig.pads().uuid_set())
        })
    }
}

/// The content of a [`Component`].
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentContent {
    /// Metadata (the message approvals are ignored on restore).
    pub metadata: ElementMetadata,
    /// Whether the component is schematic-only.
    pub schematic_only: bool,
    /// Default value.
    pub default_value: String,
    /// Prefixes.
    pub prefixes: NormDependentPrefixMap,
    /// Attributes.
    pub attributes: AttributeList,
    /// Signals.
    pub signals: ComponentSignalList,
    /// Symbol variants.
    pub symbol_variants: ComponentSymbolVariantList,
}

impl EditableElement for Component {
    type Content = ComponentContent;
    const TAB_CONTEXT: &'static str = "ComponentTab";

    fn content(&self) -> ComponentContent {
        ComponentContent {
            metadata: self.element_metadata().clone(),
            schematic_only: self.schematic_only(),
            default_value: self.default_value().clone(),
            prefixes: self.prefixes().clone(),
            attributes: self.attributes().clone(),
            signals: self.signals().clone(),
            symbol_variants: self.symbol_variants().clone(),
        }
    }

    fn set_content(&mut self, c: ComponentContent) {
        set_metadata(self, c.metadata);
        self.set_schematic_only(c.schematic_only);
        self.set_default_value(c.default_value);
        self.set_prefixes(c.prefixes);
        *self.attributes_mut() = c.attributes;
        *self.signals_mut() = c.signals;
        *self.symbol_variants_mut() = c.symbol_variants;
    }

    fn is_interface_broken(original: &ComponentContent, current: &ComponentContent) -> bool {
        // Upstream ComponentTab::isInterfaceBroken().
        if original.schematic_only != current.schematic_only
            || original.signals.uuid_set() != current.signals.uuid_set()
        {
            return true;
        }
        original.symbol_variants.iter().any(|orig| {
            let Some(cur) = current.symbol_variants.by_uuid(&orig.uuid()) else {
                return true;
            };
            if cur.symbol_items().uuid_set() != orig.symbol_items().uuid_set() {
                return true;
            }
            orig.symbol_items().iter().any(|orig_item| {
                let Some(cur_item) = cur.symbol_items().by_uuid(&orig_item.uuid()) else {
                    return true;
                };
                if cur_item.symbol_uuid() != orig_item.symbol_uuid()
                    || cur_item.pin_signal_map().uuid_set() != orig_item.pin_signal_map().uuid_set()
                {
                    return true;
                }
                orig_item.pin_signal_map().iter().any(|orig_map| {
                    cur_item
                        .pin_signal_map()
                        .by_uuid(&orig_map.pin_uuid())
                        .is_none_or(|m| m.signal_uuid() != orig_map.signal_uuid())
                })
            })
        })
    }
}

/// The content of a [`Device`].
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceContent {
    /// Metadata (the message approvals are ignored on restore).
    pub metadata: ElementMetadata,
    /// The component.
    pub component_uuid: Uuid,
    /// The package.
    pub package_uuid: Uuid,
    /// Pad-signal-map.
    pub pad_signal_map: DevicePadSignalMap,
    /// Attributes.
    pub attributes: AttributeList,
    /// Parts.
    pub parts: PartList,
}

impl EditableElement for Device {
    type Content = DeviceContent;
    const TAB_CONTEXT: &'static str = "DeviceTab";

    fn content(&self) -> DeviceContent {
        DeviceContent {
            metadata: self.element_metadata().clone(),
            component_uuid: self.component_uuid(),
            package_uuid: self.package_uuid(),
            pad_signal_map: self.pad_signal_map().clone(),
            attributes: self.attributes().clone(),
            parts: self.parts().clone(),
        }
    }

    fn set_content(&mut self, c: DeviceContent) {
        set_metadata(self, c.metadata);
        self.set_component_uuid(c.component_uuid);
        self.set_package_uuid(c.package_uuid);
        *self.pad_signal_map_mut() = c.pad_signal_map;
        *self.attributes_mut() = c.attributes;
        *self.parts_mut() = c.parts;
    }

    fn is_interface_broken(_original: &DeviceContent, _current: &DeviceContent) -> bool {
        // Upstream: devices have no interface other elements depend on.
        false
    }
}
