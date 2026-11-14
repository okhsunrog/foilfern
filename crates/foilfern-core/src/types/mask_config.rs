//! Port of libs/librepcb/core/types/maskconfig.{h,cpp}.

use super::Length;
use crate::serialization::{self, FromSExpression, SExpression, ToSExpression};

/// How a stop mask or solder paste opening is added automatically (e.g. to a
/// pad), serialized as `off`, `auto` or an offset length.
///
/// Serde: `"Off"`, `"Automatic"` or `{"Manual": <nm>}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum MaskConfig {
    /// No automatic opening.
    Off,
    /// Automatic opening with the offset from the design rules.
    Automatic,
    /// Automatic opening with a manual offset.
    Manual(Length),
}

impl MaskConfig {
    /// Returns `Manual(offset)` if an offset is given, `Off` otherwise
    /// (upstream `MaskConfig::maybe()`).
    pub fn maybe(offset: Option<Length>) -> Self {
        offset.map_or(Self::Off, Self::Manual)
    }

    /// Returns whether an opening is added.
    pub fn is_enabled(self) -> bool {
        self != Self::Off
    }

    /// Returns the manual offset, if any.
    pub fn offset(self) -> Option<Length> {
        match self {
            Self::Manual(offset) => Some(offset),
            _ => None,
        }
    }
}

impl ToSExpression for MaskConfig {
    fn to_sexpression(&self) -> SExpression {
        match self {
            Self::Off => SExpression::token("off"),
            Self::Automatic => SExpression::token("auto"),
            Self::Manual(offset) => offset.to_sexpression(),
        }
    }
}

impl FromSExpression for MaskConfig {
    fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
        match node.value()? {
            "off" => Ok(Self::Off),
            "auto" => Ok(Self::Automatic),
            _ => Ok(Self::Manual(Length::from_sexpression(node)?)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization() {
        for (config, token) in [
            (MaskConfig::Off, "off"),
            (MaskConfig::Automatic, "auto"),
            (MaskConfig::Manual(Length::new(-100_000)), "-0.1"),
        ] {
            assert_eq!(config.to_sexpression(), SExpression::token(token));
            assert_eq!(
                MaskConfig::from_sexpression(&SExpression::token(token)),
                Ok(config)
            );
        }
        assert!(MaskConfig::from_sexpression(&SExpression::token("foo")).is_err());
        assert_eq!(MaskConfig::maybe(None), MaskConfig::Off);
        assert!(MaskConfig::Automatic.is_enabled());
    }
}
