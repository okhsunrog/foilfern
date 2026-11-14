//! Port of libs/librepcb/core/utils/transform.{h,cpp}.
//!
//! The overloaded/templated `map()` of upstream is the [`Transformable`]
//! trait, implemented for points, paths, layers and collections of them.
//! Not ported: `mapPx()` (Qt graphics specific).

use crate::geometry::{NonEmptyPath, Path};
use crate::types::{Angle, Layer, Orientation, Point};

/// Objects which have a position, rotation and mirror state (upstream
/// template constructor `Transform(const T& obj)`).
pub trait HasTransform {
    /// Returns the position.
    fn position(&self) -> Point;
    /// Returns the rotation.
    fn rotation(&self) -> Angle;
    /// Returns whether the object is mirrored.
    fn mirrored(&self) -> bool;
}

/// A transformation: mirror (horizontally), then rotate, then translate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Transform {
    /// Translation.
    pub position: Point,
    /// Rotation (counter-clockwise).
    pub rotation: Angle,
    /// Whether to mirror horizontally (before rotating).
    pub mirrored: bool,
}

impl Transform {
    /// Creates a transformation.
    pub const fn new(position: Point, rotation: Angle, mirrored: bool) -> Self {
        Self {
            position,
            rotation,
            mirrored,
        }
    }

    /// Creates the transformation of an object.
    pub fn from_obj(obj: &impl HasTransform) -> Self {
        Self::new(obj.position(), obj.rotation(), obj.mirrored())
    }

    /// Maps a mirror state (inverted if the transformation mirrors).
    pub fn map_mirror(&self, mirror: bool) -> bool {
        mirror != self.mirrored
    }

    /// Maps the rotation of a mirrorable object (e.g. a polygon): mirroring
    /// inverts the angle.
    pub fn map_mirrorable(&self, angle: Angle) -> Angle {
        if self.mirrored {
            self.rotation - angle
        } else {
            self.rotation + angle
        }
    }

    /// Maps the rotation of a non-mirrorable object (e.g. a text): mirroring
    /// rotates it by 180°.
    pub fn map_non_mirrorable(&self, angle: Angle) -> Angle {
        self.rotation
            + if self.mirrored {
                Angle::DEG180 - angle
            } else {
                angle
            }
    }

    /// Maps an object (point, path, layer, or a collection of them).
    pub fn map<T: Transformable>(&self, obj: &T) -> T {
        obj.transformed(self)
    }
}

/// Objects which can be mapped by a [`Transform`].
pub trait Transformable: Sized {
    /// Returns the mapped object.
    fn transformed(&self, transform: &Transform) -> Self;
}

impl Transformable for Point {
    fn transformed(&self, t: &Transform) -> Self {
        let mut p = *self;
        if t.mirrored {
            p = p.mirrored(Orientation::Horizontal, Point::ORIGIN);
        }
        if t.rotation != Angle::DEG0 {
            p = p.rotated(t.rotation, Point::ORIGIN);
        }
        p + t.position
    }
}

impl Transformable for Path {
    fn transformed(&self, t: &Transform) -> Self {
        let mut p = self.clone();
        if t.mirrored {
            p = p.mirrored(Orientation::Horizontal, Point::ORIGIN);
        }
        if t.rotation != Angle::DEG0 {
            p = p.rotated(t.rotation, Point::ORIGIN);
        }
        if !t.position.is_origin() {
            p = p.translated(t.position);
        }
        p
    }
}

impl Transformable for NonEmptyPath {
    fn transformed(&self, t: &Transform) -> Self {
        // Transformations keep the vertex count.
        Self::new(self.get().transformed(t)).unwrap_or_else(|_| self.clone())
    }
}

impl Transformable for Layer {
    /// Mirrors the layer to the other board side if the transformation
    /// mirrors.
    fn transformed(&self, t: &Transform) -> Self {
        if t.mirrored {
            self.mirrored(None)
        } else {
            *self
        }
    }
}

impl<T: Transformable> Transformable for Vec<T> {
    fn transformed(&self, t: &Transform) -> Self {
        self.iter().map(|item| item.transformed(t)).collect()
    }
}
