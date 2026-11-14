//! Port of libs/librepcb/core/geometry/netlabel.{h,cpp}.

use super::{object_list, property};
use crate::serialization::{self, DeserializeObject, HasUuid, List, SExpression, SerializeObject};
use crate::types::{Angle, Point, Uuid};

/// A net label in a schematic.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct NetLabel {
    uuid: Uuid,
    position: Point,
    rotation: Angle,
    mirrored: bool,
}

impl NetLabel {
    /// Creates a net label.
    pub fn new(uuid: Uuid, position: Point, rotation: Angle, mirrored: bool) -> Self {
        Self {
            uuid,
            position,
            rotation,
            mirrored,
        }
    }

    property!(
        /// Returns the UUID.
        copy uuid: Uuid, set_uuid
    );
    property!(
        /// Returns the position.
        copy position: Point, set_position
    );
    property!(
        /// Returns the rotation.
        copy rotation: Angle, set_rotation
    );
    property!(
        /// Returns whether the label is mirrored.
        copy mirrored: bool, set_mirrored
    );

    /// Returns a copy with another UUID.
    pub fn with_uuid(&self, uuid: Uuid) -> Self {
        Self { uuid, ..*self }
    }
}

impl HasUuid for NetLabel {
    fn uuid(&self) -> Uuid {
        self.uuid
    }
}

object_list!(
    /// List of [`NetLabel`]s.
    NetLabelList,
    NetLabelListTag,
    NetLabel,
    "label"
);

impl SerializeObject for NetLabel {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.ensure_line_break();
        self.position.serialize(root.append_list("position"));
        root.append_child("rotation", &self.rotation);
        root.append_child("mirror", &self.mirrored);
        root.ensure_line_break();
    }
}

impl DeserializeObject for NetLabel {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            position: Point::deserialize(node.required_child("position")?)?,
            rotation: node.child_value("rotation/@0")?,
            mirrored: node.child_value("mirror/@0")?,
        })
    }
}
