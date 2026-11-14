//! Port of libs/librepcb/core/types/angle.{h,cpp} (plus the angle math of
//! libs/librepcb/rust-core/src/types/angle.rs which upstream calls via FFI).
//!
//! Angles are stored as integer microdegrees in the range ]-360°..+360°[;
//! every constructor and arithmetic operation maps the result into this range
//! (modulo, keeping the sign).

use std::fmt;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Rem, Sub, SubAssign};
use std::str::FromStr;

use super::Error;
use crate::serialization::{
    self, DeserializeObject, FromSExpression, List, SExpression, SerializeObject, ToSExpression,
};
use crate::utils::math;
use crate::utils::toolbox::{decimal_fixed_point_from_string, decimal_fixed_point_to_string};

const FULL_CIRCLE: i32 = 360_000_000;

/// An angle in integer microdegrees, interpreted as counter-clockwise
/// rotation.
///
/// Each angle except 0° has two representations (e.g. +270° and -90°); see
/// [`mapped_to_0_360deg()`](Self::mapped_to_0_360deg) and
/// [`mapped_to_180deg()`](Self::mapped_to_180deg) to normalize.
/// [`Display`](fmt::Display) and [`FromStr`] use the file format
/// representation in degrees (e.g. `"90.0"`).
///
/// Serde: serialized as the integer number of microdegrees.
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
#[serde(transparent)]
pub struct Angle(i32);

impl Angle {
    /// 0°
    pub const DEG0: Self = Self(0);
    /// 45°
    pub const DEG45: Self = Self(45_000_000);
    /// 90°
    pub const DEG90: Self = Self(90_000_000);
    /// 135°
    pub const DEG135: Self = Self(135_000_000);
    /// 180°
    pub const DEG180: Self = Self(180_000_000);
    /// 225°
    pub const DEG225: Self = Self(225_000_000);
    /// 270°
    pub const DEG270: Self = Self(270_000_000);
    /// 315°
    pub const DEG315: Self = Self(315_000_000);

    /// Creates an angle from microdegrees (mapped into ]-360°..+360°[).
    pub const fn new(microdegrees: i32) -> Self {
        Self(microdegrees % FULL_CIRCLE)
    }

    fn from_i64(microdegrees: i64) -> Self {
        // The remainder is always within ]-360e6..360e6[, i.e. fits into i32.
        Self((microdegrees % i64::from(FULL_CIRCLE)) as i32)
    }

    /// Returns the angle in microdegrees.
    pub const fn to_micro_deg(self) -> i32 {
        self.0
    }

    /// Returns the angle in degrees.
    pub fn to_deg(self) -> f64 {
        f64::from(self.0) / 1e6
    }

    /// Returns the angle in radians.
    pub fn to_rad(self) -> f64 {
        self.to_deg() / (180.0 / std::f64::consts::PI)
    }

    /// Returns the exact file format representation in degrees, e.g. `"90.0"`.
    pub fn to_deg_string(self) -> String {
        decimal_fixed_point_to_string(i64::from(self.0), 6)
    }

    /// Returns the absolute value.
    pub fn abs(self) -> Self {
        Self(self.0.abs())
    }

    /// Returns the same angle with inverted sign (e.g. 270° → -90°); 0° is
    /// kept as-is.
    pub fn inverted(self) -> Self {
        match self.0.cmp(&0) {
            std::cmp::Ordering::Greater => Self(self.0 - FULL_CIRCLE),
            std::cmp::Ordering::Less => Self(self.0 + FULL_CIRCLE),
            std::cmp::Ordering::Equal => self,
        }
    }

    /// Returns the angle rounded to the nearest multiple of `interval`
    /// (halfway cases away from zero). An interval <= 0 returns `self`.
    pub fn rounded(self, interval: Angle) -> Self {
        if interval.0 <= 0 {
            return self;
        }
        let (value, multiple) = (i64::from(self.0), i64::from(interval.0));
        let half = multiple / 2;
        let rounded = if value >= 0 {
            (value + half) / multiple * multiple
        } else {
            (value - half) / multiple * multiple
        };
        Self::from_i64(rounded)
    }

    /// Returns the angle mapped to [0°..360°[.
    pub fn mapped_to_0_360deg(self) -> Self {
        if self.0 < 0 {
            Self(self.0 + FULL_CIRCLE)
        } else {
            self
        }
    }

    /// Returns the angle mapped to [-180°..+180°[.
    pub fn mapped_to_180deg(self) -> Self {
        if self.0 < -180_000_000 {
            Self(self.0 + FULL_CIRCLE)
        } else if self.0 >= 180_000_000 {
            Self(self.0 - FULL_CIRCLE)
        } else {
            self
        }
    }

    /// Converts floating point degrees (rounded to microdegrees, halfway
    /// cases away from zero). Fails for NaN and values out of range of
    /// [`i64`] microdegrees.
    pub fn from_deg(degrees: f64) -> Result<Self, Error> {
        Self::from_udeg_f64(degrees * 1e6).ok_or(Error::Range {
            value: degrees * 1e6,
            min: i64::MIN,
            max: i64::MAX,
        })
    }

    /// Converts floating point radians. Fails for NaN and values out of range.
    pub fn from_rad(radians: f64) -> Result<Self, Error> {
        let microdegrees = math::to_degrees(radians) * 1e6;
        Self::from_udeg_f64(microdegrees).ok_or(Error::Range {
            value: microdegrees,
            min: i64::MIN,
            max: i64::MAX,
        })
    }

    fn from_udeg_f64(microdegrees: f64) -> Option<Self> {
        let rounded = microdegrees.round();
        ((rounded >= i64::MIN as f64) && (rounded <= i64::MAX as f64))
            .then(|| Self::from_i64(rounded as i64))
    }

    /// Parses the exact degree representation (at most 6 decimals, an
    /// exponent is allowed), e.g. `"90.0"`.
    pub fn from_deg_str(degrees: &str) -> Result<Self, Error> {
        decimal_fixed_point_from_string::<i32>(degrees, 6).map(Self::new)
    }
}

impl fmt::Display for Angle {
    /// Formats as degrees, see [`Angle::to_deg_string()`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_deg_string())
    }
}

impl FromStr for Angle {
    type Err = Error;
    /// Parses degrees, see [`Angle::from_deg_str()`].
    fn from_str(s: &str) -> Result<Self, Error> {
        Self::from_deg_str(s)
    }
}

impl Add for Angle {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::new(self.0 + rhs.0)
    }
}

impl Sub for Angle {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.0 - rhs.0)
    }
}

impl Neg for Angle {
    type Output = Self;
    fn neg(self) -> Self {
        Self(-self.0)
    }
}

impl Mul<i32> for Angle {
    type Output = Self;
    fn mul(self, rhs: i32) -> Self {
        Self::from_i64(i64::from(self.0) * i64::from(rhs))
    }
}

impl Div<i32> for Angle {
    type Output = Self;
    fn div(self, rhs: i32) -> Self {
        Self::new(self.0 / rhs)
    }
}

impl Rem for Angle {
    type Output = Self;
    fn rem(self, rhs: Self) -> Self {
        Self::new(self.0 % rhs.0)
    }
}

impl AddAssign for Angle {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl SubAssign for Angle {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl ToSExpression for Angle {
    fn to_sexpression(&self) -> SExpression {
        SExpression::token(self.to_deg_string())
    }
}

impl FromSExpression for Angle {
    fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self::from_deg_str(node.value()?)?)
    }
}

/// A 3D rotation `(x, y, z)`, e.g. the rotation of a 3D model.
pub type Angle3D = (Angle, Angle, Angle);

impl SerializeObject for Angle3D {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.0);
        root.append_value(&self.1);
        root.append_value(&self.2);
    }
}

impl DeserializeObject for Angle3D {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok((
            node.child_value("@0")?,
            node.child_value("@1")?,
            node.child_value("@2")?,
        ))
    }
}
