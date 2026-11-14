//! Port of libs/librepcb/core/project/board/boardholedata.{h,cpp} (and
//! `items/bi_hole.{h,cpp}`, whose state is this data; the stop mask offset
//! is computed by [`BoardHoleData::stop_mask_offset()`]).

use super::BoardDesignRules;
use crate::geometry::{NonEmptyPath, property};
use crate::serialization::{self, DeserializeObject, HasUuid, List, SExpression, SerializeObject};
use crate::types::{Length, MaskConfig, PositiveLength, Uuid};

/// A non-plated hole of a board.
///
/// Serde: an object with the fields below.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BoardHoleData {
    uuid: Uuid,
    diameter: PositiveLength,
    path: NonEmptyPath,
    stop_mask_config: MaskConfig,
    locked: bool,
}

impl BoardHoleData {
    /// Creates a hole.
    pub fn new(
        uuid: Uuid,
        diameter: PositiveLength,
        path: NonEmptyPath,
        stop_mask_config: MaskConfig,
        locked: bool,
    ) -> Self {
        Self {
            uuid,
            diameter,
            path,
            stop_mask_config,
            locked,
        }
    }

    /// Returns the UUID.
    pub fn uuid(&self) -> Uuid {
        self.uuid
    }

    /// Returns a copy with another UUID.
    pub fn with_uuid(&self, uuid: Uuid) -> Self {
        Self {
            uuid,
            ..self.clone()
        }
    }

    property!(
        /// Returns the diameter.
        copy diameter: PositiveLength, set_diameter
    );
    property!(
        /// Returns the path (one vertex for a round hole, more for a slot).
        ref path: NonEmptyPath, set_path
    );
    property!(
        /// Returns the stop mask configuration.
        copy stop_mask_config: MaskConfig, set_stop_mask_config
    );
    property!(
        /// Returns whether the hole is locked.
        copy locked: bool, set_locked
    );

    /// Returns the stop mask offset with the given design rules, `None` if
    /// there is no stop mask opening (upstream
    /// `BI_Hole::getStopMaskOffset()`).
    pub fn stop_mask_offset(&self, rules: &BoardDesignRules) -> Option<Length> {
        match self.stop_mask_config {
            MaskConfig::Off => None,
            MaskConfig::Manual(offset) => Some(offset),
            MaskConfig::Automatic => Some(*rules.stop_mask_clearance().calc_value(*self.diameter)),
        }
    }
}

impl HasUuid for BoardHoleData {
    fn uuid(&self) -> Uuid {
        self.uuid
    }
}

impl SerializeObject for BoardHoleData {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("diameter", &self.diameter);
        root.ensure_line_break();
        root.append_child("stop_mask", &self.stop_mask_config);
        root.append_child("lock", &self.locked);
        self.path.serialize(root);
    }
}

impl DeserializeObject for BoardHoleData {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            diameter: node.child_value("diameter/@0")?,
            path: NonEmptyPath::deserialize(node)?,
            stop_mask_config: node.child_value("stop_mask/@0")?,
            locked: node.child_value("lock/@0")?,
        })
    }
}
