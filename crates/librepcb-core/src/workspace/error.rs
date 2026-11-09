//! Errors of the [`workspace`](super) module.
//!
//! Replaces the `LogicError`/`RuntimeError` exceptions thrown by
//! libs/librepcb/core/workspace/*. Messages are the upstream strings; only
//! those translated upstream go through [`tr!`].

use librepcb_i18n::tr;

use super::ElementKind;
use crate::fileio::FilePath;
use crate::types::Version;

/// Result type of the workspace module.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Error returned by workspace operations.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The directory contains no `.librepcb-workspace` file.
    #[error(
        "{}",
        tr!(
            "librepcb::Workspace",
            "The directory \"{0}\" is not a valid LibrePCB workspace.",
            .0.to_native()
        )
    )]
    InvalidWorkspace(FilePath),
    /// The workspace has an unsupported file format version.
    #[error(
        "{}",
        tr!(
            "librepcb::Workspace",
            "The workspace \"{0}\" requires LibrePCB {1} or later.",
            .path.to_native(),
            .version
        )
    )]
    IncompatibleWorkspace {
        /// The workspace directory.
        path: FilePath,
        /// The workspace file format version found.
        version: Version,
    },
    /// The data directory was written by a newer LibrePCB version.
    #[error("Workspace data directory requires LibrePCB {0} or later to open..")]
    DataDirectoryTooNew(Version),
    /// A method was called with an element kind it does not support (upstream:
    /// `static_assert` / `LogicError`).
    #[error("Unsupported library element kind for this operation: {0:?}")]
    UnsupportedElementKind(ElementKind),
    /// Filtering `libraries` by library (upstream `LogicError`).
    #[error("Filtering for libraries makes no sense and doesn't work for libraries!")]
    LibraryFilterOnLibraries,
    /// The library database contains an invalid value (upstream
    /// `LogicError` or a value parse error).
    #[error("Invalid value in library database: {0}")]
    InvalidDatabaseValue(String),
    /// File access failed.
    #[error(transparent)]
    FileIo(#[from] crate::fileio::Error),
    /// A file could not be parsed.
    #[error(transparent)]
    Serialization(#[from] crate::serialization::Error),
    /// A file format migration failed.
    #[error(transparent)]
    Migration(#[from] crate::serialization::MigrationError),
    /// Opening a library or library element failed.
    #[error(transparent)]
    Library(#[from] crate::library::Error),
    /// A value is invalid.
    #[error(transparent)]
    Value(#[from] crate::types::Error),
    /// An attribute in the library database is invalid.
    #[error(transparent)]
    Attribute(#[from] crate::attribute::Error),
    /// Database access failed.
    #[error(transparent)]
    Database(#[from] crate::sqlite_database::Error),
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Database(e.into())
    }
}
