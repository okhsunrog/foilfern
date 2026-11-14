//! Errors of the [`library`](super) module.
//!
//! Replaces the `RuntimeError`/`LogicError` exceptions thrown by the upstream
//! library element classes. Messages are the upstream strings; only those
//! translated upstream go through [`tr!`].

use foilfern_i18n::tr;

use crate::fileio::FilePath;
use crate::types::Version;

/// Result type of the library module.
pub type Result<T, E = Error> = std::result::Result<T, E>;

fn native(path: &Option<FilePath>) -> String {
    path.as_ref().map(FilePath::to_native).unwrap_or_default()
}

/// Error returned when loading, saving or checking library elements fails.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// File access failed.
    #[error(transparent)]
    FileIo(#[from] crate::fileio::Error),
    /// A file could not be parsed or deserialized.
    #[error(transparent)]
    Serialization(#[from] crate::serialization::Error),
    /// A package operation (e.g. the package check) failed.
    #[error(transparent)]
    Package(#[from] super::pkg::Error),
    /// The name of an element directory is not the element's UUID.
    #[error(
        "Directory name UUID mismatch: '{dir_name}' != '{uuid}'\n\nDirectory: '{}'",
        native(.path)
    )]
    DirectoryNameUuidMismatch {
        /// Name of the directory.
        dir_name: String,
        /// UUID of the element.
        uuid: String,
        /// The directory.
        path: Option<FilePath>,
    },
    /// The element was written by a newer application (file format).
    #[error(
        "{}",
        tr!(
            "librepcb::LibraryBaseElement",
            "This library element was created with a newer application version.\n\
             You need at least LibrePCB {0} to open it.\n\n{1}",
            .version.to_pretty_str(3, 10),
            native(.path)
        )
    )]
    NewerFileFormat {
        /// File format version of the element.
        version: Version,
        /// The element directory.
        path: Option<FilePath>,
    },
    /// Upgrading the element from an older file format failed.
    #[error(transparent)]
    Migration(#[from] crate::serialization::MigrationError),
    /// A library directory does not have the suffix `.lplib`.
    #[error(
        "The library directory does not have the suffix '.lplib':\n\n{}",
        native(.path)
    )]
    InvalidLibrarySuffix {
        /// The library directory.
        path: Option<FilePath>,
    },
    /// A library is moved to a directory without the suffix `.lplib`.
    #[error(
        "{}",
        tr!("librepcb::Library", "A library directory name must have the suffix '.lplib'.")
    )]
    InvalidLibraryDestination,
}
