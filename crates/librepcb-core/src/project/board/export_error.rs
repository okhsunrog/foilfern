//! Errors of the board export glue (plane fragments builder, Gerber/Excellon
//! export, pick&place generator, IPC-D-356A netlist export).
//!
//! Not an upstream class: upstream throws `LogicError`/`RuntimeError`
//! exceptions from these classes (and from the generators they drive). The
//! messages are upstream's.

use crate::export;
use crate::fileio;
use crate::font;
use crate::project;
use crate::utils::clipper_helpers;

/// Result type of the board export glue.
pub type BoardExportResult<T, E = BoardExportError> = std::result::Result<T, E>;

/// Error of the board exports.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BoardExportError {
    /// The board, a library element or another referenced entity does not
    /// exist.
    /// (Boxed because it is large.)
    #[error(transparent)]
    Project(Box<project::Error>),
    /// A generator failed (e.g. curved slots with G85 enabled) or a file
    /// could not be written.
    #[error(transparent)]
    Export(#[from] export::Error),
    /// A file operation failed.
    #[error(transparent)]
    FileIo(#[from] fileio::Error),
    /// A polygon clipping operation failed.
    #[error(transparent)]
    Clipper(#[from] clipper_helpers::Error),
    /// The stroke font of the board does not exist.
    #[error(transparent)]
    Font(#[from] font::Error),
    /// The plane fragments could not be calculated (upstream
    /// `BoardPlaneFragmentsBuilder::Result::throwOnError()`).
    #[error("Plane rebuild failed with {count} errors. First error: {first}")]
    PlaneRebuildFailed {
        /// Number of errors.
        count: usize,
        /// Message of the first error.
        first: String,
    },
    /// The output path of an export is invalid (e.g. empty after
    /// substituting the attributes).
    #[error("Invalid output file path: \"{0}\"")]
    InvalidOutputPath(String),
    /// A path has an invalid shape (e.g. a pad outline which is not a valid
    /// straight area, upstream exception of `StraightAreaPath`).
    #[error(transparent)]
    Geometry(#[from] crate::geometry::Error),
    /// The generated S-expression could not be serialized (e.g. an invalid
    /// token in the Specctra DSN export).
    #[error(transparent)]
    Serialization(#[from] crate::serialization::Error),
    /// An internal invariant was violated (upstream `LogicError`).
    #[error("Logic error: {0}")]
    Logic(&'static str),
}

impl From<project::Error> for BoardExportError {
    fn from(e: project::Error) -> Self {
        Self::Project(Box::new(e))
    }
}
