//! The graphics view of the schematic and board tabs: rendering a canvas
//! scene into a `slint::Image` and pan/zoom navigation.
//!
//! Port of libs/librepcb/editor/graphics/slintgraphicsview.{h,cpp} on top
//! of `librepcb-canvas` (`View`, `Navigator`, `CpuRenderer` through the
//! Slint adapter's `ImageRenderer`) instead of `QGraphicsScene`/`QPainter`.
//!
//! Like upstream, the view fits the content automatically ("auto fit in
//! view") until the user pans or zooms, zoom steps are 1.3, and the fit
//! leaves margins of 40/35/40/25 pixels (left/top/right/bottom). Zooming is
//! not animated (upstream animates `zoomToSceneRect()`).

use librepcb_canvas::kurbo::{Point, Rect, Vec2};
use librepcb_canvas::peniko::Color;
use librepcb_canvas::slint_adapter::ImageRenderer;
use librepcb_canvas::{
    CpuRenderer, CpuRendererSettings, Grid, Modifiers, Navigator, PointerAction, PointerButton,
    PointerKind, Scene, View,
};
use slint::Image;

/// Zoom factor of one zoom step (upstream `zoomIn()`/`zoomOut()`).
const ZOOM_STEP: f64 = 1.3;

/// Scroll step as a fraction of the view size (upstream `sScrollFactor`).
const SCROLL_FACTOR: f64 = 0.07;

/// Upstream `SlintGraphicsView::defaultEditorMargins()` (left, top, right,
/// bottom).
const MARGINS: (f64, f64, f64, f64) = (40.0, 35.0, 40.0, 25.0);

/// Default visible area of an empty schematic (upstream
/// `defaultSchematicSceneRect()`, converted to Y-up millimetres).
pub const DEFAULT_SCHEMATIC_RECT: Rect = Rect::new(-20.0, -40.0, 280.0, 180.0);

/// Default visible area of an empty board (upstream
/// `defaultBoardSceneRect()`).
pub const DEFAULT_BOARD_RECT: Rect = Rect::new(-20.0, -20.0, 120.0, 120.0);

/// A pan/zoom view onto a canvas scene, rendered into Slint images.
pub struct CanvasView {
    view: View,
    navigator: Navigator,
    renderer: ImageRenderer<CpuRenderer>,
    default_rect: Rect,
    /// Fit the content whenever the view size changes (upstream
    /// `Projection::autoFitInView`).
    auto_fit: bool,
    /// Whether the view got a size yet (before, fitting is postponed).
    sized: bool,
    /// Content bounds to fit when the view gets its size.
    pending_fit: Option<Rect>,
}

impl std::fmt::Debug for CanvasView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CanvasView")
            .field("view", &self.view)
            .field("auto_fit", &self.auto_fit)
            .finish_non_exhaustive()
    }
}

impl CanvasView {
    /// Creates a view that fits `content` (or `default_rect` if there is no
    /// content) once it gets a size.
    pub fn new(background: Color, default_rect: Rect, content: Option<Rect>) -> Self {
        Self {
            view: View::default(),
            navigator: Navigator::new(),
            renderer: ImageRenderer::new(CpuRenderer::new(CpuRendererSettings {
                background,
                ..CpuRendererSettings::default()
            })),
            default_rect,
            auto_fit: true,
            sized: false,
            pending_fit: content,
        }
    }

    /// The view transformation.
    pub fn view(&self) -> &View {
        &self.view
    }

    /// Mirrors the view (board bottom side).
    pub fn set_mirrored(&mut self, mirrored: bool) {
        self.view.set_mirrored(mirrored);
    }

    /// Whether a pan drag is in progress.
    pub fn is_panning(&self) -> bool {
        self.navigator.is_panning()
    }

    /// Changes the background color.
    pub fn set_background(&mut self, color: Color) {
        self.renderer.renderer_mut().set_background(color);
    }

    /// Changes the grid.
    pub fn set_grid(&mut self, grid: Option<Grid>) {
        if self.renderer.renderer().settings().grid != grid {
            self.renderer.renderer_mut().set_grid(grid);
        }
    }

    /// Renders the scene for a canvas element of `width` × `height` logical
    /// pixels at the window's `scale_factor`. `content` is the bounding box
    /// used for the automatic fit.
    pub fn render(
        &mut self,
        scene: &Scene,
        content: Option<Rect>,
        width: f32,
        height: f32,
        scale_factor: f32,
    ) -> Image {
        if width < 2.0 || height < 2.0 {
            return Image::default();
        }
        let old_size = self.view.size();
        librepcb_canvas::slint_adapter::sync_view_size(&mut self.view, width, height, scale_factor);
        if !self.sized {
            self.sized = true;
            let rect = self.pending_fit.take().or(content);
            self.fit(rect);
        } else if self.auto_fit && old_size != self.view.size() {
            self.fit(content);
        }
        match self.renderer.render(scene, &self.view) {
            Ok(img) => img,
            Err(e) => {
                log::error!("Failed to render scene: {e}");
                Image::default()
            }
        }
    }

    /// Zooms to fit `content` (or the default area) and enables the
    /// automatic fit (upstream `zoomToSceneRect(..., true)`).
    pub fn zoom_fit(&mut self, content: Option<Rect>) {
        if self.sized {
            self.fit(content);
        } else {
            self.pending_fit = content;
        }
        self.auto_fit = true;
    }

    /// Zooms to a rectangle without enabling the automatic fit.
    pub fn zoom_to(&mut self, rect: Rect) {
        self.fit(Some(rect));
        self.auto_fit = false;
    }

    /// Zooms in around the view center.
    pub fn zoom_in(&mut self) {
        self.zoom_center(ZOOM_STEP);
    }

    /// Zooms out around the view center.
    pub fn zoom_out(&mut self) {
        self.zoom_center(1.0 / ZOOM_STEP);
    }

    fn zoom_center(&mut self, factor: f64) {
        let size = self.view.size();
        self.view
            .zoom_at(Point::new(size.width / 2.0, size.height / 2.0), factor);
        self.auto_fit = false;
    }

    /// Scrolls by a fraction of the view size per step (upstream
    /// `scrollLeft()`, ... with 7 % of the view size): positive `dx` shows
    /// more on the right, positive `dy` more at the bottom.
    pub fn scroll_steps(&mut self, dx: f64, dy: f64) {
        let size = self.view.size();
        self.view.pan(Vec2::new(
            -dx * size.width * SCROLL_FACTOR,
            -dy * size.height * SCROLL_FACTOR,
        ));
        self.auto_fit = false;
    }

    /// Handles a pointer event at a position in logical pixels.
    pub fn pointer_event(
        &mut self,
        kind: PointerKind,
        button: PointerButton,
        pos: Point,
    ) -> PointerAction {
        let action = self
            .navigator
            .pointer_event(&mut self.view, kind, button, pos);
        if action == PointerAction::ViewChanged {
            self.auto_fit = false;
        }
        action
    }

    /// Handles a scroll event; returns whether the view changed.
    pub fn scroll_event(&mut self, pos: Point, delta: Vec2, modifiers: Modifiers) -> bool {
        let changed = self
            .navigator
            .scroll_event(&mut self.view, pos, delta, modifiers);
        if changed {
            self.auto_fit = false;
        }
        changed
    }

    /// Maps a position in logical pixels to world coordinates (mm).
    pub fn to_world(&self, pos: Point) -> Point {
        self.view.screen_to_world(pos)
    }

    /// Fits a rectangle with upstream's margins.
    fn fit(&mut self, rect: Option<Rect>) {
        let rect = rect
            .filter(|r| r.width().is_finite() && r.height().is_finite())
            .unwrap_or(self.default_rect);
        // Avoid infinite zoom for points and lines.
        let rect = if rect.width() < 1e-3 || rect.height() < 1e-3 {
            rect.inflate(1.0, 1.0)
        } else {
            rect
        };
        let size = self.view.size();
        let (l, t, r, b) = MARGINS;
        let avail_w = size.width - l - r;
        let avail_h = size.height - t - b;
        if avail_w < 2.0 || avail_h < 2.0 {
            self.view.fit(rect, 0.0);
            return;
        }
        let scale = (avail_w / rect.width()).min(avail_h / rect.height());
        self.view.set_scale(scale);
        self.view.center_on(rect.center());
        // Shift from the viewport center to the center of the area inside
        // the margins.
        self.view.pan(Vec2::new((l - r) / 2.0, (t - b) / 2.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_fit_until_navigation() {
        let scene = Scene::new();
        let content = Rect::new(0.0, 0.0, 100.0, 50.0);
        let mut v = CanvasView::new(Color::BLACK, DEFAULT_BOARD_RECT, Some(content));
        v.render(&scene, Some(content), 400.0, 300.0, 1.0);
        // Width limited: (400 - 80) / 100.
        assert!((v.view().scale() - 3.2).abs() < 1e-9);
        let center = v.view().world_to_screen(content.center());
        assert!((center.x - 200.0).abs() < 1e-9);
        assert!((center.y - (150.0 + 5.0)).abs() < 1e-9);
        // Resizing refits.
        v.render(&scene, Some(content), 600.0, 300.0, 1.0);
        assert!((v.view().scale() - 4.8).abs() < 1e-9);
        // After zooming, resizing keeps the scale.
        v.zoom_in();
        let s = v.view().scale();
        v.render(&scene, Some(content), 700.0, 300.0, 1.0);
        assert_eq!(v.view().scale(), s);
        v.zoom_fit(Some(content));
        assert!((v.view().scale() - 4.8).abs() < 1e-9);
    }
}
