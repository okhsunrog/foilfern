//! Port of libs/librepcb/core/font (stroke fonts for stroke texts).
//!
//! - [`fontobene`]: parser of the FontoBene stroke font format (port of
//!   libs/fontobene-qt).
//! - [`StrokeFont`]: renders texts into [`Path`](crate::geometry::Path)s.
//! - [`StrokeFontPool`]: the available fonts by file name.
//! - [`StrokeTextPathBuilder`]: builds the paths of stroke texts.
//!
//! Fonts are created from file contents; reading the font files is up to
//! the caller (file I/O layer).

pub mod fontobene;
mod stroke_font;
mod stroke_font_pool;
mod stroke_text_path_builder;

use foilfern_i18n::tr;

pub use stroke_font::{StrokeFont, StrokedLine, StrokedText};
pub use stroke_font_pool::StrokeFontPool;
pub use stroke_text_path_builder::StrokeTextPathBuilder;

/// Errors of the [`font`](self) module.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The requested font does not exist in the pool.
    #[error(
        "{}",
        tr!("StrokeFontPool", "The font \"{0}\" does not exist in the font pool.", .0)
    )]
    FontNotFound(String),
}
