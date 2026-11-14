//! Port of libs/librepcb/core/font/strokefont.{h,cpp}.
//!
//! Upstream parses the font in a background thread when it is constructed
//! and blocks on first use. Here the font is parsed lazily on first use
//! (thread-safe); callers may trigger this early with
//! [`StrokeFont::load()`] on a thread of their choice. Like upstream, a font
//! which fails to load behaves like an empty font (see
//! [`StrokeFont::load_error()`]).
//!
//! Texts are processed per `char` (upstream: per UTF-16 code unit), see
//! COMPAT.md.

use std::sync::OnceLock;

use super::fontobene::{self, GlyphListAccessor, Polyline};
use crate::geometry::{Path, Vertex};
use crate::types::{Alignment, Angle, HAlign, Length, Point, PositiveLength, Ratio, VAlign};
use crate::utils::painter_path;

/// The stroked lines of a text, see [`StrokeFont::stroke()`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StrokedText {
    /// The glyph paths.
    pub paths: Vec<Path>,
    /// Bottom left corner of the text bounding box.
    pub bottom_left: Point,
    /// Top right corner of the text bounding box.
    pub top_right: Point,
}

/// A stroked line of text, see [`StrokeFont::stroke_lines()`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StrokedLine {
    /// The glyph paths (left aligned at X = 0, baseline at Y = 0).
    pub paths: Vec<Path>,
    /// The width of the line.
    pub width: Length,
}

#[derive(Debug)]
struct Loaded {
    font: fontobene::Font,
    accessor: GlyphListAccessor,
    error: Option<fontobene::Error>,
}

/// A stroke font (FontoBene format) to render stroke texts.
#[derive(Debug)]
pub struct StrokeFont {
    content: Vec<u8>,
    loaded: OnceLock<Loaded>,
}

// The lazy loading must stay thread-safe (`OnceLock`, not `OnceCell`).
static_assertions::assert_impl_all!(StrokeFont: Send, Sync);

impl StrokeFont {
    /// Creates the font from the content of a `.bene` file. Parsing is done
    /// lazily.
    pub fn new(content: impl Into<Vec<u8>>) -> Self {
        Self {
            content: content.into(),
            loaded: OnceLock::new(),
        }
    }

    /// Parses the font now (if not done yet).
    pub fn load(&self) {
        self.loaded();
    }

    /// Returns the error if the font failed to load (it is then empty).
    pub fn load_error(&self) -> Option<&fontobene::Error> {
        self.loaded().error.as_ref()
    }

    /// Returns the parsed font (empty if loading failed).
    pub fn font(&self) -> &fontobene::Font {
        &self.loaded().font
    }

    fn loaded(&self) -> &Loaded {
        self.loaded.get_or_init(|| {
            let content = String::from_utf8_lossy(&self.content);
            let (font, error) = match fontobene::Font::parse(&content) {
                Ok(font) => (font, None),
                Err(e) => (fontobene::Font::default(), Some(e)),
            };
            let mut accessor = GlyphListAccessor::new(&font);
            accessor.set_replacement_glyph(0xFFFD); // U+FFFD REPLACEMENT CHARACTER
            // MICRO SIGN <-> GREEK SMALL LETTER MU
            accessor.add_replacements(&[0x00B5, 0x03BC]);
            // OHM SIGN <-> GREEK CAPITAL LETTER OMEGA
            accessor.add_replacements(&[0x2126, 0x03A9]);
            Loaded {
                font,
                accessor,
                error,
            }
        })
    }

    /// Returns the recommended letter spacing relative to the glyph height.
    pub fn letter_spacing(&self) -> Ratio {
        Ratio::from_normalized(self.font().header.letter_spacing / 9.0)
    }

    /// Returns the recommended line spacing relative to the glyph height.
    pub fn line_spacing(&self) -> Ratio {
        Ratio::from_normalized(self.font().header.line_spacing / 9.0)
    }

    /// Strokes a (multi-line) text with the given alignment. Returns the
    /// paths and the bounding box.
    pub fn stroke(
        &self,
        text: &str,
        height: PositiveLength,
        letter_spacing: Length,
        line_spacing: Length,
        align: Alignment,
    ) -> StrokedText {
        let (lines, total_width) = self.stroke_lines(text, height, letter_spacing);
        let count = lines.len() as i64;
        let total_height = height + line_spacing * (count - 1);
        let mut paths = Vec::new();
        for (i, line) in (0i64..).zip(&lines) {
            let x = match align.h {
                HAlign::Left => Length::ZERO,
                HAlign::Right => (total_width - line.width) - total_width,
                HAlign::Center => line.width / -2,
            };
            let y = match align.v {
                VAlign::Bottom => line_spacing * (count - i - 1),
                VAlign::Top => -height - line_spacing * i,
                VAlign::Center => line_spacing * (count - i - 1) - (total_height / 2),
            };
            let pos = Point::new(x, y);
            paths.extend(line.paths.iter().map(|p| p.translated(pos)));
        }
        let (left, right) = match align.h {
            HAlign::Left => (Length::ZERO, total_width),
            HAlign::Right => (-total_width, Length::ZERO),
            HAlign::Center => (-total_width / 2, total_width / 2),
        };
        let (bottom, top) = match align.v {
            VAlign::Bottom => (Length::ZERO, total_height),
            VAlign::Top => (-total_height, Length::ZERO),
            VAlign::Center => (-total_height / 2, total_height / 2),
        };
        StrokedText {
            paths,
            bottom_left: Point::new(left, bottom),
            top_right: Point::new(right, top),
        }
    }

    /// Strokes each line of a text (separated by `\n`). Returns the lines
    /// and the maximum line width.
    pub fn stroke_lines(
        &self,
        text: &str,
        height: PositiveLength,
        letter_spacing: Length,
    ) -> (Vec<StrokedLine>, Length) {
        let mut width = Length::ZERO;
        let lines = text
            .split('\n')
            .map(|line| {
                let line = self.stroke_line(line, height, letter_spacing);
                width = width.max(line.width);
                line
            })
            .collect();
        (lines, width)
    }

    /// Strokes a single line of text (left aligned).
    pub fn stroke_line(
        &self,
        text: &str,
        height: PositiveLength,
        letter_spacing: Length,
    ) -> StrokedLine {
        let mut paths = Vec::new();
        let mut offset = Length::ZERO;
        let mut width = Length::ZERO; // same as offset, but without last letter spacing
        for (i, character) in text.chars().enumerate() {
            let (glyph_paths, glyph_spacing) = self.stroke_glyph(character, height);
            if !glyph_paths.is_empty() {
                let (bottom_left, top_right) = compute_bounding_rect(&glyph_paths);
                // Left-align first character.
                let shift = if i == 0 { -bottom_left.x } else { Length::ZERO };
                let translation = Point::new(offset + shift, Length::ZERO);
                paths.extend(glyph_paths.iter().map(|p| p.translated(translation)));
                width = offset + top_right.x + shift; // do *not* count glyph spacing as width!
                offset = width + glyph_spacing + letter_spacing;
            } else if glyph_spacing != Length::ZERO {
                // It's a whitespace-only glyph -> count additional glyph
                // spacing as width.
                width = offset + glyph_spacing;
                offset = width + letter_spacing;
            }
        }
        StrokedLine { paths, width }
    }

    /// Strokes the glyph of a single character. Returns the paths and the
    /// glyph spacing, or nothing if the glyph (and the replacement glyph)
    /// does not exist.
    pub fn stroke_glyph(&self, character: char, height: PositiveLength) -> (Vec<Path>, Length) {
        let loaded = self.loaded();
        match loaded
            .accessor
            .all_polylines_of_glyph(&loaded.font, character)
        {
            Ok((polylines, spacing)) => (
                polylines_to_paths(&polylines, height),
                convert_length(height, spacing),
            ),
            // Upstream prints a warning.
            Err(_) => (Vec::new(), Length::ZERO),
        }
    }
}

fn polylines_to_paths(polylines: &[Polyline], height: PositiveLength) -> Vec<Path> {
    polylines
        .iter()
        .filter(|p| !p.is_empty())
        .map(|p| p.iter().map(|v| convert_vertex(v, height)).collect())
        .collect()
}

fn convert_vertex(v: &fontobene::Vertex, height: PositiveLength) -> Vertex {
    let height_mm = height.to_mm();
    // Out of range values are only possible with absurd font files.
    Vertex::new(
        Point::from_mm(v.scaled_x(height_mm), v.scaled_y(height_mm)).unwrap_or_default(),
        Angle::from_deg(v.scaled_bulge(180.0)).unwrap_or_default(),
    )
}

fn convert_length(height: PositiveLength, length: f64) -> Length {
    // Truncating conversion like the implicit upstream conversion to int64.
    Length::new((height.to_nm() as f64 * length / 9.0) as i64)
}

/// Returns the bottom left and top right corner of the paths like upstream
/// (bounding rectangle of the `QPainterPath`, rounded to nanometers).
fn compute_bounding_rect(paths: &[Path]) -> (Point, Point) {
    let rect = painter_path::bounding_rect_px(paths);
    (
        Point::from_px(rect.left(), rect.bottom()).unwrap_or_default(),
        Point::from_px(rect.right(), rect.top()).unwrap_or_default(),
    )
}
