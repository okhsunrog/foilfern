//! Port of libs/librepcb/core/library/pkg/packagepad.{h,cpp}.

use crate::geometry::{impl_uuid, object_list, property};
use crate::serialization::{self, DeserializeObject, HasName, List, SExpression, SerializeObject};
use crate::types::{CircuitIdentifier, Uuid};

/// One logical pad of a package (connected to footprint pads and component
/// signals).
///
/// The UUID is part of the package's interface and must never be changed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackagePad {
    uuid: Uuid,
    name: CircuitIdentifier,
}

impl PackagePad {
    /// Creates a package pad.
    pub fn new(uuid: Uuid, name: CircuitIdentifier) -> Self {
        Self { uuid, name }
    }

    property!(
        /// Returns the name.
        ref name: CircuitIdentifier, set_name
    );
}

impl_uuid!(PackagePad);

impl HasName for PackagePad {
    fn name(&self) -> &str {
        &self.name
    }
}

object_list!(
    /// List of [`PackagePad`]s.
    PackagePadList,
    PackagePadListTag,
    PackagePad,
    "pad"
);

impl SerializeObject for PackagePad {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("name", &self.name);
    }
}

impl DeserializeObject for PackagePad {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            name: node.child_value("name/@0")?,
        })
    }
}
