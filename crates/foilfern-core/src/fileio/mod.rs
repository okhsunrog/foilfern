//! Port of libs/librepcb/core/fileio (file system access).
//!
//! - [`FilePath`]: absolute, well-formatted paths (see its module docs for
//!   why this is a wrapper type and not a plain `PathBuf`).
//! - [`file_utils`]: basic file operations (upstream `FileUtils`).
//! - [`FileSystem`], [`TransactionalFileSystem`], [`TransactionalDirectory`]:
//!   in-memory staging of modifications with atomic save, autosave and
//!   backup restore.
//! - [`DirectoryLock`]: upstream-compatible `.lock` files.
//! - [`VersionFile`], [`CsvFile`], [`OutputDirectoryWriter`]: small file
//!   formats.
//! - [`ZipArchive`], [`ZipWriter`]: ZIP reading/writing.
//! - [`AsyncCopyOperation`]: recursive copy with progress, on a caller
//!   provided (or new) thread.
//!
//! All operations are synchronous.
//!
//! # ZIP crate
//!
//! Upstream's Rust core pins `zip = "=0.6.6"` because of maintenance and
//! security concerns about the `zip` 1.x/2.x releases (yanked versions,
//! breaking changes in patch releases, a large default dependency tree). As
//! of 2026, `zip` (maintained in the zip-rs/zip2 repository) has stabilized
//! at 8.x with regular releases, security fixes and very wide adoption,
//! while the suggested alternative `rc-zip-sync` can only *read* archives
//! (we need a writer too) and was last released in 2025. So [`zip`] 8.x is
//! used, with default features disabled and only pure-Rust deflate
//! (`deflate-flate2-zlib-rs`) enabled, which keeps the dependency tree small
//! (no bzip2/zstd/lzma/AES/zopfli) and covers every archive LibrePCB writes
//! and reads (`*.lppz`, `*.zip` library downloads).

mod async_copy_operation;
mod csv_file;
mod directory_lock;
mod error;
mod file_path;
mod file_system;
pub mod file_utils;
mod output_directory_writer;
mod transactional_directory;
mod transactional_file_system;
mod version_file;
mod zip_archive;
mod zip_writer;

pub use async_copy_operation::{
    AbortHandle, AsyncCopyOperation, CopyProgress, RunningCopyOperation,
};
pub use csv_file::CsvFile;
pub use directory_lock::{DirectoryLock, LockHandler, LockStatus};
pub use error::{Error, Result};
pub use file_path::{CleanFileNameOptions, FileNameCase, FilePath};
pub use file_system::FileSystem;
pub use output_directory_writer::{
    OutputDirectoryEvent, OutputDirectoryObserver, OutputDirectoryWriter,
};
pub use transactional_directory::TransactionalDirectory;
pub use transactional_file_system::{FileFilter, RestoreMode, State, TransactionalFileSystem};
pub use version_file::VersionFile;
pub use zip_archive::ZipArchive;
pub use zip_writer::ZipWriter;
