//! Text rendering of the scene builders: the texts upstream draws with
//! TrueType fonts (`GraphicsPainter::drawText()` and `drawNetLabel()` of
//! libs/librepcb/core/export/graphicspainter.{h,cpp}) and stroke texts.
//!
//! Upstream draws schematic texts, pin names/numbers and net labels with
//! the application's sans-serif (Noto Sans) and monospace (Noto Sans Mono)
//! TrueType fonts through Qt. This crate has no TrueType rasterizer, so the
//! texts are drawn with the project's stroke font instead, laid out to
//! occupy the same box as upstream's text: a text of height `h` is a Qt
//! font whose line height is `h`, so its baseline is `DESCENT * h` above the
//! bottom of its line and its capitals are `CAP_HEIGHT * h` high (Noto Sans
//! metrics). Alignment, auto-rotation, multi-line layout and overlines
//! (`!` markup) follow upstream. Rendering only; no file is affected.

use std::collections::HashMap;

use librepcb_canvas::convert;
use librepcb_canvas::kurbo::{Affine, BezPath, Line, Rect, Shape};
use librepcb_core::font::StrokeFont;
use librepcb_core::types::{Alignment, Angle, HAlign, Length, Point, PositiveLength, VAlign};
use librepcb_core::utils::overline_markup_parser::extract_overlines;
use librepcb_core::utils::toolbox;

/// Descent of Noto Sans relative to its line height (293 / 1362).
const DESCENT: f64 = 293.0 / 1362.0;
/// Cap height of Noto Sans relative to its line height (714 / 1362).
const CAP_HEIGHT: f64 = 714.0 / 1362.0;
/// Stroke width relative to the line height (about the stem width of Noto
/// Sans Regular).
const STROKE_WIDTH: f64 = 0.07;

/// Millimetres per Qt pixel (1/72 inch).
const MM_PER_PX: f64 = 25.4 / 72.0;

/// Line height of upstream's net label font (Noto Sans Mono with a pixel
/// size of 4, not scaled), in millimetres.
pub(crate) const NET_LABEL_HEIGHT: f64 = 4.0 * MM_PER_PX * 1362.0 / 1000.0;

/// A glyph at a cap height of 1 mm.
#[derive(Debug, Clone)]
struct Glyph {
    path: BezPath,
    bbox: Option<Rect>,
    spacing: f64,
}

/// How a TrueType-like text is placed (upstream `drawText()` arguments).
#[derive(Debug, Clone, Copy)]
pub(crate) struct TextStyle {
    /// Line height in millimetres.
    pub height: f64,
    pub align: Alignment,
    pub rotation: Angle,
    pub auto_rotate: bool,
    pub mirror_in_place: bool,
    pub parse_overlines: bool,
}

/// A laid out text: the strokes in world coordinates and their width.
#[derive(Debug, Clone)]
pub(crate) struct RenderedText {
    pub path: BezPath,
    pub stroke_width: f64,
}

/// Lays out texts with a stroke font, caching the glyphs.
pub(crate) struct TextRenderer<'a> {
    font: Option<&'a StrokeFont>,
    glyphs: HashMap<char, Option<Glyph>>,
}

impl<'a> TextRenderer<'a> {
    pub fn new(font: Option<&'a StrokeFont>) -> Self {
        Self {
            font,
            glyphs: HashMap::new(),
        }
    }

    fn glyph(&mut self, c: char) -> Option<&Glyph> {
        let font = self.font?;
        self.glyphs
            .entry(c)
            .or_insert_with(|| {
                // 1 mm is a valid positive length.
                let unit = PositiveLength::new(Length::new(1_000_000)).ok()?;
                let (paths, spacing) = font.stroke_glyph(c, unit);
                let path = convert::paths(&paths);
                let bbox = (!path.elements().is_empty()).then(|| path.bounding_box());
                Some(Glyph {
                    path,
                    bbox,
                    spacing: spacing.to_mm(),
                })
            })
            .as_ref()
    }

    /// Lays out one line at cap height `cap` (mm) with the baseline at y = 0
    /// and the first glyph at x = 0 (like `StrokeFont::stroke_line()`).
    /// Returns the path, the width and the x range of every character.
    fn layout_line(&mut self, text: &str, cap: f64) -> (BezPath, f64, Vec<(f64, f64)>) {
        let letter_spacing = self
            .font
            .map_or(0.0, |f| f.letter_spacing().to_normalized() * cap);
        let mut path = BezPath::new();
        let mut ranges = Vec::new();
        let mut offset = 0.0;
        let mut width = 0.0;
        for (i, c) in text.chars().enumerate() {
            let Some(glyph) = self.glyph(c).cloned() else {
                ranges.push((offset, offset));
                continue;
            };
            let spacing = glyph.spacing * cap;
            if let Some(bbox) = glyph.bbox {
                let shift = if i == 0 { -bbox.x0 * cap } else { 0.0 };
                let x = offset + shift;
                let xf = Affine::translate((x, 0.0)) * Affine::scale(cap);
                path.extend((xf * glyph.path.clone()).elements().iter().copied());
                ranges.push((x + bbox.x0 * cap, x + bbox.x1 * cap));
                width = x + bbox.x1 * cap;
                offset = width + spacing + letter_spacing;
            } else if spacing != 0.0 {
                ranges.push((offset, offset + spacing));
                width = offset + spacing;
                offset = width + letter_spacing;
            } else {
                ranges.push((offset, offset));
            }
        }
        (path, width, ranges)
    }

    /// Lays out a text like upstream `GraphicsPainter::drawText()` at
    /// `position`. Returns `None` for blank texts or without font.
    pub fn text(&mut self, text: &str, position: Point, style: TextStyle) -> Option<RenderedText> {
        if text.trim().is_empty()
            || self.font.is_none()
            || !style.height.is_finite()
            || style.height <= 0.0
        {
            return None;
        }
        let h = style.height;
        let cap = h * CAP_HEIGHT;
        let rotate180 = style.auto_rotate && toolbox::is_text_upside_down(style.rotation);
        let mut align = if rotate180 {
            style.align.mirrored()
        } else {
            style.align
        };
        if style.mirror_in_place {
            align = align.mirrored_h();
        }
        let lines: Vec<&str> = text.split('\n').collect();
        let n = lines.len() as f64;
        let rect_top = match align.v {
            VAlign::Bottom => n * h,
            VAlign::Center => n * h / 2.0,
            VAlign::Top => 0.0,
        };
        let overline_width = h / 15.0;
        let mut local = BezPath::new();
        for (i, line) in lines.iter().enumerate() {
            let (rendered, spans) = if style.parse_overlines {
                extract_overlines(line)
            } else {
                ((*line).to_owned(), Vec::new())
            };
            let (path, width, ranges) = self.layout_line(&rendered, cap);
            let x = match align.h {
                HAlign::Left => 0.0,
                HAlign::Center => -width / 2.0,
                HAlign::Right => -width,
            };
            let baseline = rect_top - (i as f64 + 1.0) * h + DESCENT * h;
            local.extend(
                (Affine::translate((x, baseline)) * path)
                    .elements()
                    .iter()
                    .copied(),
            );
            // Overlines (byte ranges of the rendered text → char ranges).
            for span in spans {
                let first = rendered[..span.start].chars().count();
                let count = rendered[span.clone()].chars().count();
                let (Some(start), Some(end)) = (
                    ranges.get(first).map(|r| r.0),
                    ranges.get(first + count.saturating_sub(1)).map(|r| r.1),
                ) else {
                    continue;
                };
                if count == 0 {
                    continue;
                }
                let y = baseline + cap + overline_width * 2.0;
                let l = Line::new((x + start, y), (x + end, y));
                local.move_to(l.p0);
                local.line_to(l.p1);
            }
        }
        let mut xf = Affine::translate(convert::point(position).to_vec2())
            * Affine::rotate(convert::angle(style.rotation))
            * if rotate180 {
                Affine::rotate(std::f64::consts::PI)
            } else {
                Affine::IDENTITY
            };
        if style.mirror_in_place {
            xf *= Affine::FLIP_X;
        }
        Some(RenderedText {
            path: xf * local,
            stroke_width: h * STROKE_WIDTH,
        })
    }

    /// Lays out a net or bus label like upstream
    /// `GraphicsPainter::drawNetLabel()`.
    pub fn net_label(
        &mut self,
        text: &str,
        position: Point,
        rotation: Angle,
        mirrored: bool,
    ) -> Option<RenderedText> {
        let align = Alignment::new(
            if mirrored {
                HAlign::Right
            } else {
                HAlign::Left
            },
            VAlign::Bottom,
        );
        self.text(
            text,
            position,
            TextStyle {
                height: NET_LABEL_HEIGHT,
                align,
                rotation,
                auto_rotate: true,
                mirror_in_place: false,
                parse_overlines: true,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> StrokeFont {
        let path = std::path::Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
            .join("share/librepcb/fontobene/newstroke.bene");
        StrokeFont::new(std::fs::read(path).expect("font"))
    }

    fn style(align: Alignment) -> TextStyle {
        TextStyle {
            height: 2.0,
            align,
            rotation: Angle::DEG0,
            auto_rotate: true,
            mirror_in_place: false,
            parse_overlines: true,
        }
    }

    #[test]
    fn alignment_box() {
        let font = font();
        let mut r = TextRenderer::new(Some(&font));
        let t = r
            .text(
                "HELLO",
                Point::ORIGIN,
                style(Alignment::new(HAlign::Left, VAlign::Bottom)),
            )
            .unwrap();
        let b = t.path.bounding_box();
        // Baseline above the bottom by the descent, capitals CAP_HEIGHT high.
        assert!((b.y0 - 2.0 * DESCENT).abs() < 0.05, "{b:?}");
        assert!((b.y1 - 2.0 * (DESCENT + CAP_HEIGHT)).abs() < 0.05, "{b:?}");
        assert!(b.x0.abs() < 0.05 && b.x1 > 3.0, "{b:?}");

        let t = r
            .text(
                "HELLO",
                Point::ORIGIN,
                style(Alignment::new(HAlign::Right, VAlign::Top)),
            )
            .unwrap();
        let b = t.path.bounding_box();
        assert!(b.x1.abs() < 0.05 && b.y1 < 0.0, "{b:?}");
        assert!(
            r.text("  ", Point::ORIGIN, style(Alignment::default()))
                .is_none()
        );
    }

    #[test]
    fn upside_down_is_rotated() {
        let font = font();
        let mut r = TextRenderer::new(Some(&font));
        let mut s = style(Alignment::new(HAlign::Left, VAlign::Bottom));
        s.rotation = Angle::DEG180;
        let b = r.text("AB", Point::ORIGIN, s).unwrap().path.bounding_box();
        // Auto-rotated: still reads left to right, extends to the left.
        assert!(b.x1 < 0.05 && b.x0 < -1.0, "{b:?}");
    }

    #[test]
    fn overlines() {
        let font = font();
        let mut r = TextRenderer::new(Some(&font));
        let s = style(Alignment::default());
        let plain = r.text("RST", Point::ORIGIN, s).unwrap().path;
        let over = r.text("!RST", Point::ORIGIN, s).unwrap().path;
        assert!(over.bounding_box().y1 > plain.bounding_box().y1);
        assert!((over.bounding_box().x1 - plain.bounding_box().x1).abs() < 1e-9);
    }
}
