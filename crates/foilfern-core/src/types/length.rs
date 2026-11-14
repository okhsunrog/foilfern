//! Port of libs/librepcb/core/types/length.{h,cpp} (plus the length math of
//! libs/librepcb/rust-core/src/types/length.rs which upstream calls via FFI).
//!
//! All lengths are integer nanometers ([`i64`]). Like upstream, arithmetic
//! overflow is considered a bug (it panics in debug builds); values from
//! floating point computations must be converted with the range-checked
//! constructors ([`Length::from_nm_f64()`], [`Length::from_mm()`], ...).

use std::fmt;
use std::ops::{Add, AddAssign, Deref, Div, DivAssign, Mul, MulAssign, Neg, Rem, Sub, SubAssign};
use std::str::FromStr;

use super::Error;
use crate::serialization::{
    self, DeserializeObject, FromSExpression, List, SExpression, SerializeObject, ToSExpression,
};
use crate::utils::toolbox::{decimal_fixed_point_from_string, decimal_fixed_point_to_string};

const NM_PER_INCH: i64 = 25_400_000;
const NM_PER_MIL: i64 = 25_400;
const PIXELS_PER_INCH: i64 = 72;
const NM_PER_PIXEL: f64 = NM_PER_INCH as f64 / PIXELS_PER_INCH as f64;
const PIXELS_PER_NM: f64 = PIXELS_PER_INCH as f64 / NM_PER_INCH as f64;

/// A length (or coordinate) in integer nanometers.
///
/// This type is used for *all* lengths in symbols, schematics, footprints,
/// boards etc. [`Display`](fmt::Display) and [`FromStr`] use the file format
/// representation in millimeters (e.g. `"1.5"`, see
/// [`to_mm_string()`](Self::to_mm_string)).
///
/// Serde: serialized as the integer number of nanometers.
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
pub struct Length(i64);

impl Length {
    /// Zero length.
    pub const ZERO: Self = Self(0);
    /// Smallest possible length.
    pub const MIN: Self = Self(i64::MIN);
    /// Largest possible length.
    pub const MAX: Self = Self(i64::MAX);

    /// Creates a length from nanometers.
    pub const fn new(nanometers: i64) -> Self {
        Self(nanometers)
    }

    /// Returns the length in nanometers.
    pub const fn to_nm(self) -> i64 {
        self.0
    }

    /// Returns the length in micrometers (may lose precision).
    pub fn to_micrometers(self) -> f64 {
        self.0 as f64 / 1e3
    }

    /// Returns the length in millimeters (may lose precision).
    pub fn to_mm(self) -> f64 {
        self.0 as f64 / 1e6
    }

    /// Returns the length in inches (may lose precision).
    pub fn to_inch(self) -> f64 {
        self.0 as f64 / NM_PER_INCH as f64
    }

    /// Returns the length in mils (may lose precision).
    pub fn to_mil(self) -> f64 {
        self.0 as f64 / NM_PER_MIL as f64
    }

    /// Returns the length in pixels of the 72 dpi graphics scene (may lose
    /// precision).
    pub fn to_px(self) -> f64 {
        self.0 as f64 * PIXELS_PER_NM
    }

    /// Returns the exact file format representation in millimeters, e.g.
    /// `"0.0"`, `"-1.5"` or `"0.000001"`.
    pub fn to_mm_string(self) -> String {
        decimal_fixed_point_to_string(self.0, 6)
    }

    /// Returns the absolute value (saturating for [`Length::MIN`]).
    pub fn abs(self) -> Self {
        Self(self.0.saturating_abs())
    }

    /// Returns the length rounded to the nearest multiple of `grid_interval`
    /// (halfway cases away from zero). A zero interval returns `self`.
    pub fn mapped_to_grid(self, grid_interval: Length) -> Self {
        if grid_interval.0 == 0 {
            self
        } else {
            self.rounded_to(grid_interval.abs().0)
        }
    }

    /// Returns the length rounded down to the next multiple of `multiple`
    /// (towards negative infinity). A zero interval returns `self`.
    pub fn rounded_down_to(self, multiple: Length) -> Self {
        if multiple.0 == 0 {
            return self;
        }
        let multiple = multiple.abs().0;
        let rounded = self.rounded_to(multiple);
        if rounded > self {
            Self(rounded.0 - multiple)
        } else {
            rounded
        }
    }

    /// Returns the length rounded up to the next multiple of `multiple`
    /// (towards positive infinity). A zero interval returns `self`.
    pub fn rounded_up_to(self, multiple: Length) -> Self {
        if multiple.0 == 0 {
            return self;
        }
        let multiple = multiple.abs().0;
        let rounded = self.rounded_to(multiple);
        if rounded < self {
            Self(rounded.0 + multiple)
        } else {
            rounded
        }
    }

    /// Rounds to the nearest multiple of `multiple` (> 0), halfway cases away
    /// from zero.
    fn rounded_to(self, multiple: i64) -> Self {
        let half = multiple / 2;
        if self.0 >= 0 {
            Self((self.0 + half) / multiple * multiple)
        } else {
            Self((self.0 - half) / multiple * multiple)
        }
    }

    /// Returns the length scaled by `factor` (rounded to nanometers; may lose
    /// precision). Returns zero if the result is out of range, like upstream.
    pub fn scaled(self, factor: f64) -> Self {
        Self::from_nm_f64(self.0 as f64 * factor).unwrap_or_default()
    }

    /// Converts floating point nanometers (rounded to the nearest integer,
    /// halfway cases away from zero). Fails for NaN or values out of range.
    pub fn from_nm_f64(nanometers: f64) -> Result<Self, Error> {
        let rounded = nanometers.round();
        // Note: `i64::MAX as f64` is 2^63, which saturates to `i64::MAX`.
        if (rounded >= i64::MIN as f64) && (rounded <= i64::MAX as f64) {
            Ok(Self(rounded as i64))
        } else {
            Err(Error::Range {
                value: nanometers,
                min: i64::MIN,
                max: i64::MAX,
            })
        }
    }

    /// Converts floating point millimeters (may lose precision).
    pub fn from_mm(millimeters: f64) -> Result<Self, Error> {
        Self::from_nm_f64(millimeters * 1e6)
    }

    /// Converts floating point inches (may lose precision).
    pub fn from_inch(inches: f64) -> Result<Self, Error> {
        Self::from_nm_f64(inches * NM_PER_INCH as f64)
    }

    /// Converts floating point mils (may lose precision).
    pub fn from_mil(mils: f64) -> Result<Self, Error> {
        Self::from_nm_f64(mils * NM_PER_MIL as f64)
    }

    /// Converts floating point pixels of the 72 dpi graphics scene (may lose
    /// precision).
    pub fn from_px(pixels: f64) -> Result<Self, Error> {
        Self::from_nm_f64(pixels * NM_PER_PIXEL)
    }

    /// Parses the exact millimeter representation (at most 6 decimals, an
    /// exponent is allowed), e.g. `"1.5"` or `"1e-6"`.
    pub fn from_mm_str(millimeters: &str) -> Result<Self, Error> {
        decimal_fixed_point_from_string::<i64>(millimeters, 6).map(Self)
    }
}

impl fmt::Display for Length {
    /// Formats as millimeters, see [`Length::to_mm_string()`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_mm_string())
    }
}

impl FromStr for Length {
    type Err = Error;
    /// Parses millimeters, see [`Length::from_mm_str()`].
    fn from_str(s: &str) -> Result<Self, Error> {
        Self::from_mm_str(s)
    }
}

impl From<i64> for Length {
    fn from(nanometers: i64) -> Self {
        Self(nanometers)
    }
}

impl Add for Length {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl Sub for Length {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}

impl Neg for Length {
    type Output = Self;
    fn neg(self) -> Self {
        Self(-self.0)
    }
}

impl Mul<i64> for Length {
    type Output = Self;
    fn mul(self, rhs: i64) -> Self {
        Self(self.0 * rhs)
    }
}

impl Div<i64> for Length {
    type Output = Self;
    fn div(self, rhs: i64) -> Self {
        Self(self.0 / rhs)
    }
}

impl Rem for Length {
    type Output = Self;
    fn rem(self, rhs: Self) -> Self {
        Self(self.0 % rhs.0)
    }
}

impl AddAssign for Length {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl SubAssign for Length {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

impl MulAssign<i64> for Length {
    fn mul_assign(&mut self, rhs: i64) {
        self.0 *= rhs;
    }
}

impl DivAssign<i64> for Length {
    fn div_assign(&mut self, rhs: i64) {
        self.0 /= rhs;
    }
}

impl ToSExpression for Length {
    fn to_sexpression(&self) -> SExpression {
        SExpression::token(self.to_mm_string())
    }
}

impl FromSExpression for Length {
    fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self::from_mm_str(node.value()?)?)
    }
}

/// Generates a constrained wrapper around [`Length`].
macro_rules! constrained_length {
    ($(#[$meta:meta])* $name:ident, $check:expr, $err:expr) => {
        $(#[$meta])*
        ///
        /// Serde: serialized like [`Length`] (integer nanometers), the
        /// constraint is checked when deserializing.
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            serde::Serialize,
            serde::Deserialize,
        )]
        #[serde(try_from = "Length", into = "Length")]
        pub struct $name(Length);

        impl $name {
            /// Creates the value, or returns an error if `length` violates
            /// the constraint.
            pub fn new(length: Length) -> Result<Self, Error> {
                let check: fn(Length) -> bool = $check;
                if check(length) { Ok(Self(length)) } else { Err($err) }
            }

            /// Returns the wrapped length.
            pub const fn get(self) -> Length {
                self.0
            }
        }

        impl Deref for $name {
            type Target = Length;
            fn deref(&self) -> &Length {
                &self.0
            }
        }

        impl TryFrom<Length> for $name {
            type Error = Error;
            fn try_from(length: Length) -> Result<Self, Error> {
                Self::new(length)
            }
        }

        impl From<$name> for Length {
            fn from(value: $name) -> Length {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl FromStr for $name {
            type Err = Error;
            fn from_str(s: &str) -> Result<Self, Error> {
                Self::new(s.parse()?)
            }
        }

        impl PartialEq<Length> for $name {
            fn eq(&self, other: &Length) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<$name> for Length {
            fn eq(&self, other: &$name) -> bool {
                *self == other.0
            }
        }

        impl PartialOrd<Length> for $name {
            fn partial_cmp(&self, other: &Length) -> Option<std::cmp::Ordering> {
                self.0.partial_cmp(other)
            }
        }

        impl PartialOrd<$name> for Length {
            fn partial_cmp(&self, other: &$name) -> Option<std::cmp::Ordering> {
                self.partial_cmp(&other.0)
            }
        }

        impl Add<Length> for $name {
            type Output = Length;
            fn add(self, rhs: Length) -> Length {
                self.0 + rhs
            }
        }

        impl Add<$name> for Length {
            type Output = Length;
            fn add(self, rhs: $name) -> Length {
                self + rhs.0
            }
        }

        impl Sub<Length> for $name {
            type Output = Length;
            fn sub(self, rhs: Length) -> Length {
                self.0 - rhs
            }
        }

        impl Sub<$name> for Length {
            type Output = Length;
            fn sub(self, rhs: $name) -> Length {
                self - rhs.0
            }
        }

        impl Neg for $name {
            type Output = Length;
            fn neg(self) -> Length {
                -self.0
            }
        }

        impl Mul<i64> for $name {
            type Output = Length;
            fn mul(self, rhs: i64) -> Length {
                self.0 * rhs
            }
        }

        impl Div<i64> for $name {
            type Output = Length;
            fn div(self, rhs: i64) -> Length {
                self.0 / rhs
            }
        }

        impl ToSExpression for $name {
            fn to_sexpression(&self) -> SExpression {
                self.0.to_sexpression()
            }
        }

        impl FromSExpression for $name {
            fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
                Ok(Self::new(Length::from_sexpression(node)?)?)
            }
        }
    };
}

constrained_length!(
    /// A [`Length`] which is guaranteed to be >= 0.
    UnsignedLength,
    |l| l >= Length::ZERO,
    Error::NegativeLength
);

constrained_length!(
    /// A [`Length`] which is guaranteed to be > 0.
    PositiveLength,
    |l| l > Length::ZERO,
    Error::NonPositiveLength
);

impl UnsignedLength {
    /// Zero length.
    pub const ZERO: Self = Self(Length::ZERO);
}

impl Default for UnsignedLength {
    fn default() -> Self {
        Self::ZERO
    }
}

impl From<PositiveLength> for UnsignedLength {
    fn from(value: PositiveLength) -> Self {
        Self(value.0)
    }
}

// Sums of non-negative/positive lengths keep their constraint (as long as
// they do not overflow).

impl Add for UnsignedLength {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl Add for PositiveLength {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl Add<UnsignedLength> for PositiveLength {
    type Output = Self;
    fn add(self, rhs: UnsignedLength) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl Add<PositiveLength> for UnsignedLength {
    type Output = PositiveLength;
    fn add(self, rhs: PositiveLength) -> PositiveLength {
        PositiveLength(self.0 + rhs.0)
    }
}

impl AddAssign for UnsignedLength {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl AddAssign<PositiveLength> for UnsignedLength {
    fn add_assign(&mut self, rhs: PositiveLength) {
        self.0 += rhs.0;
    }
}

impl AddAssign for PositiveLength {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl AddAssign<UnsignedLength> for PositiveLength {
    fn add_assign(&mut self, rhs: UnsignedLength) {
        self.0 += rhs.0;
    }
}

impl Sub<PositiveLength> for UnsignedLength {
    type Output = Length;
    fn sub(self, rhs: PositiveLength) -> Length {
        self.0 - rhs.0
    }
}

impl Sub<UnsignedLength> for PositiveLength {
    type Output = Length;
    fn sub(self, rhs: UnsignedLength) -> Length {
        self.0 - rhs.0
    }
}

impl PartialEq<PositiveLength> for UnsignedLength {
    fn eq(&self, other: &PositiveLength) -> bool {
        self.0 == other.0
    }
}

impl PartialEq<UnsignedLength> for PositiveLength {
    fn eq(&self, other: &UnsignedLength) -> bool {
        self.0 == other.0
    }
}

impl PartialOrd<PositiveLength> for UnsignedLength {
    fn partial_cmp(&self, other: &PositiveLength) -> Option<std::cmp::Ordering> {
        self.0.partial_cmp(&other.0)
    }
}

impl PartialOrd<UnsignedLength> for PositiveLength {
    fn partial_cmp(&self, other: &UnsignedLength) -> Option<std::cmp::Ordering> {
        self.0.partial_cmp(&other.0)
    }
}

/// A 3D coordinate `(x, y, z)`, e.g. the position of a 3D model.
pub type Point3D = (Length, Length, Length);

impl SerializeObject for Point3D {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.0);
        root.append_value(&self.1);
        root.append_value(&self.2);
    }
}

impl DeserializeObject for Point3D {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok((
            node.child_value("@0")?,
            node.child_value("@1")?,
            node.child_value("@2")?,
        ))
    }
}
