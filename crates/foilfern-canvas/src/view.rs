//! The view: world ↔ screen transformation, pan, zoom, fit and device pixel
//! ratio.
//!
//! World coordinates are millimetres with the Y axis pointing up (like
//! LibrePCB's model); screen coordinates are logical pixels with the Y axis
//! pointing down (like Slint). Device pixels are logical pixels times the
//! device pixel ratio.

use kurbo::{Affine, Point, Rect, Size, Vec2};

/// Zoom limits in logical pixels per millimetre. They correspond to
/// upstream's scale limits (0.001 to 5000 screen pixels per 1/72 inch).
const MIN_SCALE: f64 = 0.001 * 72.0 / 25.4;
const MAX_SCALE: f64 = 5000.0 * 72.0 / 25.4;

/// A view onto the scene.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    scale: f64,
    offset: Vec2,
    size: Size,
    device_pixel_ratio: f64,
    mirrored: bool,
}

impl Default for View {
    fn default() -> Self {
        Self {
            scale: 10.0,
            offset: Vec2::ZERO,
            size: Size::new(1.0, 1.0),
            device_pixel_ratio: 1.0,
            mirrored: false,
        }
    }
}

impl View {
    /// Creates a view with a viewport of `size` logical pixels, showing the
    /// world origin at the top left corner at 10 px/mm.
    pub fn new(size: impl Into<Size>, device_pixel_ratio: f64) -> Self {
        let mut v = Self::default();
        v.set_size(size);
        v.set_device_pixel_ratio(device_pixel_ratio);
        v
    }

    /// Zoom factor in logical pixels per millimetre.
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// Screen position (logical pixels) of the world origin.
    pub fn offset(&self) -> Vec2 {
        self.offset
    }

    /// Viewport size in logical pixels.
    pub fn size(&self) -> Size {
        self.size
    }

    /// Device pixels per logical pixel.
    pub fn device_pixel_ratio(&self) -> f64 {
        self.device_pixel_ratio
    }

    /// Whether the view is mirrored horizontally (e.g. board bottom view).
    pub fn is_mirrored(&self) -> bool {
        self.mirrored
    }

    /// Viewport size in whole device pixels (the render buffer size).
    pub fn device_size(&self) -> (u32, u32) {
        let d = |v: f64| {
            (v * self.device_pixel_ratio)
                .round()
                .clamp(0.0, u32::MAX as f64) as u32
        };
        (d(self.size.width), d(self.size.height))
    }

    /// Changes the viewport size (logical pixels), keeping the world origin
    /// on screen where it is.
    pub fn set_size(&mut self, size: impl Into<Size>) {
        let s = size.into();
        let fix = |v: f64| if v.is_finite() { v.max(0.0) } else { 0.0 };
        self.size = Size::new(fix(s.width), fix(s.height));
    }

    /// Changes the device pixel ratio (Slint's `scale_factor`).
    pub fn set_device_pixel_ratio(&mut self, ratio: f64) {
        if ratio.is_finite() && ratio > 0.0 {
            self.device_pixel_ratio = ratio;
        }
    }

    /// Mirrors the view horizontally, keeping the center of the viewport.
    pub fn set_mirrored(&mut self, mirrored: bool) {
        if mirrored != self.mirrored {
            let center = self.size.to_vec2() / 2.0;
            let world = self.screen_to_world(center.to_point());
            self.mirrored = mirrored;
            self.offset += center - self.world_to_screen(world).to_vec2();
        }
    }

    /// World → screen (logical pixels) transformation.
    pub fn transform(&self) -> Affine {
        let sx = if self.mirrored {
            -self.scale
        } else {
            self.scale
        };
        Affine::new([sx, 0.0, 0.0, -self.scale, self.offset.x, self.offset.y])
    }

    /// World → device pixel transformation.
    pub fn device_transform(&self) -> Affine {
        Affine::scale(self.device_pixel_ratio) * self.transform()
    }

    /// World → device pixel transformation with the translation rounded to
    /// whole device pixels, and that translation. Renderers use it so that
    /// panning shifts the image by whole pixels, which allows reusing it.
    pub fn snapped_device_transform(&self) -> (Affine, (i64, i64)) {
        let [a, b, c, d, e, f] = self.device_transform().as_coeffs();
        let (x, y) = (e.round(), f.round());
        let clamp = |v: f64| v.clamp(-1e15, 1e15) as i64;
        (Affine::new([a, b, c, d, x, y]), (clamp(x), clamp(y)))
    }

    /// Maps a world point to the screen (logical pixels).
    pub fn world_to_screen(&self, p: Point) -> Point {
        self.transform() * p
    }

    /// Maps a screen point (logical pixels) to the world.
    pub fn screen_to_world(&self, p: Point) -> Point {
        self.transform().inverse() * p
    }

    /// Converts a length in logical pixels to world units (e.g. for hit
    /// test tolerances).
    pub fn pixels_to_world(&self, pixels: f64) -> f64 {
        pixels / self.scale
    }

    /// The visible world rectangle.
    pub fn visible_rect(&self) -> Rect {
        self.transform()
            .inverse()
            .transform_rect_bbox(self.size.to_rect())
    }

    /// Moves the view content by a screen distance (logical pixels).
    pub fn pan(&mut self, delta: Vec2) {
        if delta.is_finite() {
            self.offset += delta;
        }
    }

    /// Zooms by `factor` keeping the world point under `screen_pos` fixed.
    pub fn zoom_at(&mut self, screen_pos: Point, factor: f64) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let world = self.screen_to_world(screen_pos);
        self.scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        self.offset += screen_pos - self.world_to_screen(world);
    }

    /// Sets the zoom factor (logical pixels per millimetre), keeping the
    /// center of the viewport.
    pub fn set_scale(&mut self, scale: f64) {
        let center = (self.size.to_vec2() / 2.0).to_point();
        self.zoom_at(center, scale / self.scale);
    }

    /// Centers a world point in the viewport.
    pub fn center_on(&mut self, world: Point) {
        let center = (self.size.to_vec2() / 2.0).to_point();
        self.offset += center - self.world_to_screen(world);
    }

    /// Zooms and pans so that `rect` fills the viewport minus `margin`
    /// logical pixels on each side.
    pub fn fit(&mut self, rect: Rect, margin: f64) {
        let rect = rect.abs();
        let avail = Size::new(
            (self.size.width - 2.0 * margin).max(1.0),
            (self.size.height - 2.0 * margin).max(1.0),
        );
        let scale =
            (avail.width / rect.width().max(1e-9)).min(avail.height / rect.height().max(1e-9));
        if scale.is_finite() {
            self.scale = scale.clamp(MIN_SCALE, MAX_SCALE);
        }
        self.center_on(rect.center());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(a: Point, b: Point) {
        assert!((a - b).hypot() < 1e-9, "{a:?} != {b:?}");
    }

    #[test]
    fn roundtrip_and_y_up() {
        let mut v = View::new((800.0, 600.0), 2.0);
        v.pan(Vec2::new(100.0, 300.0));
        let w = Point::new(3.0, 4.0);
        let s = v.world_to_screen(w);
        assert_close(s, Point::new(100.0 + 30.0, 300.0 - 40.0));
        assert_close(v.screen_to_world(s), w);
        assert_close(v.device_transform() * w, Point::new(2.0 * s.x, 2.0 * s.y));
        assert_eq!(v.device_size(), (1600, 1200));
    }

    #[test]
    fn zoom_keeps_cursor_point() {
        let mut v = View::new((800.0, 600.0), 1.0);
        let cursor = Point::new(123.0, 456.0);
        let before = v.screen_to_world(cursor);
        v.zoom_at(cursor, 1.3);
        assert!((v.scale() - 13.0).abs() < 1e-9);
        assert_close(v.screen_to_world(cursor), before);
        v.zoom_at(cursor, 1e-30);
        assert_eq!(v.scale(), MIN_SCALE);
        assert_close(v.screen_to_world(cursor), before);
    }

    #[test]
    fn fit_centers_content() {
        let mut v = View::new((1000.0, 500.0), 1.0);
        let r = Rect::new(-10.0, 0.0, 30.0, 10.0);
        v.fit(r, 50.0);
        // Width limits: (1000 - 100) / 40 = 22.5 px/mm (height: 40 px/mm).
        assert!((v.scale() - 22.5).abs() < 1e-9);
        assert_close(v.world_to_screen(r.center()), Point::new(500.0, 250.0));
        let visible = v.visible_rect();
        assert!(visible.contains_rect(r));
    }

    #[test]
    fn mirror_keeps_center() {
        let mut v = View::new((400.0, 400.0), 1.0);
        v.center_on(Point::new(5.0, 5.0));
        v.set_mirrored(true);
        assert_close(
            v.screen_to_world(Point::new(200.0, 200.0)),
            Point::new(5.0, 5.0),
        );
        // Right of the center on screen is left in the world.
        assert!(v.screen_to_world(Point::new(300.0, 200.0)).x < 5.0);
    }

    #[test]
    fn snapped_translation_is_integer() {
        let mut v = View::new((100.0, 100.0), 1.25);
        v.pan(Vec2::new(10.3, 7.7));
        let (t, (x, y)) = v.snapped_device_transform();
        let c = t.as_coeffs();
        assert_eq!((c[4], c[5]), (x as f64, y as f64));
        assert_eq!((x, y), (13, 10));
    }
}
