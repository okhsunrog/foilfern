//! Errors of the [`fileio`](super) module.
//!
//! Replaces the `LogicError`, `RuntimeError` and `UserCanceled` exceptions
//! thrown by libs/librepcb/core/fileio/*. Messages are the upstream strings
//! with native path separators (like upstream `FilePath::toNative()`); only
//! those translated upstream go through [`tr!`], with the upstream class name
//! as translation context.

use std::io;

use librepcb_i18n::tr;

use super::FilePath;

/// Result type of the fileio module.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Error returned by file operations.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    // --- FileUtils ---------------------------------------------------------
    /// A file which must exist does not exist.
    #[error("{}", tr!("FileUtils", "The file \"{0}\" does not exist.", .0.to_native()))]
    FileNotFound(FilePath),
    /// A directory which must exist does not exist.
    #[error("{}", tr!("FileUtils", "The directory \"{0}\" does not exist.", .0.to_native()))]
    DirectoryNotFound(FilePath),
    /// A file or directory which must exist does not exist.
    #[error(
        "{}",
        tr!("FileUtils", "The file or directory \"{0}\" does not exist.", .0.to_native())
    )]
    FileOrDirectoryNotFound(FilePath),
    /// A file or directory which must not exist exists already.
    #[error(
        "{}",
        tr!("FileUtils", "The file or directory \"{0}\" exists already.", .0.to_native())
    )]
    AlreadyExists(FilePath),
    /// Opening a file for reading failed.
    #[error("{}", tr!("FileUtils", "Cannot open file \"{0}\": {1}", .path.to_native(), .source))]
    OpenFile {
        /// The file.
        path: FilePath,
        /// The underlying I/O error.
        source: io::Error,
    },
    /// Creating/opening a file for writing failed.
    #[error(
        "{}",
        tr!("FileUtils", "Could not open or create file \"{0}\": {1}", .path.to_native(), .source)
    )]
    CreateFile {
        /// The file.
        path: FilePath,
        /// The underlying I/O error.
        source: io::Error,
    },
    /// Writing (or committing) a file failed.
    #[error("{}", tr!("FileUtils", "Could not write to file \"{0}\": {1}", .path.to_native(), .source))]
    WriteFile {
        /// The file.
        path: FilePath,
        /// The underlying I/O error.
        source: io::Error,
    },
    /// Copying a file failed.
    #[error(
        "{}",
        tr!("FileUtils", "Could not copy file \"{0}\" to \"{1}\".", .source_path.to_native(), .destination.to_native())
    )]
    CopyFile {
        /// The source file.
        source_path: FilePath,
        /// The destination file.
        destination: FilePath,
        /// The underlying I/O error.
        source: io::Error,
    },
    /// Moving/renaming a file or directory failed.
    #[error(
        "{}",
        tr!("FileUtils", "Could not move \"{0}\" to \"{1}\".", .source_path.to_native(), .destination.to_native())
    )]
    Move {
        /// The source path.
        source_path: FilePath,
        /// The destination path.
        destination: FilePath,
        /// The underlying I/O error.
        source: io::Error,
    },
    /// Removing a file failed.
    #[error("{}", tr!("FileUtils", "Could not remove file \"{0}\".", .path.to_native()))]
    RemoveFile {
        /// The file.
        path: FilePath,
        /// The underlying I/O error.
        source: io::Error,
    },
    /// Removing a directory failed.
    #[error("{}", tr!("FileUtils", "Could not remove directory \"{0}\".", .path.to_native()))]
    RemoveDirectory {
        /// The directory.
        path: FilePath,
        /// The underlying I/O error.
        source: io::Error,
    },
    /// Creating a directory (with parents) failed.
    #[error(
        "{}",
        tr!("FileUtils", "Could not create directory or path \"{0}\".", .path.to_native())
    )]
    MakePath {
        /// The directory.
        path: FilePath,
        /// The underlying I/O error.
        source: io::Error,
    },
    /// Listing a directory failed (upstream silently returns an empty list).
    #[error("Could not read directory \"{}\": {source}", .path.to_native())]
    ReadDirectory {
        /// The directory.
        path: FilePath,
        /// The underlying I/O error.
        source: io::Error,
    },
    /// A path could not be represented as [`FilePath`] (upstream: invalid
    /// `FilePath` objects).
    #[error("Invalid file path: \"{0}\"")]
    InvalidPath(String),

    // --- TransactionalFileSystem -------------------------------------------
    /// A file does not exist in a transactional file system.
    #[error("{}", tr!("librepcb::TransactionalFileSystem", "File '{0}' does not exist.", .0.to_native()))]
    TransactionalFileNotFound(FilePath),
    /// A relative path points outside of a transactional file system.
    #[error("Attempted to access file outside sandboxed file system: {0}")]
    SandboxBreakout(String),
    /// Attempted to save a read-only transactional file system.
    #[error("{}", tr!("librepcb::TransactionalFileSystem", "File system is read-only."))]
    ReadOnly,
    /// An autosave backup exists and the restore mode is
    /// [`RestoreMode::Abort`](super::RestoreMode::Abort).
    #[error("Autosave backup detected in directory '{}'.", .0.to_native())]
    AutosaveDetected(FilePath),
    /// A backup/autosave index file is malformed.
    #[error(transparent)]
    Serialization(#[from] crate::serialization::Error),

    // --- DirectoryLock -----------------------------------------------------
    /// The directory to lock does not exist.
    #[error("{}", tr!("DirectoryLock", "The directory \"{0}\" does not exist.", .0.to_native()))]
    LockDirectoryNotFound(FilePath),
    /// No directory to lock was set.
    #[error("No directory to lock specified.")]
    NoDirectoryToLock,
    /// The lock file has less than 6 lines.
    #[error("{}", tr!("DirectoryLock", "The lock file \"{0}\" has too few lines.", .0.to_native()))]
    LockFileTooFewLines(FilePath),
    /// The directory is locked by someone else.
    #[error(
        "{}",
        tr!(
            "DirectoryLock",
            "Could not lock the directory \"{0}\" because it is already locked by \"{1}\". Close any application accessing this directory and try again.",
            .directory.to_native(),
            .user
        )
    )]
    AlreadyLocked {
        /// The locked directory.
        directory: FilePath,
        /// `user@host` of the lock owner.
        user: String,
    },
    /// Querying process information failed.
    #[error(transparent)]
    SystemInfo(#[from] crate::system_info::Error),

    // --- ZIP ---------------------------------------------------------------
    /// Opening a ZIP archive failed.
    #[error("Failed to open Zip file: {0}")]
    ZipOpen(String),
    /// Opening a ZIP file failed.
    #[error("Failed to open Zip file '{}': {message}", .path.to_native())]
    ZipOpenFile {
        /// The ZIP file.
        path: FilePath,
        /// Error description.
        message: String,
    },
    /// Reading an entry name failed or it is unsafe.
    #[error("Failed to get file name from Zip: {0}")]
    ZipFileName(String),
    /// Reading an entry failed.
    #[error("Failed to read from Zip: {0}")]
    ZipRead(String),
    /// Extracting an archive failed.
    #[error("Failed to extract Zip archive to '{}'.", .path.to_native())]
    ZipExtract {
        /// The target directory.
        path: FilePath,
        /// Error description.
        message: String,
    },
    /// Creating a ZIP file failed.
    #[error("Failed to create Zip file '{}': {message}", .path.to_native())]
    ZipCreateFile {
        /// The ZIP file.
        path: FilePath,
        /// Error description.
        message: String,
    },
    /// Writing an entry failed.
    #[error("Failed to write file in Zip: {0}")]
    ZipWrite(String),
    /// Finishing the archive failed.
    #[error("Failed to finish writing Zip: {0}")]
    ZipFinish(String),

    // --- CsvFile -----------------------------------------------------------
    /// A CSV row has a different number of values than the header.
    #[error("CSV value count is different to header item count.")]
    CsvValueCount,
    /// Writing CSV data failed.
    #[error("Failed to write CSV data: {0}")]
    Csv(String),

    // --- VersionFile -------------------------------------------------------
    /// Invalid version number.
    #[error(transparent)]
    Value(#[from] crate::types::Error),

    // --- OutputDirectoryWriter ---------------------------------------------
    /// The output directory index was not loaded.
    #[error("Output directory index not loaded.")]
    IndexNotLoaded,
    /// Invalid output file path.
    #[error("{}", tr!("librepcb::OutputDirectoryWriter", "The output file path '{0}' is invalid.", .0))]
    InvalidOutputPath(String),
    /// Output file path outside the output directory.
    #[error(
        "{}",
        tr!(
            "librepcb::OutputDirectoryWriter",
            "Attempted to write file '{0}' outside the output directory, which is not allowed!",
            .0
        )
    )]
    OutputPathOutsideDirectory(String),
    /// Absolute output file path.
    #[error(
        "{}",
        tr!(
            "librepcb::OutputDirectoryWriter",
            "The file path '{0}' is absolute, but only relative paths are allowed!",
            .0
        )
    )]
    AbsoluteOutputPath(String),
    /// Attempted to overwrite the index file.
    #[error("Attempted to overwrite the index file '{0}'.")]
    OverwriteOutputIndex(String),
    /// `|` in an output path.
    #[error("Sorry, the character '|' cannot be used in output filenames.")]
    PipeInOutputPath,
    /// The same output file is written multiple times.
    #[error(
        "{} {}",
        tr!("librepcb::OutputDirectoryWriter", "Attempted to write the output file '{0}' multiple times!", .0),
        tr!(
            "librepcb::OutputDirectoryWriter",
            "Make sure to specify unique output file paths, e.g. by using placeholders like '{0}' or '{1}'.",
            "{{BOARD}}",
            "{{VARIANT}}"
        )
    )]
    OutputFileWrittenMultipleTimes(String),

    // --- General -----------------------------------------------------------
    /// The operation was canceled by the user (upstream `UserCanceled`).
    #[error("User Canceled")]
    UserCanceled,
}

impl Error {
    /// Returns whether this is [`Error::UserCanceled`].
    pub fn is_user_canceled(&self) -> bool {
        matches!(self, Self::UserCanceled)
    }
}
