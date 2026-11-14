//! Slint integration (feature `slint`): rendering into a [`slint::Image`]
//! and conversions of Slint's pointer types.
//!
//! The image works with every Slint renderer (software, femtovg, skia).
//! Typical `.slint` side (see `examples/canvas_demo`):
//!
//! ```slint,ignore
//! Image { source: root.frame; image-fit: fill; width: 100%; height: 100%; }
//! TouchArea {
//!     pointer-event(ev) => {
//!         if ev.kind == PointerEventKind.down { root.pointer-down(ev.button, self.mouse-x / 1px, self.mouse-y / 1px); }
//!         if ev.kind == PointerEventKind.up { root.pointer-up(ev.button, self.mouse-x / 1px, self.mouse-y / 1px); }
//!     }
//!     moved => { root.pointer-moved(self.mouse-x / 1px, self.mouse-y / 1px); }
//!     scroll-event(ev) => {
//!         root.scrolled(self.mouse-x / 1px, self.mouse-y / 1px, ev.delta-x / 1px, ev.delta-y / 1px,
//!                       ev.modifiers.shift, ev.modifiers.control);
//!         accept
//!     }
//! }
//! ```
//!
//! Slint's `PointerEvent`/`PointerEventKind` Rust types are not public API,
//! so the `.slint` code splits them into separate callbacks; only
//! [`slint::platform::PointerEventButton`] is converted here.

use slint::platform::PointerEventButton;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use crate::navigation::PointerButton;
use crate::render::{CpuRenderer, Error, Renderer};
use crate::{Scene, View};

impl From<PointerEventButton> for PointerButton {
    fn from(b: PointerEventButton) -> Self {
        match b {
            PointerEventButton::Left => Self::Left,
            PointerEventButton::Right => Self::Right,
            PointerEventButton::Middle => Self::Middle,
            _ => Self::Other,
        }
    }
}

/// Updates the view for the canvas element's size (logical pixels) and the
/// window's scale factor. Returns whether anything changed.
pub fn sync_view_size(view: &mut View, width: f32, height: f32, scale_factor: f32) -> bool {
    let before = *view;
    view.set_size((f64::from(width), f64::from(height)));
    view.set_device_pixel_ratio(f64::from(scale_factor));
    before != *view
}

/// Renders frames into [`slint::Image`]s, returning the previous image when
/// neither the scene nor the view changed.
pub struct ImageRenderer<R = CpuRenderer> {
    renderer: R,
    last: Option<(u64, View, Image)>,
    /// The pixels of the last image (shared with it, never modified): the
    /// renderer reuses them for panning and partial repaints.
    previous: Option<SharedPixelBuffer<Rgba8Pixel>>,
}

impl<R: Renderer> ImageRenderer<R> {
    /// Wraps a render backend.
    pub fn new(renderer: R) -> Self {
        Self {
            renderer,
            last: None,
            previous: None,
        }
    }

    /// The render backend (e.g. for [`Renderer::stats`]).
    pub fn renderer(&self) -> &R {
        &self.renderer
    }

    /// The render backend, mutably (e.g. to change settings).
    pub fn renderer_mut(&mut self) -> &mut R {
        self.last = None;
        self.previous = None;
        &mut self.renderer
    }

    /// Renders the scene (or returns the cached image if nothing changed).
    pub fn render(&mut self, scene: &Scene, view: &View) -> Result<Image, Error> {
        if let Some((rev, v, img)) = &self.last
            && *rev == scene.rev()
            && v == view
        {
            return Ok(img.clone());
        }
        let (w, h) = view.device_size();
        if w == 0 || h == 0 {
            // Slint does not accept empty pixel buffers.
            self.last = None;
            self.previous = None;
            return Ok(Image::default());
        }
        // Every frame gets a new buffer: Slint may still show (or have
        // uploaded) the previous image, so buffers are never modified in
        // place.
        let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
        let previous = self.previous.take();
        let result = self.renderer.render_into(
            scene,
            view,
            previous.as_ref().map(SharedPixelBuffer::as_bytes),
            buf.make_mut_bytes(),
        );
        if let Err(e) = result {
            self.last = None;
            return Err(e);
        }
        self.previous = Some(buf.clone());
        let img = Image::from_rgba8_premultiplied(buf);
        self.last = Some((scene.rev(), *view, img.clone()));
        Ok(img)
    }
}

impl Default for ImageRenderer<CpuRenderer> {
    fn default() -> Self {
        Self::new(CpuRenderer::default())
    }
}
