//! Port of libs/librepcb/core/fileio/transactionaldirectory.{h,cpp}.

use std::sync::Arc;

use super::error::Result;
use super::file_path::FilePath;
use super::file_system::FileSystem;
use super::file_utils;
use super::transactional_file_system::TransactionalFileSystem;

/// A subdirectory of a [`TransactionalFileSystem`], accessed as if it was the
/// root of a file system. Also allows copying/moving whole directories
/// between file systems.
///
/// Write access requires `&mut self`; like upstream, a (writable) view of a
/// subdirectory can only be derived from a mutable directory
/// ([`subdir()`](Self::subdir)).
#[derive(Debug)]
pub struct TransactionalDirectory {
    fs: Arc<TransactionalFileSystem>,
    path: String,
}

static_assertions::assert_impl_all!(TransactionalDirectory: Send, Sync);

impl TransactionalDirectory {
    /// Creates a directory in a new, empty temporary file system (opened
    /// read-only, i.e. without lock file and never saved).
    pub fn new_temporary() -> Result<Self> {
        let fs = TransactionalFileSystem::open_ro(&file_utils::random_temp_path())?;
        Ok(Self::new(Arc::new(fs), ""))
    }

    /// Creates a view of the directory `dir` in `fs`.
    pub fn new(fs: Arc<TransactionalFileSystem>, dir: &str) -> Self {
        Self {
            fs,
            path: TransactionalFileSystem::clean_path(dir),
        }
    }

    /// Creates a view of the subdirectory `subdir` of this directory
    /// (upstream copy constructor).
    pub fn subdir(&mut self, subdir: &str) -> Self {
        Self {
            fs: Arc::clone(&self.fs),
            path: TransactionalFileSystem::clean_path(&format!("{}/{subdir}", self.path)),
        }
    }

    /// Returns the underlying file system.
    pub fn file_system(&self) -> &Arc<TransactionalFileSystem> {
        &self.fs
    }

    /// Returns the path of this directory within the file system (cleaned,
    /// `""` for the root).
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns whether the file system is writable.
    pub fn is_writable(&self) -> bool {
        self.fs.is_writable()
    }

    /// Returns whether the file system was restored from an autosave backup.
    pub fn is_restored_from_autosave(&self) -> bool {
        self.fs.is_restored_from_autosave()
    }

    fn full(&self, path: &str) -> String {
        format!("{}/{path}", self.path)
    }

    /// Copies all files of this directory into `dest`.
    pub fn copy_to(&self, dest: &mut TransactionalDirectory) -> Result<()> {
        copy_dir_recursively(
            &self.fs,
            &with_trailing_slash(&self.path),
            &dest.fs,
            &with_trailing_slash(&dest.path),
        )
    }

    /// Copies all files into `dest` and makes this object refer to `dest`
    /// afterwards ("save as").
    pub fn save_to(&mut self, dest: &mut TransactionalDirectory) -> Result<()> {
        self.copy_to(dest)?;
        self.fs = Arc::clone(&dest.fs);
        self.path = dest.path.clone();
        Ok(())
    }

    /// Moves all files into `dest` (removing them here) and makes this object
    /// refer to `dest` afterwards.
    pub fn move_to(&mut self, dest: &mut TransactionalDirectory) -> Result<()> {
        self.copy_to(dest)?;
        self.fs.remove_dir_recursively(&self.path)?;
        self.fs = Arc::clone(&dest.fs);
        self.path = dest.path.clone();
        Ok(())
    }
}

impl FileSystem for TransactionalDirectory {
    fn abs_path(&self, path: &str) -> Option<FilePath> {
        self.fs.abs_path(&self.full(path))
    }
    fn dirs(&self, path: &str) -> Vec<String> {
        self.fs.dirs(&self.full(path))
    }
    fn files(&self, path: &str) -> Vec<String> {
        self.fs.files(&self.full(path))
    }
    fn file_exists(&self, path: &str) -> bool {
        self.fs.file_exists(&self.full(path))
    }
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        self.fs.read(&self.full(path))
    }
    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.fs.read_if_exists(&self.full(path))
    }
    fn write(&mut self, path: &str, content: &[u8]) -> Result<()> {
        self.fs.write(&self.full(path), content)
    }
    fn rename_file(&mut self, src: &str, dst: &str) -> Result<()> {
        self.fs.rename_file(&self.full(src), &self.full(dst))
    }
    fn remove_file(&mut self, path: &str) -> Result<()> {
        self.fs.remove_file(&self.full(path))
    }
    fn remove_dir_recursively(&mut self, path: &str) -> Result<()> {
        self.fs.remove_dir_recursively(&self.full(path))
    }
}

fn with_trailing_slash(path: &str) -> String {
    if path.is_empty() {
        String::new()
    } else {
        format!("{path}/")
    }
}

fn copy_dir_recursively(
    src_fs: &TransactionalFileSystem,
    src_dir: &str,
    dst_fs: &TransactionalFileSystem,
    dst_dir: &str,
) -> Result<()> {
    for file in src_fs.files(src_dir) {
        dst_fs.write(
            &format!("{dst_dir}{file}"),
            &src_fs.read(&format!("{src_dir}{file}"))?,
        )?;
    }
    for dir in src_fs.dirs(src_dir) {
        copy_dir_recursively(
            src_fs,
            &format!("{src_dir}{dir}/"),
            dst_fs,
            &format!("{dst_dir}{dir}/"),
        )?;
    }
    Ok(())
}
