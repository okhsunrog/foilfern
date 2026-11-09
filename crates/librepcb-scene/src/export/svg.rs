//! SVG backend of the graphics export (upstream uses `QSvgGenerator`):
//! the page size in millimeters, a view box in pixels of the export
//! resolution, and one `<path>` per paint. Hand-written because the output
//! is a handful of elements (no crate needed for that).

use std::fmt::Write as _;

use librepcb_canvas::kurbo::{Cap, Join};
use librepcb_canvas::peniko::Color;

use super::{ExportPage, PageLayout, PrimitivePaint, kurbo_stroke};

/// Creates the SVG document of a page.
pub(crate) fn write(page: &ExportPage, layout: &PageLayout, dpi: f64, title: &str) -> String {
    let (w, h) = (layout.size.width, layout.size.height);
    let mm = |px: f64| (px / dpi * 25.4 * 1e4).round() / 1e4;
    let mut s = String::new();
    // Writing into a String cannot fail.
    let _ = writeln!(
        s,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>"#
    );
    let _ = writeln!(
        s,
        r#"<svg width="{}mm" height="{}mm" viewBox="0 0 {w} {h}" xmlns="http://www.w3.org/2000/svg" version="1.2" baseProfile="tiny">"#,
        mm(w),
        mm(h),
    );
    let _ = writeln!(s, "<title>{}</title>", escape(title));
    let _ = writeln!(s, "<desc>Generated with LibrePCB</desc>");
    if layout.background.components[3] > 0.0 {
        let _ = writeln!(
            s,
            r#"<rect x="0" y="0" width="{w}" height="{h}" fill="{}" fill-opacity="{}" stroke="none"/>"#,
            hex(layout.background),
            opacity(layout.background)
        );
    }
    let [a, b, c, d, e, f] = layout.transform.as_coeffs();
    let _ = writeln!(s, r#"<g transform="matrix({a} {b} {c} {d} {e} {f})">"#);
    for primitive in page.drawing.primitives() {
        let data = primitive.path.to_svg();
        match &primitive.paint {
            PrimitivePaint::Fill(color) => {
                let _ = writeln!(
                    s,
                    r#"<path d="{data}" fill="{}" fill-opacity="{}" fill-rule="nonzero" stroke="none"/>"#,
                    hex(*color),
                    opacity(*color)
                );
            }
            PrimitivePaint::Stroke(color, style) => {
                let stroke = kurbo_stroke(style, layout.min_line_width);
                let cap = match stroke.start_cap {
                    Cap::Butt => "butt",
                    Cap::Round => "round",
                    Cap::Square => "square",
                };
                let join = match stroke.join {
                    Join::Bevel => "bevel",
                    Join::Miter => "miter",
                    Join::Round => "round",
                };
                let _ = write!(
                    s,
                    r#"<path d="{data}" fill="none" stroke="{}" stroke-opacity="{}" stroke-width="{}" stroke-linecap="{cap}" stroke-linejoin="{join}""#,
                    hex(*color),
                    opacity(*color),
                    stroke.width
                );
                if !stroke.dash_pattern.is_empty() {
                    let dashes: Vec<String> =
                        stroke.dash_pattern.iter().map(|d| d.to_string()).collect();
                    let _ = write!(
                        s,
                        r#" stroke-dasharray="{}" stroke-dashoffset="{}""#,
                        dashes.join(","),
                        stroke.dash_offset
                    );
                }
                let _ = writeln!(s, "/>");
            }
        }
    }
    let _ = writeln!(s, "</g>");
    let _ = writeln!(s, "</svg>");
    s
}

fn hex(c: Color) -> String {
    let c = c.to_rgba8();
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

fn opacity(c: Color) -> String {
    let a = f64::from(c.to_rgba8().a) / 255.0;
    format!("{}", (a * 1000.0).round() / 1000.0)
}

/// Escapes text content for XML.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{Drawing, layout};
    use librepcb_canvas::kurbo::{Rect, Shape};
    use librepcb_core::export::GraphicsExportSettings;

    #[test]
    fn document() {
        let mut drawing = Drawing::new();
        drawing.push(
            Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-3),
            PrimitivePaint::Fill(Color::from_rgba8(255, 0, 0, 128)),
        );
        let page = ExportPage {
            drawing,
            settings: GraphicsExportSettings::default(),
        };
        let l = layout(&page, 600.0);
        let svg = write(&page, &l, 600.0, "A & B");
        assert!(svg.contains("<title>A &amp; B</title>"), "{svg}");
        assert!(
            svg.contains(r##"fill="#ff0000" fill-opacity="0.502""##),
            "{svg}"
        );
        assert!(svg.contains(r#"viewBox="0 0 708 708""#), "{svg}");
    }
}
