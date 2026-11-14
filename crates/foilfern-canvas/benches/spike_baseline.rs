//! The canvas spike's vello_cpu renderer (`spikes/canvas/src/raster.rs`,
//! `VelloCpu`) with the spike's grouping, reimplemented on canvas items so
//! both render identical content. Shared by the benchmark and the timing
//! test.

use std::collections::BTreeMap;

use foilfern_canvas::stress::{BOARD_LAYERS, argb};
use foilfern_canvas::{Geometry, Item, View};
use kurbo::{BezPath, Rect};
use vello_cpu::{PixmapMut, RenderContext, RenderSettings, Resources};

/// 8 × 8 tiles over the scene bounds, one path per (layer, style, tile).
pub struct Spike {
    pub groups: Vec<(BezPath, u16, Option<f64>, Rect)>,
    ctx: RenderContext,
    resources: Resources,
}

impl Spike {
    pub fn build(items: &[Item], threads: u16, w: u16, h: u16) -> Self {
        let bbox = items
            .iter()
            .map(Item::bounding_box)
            .reduce(|a, b| a.union(b))
            .unwrap_or(Rect::ZERO);
        let tiles = 8usize;
        // Key: layer, stroke width in nm (None = fill), tile.
        let mut map: BTreeMap<(u16, Option<i64>, usize), (BezPath, Rect)> = BTreeMap::new();
        for it in items {
            let b = it.bounding_box();
            let c = b.center();
            let tx = (((c.x - bbox.x0) / bbox.width() * tiles as f64) as usize).min(tiles - 1);
            let ty = (((c.y - bbox.y0) / bbox.height() * tiles as f64) as usize).min(tiles - 1);
            let mut add = |style: Option<i64>| {
                let e = map
                    .entry((it.layer.0, style, ty * tiles + tx))
                    .or_insert_with(|| (BezPath::new(), b));
                e.1 = e.1.union(b);
                match &it.geometry {
                    Geometry::Line(l) => {
                        e.0.move_to(l.p0);
                        e.0.line_to(l.p1);
                    }
                    Geometry::Path(p) => e.0.extend(p.elements().iter().copied()),
                    Geometry::Circle(c) => e.0.extend(kurbo::Shape::path_elements(c, 1e-4)),
                }
            };
            if it.style.fill.is_some() {
                add(None);
            }
            if let Some(s) = &it.style.stroke {
                // The spike drew unfilled outlines at least 0.05 mm wide.
                add(Some((s.width.max(0.05) * 1e6).round() as i64));
            }
        }
        let groups = map
            .into_iter()
            .map(|((layer, style, _), (path, bbox))| {
                (path, layer, style.map(|nm| nm as f64 * 1e-6), bbox)
            })
            .collect();
        Self {
            groups,
            ctx: RenderContext::new_with(
                w,
                h,
                RenderSettings {
                    num_threads: threads,
                    ..Default::default()
                },
            ),
            resources: Resources::new(),
        }
    }

    pub fn render(&mut self, view: &View, out: &mut [u8]) {
        let (w, h) = view.device_size();
        let (w16, h16) = (w as u16, h as u16);
        let ctx = &mut self.ctx;
        if ctx.width() != w16 || ctx.height() != h16 {
            ctx.reset_and_resize(w16, h16);
        } else {
            ctx.reset();
        }
        let visible = view.visible_rect();
        ctx.set_paint(peniko::Color::BLACK);
        ctx.fill_rect(&Rect::new(0.0, 0.0, f64::from(w), f64::from(h)));
        ctx.set_transform(view.device_transform());
        for (path, layer, width, bbox) in &self.groups {
            if !bbox.overlaps(visible) {
                continue;
            }
            ctx.set_paint(argb(BOARD_LAYERS[usize::from(*layer)].1));
            match width {
                None => ctx.fill_path(path),
                Some(sw) => {
                    ctx.set_stroke(
                        kurbo::Stroke::new(*sw)
                            .with_caps(kurbo::Cap::Round)
                            .with_join(kurbo::Join::Round),
                    );
                    ctx.stroke_path(path);
                }
            }
        }
        ctx.reset_transform();
        ctx.flush();
        if let Some(pm) = PixmapMut::new(w16, h16, out) {
            ctx.render(pm, &mut self.resources);
        }
    }
}
