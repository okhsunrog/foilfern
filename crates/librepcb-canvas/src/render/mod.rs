//! Render backends.
//!
//! A backend turns a [`Scene`] seen through a [`View`] into pixels. It keeps
//! its own encoding of every render group (keyed by [`GroupId`], re-encoded
//! when [`GroupView::rev`] changes) and uses [`Scene::damage_since`] and
//! [`Scene::selection_rev`] to repaint only what changed.
//!
//! [`CpuRenderer`] (vello_cpu) is the only backend so far. A GPU backend
//! (vello on Slint's wgpu device) will implement the same trait, plus a
//! texture-based path in the Slint adapter.
//!
//! [`GroupId`]: crate::GroupId
//! [`GroupView::rev`]: crate::GroupView::rev

mod cpu;
mod filled_paths;

pub use cpu::{CpuRenderer, CpuRendererSettings};
pub use filled_paths::rasterize_filled_paths;

use crate::{Scene, View};

/// Errors of the render backends.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The target buffer does not match the view's device size.
    #[error("render target has {actual} bytes, expected {expected}")]
    TargetSize {
        /// Required bytes (width × height × 4).
        expected: usize,
        /// Provided bytes.
        actual: usize,
    },
    /// The viewport exceeds the renderer's size limit.
    #[error("viewport {width}x{height} exceeds the maximum of 65535x65535 pixels")]
    TooLarge {
        /// Width in device pixels.
        width: u32,
        /// Height in device pixels.
        height: u32,
    },
}

/// What the last frame cost.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenderStats {
    /// Whether the whole base image was repainted.
    pub full_repaint: bool,
    /// Repainted base regions in device pixels (x, y, width, height).
    pub regions: Vec<(u32, u32, u32, u32)>,
    /// Groups (re-)encoded this frame.
    pub encoded_groups: usize,
    /// Draw calls for the base image.
    pub drawn_groups: usize,
    /// Draw calls for the selection overlay.
    pub overlay_groups: usize,
    /// Wall time of the frame.
    pub time: std::time::Duration,
}

/// A render backend producing premultiplied RGBA8 pixels.
pub trait Renderer {
    /// Renders the scene into `target`, a buffer of premultiplied RGBA8,
    /// row-major, `view.device_size()` pixels, 4 bytes per pixel, no
    /// padding (e.g. a new `slint::SharedPixelBuffer`).
    ///
    /// `previous` must be the unmodified `target` of the previous call (or
    /// `None`); renderers reuse it for panning and partial repaints instead
    /// of keeping their own copy. Full repaints render straight into
    /// `target`.
    fn render_into(
        &mut self,
        scene: &Scene,
        view: &View,
        previous: Option<&[u8]>,
        target: &mut [u8],
    ) -> Result<(), Error>;

    /// Renders the scene into a buffer owned by the renderer and returns it
    /// (same format as [`render_into`](Self::render_into)). The buffer is
    /// valid until the next call.
    fn render(&mut self, scene: &Scene, view: &View) -> Result<&[u8], Error>;

    /// Statistics of the last [`render`](Self::render) call.
    fn stats(&self) -> &RenderStats;
}
