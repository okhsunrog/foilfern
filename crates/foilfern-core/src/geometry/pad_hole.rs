//! Port of libs/librepcb/core/geometry/padhole.{h,cpp}.

use super::{NonEmptyPath, impl_uuid, object_list, property};
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::{PositiveLength, Uuid};

/// A plated hole (circular drill or slot) of a pad.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct PadHole {
    uuid: Uuid,
    diameter: PositiveLength,
    path: NonEmptyPath,
}

impl PadHole {
    /// Creates a pad hole.
    pub fn new(uuid: Uuid, diameter: PositiveLength, path: NonEmptyPath) -> Self {
        Self {
            uuid,
            diameter,
            path,
        }
    }

    property!(
        /// Returns the diameter.
        copy diameter: PositiveLength, set_diameter
    );
    property!(
        /// Returns the path (one vertex for circular holes, more for slots).
        ref path: NonEmptyPath, set_path
    );

    /// Returns whether the hole is a slot (more than one vertex).
    pub fn is_slot(&self) -> bool {
        self.path.vertices().len() > 1
    }

    /// Returns whether the hole is a slot with more than one segment.
    pub fn is_multi_segment_slot(&self) -> bool {
        self.path.vertices().len() > 2
    }

    /// Returns whether the hole is a slot containing arcs.
    pub fn is_curved_slot(&self) -> bool {
        self.path.is_curved()
    }
}

impl_uuid!(PadHole);

object_list!(
    /// List of [`PadHole`]s.
    PadHoleList,
    PadHoleListTag,
    PadHole,
    "hole"
);

impl SerializeObject for PadHole {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("diameter", &self.diameter);
        self.path.serialize(root);
    }
}

impl DeserializeObject for PadHole {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            diameter: node.child_value("diameter/@0")?,
            path: NonEmptyPath::deserialize(node)?,
        })
    }
}
