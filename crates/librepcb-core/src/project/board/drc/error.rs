//! Errors of the [`drc`](super) module.

use crate::font;
use crate::project;
use crate::project::board::BoardExportError;
use crate::types;
use crate::utils::clipper_helpers;

/// Errors of the design rule check (upstream exceptions thrown while
/// extracting the input data or while running a check).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The board or an element of the project does not exist (boxed
    /// because it is large).
    #[error(transparent)]
    Project(Box<project::Error>),
    /// A polygon operation failed.
    #[error(transparent)]
    Clipper(#[from] clipper_helpers::Error),
    /// The stroke font of the board is not available.
    #[error(transparent)]
    Font(#[from] font::Error),
    /// A calculated value is out of range (e.g. a non-positive diameter).
    #[error(transparent)]
    Value(#[from] types::Error),
    /// An air wire anchor refers to an object which is not part of the
    /// input data (upstream `LogicError`).
    #[error("Invalid air wire anchor!")]
    InvalidAirWireAnchor,
    /// A trace refers to an anchor which does not exist (cannot happen in
    /// a validated project).
    #[error("Invalid trace anchor: {0}")]
    InvalidTraceAnchor(crate::types::Uuid),
    /// The plane fragments could not be rebuilt (e.g. the stroke font of
    /// the board is missing). Boxed because it is large.
    #[error(transparent)]
    PlaneRebuild(Box<BoardExportError>),
}

impl From<BoardExportError> for Error {
    fn from(e: BoardExportError) -> Self {
        Self::PlaneRebuild(Box::new(e))
    }
}

impl From<project::Error> for Error {
    fn from(e: project::Error) -> Self {
        Self::Project(Box::new(e))
    }
}

/// Result type of the [`drc`](super) module.
pub type Result<T, E = Error> = std::result::Result<T, E>;
