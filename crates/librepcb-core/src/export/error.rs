//! Errors of the [`export`](super) module.
//!
//! Replaces the `RuntimeError` exceptions thrown by the upstream generators.
//! Messages are the upstream strings; only those translated upstream go
//! through [`tr!`].

use librepcb_i18n::tr;

use crate::fileio;

/// Result type of the export module.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Error returned by the export generators.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A curved slot was exported with the G85 slot command enabled.
    #[error(
        "{}",
        tr!(
            "ExcellonGenerator",
            "Using the G85 slot command is not possible for curved slots. Either remove curved slots or disable the G85 export option."
        )
    )]
    CurvedSlotG85,
    /// The interactive HTML BOM could not be generated (invalid data).
    #[error("Failed to generate interactive HTML BOM: {0}")]
    InteractiveHtmlBom(String),
    /// A polygon clipping operation failed (e.g. calculating pad outlines).
    #[error(transparent)]
    Clipper(#[from] crate::utils::clipper_helpers::Error),
    /// Writing the output file (or building the CSV content) failed.
    #[error(transparent)]
    FileIo(#[from] fileio::Error),
}
