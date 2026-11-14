//! Port of libs/librepcb/core/geometry/vertex.{h,cpp} (and
//! libs/librepcb/rust-core/src/types/vertex.rs).

use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::{Angle, Point};

/// A vertex of a [`Path`](super::Path): a position and the angle of the
/// (arc) segment from this vertex to the next one.
///
/// The ordering compares the position first, then the angle (e.g. for
/// canonical order in files).
///
/// Serde: `{"pos": {"x", "y"}, "angle": <µdeg>}`.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct Vertex {
    /// Position.
    pub pos: Point,
    /// Angle of the segment to the next vertex (0° = straight line).
    pub angle: Angle,
}

impl Vertex {
    /// Creates a vertex.
    pub const fn new(pos: Point, angle: Angle) -> Self {
        Self { pos, angle }
    }

    /// Creates a vertex with a straight segment (0°) to the next vertex.
    pub const fn at(pos: Point) -> Self {
        Self::new(pos, Angle::DEG0)
    }
}

impl From<Point> for Vertex {
    fn from(pos: Point) -> Self {
        Self::at(pos)
    }
}

impl SerializeObject for Vertex {
    /// Appends `(position x y) (angle a)`.
    fn serialize(&self, root: &mut List) {
        self.pos.serialize(root.append_list("position"));
        root.append_child("angle", &self.angle);
    }
}

impl DeserializeObject for Vertex {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self::new(
            Point::deserialize(node.required_child("position")?)?,
            node.child_value("angle/@0")?,
        ))
    }
}
