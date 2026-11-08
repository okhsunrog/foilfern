//! Port of libs/librepcb/core/export/{graphicspainter,graphicsexport,
//! graphicsexportsettings}.{h,cpp} and the library/project page painters.
//!
//! Model geometry uses the existing core/canvas conversions. SVG composition,
//! font shaping, rasterization and PDF encoding use maintained Rust libraries.
//! Only LibrePCB's drawing order, units, settings and page semantics are ported.

mod drawing;
mod error;
mod export;
mod library;
mod palette;
mod schematic;
mod settings;

pub use drawing::Drawing;
pub use error::{Error, Result};
pub use export::{GraphicsExport, GraphicsFile, GraphicsPage};
pub use settings::GraphicsSettings;

pub use library::{default_stroke_font, footprint_drawing, symbol_drawing};
pub use schematic::schematic_drawing;
