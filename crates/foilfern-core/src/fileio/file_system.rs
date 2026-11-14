//! Port of libs/librepcb/core/fileio/filesystem.h.

use super::error::Result;
use super::file_path::FilePath;

/// Interface of file system implementations
/// ([`TransactionalFileSystem`](super::TransactionalFileSystem),
/// [`TransactionalDirectory`](super::TransactionalDirectory)).
///
/// All paths are relative to the root of the file system, use `/` as
/// separator and are cleaned with
/// [`TransactionalFileSystem::clean_path()`](super::TransactionalFileSystem::clean_path).
/// An empty path denotes the root directory. Paths leaving the root (`..`)
/// are rejected.
pub trait FileSystem {
    /// Returns the absolute path of `path`, or `None` if it is outside of
    /// the file system.
    fn abs_path(&self, path: &str) -> Option<FilePath>;

    /// Returns the names of all directories in `path` (sorted).
    fn dirs(&self, path: &str) -> Vec<String>;

    /// Returns the names of all files in `path` (sorted).
    fn files(&self, path: &str) -> Vec<String>;

    /// Returns whether the file `path` exists.
    fn file_exists(&self, path: &str) -> bool;

    /// Reads a file; fails if it does not exist.
    fn read(&self, path: &str) -> Result<Vec<u8>>;

    /// Reads a file; `None` if it does not exist.
    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>>;

    /// Writes (creates or overwrites) a file.
    fn write(&mut self, path: &str, content: &[u8]) -> Result<()>;

    /// Renames a file.
    fn rename_file(&mut self, src: &str, dst: &str) -> Result<()>;

    /// Removes a file.
    fn remove_file(&mut self, path: &str) -> Result<()>;

    /// Removes a directory recursively (`""` removes everything).
    fn remove_dir_recursively(&mut self, path: &str) -> Result<()>;
}
