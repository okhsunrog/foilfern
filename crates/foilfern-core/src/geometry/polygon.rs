//! Port of libs/librepcb/core/geometry/polygon.{h,cpp}.

use super::{Path, impl_uuid, object_list, property};
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::{Layer, UnsignedLength, Uuid};

/// A polygon (or polyline) on a layer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Polygon {
    uuid: Uuid,
    layer: Layer,
    line_width: UnsignedLength,
    is_filled: bool,
    is_grab_area: bool,
    path: Path,
}

impl Polygon {
    /// Creates a polygon.
    pub fn new(
        uuid: Uuid,
        layer: Layer,
        line_width: UnsignedLength,
        is_filled: bool,
        is_grab_area: bool,
        path: Path,
    ) -> Self {
        Self {
            uuid,
            layer,
            line_width,
            is_filled,
            is_grab_area,
            path,
        }
    }

    property!(
        /// Returns the layer.
        copy layer: Layer, set_layer
    );
    property!(
        /// Returns the outline width.
        copy line_width: UnsignedLength, set_line_width
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
        /// Returns the path.
        ref path: Path, set_path
    );

    /// Returns the path to render: closed if the layer represents areas
    /// (e.g. courtyards), otherwise as-is.
    pub fn path_for_rendering(&self) -> Path {
        let mut path = self.path.clone();
        if self.layer.polygons_represent_areas() {
            path.close();
        }
        path
    }
}

impl_uuid!(Polygon);

object_list!(
    /// List of [`Polygon`]s.
    PolygonList,
    PolygonListTag,
    Polygon,
    "polygon"
);

impl SerializeObject for Polygon {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("layer", &self.layer);
        root.ensure_line_break();
        root.append_child("width", &self.line_width);
        root.append_child("fill", &self.is_filled);
        root.append_child("grab_area", &self.is_grab_area);
        root.ensure_line_break();
        self.path.serialize(root);
        root.ensure_line_break();
    }
}

impl DeserializeObject for Polygon {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            layer: node.child_value("layer/@0")?,
            line_width: node.child_value("width/@0")?,
            is_filled: node.child_value("fill/@0")?,
            is_grab_area: node.child_value("grab_area/@0")?,
            path: Path::deserialize(node)?,
        })
    }
}
