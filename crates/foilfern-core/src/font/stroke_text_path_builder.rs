//! Port of libs/librepcb/core/font/stroketextpathbuilder.{h,cpp}.

use super::StrokeFont;
use crate::geometry::Path;
use crate::types::{Alignment, Angle, Length, PositiveLength, StrokeTextSpacing, UnsignedLength};
use crate::utils::toolbox;

/// Builds the paths of stroke texts.
#[derive(Debug, Clone, Copy)]
pub struct StrokeTextPathBuilder;

impl StrokeTextPathBuilder {
    /// Returns the paths of a stroke text (relative to its position, not
    /// rotated or mirrored). With `auto_rotate`, texts which would be upside
    /// down with the given `rotation` are rotated by 180° around their
    /// center.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        font: &StrokeFont,
        letter_spacing: StrokeTextSpacing,
        line_spacing: StrokeTextSpacing,
        height: PositiveLength,
        stroke_width: UnsignedLength,
        align: Alignment,
        rotation: Angle,
        auto_rotate: bool,
        text: &str,
    ) -> Vec<Path> {
        let stroked = font.stroke(
            text,
            height,
            Self::calc_letter_spacing(font, letter_spacing, height, stroke_width),
            Self::calc_line_spacing(font, line_spacing, height, stroke_width),
            align,
        );
        if auto_rotate && toolbox::is_text_upside_down(rotation) {
            let center = (stroked.bottom_left + stroked.top_right) / 2;
            stroked
                .paths
                .iter()
                .map(|p| p.rotated(Angle::DEG180, center))
                .collect()
        } else {
            stroked.paths
        }
    }

    /// Returns the letter spacing: the given ratio of the height, or the
    /// recommended spacing of the font plus the stroke width (to avoid
    /// overlapping glyphs caused by thick lines).
    pub fn calc_letter_spacing(
        font: &StrokeFont,
        spacing: StrokeTextSpacing,
        height: PositiveLength,
        stroke_width: UnsignedLength,
    ) -> Length {
        match spacing {
            StrokeTextSpacing::Manual(ratio) => scale(height, ratio.to_normalized()),
            StrokeTextSpacing::Auto => {
                scale(height, font.letter_spacing().to_normalized()) + stroke_width
            }
        }
    }

    /// Returns the line spacing: the given ratio of the height, or the
    /// recommended spacing of the font plus the stroke width.
    pub fn calc_line_spacing(
        font: &StrokeFont,
        spacing: StrokeTextSpacing,
        height: PositiveLength,
        stroke_width: UnsignedLength,
    ) -> Length {
        match spacing {
            StrokeTextSpacing::Manual(ratio) => scale(height, ratio.to_normalized()),
            StrokeTextSpacing::Auto => {
                scale(height, font.line_spacing().to_normalized()) + stroke_width
            }
        }
    }
}

/// `Length(height->toNm() * factor)` (truncating like the implicit upstream
/// conversion to int64).
fn scale(height: PositiveLength, factor: f64) -> Length {
    Length::new((height.to_nm() as f64 * factor) as i64)
}
