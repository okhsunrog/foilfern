//! Port of libs/librepcb/core/types/lengthunit.{h,cpp}.
//!
//! Not ported: `LengthUnit::format()`, which depends on `QLocale` number
//! formatting (deferred to the UI layer).

use std::fmt;
use std::str::FromStr;

use foilfern_i18n::tr;

use super::{Error, Length, Point};
use crate::serialization::{self, FromSExpression, SExpression, ToSExpression};

/// A length unit used to display/enter lengths (lengths themselves are
/// always stored in nanometers).
///
/// The variant order defines the order in UI lists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum LengthUnit {
    /// Millimeters (default).
    #[default]
    Millimeters,
    /// Micrometers.
    Micrometers,
    /// Nanometers.
    Nanometers,
    /// Inches.
    Inches,
    /// Mils (1/1000 inch).
    Mils,
}

impl LengthUnit {
    /// All units in UI order.
    pub const ALL: [LengthUnit; 5] = [
        Self::Millimeters,
        Self::Micrometers,
        Self::Nanometers,
        Self::Inches,
        Self::Mils,
    ];

    /// Returns the index in [`LengthUnit::ALL`] (not stable across versions,
    /// never store it in files).
    pub fn index(self) -> usize {
        self as usize
    }

    /// Returns the unit at `index` in [`LengthUnit::ALL`].
    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    /// Returns the serialization identifier, e.g. `"millimeters"`.
    pub fn to_str(self) -> &'static str {
        match self {
            Self::Millimeters => "millimeters",
            Self::Micrometers => "micrometers",
            Self::Nanometers => "nanometers",
            Self::Inches => "inches",
            Self::Mils => "mils",
        }
    }

    /// Returns the translated name, e.g. "Millimeters".
    pub fn to_string_tr(self) -> String {
        match self {
            Self::Millimeters => tr!("LengthUnit", "Millimeters"),
            Self::Micrometers => tr!("LengthUnit", "Micrometers"),
            Self::Nanometers => tr!("LengthUnit", "Nanometers"),
            Self::Inches => tr!("LengthUnit", "Inches"),
            Self::Mils => tr!("LengthUnit", "Mils"),
        }
    }

    /// Returns the short unit symbol, e.g. "mm" or "μm".
    pub fn to_short_str(self) -> &'static str {
        match self {
            Self::Millimeters => "mm",
            Self::Micrometers => "μm",
            Self::Nanometers => "nm",
            Self::Inches => "″",
            Self::Mils => "mils",
        }
    }

    /// Returns a reasonable number of decimals to display values in this
    /// unit (not enough to represent every nanometer exactly).
    pub fn reasonable_number_of_decimals(self) -> usize {
        match self {
            Self::Millimeters => 3,
            Self::Micrometers => 1,
            Self::Nanometers => 0,
            Self::Inches => 5,
            Self::Mils => 2,
        }
    }

    /// Returns the suffixes a user might type for this unit.
    pub fn user_input_suffixes(self) -> &'static [&'static str] {
        match self {
            Self::Millimeters => &["mm"],
            Self::Micrometers => &["μm", "um"],
            Self::Nanometers => &["nm"],
            Self::Inches => &["″", "\"", "in", "inch", "inches"],
            Self::Mils => &["mil", "mils"],
        }
    }

    /// Converts a length to this unit (may lose precision).
    pub fn convert_to_unit(self, length: Length) -> f64 {
        match self {
            Self::Millimeters => length.to_mm(),
            Self::Micrometers => length.to_mm() * 1000.0,
            Self::Nanometers => length.to_nm() as f64,
            Self::Inches => length.to_inch(),
            Self::Mils => length.to_mil(),
        }
    }

    /// Converts a point to this unit (may lose precision).
    pub fn convert_point_to_unit(self, point: Point) -> (f64, f64) {
        match self {
            Self::Millimeters => point.to_mm(),
            Self::Micrometers => {
                let (x, y) = point.to_mm();
                (x * 1000.0, y * 1000.0)
            }
            Self::Nanometers => {
                let (x, y) = point.to_mm();
                (x * 1_000_000.0, y * 1_000_000.0)
            }
            Self::Inches => point.to_inch(),
            Self::Mils => point.to_mil(),
        }
    }

    /// Converts a value in this unit to a length (may lose precision).
    pub fn convert_from_unit(self, value: f64) -> Result<Length, Error> {
        match self {
            Self::Millimeters => Length::from_mm(value),
            Self::Micrometers => Length::from_mm(value / 1000.0),
            Self::Nanometers => Length::from_mm(value / 1_000_000.0),
            Self::Inches => Length::from_inch(value),
            Self::Mils => Length::from_mil(value),
        }
    }

    /// Converts a point in this unit to a [`Point`] (may lose precision).
    pub fn convert_point_from_unit(self, (x, y): (f64, f64)) -> Result<Point, Error> {
        match self {
            Self::Millimeters => Point::from_mm(x, y),
            Self::Micrometers => Point::from_mm(x / 1000.0, y / 1000.0),
            Self::Nanometers => Point::from_mm(x / 1_000_000.0, y / 1_000_000.0),
            Self::Inches => Point::from_inch(x, y),
            Self::Mils => Point::from_mil(x, y),
        }
    }

    /// Extracts a unit suffix from a user input expression. If found, the
    /// suffix is removed from `expression` (e.g. `"2*0.5mm"` → `"2*0.5"`).
    pub fn extract_from_expression(expression: &mut String) -> Option<Self> {
        for unit in Self::ALL {
            for suffix in unit.user_input_suffixes() {
                if expression.ends_with(suffix) {
                    expression.truncate(expression.len() - suffix.len());
                    return Some(unit);
                }
            }
        }
        None
    }
}

impl fmt::Display for LengthUnit {
    /// Formats the serialization identifier, see [`LengthUnit::to_str()`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.to_str())
    }
}

// Serde: serialized as the identifier, e.g. `"millimeters"`.
crate::utils::serde_string::serde_string!(LengthUnit);

impl FromStr for LengthUnit {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        Self::ALL
            .into_iter()
            .find(|u| u.to_str() == s)
            .ok_or_else(|| Error::InvalidLengthUnit(s.to_owned()))
    }
}

impl ToSExpression for LengthUnit {
    fn to_sexpression(&self) -> SExpression {
        SExpression::token(self.to_str())
    }
}

impl FromSExpression for LengthUnit {
    fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
        Ok(node.value()?.parse()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_roundtrip() {
        for unit in LengthUnit::ALL {
            assert_eq!(unit.to_str().parse::<LengthUnit>().unwrap(), unit);
            assert_eq!(LengthUnit::from_index(unit.index()), Some(unit));
        }
        assert!("foo".parse::<LengthUnit>().is_err());
    }

    #[test]
    fn extract() {
        let mut s = String::from("2*0.5mm");
        assert_eq!(
            LengthUnit::extract_from_expression(&mut s),
            Some(LengthUnit::Millimeters)
        );
        assert_eq!(s, "2*0.5");
        let mut s = String::from("10mils");
        assert_eq!(
            LengthUnit::extract_from_expression(&mut s),
            Some(LengthUnit::Mils)
        );
        assert_eq!(s, "10");
        let mut s = String::from("5μm");
        assert_eq!(
            LengthUnit::extract_from_expression(&mut s),
            Some(LengthUnit::Micrometers)
        );
        assert_eq!(s, "5");
        let mut s = String::from("5");
        assert_eq!(LengthUnit::extract_from_expression(&mut s), None);
    }
}
