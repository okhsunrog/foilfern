//! Scene builders turning schematic pages and boards of a loaded
//! [`Project`](librepcb_core::project::Project) into
//! [`librepcb_canvas`] scenes, and a headless PNG rendering API (used by
//! the MCP `render` tool and later by the viewer).
//!
//! Ports the drawing semantics of upstream's painters
//! (libs/librepcb/core/project/schematic/schematicpainter.cpp,
//! libs/librepcb/core/project/board/boardpainter.cpp,
//! libs/librepcb/core/export/graphicspainter.cpp), the graphics items of
//! the editor (libs/librepcb/editor/project/{schematic,board}/graphicsitems)
//! and the default color schemes
//! (libs/librepcb/core/workspace/basecolorscheme.cpp).
//!
//! - [`SchematicScene`] and [`BoardScene`] build a [`Scene`] and map its
//!   items back to model objects ([`SchematicObject`], [`BoardObject`]) for
//!   hit testing. Canvas layers are color roles (schematic) or board layers
//!   and their derived layers ([`BoardSceneLayer`]).
//! - [`SymbolScene`] and [`FootprintScene`] build scenes of library
//!   elements.
//! - [`render_schematic_png()`] and [`render_board_png()`] render a page or
//!   a board side into a PNG, fitted to the content ([`RenderOptions`]).
//! - [`export`]: the graphics export (PDF, SVG, images) of upstream's
//!   `GraphicsExport` with the painters of schematics, boards, symbols and
//!   footprints, and the [`export::ProjectGraphicsExporter`] of the output
//!   job runner.
//!
//! - [`SchematicScene::apply_changes()`] and [`BoardScene::apply_changes()`]
//!   update a built scene incrementally from the project's change journal
//!   (only the items of the affected model objects are rebuilt);
//!   [`SceneSync`] keeps the journal cursor of a scene.
//!
//! The canvas crate stays generic; everything LibrePCB specific about
//! drawing lives here.
//!
//! [`Scene`]: librepcb_canvas::Scene

mod attributes;
mod board;
mod colors;
mod error;
pub mod export;
mod footprint;
mod render;
pub mod resources;
mod schematic;
mod shapes;
mod symbol;
mod sync;
mod text;
mod units;

pub use board::{BoardObject, BoardScene, BoardSceneLayer, BoardSide};
pub use colors::ColorScheme;
pub use error::{Error, Result};
pub use footprint::{FootprintScene, FootprintSceneObject};
pub use render::{
    MAX_IMAGE_SIZE, RenderOptions, RenderSize, RgbaImage, render_board_png, render_board_scene,
    render_scene, render_schematic_png, render_schematic_scene, visible_bounds,
};
pub use schematic::{SchematicObject, SchematicScene};
pub use symbol::{SymbolScene, SymbolSceneObject};
pub use sync::{IncrementalScene, SceneSync};

/// Loads the application's default stroke font (upstream
/// `Application::getDefaultStrokeFont()`, `newstroke.bene`) from the
/// resources directory (`$LIBREPCB_SHARE/fontobene` or
/// `../share/librepcb/fontobene` relative to the executable), else the font
/// embedded into the binary (see [`resources`]). Always `Some` (kept as
/// `Option` for callers which handle a missing font).
pub fn default_stroke_font() -> Option<librepcb_core::font::StrokeFont> {
    Some(librepcb_core::font::StrokeFont::new(
        resources::default_stroke_font_data(),
    ))
}

pub use librepcb_canvas;
