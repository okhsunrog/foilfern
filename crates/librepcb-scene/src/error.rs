//! Errors of the scene builders and the PNG rendering.

use librepcb_core::project::{BoardId, SchematicId};

/// Errors of this crate.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The project has no schematic with this ID.
    #[error("schematic {} does not exist", .0.0)]
    SchematicNotFound(SchematicId),
    /// The project has no board with this ID.
    #[error("board {} does not exist", .0.0)]
    BoardNotFound(BoardId),
    /// A model item could not be resolved (e.g. a missing library element).
    #[error(transparent)]
    Project(Box<librepcb_core::project::Error>),
    /// The requested image size is invalid (zero or too large).
    #[error("invalid image size {width}x{height}")]
    InvalidSize {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
    },
    /// Rendering failed.
    #[error(transparent)]
    Render(#[from] librepcb_canvas::render::Error),
    /// A polygon operation of the realistic board rendering failed.
    #[error(transparent)]
    Clipper(#[from] librepcb_core::utils::clipper_helpers::Error),
    /// PNG encoding failed.
    #[error("PNG encoding failed: {0}")]
    Png(#[from] png::EncodingError),
}

impl From<librepcb_core::project::Error> for Error {
    fn from(e: librepcb_core::project::Error) -> Self {
        Self::Project(Box::new(e))
    }
}

/// Result type of this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;
