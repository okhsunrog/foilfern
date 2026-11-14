//! Port of libs/librepcb/core/project/board/boardpolygondata.{h,cpp} (and
//! `items/bi_polygon.{h,cpp}`, whose state is this data).

use crate::geometry::{Path, property};
use crate::serialization::{self, DeserializeObject, HasUuid, List, SExpression, SerializeObject};
use crate::types::{Layer, UnsignedLength, Uuid};

/// A polygon of a board (e.g. the board outline).
///
/// Serde: an object with the fields below.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BoardPolygonData {
    uuid: Uuid,
    layer: Layer,
    line_width: UnsignedLength,
    path: Path,
    is_filled: bool,
    is_grab_area: bool,
    locked: bool,
}

impl BoardPolygonData {
    /// Creates a polygon.
    pub fn new(
        uuid: Uuid,
        layer: Layer,
        line_width: UnsignedLength,
        path: Path,
        is_filled: bool,
        is_grab_area: bool,
        locked: bool,
    ) -> Self {
        Self {
            uuid,
            layer,
            line_width,
            path,
            is_filled,
            is_grab_area,
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
        /// Returns the layer.
        copy layer: Layer, set_layer
    );
    property!(
        /// Returns the line width.
        copy line_width: UnsignedLength, set_line_width
    );
    property!(
        /// Returns the path.
        ref path: Path, set_path
    );
    property!(
        /// Returns whether the polygon is filled.
        copy is_filled: bool, set_is_filled
    );
    property!(
        /// Returns whether the polygon is a grab area.
        copy is_grab_area: bool, set_is_grab_area
    );
    property!(
        /// Returns whether the polygon is locked.
        copy locked: bool, set_locked
    );
}

impl HasUuid for BoardPolygonData {
    fn uuid(&self) -> Uuid {
        self.uuid
    }
}

impl SerializeObject for BoardPolygonData {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("layer", &self.layer);
        root.ensure_line_break();
        root.append_child("width", &self.line_width);
        root.append_child("fill", &self.is_filled);
        root.append_child("grab_area", &self.is_grab_area);
        root.append_child("lock", &self.locked);
        root.ensure_line_break();
        self.path.serialize(root);
        root.ensure_line_break();
    }
}

impl DeserializeObject for BoardPolygonData {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            layer: node.child_value("layer/@0")?,
            line_width: node.child_value("width/@0")?,
            path: Path::deserialize(node)?,
            is_filled: node.child_value("fill/@0")?,
            is_grab_area: node.child_value("grab_area/@0")?,
            locked: node.child_value("lock/@0")?,
        })
    }
}
