//! RGBA color, replacing `QColor` where upstream stores colors in files
//! (`serialize<QColor>()`/`deserialize<QColor>()` in
//! libs/librepcb/core/serialization/sexpression.cpp).
//!
//! Parsing supports the hexadecimal forms `#RGB`, `#RRGGBB` and `#AARRGGBB`
//! (as written by LibrePCB), plus `transparent`. Unlike `QColor`, SVG color
//! names and the 12/16 bit hex forms are not supported.

use std::fmt;
use std::str::FromStr;

use super::Error;

/// An 8 bit per channel RGBA color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
    /// Alpha (255 = opaque).
    pub a: u8,
}

impl Color {
    /// Opaque black (`Qt::black`).
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    /// Opaque white (`Qt::white`).
    pub const WHITE: Self = Self::rgb(255, 255, 255);
    /// Opaque green (`Qt::green`).
    pub const GREEN: Self = Self::rgb(0, 255, 0);
    /// Fully transparent black (`Qt::transparent`).
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0);

    /// Creates an opaque color.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::rgba(r, g, b, 255)
    }

    /// Creates a color with alpha channel.
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}

impl fmt::Display for Color {
    /// Formats as `#aarrggbb` (like `QColor::name(QColor::HexArgb)`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "#{:02x}{:02x}{:02x}{:02x}",
            self.a, self.r, self.g, self.b
        )
    }
}

impl FromStr for Color {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        let invalid = || Error::InvalidColor(s.to_owned());
        if s.eq_ignore_ascii_case("transparent") {
            return Ok(Self::rgba(0, 0, 0, 0));
        }
        let hex = s.strip_prefix('#').ok_or_else(invalid)?;
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid());
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| invalid());
        let nibble = |i: usize| {
            u8::from_str_radix(&hex[i..i + 1], 16)
                .map(|v| v * 17)
                .map_err(|_| invalid())
        };
        match hex.len() {
            3 => Ok(Self::rgb(nibble(0)?, nibble(1)?, nibble(2)?)),
            6 => Ok(Self::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Ok(Self::rgba(byte(2)?, byte(4)?, byte(6)?, byte(0)?)),
            _ => Err(invalid()),
        }
    }
}

// Serde: the `#aarrggbb` string.
crate::utils::serde_string::serde_string!(Color);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_format() {
        assert_eq!("#f00".parse::<Color>().unwrap(), Color::rgb(255, 0, 0));
        assert_eq!("#00FF00".parse::<Color>().unwrap(), Color::GREEN);
        assert_eq!(
            "#7f0000ff".parse::<Color>().unwrap(),
            Color::rgba(0, 0, 255, 127)
        );
        assert_eq!(Color::rgba(1, 2, 3, 4).to_string(), "#04010203");
        assert!("".parse::<Color>().is_err());
        assert!("#12345".parse::<Color>().is_err());
        assert!("#gg0000".parse::<Color>().is_err());
    }
}
