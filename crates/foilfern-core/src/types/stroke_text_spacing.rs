//! Port of libs/librepcb/core/types/stroketextspacing.{h,cpp}.

use super::Ratio;
use crate::serialization::{self, FromSExpression, SExpression, ToSExpression};

/// Letter or line spacing of a stroke text, serialized as `auto` or a ratio.
///
/// Serde: `"Auto"` or `{"Manual": <ppm>}`.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum StrokeTextSpacing {
    /// Spacing defined by the font (default).
    #[default]
    Auto,
    /// Manual spacing relative to the text height.
    Manual(Ratio),
}

impl StrokeTextSpacing {
    /// Returns the manual ratio, or `None` for automatic spacing.
    pub fn ratio(self) -> Option<Ratio> {
        match self {
            Self::Auto => None,
            Self::Manual(ratio) => Some(ratio),
        }
    }
}

impl From<Option<Ratio>> for StrokeTextSpacing {
    fn from(ratio: Option<Ratio>) -> Self {
        ratio.map_or(Self::Auto, Self::Manual)
    }
}

impl ToSExpression for StrokeTextSpacing {
    fn to_sexpression(&self) -> SExpression {
        match self {
            Self::Auto => SExpression::token("auto"),
            Self::Manual(ratio) => ratio.to_sexpression(),
        }
    }
}

impl FromSExpression for StrokeTextSpacing {
    fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
        match node.value()? {
            "auto" => Ok(Self::Auto),
            _ => Ok(Self::Manual(Ratio::from_sexpression(node)?)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization() {
        for (spacing, token) in [
            (StrokeTextSpacing::Auto, "auto"),
            (StrokeTextSpacing::Manual(Ratio::new(1_500_000)), "1.5"),
        ] {
            assert_eq!(spacing.to_sexpression(), SExpression::token(token));
            assert_eq!(
                StrokeTextSpacing::from_sexpression(&SExpression::token(token)),
                Ok(spacing)
            );
        }
        assert!(StrokeTextSpacing::from_sexpression(&SExpression::token("foo")).is_err());
    }
}
