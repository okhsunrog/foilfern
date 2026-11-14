//! Port of libs/librepcb/core/types/point.{h,cpp}.
//!
//! `QPointF` conversions are replaced by `(f64, f64)` tuples, and
//! `Qt::Orientation` by [`Orientation`].

use std::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Rem, Sub, SubAssign};

use super::{Angle, Error, Length, PositiveLength, UnsignedLength};
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::utils::math;

/// Mirror orientation (upstream `Qt::Orientation`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Orientation {
    /// Mirror the X coordinate (at a vertical axis).
    Horizontal,
    /// Mirror the Y coordinate (at a horizontal axis).
    Vertical,
}

/// A 2D point/vector consisting of two [`Length`]s.
///
/// The Y axis points upwards (unlike the Qt graphics scene, see
/// [`to_px()`](Self::to_px) and [`from_px()`](Self::from_px)).
///
/// The ordering compares X first, then Y. It has no geometric meaning but
/// allows using points as keys in sorted containers.
///
/// Serde: `{"x": <nm>, "y": <nm>}` with integer nanometers.
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
pub struct Point {
    /// X coordinate.
    pub x: Length,
    /// Y coordinate.
    pub y: Length,
}

impl Point {
    /// The origin (0, 0).
    pub const ORIGIN: Self = Self::new(Length::ZERO, Length::ZERO);

    /// Creates a point from its coordinates.
    pub const fn new(x: Length, y: Length) -> Self {
        Self { x, y }
    }

    /// Creates a point from nanometer coordinates.
    pub const fn from_nm(x: i64, y: i64) -> Self {
        Self::new(Length::new(x), Length::new(y))
    }

    /// Converts floating point millimeters (may lose precision).
    pub fn from_mm(x: f64, y: f64) -> Result<Self, Error> {
        Ok(Self::new(Length::from_mm(x)?, Length::from_mm(y)?))
    }

    /// Converts floating point inches (may lose precision).
    pub fn from_inch(x: f64, y: f64) -> Result<Self, Error> {
        Ok(Self::new(Length::from_inch(x)?, Length::from_inch(y)?))
    }

    /// Converts floating point mils (may lose precision).
    pub fn from_mil(x: f64, y: f64) -> Result<Self, Error> {
        Ok(Self::new(Length::from_mil(x)?, Length::from_mil(y)?))
    }

    /// Converts graphics scene pixels (the Y axis is inverted).
    pub fn from_px(x: f64, y: f64) -> Result<Self, Error> {
        Ok(Self::new(Length::from_px(x)?, Length::from_px(-y)?))
    }

    /// Returns the length of the vector (distance from the origin),
    /// truncated to nanometers.
    pub fn length(self) -> UnsignedLength {
        let length = libm::hypot(self.x.to_nm() as f64, self.y.to_nm() as f64);
        // `as` saturates, and the hypotenuse is never negative.
        UnsignedLength::new(Length::new(length as i64)).unwrap_or_default()
    }

    /// Returns whether this is the origin.
    pub fn is_origin(self) -> bool {
        self == Self::ORIGIN
    }

    /// Returns the coordinates in millimeters (may lose precision).
    pub fn to_mm(self) -> (f64, f64) {
        (self.x.to_mm(), self.y.to_mm())
    }

    /// Returns the coordinates in inches (may lose precision).
    pub fn to_inch(self) -> (f64, f64) {
        (self.x.to_inch(), self.y.to_inch())
    }

    /// Returns the coordinates in mils (may lose precision).
    pub fn to_mil(self) -> (f64, f64) {
        (self.x.to_mil(), self.y.to_mil())
    }

    /// Returns the coordinates in graphics scene pixels (the Y axis is
    /// inverted).
    pub fn to_px(self) -> (f64, f64) {
        (self.x.to_px(), -self.y.to_px())
    }

    /// Returns the point mapped to the grid (see [`Length::mapped_to_grid()`]).
    pub fn mapped_to_grid(self, grid_interval: PositiveLength) -> Self {
        Self::new(
            self.x.mapped_to_grid(*grid_interval),
            self.y.mapped_to_grid(*grid_interval),
        )
    }

    /// Returns whether the point lies on the grid.
    pub fn is_on_grid(self, grid_interval: PositiveLength) -> bool {
        self.mapped_to_grid(grid_interval) == self
    }

    /// Returns the point rotated counter-clockwise by `angle` around
    /// `center`.
    ///
    /// Multiples of 90° are exact (integer math), other angles use floating
    /// point math (rounded to nanometers).
    pub fn rotated(self, angle: Angle, center: Point) -> Self {
        let dx = self.x - center.x;
        let dy = self.y - center.y;
        let angle_0_360 = angle.mapped_to_0_360deg();
        if angle_0_360 == Angle::DEG90 {
            Self::new(center.x - dy, center.y + dx)
        } else if angle_0_360 == Angle::DEG180 {
            Self::new(center.x - dx, center.y - dy)
        } else if angle_0_360 == Angle::DEG270 {
            Self::new(center.x + dy, center.y - dx)
        } else if angle != Angle::DEG0 {
            let (x, y) =
                math::rotate_point(dx.to_nm() as f64, dy.to_nm() as f64, angle_0_360.to_deg());
            // Rotation preserves the vector length, so the result can only be
            // out of range for points close to the numeric limits. `as`
            // saturates in that case (upstream would terminate). For values
            // in range, this is identical to `Length::from_nm_f64()`.
            let to_length = |v: f64| Length::new(v.round() as i64);
            Self::new(center.x + to_length(x), center.y + to_length(y))
        } else {
            self
        }
    }

    /// Returns the point mirrored around `center`.
    pub fn mirrored(self, orientation: Orientation, center: Point) -> Self {
        match orientation {
            Orientation::Horizontal => Self::new(self.x + (center.x - self.x) * 2, self.y),
            Orientation::Vertical => Self::new(self.x, self.y + (center.y - self.y) * 2),
        }
    }
}

impl SerializeObject for Point {
    /// Appends the X and Y coordinates (e.g. `(position 1.0 2.0)`).
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.x);
        root.append_value(&self.y);
    }
}

impl DeserializeObject for Point {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self::new(node.child_value("@0")?, node.child_value("@1")?))
    }
}

impl Add for Point {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Point {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Neg for Point {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y)
    }
}

impl Mul<i64> for Point {
    type Output = Self;
    fn mul(self, rhs: i64) -> Self {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

impl Div<i64> for Point {
    type Output = Self;
    fn div(self, rhs: i64) -> Self {
        Self::new(self.x / rhs, self.y / rhs)
    }
}

impl Rem<Length> for Point {
    type Output = Self;
    fn rem(self, rhs: Length) -> Self {
        Self::new(self.x % rhs, self.y % rhs)
    }
}

impl AddAssign for Point {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl SubAssign for Point {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl MulAssign<i64> for Point {
    fn mul_assign(&mut self, rhs: i64) {
        *self = *self * rhs;
    }
}

impl DivAssign<i64> for Point {
    fn div_assign(&mut self, rhs: i64) {
        *self = *self / rhs;
    }
}
