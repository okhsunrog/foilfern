//! Port of libs/librepcb/core/types/ratio.{h,cpp}.
//!
//! Ratios are stored as integer parts per million (ppm). The upstream
//! setters are replaced by the equivalent constructors.

use std::fmt;
use std::ops::{Add, AddAssign, Deref, Div, DivAssign, Mul, MulAssign, Neg, Rem, Sub, SubAssign};
use std::str::FromStr;

use super::Error;
use crate::serialization::{self, FromSExpression, SExpression, ToSExpression};
use crate::utils::toolbox::{decimal_fixed_point_from_string, decimal_fixed_point_to_string};

/// A ratio (e.g. a percentage) in integer parts per million.
///
/// [`Display`](fmt::Display) and [`FromStr`] use the normalized file format
/// representation (e.g. `"0.5"` for 50%).
///
/// Serde: integer ppm (`500000` for 50%).
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
pub struct Ratio(i32);

impl Ratio {
    /// 0%
    pub const ZERO: Self = Self(0);
    /// 100%
    pub const ONE: Self = Self(1_000_000);

    /// Creates a ratio from ppm.
    pub const fn new(ppm: i32) -> Self {
        Self(ppm)
    }

    /// Creates an exact ratio from integer percent.
    pub fn from_percent(percent: i32) -> Self {
        Self::from_f64(f64::from(percent) * 1e4)
    }

    /// Creates a ratio from floating point percent (may lose precision).
    pub fn from_percent_f64(percent: f64) -> Self {
        Self::from_f64(percent * 1e4)
    }

    /// Creates a ratio from a floating point normalized value, i.e. 1.0 for
    /// 100% (may lose precision).
    pub fn from_normalized(normalized: f64) -> Self {
        Self::from_f64(normalized * 1e6)
    }

    /// Truncating float to integer conversion, like the implicit conversion
    /// of upstream (saturating instead of undefined behavior on overflow).
    fn from_f64(ppm: f64) -> Self {
        Self(ppm as i32)
    }

    /// Parses the exact normalized representation (at most 6 decimals), e.g.
    /// `"0.1234"` for 12.34%.
    pub fn from_normalized_str(normalized: &str) -> Result<Self, Error> {
        decimal_fixed_point_from_string::<i32>(normalized, 6).map(Self)
    }

    /// Returns the ratio in ppm.
    pub const fn to_ppm(self) -> i32 {
        self.0
    }

    /// Returns the ratio in percent.
    pub fn to_percent(self) -> f64 {
        f64::from(self.0) / 1e4
    }

    /// Returns the normalized ratio (1.0 for 100%).
    pub fn to_normalized(self) -> f64 {
        f64::from(self.0) / 1e6
    }

    /// Returns the exact normalized file format representation, e.g. `"0.5"`.
    pub fn to_normalized_string(self) -> String {
        decimal_fixed_point_to_string(i64::from(self.0), 6)
    }
}

impl fmt::Display for Ratio {
    /// Formats the normalized value, see [`Ratio::to_normalized_string()`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_normalized_string())
    }
}

impl FromStr for Ratio {
    type Err = Error;
    /// Parses the normalized value, see [`Ratio::from_normalized_str()`].
    fn from_str(s: &str) -> Result<Self, Error> {
        Self::from_normalized_str(s)
    }
}

// Ratio ± Ratio and scaling by an integer factor; there is no Ratio × Ratio
// (the ppm representation would make its result meaningless).

impl Add for Ratio {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl Sub for Ratio {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}

impl Mul<i32> for Ratio {
    type Output = Self;
    fn mul(self, rhs: i32) -> Self {
        Self(self.0 * rhs)
    }
}

impl Div<i32> for Ratio {
    type Output = Self;
    fn div(self, rhs: i32) -> Self {
        Self(self.0 / rhs)
    }
}

impl AddAssign for Ratio {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl SubAssign for Ratio {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

impl MulAssign<i32> for Ratio {
    fn mul_assign(&mut self, rhs: i32) {
        self.0 *= rhs;
    }
}

impl DivAssign<i32> for Ratio {
    fn div_assign(&mut self, rhs: i32) {
        self.0 /= rhs;
    }
}

impl Rem for Ratio {
    type Output = Self;
    fn rem(self, rhs: Self) -> Self {
        Self(self.0 % rhs.0)
    }
}

impl Neg for Ratio {
    type Output = Self;
    fn neg(self) -> Self {
        Self(-self.0)
    }
}

impl ToSExpression for Ratio {
    fn to_sexpression(&self) -> SExpression {
        SExpression::token(self.to_normalized_string())
    }
}

impl FromSExpression for Ratio {
    fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self::from_normalized_str(node.value()?)?)
    }
}

/// Generates a constrained wrapper around [`Ratio`].
macro_rules! constrained_ratio {
    ($(#[$meta:meta])* $name:ident, $check:expr, $err:expr) => {
        $(#[$meta])*
        ///
        /// Serde: serialized like [`Ratio`] (integer ppm), the constraint is
        /// checked when deserializing.
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
        #[serde(try_from = "Ratio", into = "Ratio")]
        pub struct $name(Ratio);

        impl $name {
            /// Creates the value, or returns an error if `ratio` violates the
            /// constraint.
            pub fn new(ratio: Ratio) -> Result<Self, Error> {
                let check: fn(Ratio) -> bool = $check;
                if check(ratio) { Ok(Self(ratio)) } else { Err($err) }
            }

            /// Returns the wrapped ratio.
            pub const fn get(self) -> Ratio {
                self.0
            }
        }

        impl Deref for $name {
            type Target = Ratio;
            fn deref(&self) -> &Ratio {
                &self.0
            }
        }

        impl TryFrom<Ratio> for $name {
            type Error = Error;
            fn try_from(ratio: Ratio) -> Result<Self, Error> {
                Self::new(ratio)
            }
        }

        impl From<$name> for Ratio {
            fn from(value: $name) -> Ratio {
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

        impl ToSExpression for $name {
            fn to_sexpression(&self) -> SExpression {
                self.0.to_sexpression()
            }
        }

        impl FromSExpression for $name {
            fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
                Ok(Self::new(Ratio::from_sexpression(node)?)?)
            }
        }
    };
}

constrained_ratio!(
    /// A [`Ratio`] which is guaranteed to be >= 0.
    UnsignedRatio,
    |r| r >= Ratio::ZERO,
    Error::NegativeRatio
);

constrained_ratio!(
    /// A [`Ratio`] which is guaranteed to be in the range 0..1 (0..100%).
    UnsignedLimitedRatio,
    |r| (r >= Ratio::ZERO) && (r <= Ratio::ONE),
    Error::RatioNotInUnitRange
);
