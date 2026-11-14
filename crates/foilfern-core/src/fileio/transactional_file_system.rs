//! Port of libs/librepcb/core/fileio/transactionalfilesystem.{h,cpp}.
//!
//! All modifications are kept in memory until [`save()`] writes them to disk.
//! The on-disk protocol is identical to upstream, so C++ and Rust LibrePCB
//! instances can restore each other's backups:
//!
//! - In read-write mode, the directory is locked with a [`DirectoryLock`].
//! - [`autosave()`] writes the pending modifications to `.autosave/`:
//!   the modified files into `.autosave/<yyyy-MM-dd_hh-mm-ss-zzz>/` and the
//!   index file `.autosave/autosave.lp` (an S-expression
//!   `(librepcb_autosave (created ...) (modified_files_directory "...")
//!   (modified_file "...")... (removed_file "...")... (removed_directory
//!   "...")...)`) last, which marks the diff as complete. After a crash, the
//!   next opening asks (via [`RestoreMode`]) whether to restore it.
//! - [`save()`] first writes the same diff to `.backup/` (`backup.lp`),
//!   removes `.autosave/`, applies all modifications (removed directories,
//!   removed files, then written files, each written atomically) and finally
//!   removes `.backup/` (index file first). If saving fails in between, the
//!   next opening restores `.backup/` automatically, so no modification is
//!   ever lost.
//! - Dropping a writable file system removes `.autosave/`, unless it was
//!   restored from an autosave and not saved since.
//!
//! All methods take `&self` and are thread-safe (the state is protected by a
//! mutex), so a file system can be shared with `Arc` like upstream's
//! `std::shared_ptr`.
//!
//! The staging logic and [`TransactionalFileSystem::clean_path()`] are
//! handwritten: the protocol is LibrePCB specific, and the relative paths are
//! platform independent strings (always `/`, no drive prefixes), which
//! `std::path`-based crates like `path-clean` do not model.
//!
//! [`save()`]: TransactionalFileSystem::save
//! [`autosave()`]: TransactionalFileSystem::autosave

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use chrono::{Local, Utc};

use super::directory_lock::{DirectoryLock, LockHandler};
use super::error::{Error, Result};
use super::file_path::FilePath;
use super::file_system::FileSystem;
use super::file_utils;
use super::zip_archive::ZipArchive;
use super::zip_writer::ZipWriter;
use crate::serialization::{List, Mode, SExpression};

/// Decides whether an existing autosave backup is restored when opening a
/// file system (upstream `RestoreCallback` / `RestoreMode`).
pub enum RestoreMode<'a> {
    /// Never restore a backup.
    No,
    /// Always restore the backup, if there is any.
    Yes,
    /// Fail with [`Error::AutosaveDetected`] if there is a backup.
    Abort,
    /// Ask the callback (called with the directory only if there is a
    /// backup); it may return an error to abort opening.
    Ask(&'a mut dyn FnMut(&FilePath) -> Result<bool>),
}

/// Filter for [`TransactionalFileSystem::export_to_zip()`]: returns whether
/// a file (relative path) is included.
pub type FileFilter<'a> = &'a dyn Fn(&str) -> bool;

/// Snapshot of the in-memory modifications (upstream `State`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    /// New or modified files (relative path → content).
    pub modified_files: HashMap<String, Vec<u8>>,
    /// Removed files (relative paths).
    pub removed_files: HashSet<String>,
    /// Removed directories (relative paths with trailing `/`, or `""` for
    /// the root).
    pub removed_dirs: HashSet<String>,
}

impl State {
    /// Checks whether `path` (a file, or a directory with trailing `/`) is
    /// removed (upstream `isRemoved()`).
    fn is_removed(&self, path: &str) -> bool {
        self.removed_files.contains(path) || self.removed_dirs.iter().any(|d| path.starts_with(d))
    }
}

struct Inner {
    state: Arc<State>,
    writable: bool,
    lock: DirectoryLock,
    restored_from_autosave: bool,
}

/// Transactional file system (see the [module docs](self)).
pub struct TransactionalFileSystem {
    path: FilePath,
    inner: Mutex<Inner>,
}

// Shared between threads via `Arc` (e.g. by `TransactionalDirectory`).
static_assertions::assert_impl_all!(TransactionalFileSystem: Send, Sync);

impl TransactionalFileSystem {
    /// Opens a directory (which may not exist yet).
    ///
    /// If `.backup/backup.lp` exists (i.e. the last save failed), the backup
    /// is restored. In `writable` mode, the directory is created and locked
    /// (`lock_handler` decides about overriding an existing lock). If
    /// `.autosave/autosave.lp` exists, `restore` decides whether it is
    /// restored.
    pub fn open(
        path: &FilePath,
        writable: bool,
        restore: RestoreMode<'_>,
        lock_handler: Option<LockHandler<'_>>,
    ) -> Result<Self> {
        // Note: `Self` is only constructed on success, since its `Drop` would
        // remove the autosave directory (upstream does not run the destructor
        // if the constructor throws).
        let mut inner = Inner {
            state: Arc::default(),
            writable,
            lock: DirectoryLock::new(path),
            restored_from_autosave: false,
        };

        // Load the backup if there is one (i.e. last save operation failed).
        let backup_file = path.path_to(".backup/backup.lp");
        if backup_file.is_existing_file() {
            log::debug!(
                "Restore file system from backup: {}",
                backup_file.to_native()
            );
            inner.state = load_diff(&backup_file)?.into();
        }

        // Lock directory if the file system is opened in R/W mode.
        if writable {
            file_utils::make_path(path)?;
            inner.lock.try_lock(lock_handler)?;
        }

        // If there is an autosave backup, load it according to the restore
        // mode.
        let autosave_file = path.path_to(".autosave/autosave.lp");
        if autosave_file.is_existing_file() {
            let restore = match restore {
                RestoreMode::No => false,
                RestoreMode::Yes => true,
                RestoreMode::Abort => return Err(Error::AutosaveDetected(path.clone())),
                RestoreMode::Ask(callback) => callback(path)?,
            };
            if restore {
                log::debug!(
                    "Restore file system from autosave backup: {}",
                    autosave_file.to_native()
                );
                inner.state = load_diff(&autosave_file)?.into();
                inner.restored_from_autosave = true;
            }
        }

        Ok(Self {
            path: path.clone(),
            inner: Mutex::new(inner),
        })
    }

    /// Opens a directory in read-only mode, without restoring autosave
    /// backups (upstream `openRO()`).
    pub fn open_ro(path: &FilePath) -> Result<Self> {
        Self::open(path, false, RestoreMode::No, None)
    }

    /// Opens a directory in read-write mode, without restoring autosave
    /// backups and without overriding locks (upstream `openRW()`).
    pub fn open_rw(path: &FilePath) -> Result<Self> {
        Self::open(path, true, RestoreMode::No, None)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Returns the root directory.
    pub fn path(&self) -> &FilePath {
        &self.path
    }

    /// Returns whether the file system is writable (i.e. can be saved).
    pub fn is_writable(&self) -> bool {
        self.lock().writable
    }

    /// Returns whether an autosave backup was restored when opening.
    pub fn is_restored_from_autosave(&self) -> bool {
        self.lock().restored_from_autosave
    }

    /// Returns the absolute path of `path`, or `None` if it is outside of
    /// the file system.
    pub fn abs_path(&self, path: &str) -> Option<FilePath> {
        let cleaned = Self::clean_path(path);
        self.check_if_path_is_safe(&cleaned)
            .then(|| self.path.path_to(&cleaned))
    }

    /// Returns the names of all directories in `path` (on disk and of new
    /// files, without removed ones; sorted).
    pub fn dirs(&self, path: &str) -> Vec<String> {
        let dir = Self::clean_path(path);
        if !self.check_if_path_is_safe(&dir) {
            return Vec::new();
        }
        dirs_locked(&self.path, &self.lock().state, &dir)
    }

    /// Returns the names of all files in `path` (on disk and new ones,
    /// without removed ones; sorted).
    pub fn files(&self, path: &str) -> Vec<String> {
        let dir = Self::clean_path(path);
        if !self.check_if_path_is_safe(&dir) {
            return Vec::new();
        }
        files_locked(&self.path, &self.lock().state, &dir)
    }

    /// Returns whether the file `path` exists.
    pub fn file_exists(&self, path: &str) -> bool {
        let cleaned = Self::clean_path(path);
        if !self.check_if_path_is_safe(&cleaned) {
            return false;
        }
        let inner = self.lock();
        if inner.state.modified_files.contains_key(&cleaned) {
            true
        } else if inner.state.is_removed(&cleaned) {
            false
        } else {
            self.path.path_to(&cleaned).is_existing_file()
        }
    }

    /// Reads a file; fails if it does not exist.
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        self.read_if_exists(path)?.ok_or_else(|| {
            Error::TransactionalFileNotFound(self.path.path_to(&Self::clean_path(path)))
        })
    }

    /// Reads a file; `None` if it does not exist.
    pub fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let cleaned = Self::clean_path(path);
        self.sanitize_path(&cleaned)?;
        let inner = self.lock();
        read_locked(&self.path, &inner.state, &cleaned)
    }

    /// Writes (creates or overwrites) a file in memory.
    pub fn write(&self, path: &str, content: &[u8]) -> Result<()> {
        let cleaned = Self::clean_path(path);
        self.sanitize_path(&cleaned)?;
        let mut inner = self.lock();
        let state = Arc::make_mut(&mut inner.state);
        state.removed_files.remove(&cleaned);
        state.modified_files.insert(cleaned, content.to_vec());
        Ok(())
    }

    /// Renames a file in memory.
    pub fn rename_file(&self, src: &str, dst: &str) -> Result<()> {
        self.write(dst, &self.read(src)?)?;
        self.remove_file(src)
    }

    /// Removes a file in memory.
    pub fn remove_file(&self, path: &str) -> Result<()> {
        let cleaned = Self::clean_path(path);
        self.sanitize_path(&cleaned)?;
        let mut inner = self.lock();
        let state = Arc::make_mut(&mut inner.state);
        state.modified_files.remove(&cleaned);
        state.removed_files.insert(cleaned);
        Ok(())
    }

    /// Removes a directory recursively in memory (`""` removes everything).
    pub fn remove_dir_recursively(&self, path: &str) -> Result<()> {
        let mut dirpath = Self::clean_path(path);
        self.sanitize_path(&dirpath)?;
        if !dirpath.is_empty() {
            dirpath.push('/');
        }
        let mut inner = self.lock();
        let state = Arc::make_mut(&mut inner.state);
        state
            .modified_files
            .retain(|fp, _| !fp.starts_with(&dirpath));
        state.removed_files.retain(|fp| !fp.starts_with(&dirpath));
        state.removed_dirs.insert(dirpath);
        Ok(())
    }

    /// Returns a snapshot of the in-memory modifications.
    pub fn save_state(&self) -> State {
        self.lock().state.as_ref().clone()
    }

    /// Restores a snapshot taken with [`save_state()`](Self::save_state).
    pub fn restore_state(&self, state: State) {
        self.lock().state = state.into();
    }

    /// Retains the overlay cheaply until a write needs a private copy.
    pub(crate) fn snapshot_overlay(&self) -> Arc<State> {
        Arc::clone(&self.lock().state)
    }

    pub(crate) fn restore_overlay(&self, state: Arc<State>) {
        self.lock().state = state;
    }

    /// Writes all files of an in-memory ZIP archive into the file system.
    pub fn load_from_zip_bytes(&self, content: impl Into<Vec<u8>>) -> Result<()> {
        self.load_from_zip_archive(ZipArchive::from_bytes(content)?)
    }

    /// Writes all files of a ZIP file into the file system.
    pub fn load_from_zip(&self, fp: &FilePath) -> Result<()> {
        self.load_from_zip_archive(ZipArchive::open(fp)?)
    }

    fn load_from_zip_archive(&self, mut zip: ZipArchive) -> Result<()> {
        let mut files = Vec::new();
        for i in 0..zip.len() {
            let name = zip.file_name(i)?;
            if !name.ends_with('/') && !name.ends_with('\\') {
                files.push((name, zip.read_file(i)?));
            }
        }
        for (name, content) in files {
            self.write(&name, &content)?;
        }
        Ok(())
    }

    /// Exports the file system (without dot-directories and the lock file)
    /// to an in-memory ZIP archive.
    pub fn export_to_zip(&self, filter: Option<FileFilter<'_>>) -> Result<Vec<u8>> {
        let mut zip = ZipWriter::new_in_memory();
        {
            let inner = self.lock();
            self.export_dir_to_zip(&inner.state, &mut zip, None, "", filter)?;
        }
        zip.finish()
    }

    /// Exports the file system to a ZIP file (see
    /// [`export_to_zip()`](Self::export_to_zip)). An incomplete file is
    /// removed on error.
    pub fn export_to_zip_file(&self, fp: &FilePath, filter: Option<FileFilter<'_>>) -> Result<()> {
        let result = ZipWriter::create(fp).and_then(|mut zip| {
            {
                let inner = self.lock();
                self.export_dir_to_zip(&inner.state, &mut zip, Some(fp), "", filter)?;
            }
            zip.finish()
        });
        if result.is_err() {
            // Remove the ZIP file because it is not complete.
            let _ = std::fs::remove_file(fp);
        }
        result
    }

    /// Discards all in-memory modifications.
    pub fn discard_changes(&self) {
        self.lock().state = Arc::default();
    }

    /// Returns the paths of all modifications which differ from the files
    /// on disk (removed directories have a trailing `/`).
    pub fn check_for_modifications(&self) -> Result<Vec<String>> {
        let inner = self.lock();
        let state = &inner.state;
        let mut modifications = Vec::new();
        // Removed directories.
        for dir in &state.removed_dirs {
            if self.path.path_to(dir).is_existing_dir() {
                modifications.push(dir.clone());
            }
        }
        // Removed files.
        for filepath in &state.removed_files {
            if self.path.path_to(filepath).is_existing_file() {
                modifications.push(filepath.clone());
            }
        }
        // New or modified files.
        for (filepath, content) in &state.modified_files {
            let fp = self.path.path_to(filepath);
            if !fp.is_existing_file() || file_utils::read_file(&fp)? != *content {
                modifications.push(filepath.clone());
            }
        }
        Ok(modifications)
    }

    /// Writes the in-memory modifications to the autosave directory.
    pub fn autosave(&self) -> Result<()> {
        let inner = self.lock();
        self.save_diff(&inner, "autosave")
    }

    /// Writes all in-memory modifications to disk (see the
    /// [module docs](self)).
    pub fn save(&self) -> Result<()> {
        let mut inner = self.lock();

        // Save to backup directory.
        self.save_diff(&inner, "backup")?;

        // Modifications are now saved to the backup directory, so there is
        // no risk of losing a restored autosave backup.
        inner.restored_from_autosave = false;

        // Remove the autosave directory because it is now older than the
        // backup content.
        self.remove_diff("autosave")?;

        let state = &inner.state;
        // Remove directories.
        for dir in &state.removed_dirs {
            let fp = self.path.path_to(dir);
            if fp.is_existing_dir() {
                file_utils::remove_dir_recursively(&fp)?;
            }
        }
        // Remove files.
        for filepath in &state.removed_files {
            let fp = self.path.path_to(filepath);
            if fp.is_existing_file() {
                file_utils::remove_file(&fp)?;
            }
        }
        // Save new or modified files.
        for (filepath, content) in &state.modified_files {
            file_utils::write_file(&self.path.path_to(filepath), content)?;
        }

        // Remove backup.
        self.remove_diff("backup")?;

        // Clear state.
        inner.state = Arc::default();
        Ok(())
    }

    /// Makes the file system read-only and releases the directory lock.
    pub fn release_lock(&self) -> Result<()> {
        let mut inner = self.lock();
        inner.writable = false;
        inner.lock.unlock_if_locked()?;
        Ok(())
    }

    /// Cleans a relative path: trims whitespace, converts `\` to `/`, removes
    /// empty and `.` segments and resolves `..` (leading `..` are kept, which
    /// makes the path invalid for all operations).
    ///
    /// Unlike upstream (which resolves `..` before converting backslashes on
    /// Unix), backslashes are always converted first, so `a\..\..\x` is
    /// rejected instead of escaping the sandbox via the later
    /// [`FilePath`] cleaning. A leading `/` is ignored, so `/../x` is
    /// rejected too (see COMPAT.md).
    pub fn clean_path(path: &str) -> String {
        let path = path.trim().replace('\\', "/");
        let mut segments: Vec<&str> = Vec::new();
        for segment in path.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    if segments.last().is_some_and(|s| *s != "..") {
                        segments.pop();
                    } else {
                        segments.push("..");
                    }
                }
                s => segments.push(s),
            }
        }
        segments.join("/").trim().to_owned()
    }

    fn check_if_path_is_safe(&self, cleaned: &str) -> bool {
        if cleaned.starts_with("..") {
            log::error!(
                "Attempted to access file outside sandboxed file system: {}",
                self.path.path_to(cleaned).to_native()
            );
            false
        } else {
            true
        }
    }

    fn sanitize_path(&self, cleaned: &str) -> Result<()> {
        if self.check_if_path_is_safe(cleaned) {
            Ok(())
        } else {
            Err(Error::SandboxBreakout(
                self.path.path_to(cleaned).to_native(),
            ))
        }
    }

    fn export_dir_to_zip<W: std::io::Write + std::io::Seek>(
        &self,
        state: &State,
        zip: &mut ZipWriter<W>,
        zip_fp: Option<&FilePath>,
        dir: &str,
        filter: Option<FileFilter<'_>>,
    ) -> Result<()> {
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        // Export directories, skipping dot-directories (e.g. ".git",
        // ".autosave", ".backup").
        for dirname in dirs_locked(&self.path, state, dir) {
            if !dirname.starts_with('.') {
                self.export_dir_to_zip(state, zip, zip_fp, &format!("{prefix}{dirname}"), filter)?;
            }
        }
        // Export files.
        let zip_rel = zip_fp.map(|fp| fp.to_relative(&self.path));
        for filename in files_locked(&self.path, state, dir) {
            let filepath = format!("{prefix}{filename}");
            // Skip the exported ZIP file itself if it is inside this file
            // system, the lock file and filtered files.
            if zip_rel.as_deref() == Some(filepath.as_str())
                || filename == ".lock"
                || filter.is_some_and(|f| !f(&filepath))
            {
                continue;
            }
            let content = read_locked(&self.path, state, &filepath)?
                .ok_or_else(|| Error::TransactionalFileNotFound(self.path.path_to(&filepath)))?;
            zip.write_file(&filepath, &content, 0o644)?;
        }
        Ok(())
    }

    fn save_diff(&self, inner: &Inner, kind: &str) -> Result<()> {
        let now = Local::now();
        let dir = self.path.path_to(&format!(".{kind}"));
        let files_dir = dir.path_to(&now.format("%Y-%m-%d_%H-%M-%S-%3f").to_string());

        if !inner.writable {
            return Err(Error::ReadOnly);
        }

        let state = &inner.state;
        let mut root = List::new(format!("librepcb_{kind}"));
        root.ensure_line_break();
        root.append_child("created", &now.with_timezone(&Utc));
        root.ensure_line_break();
        root.append_child("modified_files_directory", files_dir.file_name());
        let mut modified_files: Vec<_> = state.modified_files.iter().collect();
        modified_files.sort_unstable_by_key(|(path, _)| *path);
        for (filepath, content) in modified_files {
            root.ensure_line_break();
            root.append_child("modified_file", filepath);
            file_utils::write_file(&files_dir.path_to(filepath), content)?;
        }
        for filepath in sorted(&state.removed_files) {
            root.ensure_line_break();
            root.append_child("removed_file", filepath);
        }
        for filepath in sorted(&state.removed_dirs) {
            root.ensure_line_break();
            root.append_child("removed_directory", filepath);
        }
        root.ensure_line_break();

        // Writing the index file must be the last operation to "mark" this
        // diff as complete!
        let content = SExpression::from(root).to_byte_array(Mode::LibrePcb)?;
        file_utils::write_file(&dir.path_to(&format!("{kind}.lp")), &content)
    }

    fn remove_diff(&self, kind: &str) -> Result<()> {
        let dir = self.path.path_to(&format!(".{kind}"));
        let file = dir.path_to(&format!("{kind}.lp"));
        // Remove the index file first to mark the diff as incomplete.
        if file.is_existing_file() {
            file_utils::remove_file(&file)?;
        }
        file_utils::remove_dir_recursively(&dir)
    }
}

impl Drop for TransactionalFileSystem {
    fn drop(&mut self) {
        // The autosave directory is only needed after a crash. But in
        // read-only mode, or if an autosave was restored but not saved since,
        // it must be kept.
        let inner = self.inner.get_mut().unwrap_or_else(PoisonError::into_inner);
        if inner.writable
            && !inner.restored_from_autosave
            && let Err(e) = self.remove_diff("autosave")
        {
            log::warn!("Failed to remove autosave directory: {e}");
        }
    }
}

impl std::fmt::Debug for TransactionalFileSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransactionalFileSystem")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl FileSystem for TransactionalFileSystem {
    fn abs_path(&self, path: &str) -> Option<FilePath> {
        Self::abs_path(self, path)
    }
    fn dirs(&self, path: &str) -> Vec<String> {
        Self::dirs(self, path)
    }
    fn files(&self, path: &str) -> Vec<String> {
        Self::files(self, path)
    }
    fn file_exists(&self, path: &str) -> bool {
        Self::file_exists(self, path)
    }
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        Self::read(self, path)
    }
    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>> {
        Self::read_if_exists(self, path)
    }
    fn write(&mut self, path: &str, content: &[u8]) -> Result<()> {
        Self::write(self, path, content)
    }
    fn rename_file(&mut self, src: &str, dst: &str) -> Result<()> {
        Self::rename_file(self, src, dst)
    }
    fn remove_file(&mut self, path: &str) -> Result<()> {
        Self::remove_file(self, path)
    }
    fn remove_dir_recursively(&mut self, path: &str) -> Result<()> {
        Self::remove_dir_recursively(self, path)
    }
}

/// Reads a file of the file system with an already locked state.
fn read_locked(root: &FilePath, state: &State, cleaned: &str) -> Result<Option<Vec<u8>>> {
    if let Some(content) = state.modified_files.get(cleaned) {
        return Ok(Some(content.clone()));
    }
    if !state.is_removed(cleaned) {
        let fp = root.path_to(cleaned);
        if fp.is_existing_file() {
            return file_utils::read_file(&fp).map(Some);
        }
    }
    Ok(None)
}

/// [`TransactionalFileSystem::dirs()`] with an already locked state (`dir`
/// must be clean and safe).
fn dirs_locked(root: &FilePath, state: &State, dir: &str) -> Vec<String> {
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let mut names = std::collections::BTreeSet::new();
    for name in dir_entries(&root.path_to(dir), true) {
        if !state.is_removed(&format!("{prefix}{name}/")) {
            names.insert(name);
        }
    }
    for filepath in state.modified_files.keys() {
        if let Some(rel) = filepath.strip_prefix(&prefix)
            && let Some((first, _)) = rel.split_once('/')
        {
            names.insert(first.to_owned());
        }
    }
    names.into_iter().collect()
}

/// [`TransactionalFileSystem::files()`] with an already locked state.
fn files_locked(root: &FilePath, state: &State, dir: &str) -> Vec<String> {
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let mut names = std::collections::BTreeSet::new();
    for name in dir_entries(&root.path_to(dir), false) {
        if !state.is_removed(&format!("{prefix}{name}")) {
            names.insert(name);
        }
    }
    for filepath in state.modified_files.keys() {
        if let Some(rel) = filepath.strip_prefix(&prefix)
            && !rel.contains('/')
        {
            names.insert(rel.to_owned());
        }
    }
    names.into_iter().collect()
}

/// Returns the names of all (including hidden) directories or files in
/// `dir` on disk; empty if it does not exist.
fn dir_entries(dir: &FilePath, directories: bool) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            let path = e.path();
            if directories {
                path.is_dir()
            } else {
                path.is_file()
            }
        })
        .filter_map(|e| e.file_name().into_string().ok())
        .collect()
}

/// Returns the strings sorted (upstream sorts by UTF-16 code units, which
/// only differs for non-BMP characters; see COMPAT.md).
fn sorted<'a>(items: impl IntoIterator<Item = &'a String>) -> Vec<&'a String> {
    let mut v: Vec<_> = items.into_iter().collect();
    v.sort_unstable();
    v
}

/// Loads a diff index file written by `save_diff()` (upstream `loadDiff()`).
fn load_diff(fp: &FilePath) -> Result<State> {
    let root = SExpression::parse(
        &file_utils::read_file(fp)?,
        Some(fp.as_path()),
        Mode::LibrePcb,
    )?;
    let dir_name = root
        .required_child("modified_files_directory/@0")?
        .value()?;
    let modified_files_dir = fp
        .parent_dir()
        .unwrap_or_else(|| fp.clone())
        .path_to(dir_name);
    let mut state = State::default();
    for node in root.children_named("modified_file") {
        let rel_path = node.required_child("@0")?.value()?;
        let content = file_utils::read_file(&modified_files_dir.path_to(rel_path))?;
        state.modified_files.insert(rel_path.to_owned(), content);
    }
    for node in root.children_named("removed_file") {
        state
            .removed_files
            .insert(node.required_child("@0")?.value()?.to_owned());
    }
    for node in root.children_named("removed_directory") {
        state
            .removed_dirs
            .insert(node.required_child("@0")?.value()?.to_owned());
    }
    Ok(state)
}
