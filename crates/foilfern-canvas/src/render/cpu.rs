//! vello_cpu backend with a frame cache.
//!
//! The base image (scene without selection) is reused between frames:
//! - panning by whole device pixels copies it shifted and paints only the
//!   exposed strips;
//! - item changes repaint only the damaged rectangles;
//! - selection changes repaint nothing but the overlay, which is drawn on a
//!   copy of the base image.
//!
//! Zooming, resizing and layer changes repaint the whole base image, directly
//! into the target buffer (no extra copy).
//!
//! The base image lives either in the caller's previous target (passed back
//! as `previous` to [`Renderer::render_into`]) or, when the previous frame
//! got a selection overlay, in a private copy.

use std::time::Instant;

use kurbo::{Affine, BezPath, Rect};
use peniko::{Color, Fill};
use slotmap::SecondaryMap;
use vello_cpu::{
    CompositeMode, PixmapMut, RasterizerSettings, RenderContext, RenderSettings, Resources,
};

use super::{Error, RenderStats, Renderer};
use crate::scene::{Damage, GroupId, OverlayGroup, Paint};
use crate::style::StrokeStyle;
use crate::{Scene, View};

/// Settings of the [`CpuRenderer`].
#[derive(Debug, Clone, PartialEq)]
pub struct CpuRendererSettings {
    /// Background color (opaque recommended).
    pub background: Color,
    /// Worker threads; 0 renders on the calling thread. Defaults to the
    /// number of CPUs minus one, at most 8 (vello_cpu's default).
    pub threads: u16,
    /// Reuse the previous frame when panning, editing or selecting. Only
    /// disable it for benchmarks.
    pub frame_cache: bool,
    /// Background grid, drawn below all items.
    pub grid: Option<Grid>,
}

/// How the background grid is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridStyle {
    /// A dot at every grid point.
    Dots,
    /// Horizontal and vertical lines.
    Lines,
}

/// A background grid like upstream's `GraphicsScene::drawBackground()`:
/// the interval is multiplied by 10 until grid points are at least 7 device
/// pixels apart; every 10th line or dot is drawn more opaque.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    /// Grid interval in world units (millimetres).
    pub interval: f64,
    /// Dots or lines.
    pub style: GridStyle,
    /// Color of the major (every 10th) lines or dots; minor ones are drawn
    /// with a reduced alpha.
    pub color: Color,
}

/// Minimum distance between grid points in device pixels.
const MIN_GRID_SPACING: f64 = 7.0;

impl Default for CpuRendererSettings {
    fn default() -> Self {
        Self {
            background: Color::BLACK,
            threads: RenderSettings::default().num_threads,
            frame_cache: true,
            grid: None,
        }
    }
}

/// A pixel rectangle `[x0, x1) × [y0, y1)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PxRect {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
}

impl PxRect {
    fn width(&self) -> u32 {
        self.x1 - self.x0
    }
    fn height(&self) -> u32 {
        self.y1 - self.y0
    }
    fn area(&self) -> u64 {
        u64::from(self.width()) * u64::from(self.height())
    }
    fn union(&self, o: &Self) -> Self {
        Self {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }
    /// The pixels touched by a device rectangle, clipped to `w` × `h`.
    fn covering(r: Rect, w: u32, h: u32) -> Option<Self> {
        let clip = |v: f64, max: u32| v.clamp(0.0, f64::from(max)) as u32;
        let px = Self {
            x0: clip(r.x0.floor(), w),
            y0: clip(r.y0.floor(), h),
            x1: clip(r.x1.ceil(), w),
            y1: clip(r.y1.ceil(), h),
        };
        (px.x0 < px.x1 && px.y0 < px.y1).then_some(px)
    }
}

/// What the cached base image shows.
#[derive(Debug, Clone, Copy, PartialEq)]
struct BaseState {
    size: (u32, u32),
    linear: [f64; 4],
    offset: (i64, i64),
    rev: u64,
}

/// Where the base image of the last frame is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BaseLocation {
    /// In the caller's last target (no overlay was drawn on it).
    Target,
    /// In `CpuRenderer::base`.
    Own,
}

/// What to repaint for a frame.
struct Plan {
    state: BaseState,
    /// Shift of the previous base image, `None` for a full repaint.
    reuse: Option<(i64, i64)>,
    regions: Vec<PxRect>,
}

#[derive(Debug, Default)]
struct Encoded {
    rev: u64,
    path: BezPath,
}

/// Renders with [vello_cpu](https://docs.rs/vello_cpu) into RGBA buffers.
pub struct CpuRenderer {
    settings: CpuRendererSettings,
    ctx: Option<RenderContext>,
    resources: Resources,
    encoded: SecondaryMap<GroupId, Encoded>,
    base_state: Option<BaseState>,
    base_location: BaseLocation,
    /// Private copy of the base image (when the last frame had an overlay).
    base: Vec<u8>,
    region_buf: Vec<u8>,
    /// Double buffer for [`Renderer::render`].
    front: Vec<u8>,
    back: Vec<u8>,
    overlay: Vec<OverlayGroup>,
    overlay_rev: u64,
    stats: RenderStats,
}

impl Default for CpuRenderer {
    fn default() -> Self {
        Self::new(CpuRendererSettings::default())
    }
}

impl std::fmt::Debug for CpuRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CpuRenderer")
            .field("settings", &self.settings)
            .field("base_state", &self.base_state)
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl CpuRenderer {
    /// Creates a renderer.
    pub fn new(settings: CpuRendererSettings) -> Self {
        Self {
            settings,
            ctx: None,
            resources: Resources::new(),
            encoded: SecondaryMap::new(),
            base_state: None,
            base_location: BaseLocation::Own,
            base: Vec::new(),
            region_buf: Vec::new(),
            front: Vec::new(),
            back: Vec::new(),
            overlay: Vec::new(),
            overlay_rev: 0,
            stats: RenderStats::default(),
        }
    }

    /// The settings.
    pub fn settings(&self) -> &CpuRendererSettings {
        &self.settings
    }

    /// Changes the background color (repaints everything).
    pub fn set_background(&mut self, color: Color) {
        if self.settings.background != color {
            self.settings.background = color;
            self.invalidate();
        }
    }

    /// Changes the background grid (repaints everything).
    pub fn set_grid(&mut self, grid: Option<Grid>) {
        if self.settings.grid != grid {
            self.settings.grid = grid;
            self.invalidate();
        }
    }

    /// Forgets the cached frame (the next frame is repainted completely).
    pub fn invalidate(&mut self) {
        self.base_state = None;
    }

    /// Decides what to repaint.
    fn plan(&self, scene: &Scene, view: &View, xf: Affine, offset: (i64, i64)) -> Plan {
        let (w, h) = view.device_size();
        let c = xf.as_coeffs();
        let state = BaseState {
            size: (w, h),
            linear: [c[0], c[1], c[2], c[3]],
            offset,
            rev: scene.rev(),
        };
        let full = Plan {
            state,
            reuse: None,
            regions: vec![PxRect {
                x0: 0,
                y0: 0,
                x1: w,
                y1: h,
            }],
        };
        let Some(old) = self.base_state else {
            return full;
        };
        if !self.settings.frame_cache || old.size != state.size || old.linear != state.linear {
            return full;
        }
        let damage = scene.damage_since(old.rev);
        let (dx, dy) = (offset.0 - old.offset.0, offset.1 - old.offset.1);
        if damage == Damage::All
            || dx.unsigned_abs() >= u64::from(w)
            || dy.unsigned_abs() >= u64::from(h)
        {
            return full;
        }
        let mut regions = Vec::new();
        let (dxu, dyu) = (dx.unsigned_abs() as u32, dy.unsigned_abs() as u32);
        let strip = |x0, y0, x1, y1| PxRect { x0, y0, x1, y1 };
        match dx.signum() {
            1 => regions.push(strip(0, 0, dxu, h)),
            -1 => regions.push(strip(w - dxu, 0, w, h)),
            _ => {}
        }
        match dy.signum() {
            1 => regions.push(strip(0, 0, w, dyu)),
            -1 => regions.push(strip(0, h - dyu, w, h)),
            _ => {}
        }
        if let Damage::Rects(rects) = damage {
            // Anti-aliasing and hairlines reach beyond the geometry.
            let margin = 2.0 + view.device_pixel_ratio();
            regions.extend(rects.iter().filter_map(|r| {
                PxRect::covering(xf.transform_rect_bbox(*r).inflate(margin, margin), w, h)
            }));
        }
        // Merge many regions into one, repaint everything if large.
        if regions.len() > 8 {
            let union = regions.iter().skip(1).fold(regions[0], |a, b| a.union(b));
            regions = vec![union];
        }
        let area: u64 = regions.iter().map(PxRect::area).sum();
        if area * 10 > u64::from(w) * u64::from(h) * 7 {
            return full;
        }
        Plan {
            state,
            reuse: Some((dx, dy)),
            regions,
        }
    }

    /// Paints the base scene into a region of `dst` (the full frame).
    #[allow(clippy::too_many_arguments)]
    fn paint_region(
        &mut self,
        scene: &Scene,
        view: &View,
        xf: Affine,
        r: PxRect,
        full: (u32, u32),
        dst: &mut [u8],
    ) {
        // Sizes were checked against u16::MAX by the caller.
        let (rw, rh) = (r.width() as u16, r.height() as u16);
        let background = self.settings.background;
        let local = Affine::translate((-f64::from(r.x0), -f64::from(r.y0))) * xf;
        let px = 1.0 / (view.scale() * view.device_pixel_ratio());
        let hairline = 1.0 / view.scale();
        let world = local
            .inverse()
            .transform_rect_bbox(Rect::new(0.0, 0.0, f64::from(rw), f64::from(rh)))
            .inflate(2.0 * px + hairline, 2.0 * px + hairline);

        // Split borrows: the context and the encoding cache are both used in
        // the draw loop.
        let ctx = context(&mut self.ctx, self.settings.threads, rw, rh);
        ctx.set_paint(background);
        ctx.fill_rect(&Rect::new(0.0, 0.0, f64::from(rw), f64::from(rh)));
        if let Some(grid) = self.settings.grid {
            draw_grid(ctx, &grid, xf, r, view.scale() * view.device_pixel_ratio());
        }
        ctx.set_transform(local);
        ctx.set_fill_rule(Fill::NonZero);
        let mut drawn = 0;
        let mut encoded_count = 0;
        for g in scene.draw_groups() {
            if !g.bounding_box().overlaps(world) {
                continue;
            }
            let needs_encoding = self.encoded.get(g.id()).is_none_or(|e| e.rev != g.rev());
            if needs_encoding {
                let mut e = self.encoded.remove(g.id()).unwrap_or_default();
                e.path.truncate(0);
                g.encode(&mut e.path);
                e.rev = g.rev();
                self.encoded.insert(g.id(), e);
                encoded_count += 1;
            }
            let Some(e) = self.encoded.get(g.id()) else {
                continue;
            };
            draw(ctx, &e.path, g.paint(), hairline);
            drawn += 1;
        }
        ctx.flush();

        let is_full = r.x0 == 0 && r.y0 == 0 && (r.x1, r.y1) == full;
        if is_full {
            if let Some(pm) = PixmapMut::new(rw, rh, dst) {
                ctx.render(pm, &mut self.resources);
            }
        } else {
            self.region_buf
                .resize(usize::from(rw) * usize::from(rh) * 4, 0);
            if let Some(pm) = PixmapMut::new(rw, rh, &mut self.region_buf) {
                ctx.render(pm, &mut self.resources);
            }
            let stride = full.0 as usize * 4;
            let row = usize::from(rw) * 4;
            for (y, src) in self.region_buf.chunks_exact(row).enumerate() {
                let start = (r.y0 as usize + y) * stride + r.x0 as usize * 4;
                dst[start..start + row].copy_from_slice(src);
            }
        }
        self.stats.drawn_groups += drawn;
        self.stats.encoded_groups += encoded_count;
        self.stats.regions.push((r.x0, r.y0, r.width(), r.height()));
    }

    fn render_overlay(&mut self, scene: &Scene, view: &View, xf: Affine, target: &mut [u8]) {
        if scene.selection_rev() != self.overlay_rev {
            self.overlay = scene.selection_overlay();
            self.overlay_rev = scene.selection_rev();
        }
        if self.overlay.is_empty() {
            return;
        }
        // The caller checked the size against u16::MAX.
        let (w, h) = view.device_size();
        let (w, h) = (w as u16, h as u16);
        let hairline = 1.0 / view.scale();
        let world = view.visible_rect().inflate(4.0 * hairline, 4.0 * hairline);
        let ctx = context(&mut self.ctx, self.settings.threads, w, h);
        ctx.set_transform(xf);
        ctx.set_fill_rule(Fill::NonZero);
        let mut drawn = 0;
        for g in &self.overlay {
            if !g.bbox.overlaps(world) {
                continue;
            }
            let paint = match &g.stroke {
                None => Paint::Fill(g.color),
                Some(s) => Paint::Stroke(g.color, s),
            };
            draw(ctx, &g.path, paint, hairline);
            drawn += 1;
        }
        ctx.flush();
        if let Some(pm) = PixmapMut::new(w, h, target) {
            ctx.render_with(
                pm,
                &mut self.resources,
                RasterizerSettings {
                    composite_mode: CompositeMode::SrcOver,
                    ..RasterizerSettings::default()
                },
            );
        }
        self.stats.overlay_groups = drawn;
    }

    fn prune_cache(&mut self, scene: &Scene) {
        if self.encoded.len() > 2 * scene.group_count() + 64 {
            self.encoded.retain(|id, _| scene.contains_group(id));
        }
    }
}

/// Copies `src` to `dst` shifted by (dx, dy) pixels (uncovered pixels are
/// left as they are; they get repainted).
fn copy_shifted(src: &[u8], dst: &mut [u8], w: u32, dx: i64, dy: i64) {
    let stride = w as usize * 4;
    if dx == 0 && dy == 0 {
        dst.copy_from_slice(src);
        return;
    }
    let (sx, tx) = if dx >= 0 {
        (0, dx as usize * 4)
    } else {
        (dx.unsigned_abs() as usize * 4, 0)
    };
    let len = stride - dx.unsigned_abs() as usize * 4;
    let rows = src.len() / stride;
    let skip = dy.unsigned_abs() as usize;
    for y in 0..rows.saturating_sub(skip) {
        let (sy, ty) = if dy >= 0 {
            (y, y + skip)
        } else {
            (y + skip, y)
        };
        let s = sy * stride + sx;
        let t = ty * stride + tx;
        dst[t..t + len].copy_from_slice(&src[s..s + len]);
    }
}

/// Returns the (lazily created) render context, reset to `w` × `h` pixels.
fn context(slot: &mut Option<RenderContext>, threads: u16, w: u16, h: u16) -> &mut RenderContext {
    let ctx = slot.get_or_insert_with(|| {
        RenderContext::new_with(
            w,
            h,
            RenderSettings {
                num_threads: threads,
                ..RenderSettings::default()
            },
        )
    });
    if ctx.width() != w || ctx.height() != h {
        ctx.reset_and_resize(w, h);
    } else {
        ctx.reset();
    }
    ctx
}

/// Draws the grid into the region `r` (device pixels) of the frame whose
/// world → device transformation is `xf`; `scale` is device pixels per world
/// unit.
fn draw_grid(ctx: &mut RenderContext, grid: &Grid, xf: Affine, r: PxRect, scale: f64) {
    if !(grid.interval.is_finite() && grid.interval > 0.0 && scale.is_finite() && scale > 0.0) {
        return;
    }
    let mut interval = grid.interval;
    while interval * scale < MIN_GRID_SPACING {
        interval *= 10.0;
    }
    let region = Rect::new(
        f64::from(r.x0),
        f64::from(r.y0),
        f64::from(r.x1),
        f64::from(r.y1),
    );
    let world = xf.inverse().transform_rect_bbox(region.inflate(2.0, 2.0));
    // Grid indices of the visible range (the view may be mirrored, hence
    // the bounding box).
    let range = |a: f64, b: f64| {
        let i0 = (a / interval).floor() as i64;
        let i1 = (b / interval).ceil() as i64;
        i0..=i1
    };
    let (xs, ys) = (range(world.x0, world.x1), range(world.y0, world.y1));
    // Guard against absurd counts (should not happen thanks to the minimum
    // spacing, but the region might be huge).
    let count = |r: &std::ops::RangeInclusive<i64>| (r.end() - r.start()).unsigned_abs();
    if count(&xs) > 20_000 || count(&ys) > 20_000 {
        return;
    }
    let to_device =
        |x: i64, y: i64| xf * kurbo::Point::new(x as f64 * interval, y as f64 * interval);
    ctx.set_transform(Affine::translate((-region.x0, -region.y0)));
    let mut minor = BezPath::new();
    let mut major = BezPath::new();
    match grid.style {
        GridStyle::Dots => {
            for x in xs.clone() {
                for y in ys.clone() {
                    let p = to_device(x, y);
                    let (px, py) = (p.x.round(), p.y.round());
                    let dot = Rect::new(px - 1.0, py - 1.0, px + 1.0, py + 1.0);
                    let path = if x % 10 == 0 && y % 10 == 0 {
                        &mut major
                    } else {
                        &mut minor
                    };
                    path.extend(kurbo::Shape::path_elements(&dot, 0.1));
                }
            }
            fill_grid(ctx, &minor, grid.color.multiply_alpha(0.6));
            fill_grid(ctx, &major, grid.color);
        }
        GridStyle::Lines => {
            for x in xs.clone() {
                let px = to_device(x, 0).x.round();
                let line = Rect::new(px, region.y0, px + 1.0, region.y1);
                let path = if x % 10 == 0 { &mut major } else { &mut minor };
                path.extend(kurbo::Shape::path_elements(&line, 0.1));
            }
            for y in ys {
                let py = to_device(0, y).y.round();
                let line = Rect::new(region.x0, py, region.x1, py + 1.0);
                let path = if y % 10 == 0 { &mut major } else { &mut minor };
                path.extend(kurbo::Shape::path_elements(&line, 0.1));
            }
            fill_grid(ctx, &minor, grid.color.multiply_alpha(0.4));
            fill_grid(ctx, &major, grid.color.multiply_alpha(0.7));
        }
    }
}

fn fill_grid(ctx: &mut RenderContext, path: &BezPath, color: Color) {
    if !path.is_empty() {
        ctx.set_paint(color);
        ctx.set_fill_rule(Fill::NonZero);
        ctx.fill_path(path);
    }
}

/// Issues one fill or stroke call.
fn draw(ctx: &mut RenderContext, path: &BezPath, paint: Paint<'_>, hairline: f64) {
    match paint {
        Paint::Fill(color) => {
            ctx.set_paint(color);
            ctx.fill_path(path);
        }
        Paint::Stroke(color, style) => {
            ctx.set_paint(color);
            ctx.set_stroke(stroke(style, hairline));
            ctx.stroke_path(path);
        }
    }
}

fn stroke(style: &StrokeStyle, hairline: f64) -> kurbo::Stroke {
    let width = if style.is_hairline() {
        hairline
    } else {
        style.width
    };
    style.to_kurbo(width)
}

fn check_size(view: &View) -> Result<(u32, u32), Error> {
    let (w, h) = view.device_size();
    if w > u32::from(u16::MAX) || h > u32::from(u16::MAX) {
        return Err(Error::TooLarge {
            width: w,
            height: h,
        });
    }
    Ok((w, h))
}

impl Renderer for CpuRenderer {
    fn render_into(
        &mut self,
        scene: &Scene,
        view: &View,
        previous: Option<&[u8]>,
        target: &mut [u8],
    ) -> Result<(), Error> {
        let start = Instant::now();
        let (w, h) = check_size(view)?;
        let expected = w as usize * h as usize * 4;
        if target.len() != expected {
            return Err(Error::TargetSize {
                expected,
                actual: target.len(),
            });
        }
        self.stats = RenderStats::default();
        if expected == 0 {
            self.base_state = None;
            return Ok(());
        }
        let (xf, offset) = view.snapped_device_transform();
        let mut plan = self.plan(scene, view, xf, offset);

        // Start from the previous base image, if any.
        let mut own = std::mem::take(&mut self.base);
        if let Some((dx, dy)) = plan.reuse {
            let source = match self.base_location {
                BaseLocation::Own => Some(own.as_slice()),
                BaseLocation::Target => previous,
            };
            match source.filter(|s| s.len() == expected) {
                Some(src) => copy_shifted(src, target, w, dx, dy),
                None => {
                    plan.reuse = None;
                    plan.regions = vec![PxRect {
                        x0: 0,
                        y0: 0,
                        x1: w,
                        y1: h,
                    }];
                }
            }
        }
        self.stats.full_repaint = plan.reuse.is_none();
        for r in &plan.regions {
            self.paint_region(scene, view, xf, *r, (w, h), target);
        }
        self.base_state = Some(plan.state);
        self.prune_cache(scene);

        if scene.selection().next().is_some() {
            // Keep the base image before drawing the overlay on the target.
            own.clear();
            own.extend_from_slice(target);
            self.base_location = BaseLocation::Own;
            self.render_overlay(scene, view, xf, target);
        } else {
            self.overlay.clear();
            self.overlay_rev = scene.selection_rev();
            self.base_location = BaseLocation::Target;
        }
        self.base = own;
        self.stats.time = start.elapsed();
        Ok(())
    }

    fn render(&mut self, scene: &Scene, view: &View) -> Result<&[u8], Error> {
        let (w, h) = check_size(view)?;
        let expected = w as usize * h as usize * 4;
        let front = std::mem::take(&mut self.front);
        let mut back = std::mem::take(&mut self.back);
        back.resize(expected, 0);
        let result = self.render_into(scene, view, Some(&front), &mut back);
        self.front = back;
        self.back = front;
        result.map(|()| self.front.as_slice())
    }

    fn stats(&self) -> &RenderStats {
        &self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifted_copy() {
        // 3 × 2 pixels, one byte value per pixel.
        let px = |v: &[u8]| v.iter().flat_map(|b| [*b; 4]).collect::<Vec<u8>>();
        let src = px(&[1, 2, 3, 4, 5, 6]);
        let mut dst = px(&[0; 6]);
        copy_shifted(&src, &mut dst, 3, 1, 1);
        assert_eq!(dst, px(&[0, 0, 0, 0, 1, 2]));
        let mut dst = px(&[0; 6]);
        copy_shifted(&src, &mut dst, 3, -2, -1);
        assert_eq!(dst, px(&[6, 0, 0, 0, 0, 0]));
        let mut dst = px(&[0; 6]);
        copy_shifted(&src, &mut dst, 3, 0, 0);
        assert_eq!(dst, src);
    }
}
