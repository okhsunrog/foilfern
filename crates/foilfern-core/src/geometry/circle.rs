//! Port of libs/librepcb/core/geometry/circle.{h,cpp}.

use super::{impl_uuid, object_list, property};
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::{Layer, Point, PositiveLength, UnsignedLength, Uuid};

/// A circle on a layer (outline and/or filled).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Circle {
    uuid: Uuid,
    layer: Layer,
    line_width: UnsignedLength,
    is_filled: bool,
    is_grab_area: bool,
    center: Point,
    diameter: PositiveLength,
}

impl Circle {
    /// Creates a circle.
    pub fn new(
        uuid: Uuid,
        layer: Layer,
        line_width: UnsignedLength,
        is_filled: bool,
        is_grab_area: bool,
        center: Point,
        diameter: PositiveLength,
    ) -> Self {
        Self {
            uuid,
            layer,
            line_width,
            is_filled,
            is_grab_area,
            center,
            diameter,
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
        /// Returns whether the circle is filled.
        copy is_filled: bool, set_is_filled
    );
    property!(
        /// Returns whether the circle is a grab area.
        copy is_grab_area: bool, set_is_grab_area
    );
    property!(
        /// Returns the center.
        copy center: Point, set_center
    );
    property!(
        /// Returns the diameter.
        copy diameter: PositiveLength, set_diameter
    );
}

impl_uuid!(Circle);

object_list!(
    /// List of [`Circle`]s.
    CircleList,
    CircleListTag,
    Circle,
    "circle"
);

impl SerializeObject for Circle {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("layer", &self.layer);
        root.ensure_line_break();
        root.append_child("width", &self.line_width);
        root.append_child("fill", &self.is_filled);
        root.append_child("grab_area", &self.is_grab_area);
        root.append_child("diameter", &self.diameter);
        self.center.serialize(root.append_list("position"));
        root.ensure_line_break();
    }
}

impl DeserializeObject for Circle {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            layer: node.child_value("layer/@0")?,
            line_width: node.child_value("width/@0")?,
            is_filled: node.child_value("fill/@0")?,
            is_grab_area: node.child_value("grab_area/@0")?,
            center: Point::deserialize(node.required_child("position")?)?,
            diameter: node.child_value("diameter/@0")?,
        })
    }
}
