//! Port of libs/librepcb/core/types/pcbcolor.{h,cpp}.

use std::fmt;
use std::str::FromStr;

use foilfern_i18n::tr;

use super::{Color, Error};
use crate::serialization::{self, FromSExpression, SExpression, ToSExpression};

/// Predefined colors relevant for PCB fabrication (solder resist and
/// silkscreen).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PcbColor {
    /// Black.
    Black,
    /// Black matte.
    BlackMatte,
    /// Blue.
    Blue,
    /// Clear (not available yet for any purpose).
    Clear,
    /// Green.
    Green,
    /// Green matte.
    GreenMatte,
    /// Purple.
    Purple,
    /// Red.
    Red,
    /// White.
    White,
    /// Yellow.
    Yellow,
    /// Any other color.
    Other,
}

impl PcbColor {
    const ALL_UNSORTED: [PcbColor; 10] = [
        Self::Black,
        Self::BlackMatte,
        Self::Blue,
        Self::Clear,
        Self::Green,
        Self::GreenMatte,
        Self::Purple,
        Self::Red,
        Self::White,
        Self::Yellow,
    ];

    /// Returns all colors, sorted by translated name, with [`PcbColor::Other`]
    /// at the end.
    pub fn all() -> Vec<PcbColor> {
        let mut list = Self::ALL_UNSORTED.to_vec();
        list.sort_by_cached_key(|c| c.name_tr());
        list.push(Self::Other);
        list
    }

    /// Returns the color with the given identifier, if it exists (see also
    /// [`FromStr`], which returns an error instead).
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL_UNSORTED
            .into_iter()
            .chain([Self::Other])
            .find(|c| c.id() == id)
    }

    /// Returns the serialization identifier (lower_snake_case).
    pub fn id(self) -> &'static str {
        match self {
            Self::Black => "black",
            Self::BlackMatte => "black_matte",
            Self::Blue => "blue",
            Self::Clear => "clear",
            Self::Green => "green",
            Self::GreenMatte => "green_matte",
            Self::Purple => "purple",
            Self::Red => "red",
            Self::White => "white",
            Self::Yellow => "yellow",
            Self::Other => "other",
        }
    }

    /// Returns the translated, human readable name.
    pub fn name_tr(self) -> String {
        match self {
            Self::Black => tr!("PcbColor", "Black"),
            Self::BlackMatte => tr!("PcbColor", "Black Matte"),
            Self::Blue => tr!("PcbColor", "Blue"),
            Self::Clear => tr!("PcbColor", "Clear"),
            Self::Green => tr!("PcbColor", "Green"),
            Self::GreenMatte => tr!("PcbColor", "Green Matte"),
            Self::Purple => tr!("PcbColor", "Purple"),
            Self::Red => tr!("PcbColor", "Red"),
            Self::White => tr!("PcbColor", "White"),
            Self::Yellow => tr!("PcbColor", "Yellow"),
            Self::Other => tr!("PcbColor", "Other"),
        }
    }

    /// Returns whether the color is available for solder resist.
    pub fn is_available_for_solder_resist(self) -> bool {
        self != Self::Clear
    }

    /// Returns whether the color is available for silkscreen.
    pub fn is_available_for_silkscreen(self) -> bool {
        matches!(
            self,
            Self::Black | Self::Blue | Self::Red | Self::White | Self::Yellow | Self::Other
        )
    }

    fn colors(self) -> (Option<Color>, Option<Color>) {
        let (solder_resist, silkscreen) = match self {
            Self::Black => (Color::rgba(0, 0, 0, 210), Color::rgb(40, 40, 40)),
            Self::BlackMatte => (Color::rgba(0, 0, 0, 210), Color::BLACK),
            Self::Blue => (Color::rgba(0, 20, 100, 220), Color::rgb(0, 0, 110)),
            Self::Clear => (Color::rgba(50, 50, 50, 50), Color::WHITE),
            Self::Green => (Color::rgba(0, 50, 0, 180), Color::GREEN),
            Self::GreenMatte => (Color::rgba(0, 50, 0, 180), Color::GREEN),
            Self::Purple => (Color::rgba(80, 0, 130, 180), Color::rgb(100, 0, 160)),
            Self::Red => (Color::rgba(160, 0, 0, 180), Color::rgb(140, 0, 0)),
            Self::White => (Color::rgba(220, 220, 220, 210), Color::WHITE),
            Self::Yellow => (Color::rgba(220, 220, 0, 160), Color::rgb(210, 210, 0)),
            Self::Other => return (None, None),
        };
        (Some(solder_resist), Some(silkscreen))
    }

    /// Returns the color for solder resist rendering (the green solder resist
    /// color for [`PcbColor::Other`]).
    pub fn to_solder_resist_color(self) -> Color {
        self.colors()
            .0
            .unwrap_or_else(|| Self::Green.to_solder_resist_color())
    }

    /// Returns the color for silkscreen rendering (the white silkscreen color
    /// for [`PcbColor::Other`]).
    pub fn to_silkscreen_color(self) -> Color {
        self.colors()
            .1
            .unwrap_or_else(|| Self::White.to_silkscreen_color())
    }
}

impl fmt::Display for PcbColor {
    /// Formats the serialization identifier.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for PcbColor {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        Self::from_id(s).ok_or_else(|| Error::UnknownPcbColor(s.to_owned()))
    }
}

impl ToSExpression for PcbColor {
    fn to_sexpression(&self) -> SExpression {
        SExpression::token(self.id())
    }
}

// Serde: the identifier (e.g. `"green"`).
crate::utils::serde_string::serde_string!(PcbColor);

impl FromSExpression for PcbColor {
    fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
        Ok(node.value()?.parse()?)
    }
}

impl ToSExpression for Option<PcbColor> {
    /// Serializes `None` as token `none`.
    fn to_sexpression(&self) -> SExpression {
        match self {
            Some(color) => color.to_sexpression(),
            None => SExpression::token("none"),
        }
    }
}

impl FromSExpression for Option<PcbColor> {
    fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
        match node.value()? {
            "none" => Ok(None),
            id => Ok(Some(id.parse()?)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_sorted_by_name() {
        let ids: Vec<_> = PcbColor::all().into_iter().map(PcbColor::id).collect();
        assert_eq!(
            ids,
            [
                "black",
                "black_matte",
                "blue",
                "clear",
                "green",
                "green_matte",
                "purple",
                "red",
                "white",
                "yellow",
                "other"
            ]
        );
    }

    #[test]
    fn serialization() {
        assert_eq!(
            PcbColor::GreenMatte.to_sexpression(),
            SExpression::token("green_matte")
        );
        assert_eq!(
            None::<PcbColor>.to_sexpression(),
            SExpression::token("none")
        );
        assert_eq!(
            Option::<PcbColor>::from_sexpression(&SExpression::token("none")),
            Ok(None)
        );
        assert_eq!(
            PcbColor::from_sexpression(&SExpression::token("red")),
            Ok(PcbColor::Red)
        );
        assert!(PcbColor::from_sexpression(&SExpression::token("none")).is_err());
    }

    #[test]
    fn fallback_colors() {
        assert_eq!(
            PcbColor::Other.to_solder_resist_color(),
            PcbColor::Green.to_solder_resist_color()
        );
        assert_eq!(PcbColor::Other.to_silkscreen_color(), Color::WHITE);
    }
}
