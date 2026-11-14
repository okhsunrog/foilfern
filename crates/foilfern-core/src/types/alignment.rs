//! Port of libs/librepcb/core/types/alignment.{h,cpp}.
//!
//! Not ported: the conversions from/to `Qt::Alignment` (UI specific).

use std::fmt;
use std::str::FromStr;

use super::Error;
use crate::serialization::{
    self, DeserializeObject, FromSExpression, List, SExpression, SerializeObject, ToSExpression,
};

/// Horizontal alignment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum HAlign {
    /// Left (default).
    #[default]
    Left,
    /// Center.
    Center,
    /// Right.
    Right,
}

impl HAlign {
    /// Returns the alignment mirrored at a vertical axis (left ↔ right).
    pub fn mirrored(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Center => Self::Center,
            Self::Right => Self::Left,
        }
    }

    /// Returns the serialization token.
    pub fn to_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
        }
    }
}

/// Vertical alignment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum VAlign {
    /// Top (default).
    #[default]
    Top,
    /// Center.
    Center,
    /// Bottom.
    Bottom,
}

impl VAlign {
    /// Returns the alignment mirrored at a horizontal axis (top ↔ bottom).
    pub fn mirrored(self) -> Self {
        match self {
            Self::Top => Self::Bottom,
            Self::Center => Self::Center,
            Self::Bottom => Self::Top,
        }
    }

    /// Returns the serialization token.
    pub fn to_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Center => "center",
            Self::Bottom => "bottom",
        }
    }
}

macro_rules! impl_align_traits {
    ($name:ident, $err:path, [$($variant:ident),*]) => {
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.to_str())
            }
        }

        impl FromStr for $name {
            type Err = Error;
            fn from_str(s: &str) -> Result<Self, Error> {
                [$(Self::$variant),*]
                    .into_iter()
                    .find(|a| a.to_str() == s)
                    .ok_or_else(|| $err(s.to_owned()))
            }
        }

        impl ToSExpression for $name {
            fn to_sexpression(&self) -> SExpression {
                SExpression::token(self.to_str())
            }
        }

        impl FromSExpression for $name {
            fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
                Ok(node.value()?.parse()?)
            }
        }
    };
}

impl_align_traits!(HAlign, Error::InvalidHAlign, [Left, Center, Right]);
impl_align_traits!(VAlign, Error::InvalidVAlign, [Top, Center, Bottom]);
crate::utils::serde_string::serde_string!(HAlign);
crate::utils::serde_string::serde_string!(VAlign);

/// A horizontal and vertical alignment, serialized as e.g.
/// `(align left bottom)`.
///
/// Serde: `{"h": "left", "v": "bottom"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Alignment {
    /// Horizontal alignment.
    pub h: HAlign,
    /// Vertical alignment.
    pub v: VAlign,
}

impl Alignment {
    /// Creates an alignment.
    pub const fn new(h: HAlign, v: VAlign) -> Self {
        Self { h, v }
    }

    /// Returns the alignment mirrored in both directions.
    pub fn mirrored(self) -> Self {
        Self::new(self.h.mirrored(), self.v.mirrored())
    }

    /// Returns the alignment with mirrored horizontal alignment.
    pub fn mirrored_h(self) -> Self {
        Self::new(self.h.mirrored(), self.v)
    }

    /// Returns the alignment with mirrored vertical alignment.
    pub fn mirrored_v(self) -> Self {
        Self::new(self.h, self.v.mirrored())
    }
}

impl Default for Alignment {
    /// Left bottom.
    fn default() -> Self {
        Self::new(HAlign::Left, VAlign::Bottom)
    }
}

impl SerializeObject for Alignment {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.h);
        root.append_value(&self.v);
    }
}

impl DeserializeObject for Alignment {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self::new(node.child_value("@0")?, node.child_value("@1")?))
    }
}
