//! Port of libs/librepcb/core/geometry/hole.{h,cpp}.

use super::{NonEmptyPath, impl_uuid, object_list, property};
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::{Length, MaskConfig, PositiveLength, Uuid};

/// A non-plated hole (circular drill or slot) with optional stop mask
/// opening.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hole {
    uuid: Uuid,
    diameter: PositiveLength,
    path: NonEmptyPath,
    stop_mask_config: MaskConfig,
}

impl Hole {
    /// Creates a hole.
    pub fn new(
        uuid: Uuid,
        diameter: PositiveLength,
        path: NonEmptyPath,
        stop_mask_config: MaskConfig,
    ) -> Self {
        Self {
            uuid,
            diameter,
            path,
            stop_mask_config,
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
    property!(
        /// Returns the stop mask configuration.
        copy stop_mask_config: MaskConfig, set_stop_mask_config
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

    /// Returns the stop mask offset to use for previews (automatic offsets
    /// are shown as 0.1mm), or `None` if there is no stop mask opening.
    pub fn preview_stop_mask_offset(&self) -> Option<Length> {
        match self.stop_mask_config {
            MaskConfig::Off => None,
            MaskConfig::Manual(offset) => Some(offset),
            MaskConfig::Automatic => Some(Length::new(100_000)), // Only for preview.
        }
    }
}

impl_uuid!(Hole);

object_list!(
    /// List of [`Hole`]s.
    HoleList,
    HoleListTag,
    Hole,
    "hole"
);

impl SerializeObject for Hole {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("diameter", &self.diameter);
        root.ensure_line_break();
        root.append_child("stop_mask", &self.stop_mask_config);
        self.path.serialize(root);
    }
}

impl DeserializeObject for Hole {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            diameter: node.child_value("diameter/@0")?,
            path: NonEmptyPath::deserialize(node)?,
            stop_mask_config: node.child_value("stop_mask/@0")?,
        })
    }
}
