//! 2D canvas for schematics, boards, symbols and footprints.
//!
//! No upstream counterpart: this replaces the `QGraphicsScene` based
//! graphics items of `libs/librepcb/editor/graphics` with a retained scene
//! rendered by [vello_cpu](https://docs.rs/vello_cpu) into an image that any
//! Slint renderer can show. The architecture was chosen by the canvas spike
//! (`spikes/canvas/README.md`).
//!
//! # Model
//!
//! - A [`Scene`] holds [`Item`]s: kurbo [`Geometry`] in world coordinates
//!   (millimetres, **Y axis up**, like the LibrePCB model), a [`LayerId`], a
//!   z value and a [`Style`] (fill and/or stroke, layer or fixed colors,
//!   caps, joins, dashes, hairlines).
//! - Layers ([`Layer`]) carry colors, highlight colors, visibility and draw
//!   order; changing them repaints without re-encoding geometry.
//! - Items are batched into render groups per (z, layer, paint pass, world
//!   tile). Inserting, updating or removing an item re-encodes only its
//!   groups and repaints only its screen area.
//! - Selected items are drawn on top in their layer's highlight color.
//! - An `rstar` index plus exact kurbo tests answer point and rectangle
//!   queries ([`Scene::hit_test`], [`Scene::items_at`],
//!   [`Scene::items_in_rect`]), skipping hidden layers.
//!
//! The scene knows nothing about the LibrePCB model: board and schematic
//! scene builders map model objects to items (keeping their own
//! object → [`ItemId`] maps) and use [`convert`] for core geometry.
//!
//! # Rendering
//!
//! A [`View`] maps world to screen (pan, zoom around a point, fit, mirror,
//! device pixel ratio). A [`Renderer`] (currently [`CpuRenderer`]) paints the
//! scene through the view into premultiplied RGBA8, reusing the previous
//! frame where possible. With the `slint` feature, `slint_adapter` turns
//! frames into `slint::Image`s and converts Slint pointer buttons for the
//! toolkit independent [`Navigator`] (pan and zoom like upstream).
//!
//! ```
//! use librepcb_canvas::{CpuRenderer, Item, Layer, LayerId, Renderer, Scene, Style, View};
//! use librepcb_canvas::kurbo::{Circle, Point};
//! use librepcb_canvas::peniko::Color;
//!
//! let mut scene = Scene::new();
//! let top = LayerId(0);
//! scene.set_layer(top, Layer::new(Color::from_rgb8(0xcc, 0x08, 0x02)));
//! let pad = scene.insert(Item::new(top, Circle::new((1.0, 2.0), 0.5), Style::fill()));
//!
//! let mut view = View::new((320.0, 200.0), 1.0);
//! view.fit(scene.bounding_box().unwrap(), 10.0);
//! let mut renderer = CpuRenderer::default();
//! let (w, h) = view.device_size();
//! let rgba: &[u8] = renderer.render(&scene, &view).unwrap();
//! assert_eq!(rgba.len(), (w * h * 4) as usize);
//!
//! let world = view.screen_to_world(Point::new(160.0, 100.0));
//! assert_eq!(scene.hit_test(world, view.pixels_to_world(3.0)), Some(pad));
//! ```

pub mod convert;
mod geometry;
pub mod navigation;
pub mod render;
mod scene;
#[cfg(feature = "slint")]
pub mod slint_adapter;
pub mod stress;
mod style;
mod view;

pub use geometry::Geometry;
pub use navigation::{Modifiers, Navigator, PointerAction, PointerButton, PointerKind};
pub use render::{CpuRenderer, CpuRendererSettings, Grid, GridStyle, RenderStats, Renderer};
pub use scene::{
    Damage, GroupId, GroupView, Item, ItemId, Layer, LayerId, OverlayGroup, Paint, Scene,
    SelectionMode,
};
pub use style::{Brush, Dash, StrokeStyle, Style};
pub use view::View;

pub use kurbo;
pub use peniko;
