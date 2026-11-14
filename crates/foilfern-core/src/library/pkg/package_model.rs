//! Port of libs/librepcb/core/library/pkg/packagemodel.{h,cpp}.

use crate::geometry::{impl_uuid, object_list, property};
use crate::serialization::{self, DeserializeObject, HasName, List, SExpression, SerializeObject};
use crate::types::{ElementName, Uuid};

/// A 3D model of a package, stored as `<uuid>.step` in the package
/// directory.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackageModel {
    uuid: Uuid,
    name: ElementName,
}

impl PackageModel {
    /// Creates a 3D model.
    pub fn new(uuid: Uuid, name: ElementName) -> Self {
        Self { uuid, name }
    }

    property!(
        /// Returns the name.
        ref name: ElementName, set_name
    );

    /// Returns the name of the STEP file in the package directory.
    pub fn file_name(&self) -> String {
        format!("{}.step", self.uuid)
    }
}

impl_uuid!(PackageModel);

impl HasName for PackageModel {
    fn name(&self) -> &str {
        &self.name
    }
}

object_list!(
    /// List of [`PackageModel`]s.
    PackageModelList,
    PackageModelListTag,
    PackageModel,
    "3d_model"
);

impl SerializeObject for PackageModel {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("name", &self.name);
    }
}

impl DeserializeObject for PackageModel {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            name: node.child_value("name/@0")?,
        })
    }
}
