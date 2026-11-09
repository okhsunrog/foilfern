//! PDF backend of the graphics export (upstream uses `QPdfWriter`): one
//! page per [`ExportPage`], vector paths in the page's coordinate system,
//! transparency through extended graphics states. Written with the
//! `pdf-writer` crate.

use std::collections::BTreeSet;

use librepcb_canvas::kurbo::{Affine, BezPath, Cap, Join, PathEl};
use librepcb_canvas::peniko::Color;
use pdf_writer::types::{LineCapStyle, LineJoinStyle};
use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, TextStr};

use super::{ExportPage, PrimitivePaint, kurbo_stroke, layout};

/// Points per inch.
const DPI: f64 = 72.0;

/// Writes all pages into one PDF document.
pub(crate) fn write(pages: &[ExportPage], creator: &str, title: &str) -> Vec<u8> {
    let mut alloc = Ref::new(1);
    let mut next = || alloc.bump();
    let catalog_id = next();
    let tree_id = next();
    let info_id = next();
    let page_ids: Vec<(Ref, Ref)> = pages.iter().map(|_| (next(), next())).collect();

    let mut pdf = Pdf::new();
    pdf.catalog(catalog_id).pages(tree_id);
    pdf.pages(tree_id)
        .kids(page_ids.iter().map(|(p, _)| *p))
        .count(pages.len() as i32);
    let mut info = pdf.document_info(info_id);
    info.title(TextStr(title));
    info.creator(TextStr(creator));
    info.producer(TextStr(creator));
    info.finish();

    for (page, (page_id, content_id)) in pages.iter().zip(&page_ids) {
        let layout = layout(page, DPI);
        let (w, h) = (layout.size.width, layout.size.height);

        // Transparency levels used on this page.
        let mut alphas: BTreeSet<u8> = page
            .drawing
            .primitives()
            .iter()
            .map(|p| match &p.paint {
                PrimitivePaint::Fill(c) | PrimitivePaint::Stroke(c, _) => alpha(*c),
            })
            .collect();
        alphas.insert(alpha(layout.background));
        alphas.retain(|a| *a < 255);
        let gs_ids: Vec<(u8, Ref)> = alphas.iter().map(|a| (*a, next())).collect();
        let gs_names: Vec<(u8, String)> = alphas.iter().map(|a| (*a, format!("A{a}"))).collect();
        let set_alpha = |content: &mut Content, color: Color| {
            let a = alpha(color);
            let name = gs_names
                .iter()
                .find(|(x, _)| *x == a)
                .map_or("A255", |(_, n)| n.as_str());
            if a < 255 {
                content.set_parameters(Name(name.as_bytes()));
            }
        };

        let mut content = Content::new();
        // Background.
        if alpha(layout.background) > 0 {
            content.save_state();
            set_alpha(&mut content, layout.background);
            let [r, g, b] = rgb(layout.background);
            content.set_fill_rgb(r, g, b);
            content.rect(0.0, 0.0, w as f32, h as f32);
            content.fill_nonzero();
            content.restore_state();
        }
        // Content: world → device (Y down) → PDF (Y up).
        let xf = Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, h]) * layout.transform;
        content.save_state();
        content.transform(xf.as_coeffs().map(|v| v as f32));
        for primitive in page.drawing.primitives() {
            content.save_state();
            match &primitive.paint {
                PrimitivePaint::Fill(color) => {
                    set_alpha(&mut content, *color);
                    let [r, g, b] = rgb(*color);
                    content.set_fill_rgb(r, g, b);
                    path(&mut content, &primitive.path);
                    content.fill_nonzero();
                }
                PrimitivePaint::Stroke(color, style) => {
                    set_alpha(&mut content, *color);
                    let [r, g, b] = rgb(*color);
                    content.set_stroke_rgb(r, g, b);
                    let stroke = kurbo_stroke(style, layout.min_line_width);
                    content.set_line_width(stroke.width as f32);
                    content.set_line_cap(match stroke.start_cap {
                        Cap::Butt => LineCapStyle::ButtCap,
                        Cap::Round => LineCapStyle::RoundCap,
                        Cap::Square => LineCapStyle::ProjectingSquareCap,
                    });
                    content.set_line_join(match stroke.join {
                        Join::Bevel => LineJoinStyle::BevelJoin,
                        Join::Miter => LineJoinStyle::MiterJoin,
                        Join::Round => LineJoinStyle::RoundJoin,
                    });
                    if !stroke.dash_pattern.is_empty() {
                        content.set_dash_pattern(
                            stroke.dash_pattern.iter().map(|d| *d as f32),
                            stroke.dash_offset as f32,
                        );
                    }
                    path(&mut content, &primitive.path);
                    content.stroke();
                }
            }
            content.restore_state();
        }
        content.restore_state();

        let mut page_writer = pdf.page(*page_id);
        page_writer
            .parent(tree_id)
            .media_box(Rect::new(0.0, 0.0, w as f32, h as f32))
            .contents(*content_id);
        {
            let mut resources = page_writer.resources();
            let mut states = resources.ext_g_states();
            for ((_, id), (_, name)) in gs_ids.iter().zip(&gs_names) {
                states.pair(Name(name.as_bytes()), *id);
            }
        }
        page_writer.finish();
        pdf.stream(*content_id, &content.finish());
        for (a, id) in &gs_ids {
            let value = f32::from(*a) / 255.0;
            pdf.ext_graphics(*id)
                .non_stroking_alpha(value)
                .stroking_alpha(value);
        }
    }
    pdf.finish()
}

fn alpha(c: Color) -> u8 {
    c.to_rgba8().a
}

fn rgb(c: Color) -> [f32; 3] {
    let c = c.to_rgba8();
    [c.r, c.g, c.b].map(|v| f32::from(v) / 255.0)
}

/// Appends the path construction operators.
fn path(content: &mut Content, path: &BezPath) {
    let mut last = librepcb_canvas::kurbo::Point::ZERO;
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                content.move_to(p.x as f32, p.y as f32);
                last = p;
            }
            PathEl::LineTo(p) => {
                content.line_to(p.x as f32, p.y as f32);
                last = p;
            }
            PathEl::QuadTo(p1, p2) => {
                let c1 = last + (p1 - last) * (2.0 / 3.0);
                let c2 = p2 + (p1 - p2) * (2.0 / 3.0);
                content.cubic_to(
                    c1.x as f32,
                    c1.y as f32,
                    c2.x as f32,
                    c2.y as f32,
                    p2.x as f32,
                    p2.y as f32,
                );
                last = p2;
            }
            PathEl::CurveTo(p1, p2, p3) => {
                content.cubic_to(
                    p1.x as f32,
                    p1.y as f32,
                    p2.x as f32,
                    p2.y as f32,
                    p3.x as f32,
                    p3.y as f32,
                );
                last = p3;
            }
            PathEl::ClosePath => {
                content.close_path();
            }
        }
    }
}
