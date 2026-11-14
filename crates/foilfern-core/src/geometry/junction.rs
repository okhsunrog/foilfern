//! Port of libs/librepcb/core/geometry/junction.{h,cpp}.

use super::{object_list, property};
use crate::serialization::{self, DeserializeObject, HasUuid, List, SExpression, SerializeObject};
use crate::types::{Point, Uuid};

/// A junction of net lines (schematics) or traces (boards).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Junction {
    uuid: Uuid,
    position: Point,
}

impl Junction {
    /// Creates a junction.
    pub fn new(uuid: Uuid, position: Point) -> Self {
        Self { uuid, position }
    }

    property!(
        /// Returns the UUID.
        copy uuid: Uuid, set_uuid
    );
    property!(
        /// Returns the position.
        copy position: Point, set_position
    );

    /// Returns a copy with another UUID.
    pub fn with_uuid(&self, uuid: Uuid) -> Self {
        Self { uuid, ..*self }
    }
}

impl HasUuid for Junction {
    fn uuid(&self) -> Uuid {
        self.uuid
    }
}

object_list!(
    /// List of [`Junction`]s.
    JunctionList,
    JunctionListTag,
    Junction,
    "junction"
);

impl SerializeObject for Junction {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        self.position.serialize(root.append_list("position"));
    }
}

impl DeserializeObject for Junction {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            position: Point::deserialize(node.required_child("position")?)?,
        })
    }
}
