//! Port of libs/librepcb/core/fileio/fileutils.{h,cpp}.
//!
//! Free functions instead of a class with static methods. Crates used:
//! [`tempfile`] for atomic writes (upstream `QSaveFile`), [`walkdir`] for
//! recursive directory traversal and [`glob`] for name filters (upstream
//! `QDir::setNameFilters()`, case-insensitive wildcards).
//!
//! Also contains [`temp_dir()`] and [`random_temp_path()`], which are
//! `Application::getTempDir()` / `Application::getRandomTempPath()` upstream
//! (needed by [`TransactionalDirectory`](super::TransactionalDirectory)).

use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::OnceLock;

use walkdir::WalkDir;

use super::error::{Error, Result};
use super::file_path::FilePath;

/// Reads the content of a file.
pub fn read_file(filepath: &FilePath) -> Result<Vec<u8>> {
    if !filepath.is_existing_file() {
        return Err(Error::FileNotFound(filepath.clone()));
    }
    fs::read(filepath).map_err(|source| Error::OpenFile {
        path: filepath.clone(),
        source,
    })
}

/// Writes a file atomically (like `QSaveFile`), creating all parent
/// directories.
///
/// The content is written to a temporary file in the same directory which
/// then replaces the destination. Permissions of an existing file are kept,
/// new files get the default permissions (0666 & ~umask).
pub fn write_file(filepath: &FilePath, content: &[u8]) -> Result<()> {
    if let Some(parent) = filepath.parent_dir() {
        make_path(&parent)?;
    }
    let dir = filepath.parent_dir().unwrap_or_else(|| filepath.clone());
    let create_err = |source| Error::CreateFile {
        path: filepath.clone(),
        source,
    };
    let write_err = |source| Error::WriteFile {
        path: filepath.clone(),
        source,
    };
    let existing_permissions = fs::metadata(filepath).ok().map(|m| m.permissions());
    let mut builder = tempfile::Builder::new();
    builder.prefix(".").suffix(".tmp");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Applied through open(2), i.e. the umask is respected.
        builder.permissions(fs::Permissions::from_mode(0o666));
    }
    let mut file = builder.tempfile_in(dir.as_path()).map_err(create_err)?;
    file.write_all(content).map_err(write_err)?;
    file.as_file().sync_all().map_err(write_err)?;
    if let Some(permissions) = existing_permissions {
        // Best effort, like QSaveFile.
        let _ = file.as_file().set_permissions(permissions);
    }
    file.persist(filepath).map_err(|err| write_err(err.error))?;
    Ok(())
}

/// Copies a single file. `dest` must not exist.
pub fn copy_file(source: &FilePath, dest: &FilePath) -> Result<()> {
    if !source.is_existing_file() {
        return Err(Error::FileNotFound(source.clone()));
    }
    if exists(dest) {
        return Err(Error::AlreadyExists(dest.clone()));
    }
    fs::copy(source, dest)
        .map(|_| ())
        .map_err(|err| Error::CopyFile {
            source_path: source.clone(),
            destination: dest.clone(),
            source: err,
        })
}

/// Copies a directory recursively. `dest` must not exist.
///
/// Like upstream, hidden files are copied but hidden subdirectories (e.g.
/// `.git`) are skipped.
pub fn copy_dir_recursively(source: &FilePath, dest: &FilePath) -> Result<()> {
    if !source.is_existing_dir() {
        return Err(Error::DirectoryNotFound(source.clone()));
    }
    if exists(dest) {
        return Err(Error::AlreadyExists(dest.clone()));
    }
    make_path(dest)?;
    let walker = WalkDir::new(source)
        .min_depth(1)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| !(e.file_type().is_dir() && is_hidden(e)));
    for entry in walker {
        let entry = entry.map_err(|err| walkdir_error(source, err))?;
        let src = to_file_path(entry.path())?;
        let dst = dest.path_to(&src.to_relative(source));
        if entry.file_type().is_dir() {
            make_path(&dst)?;
        } else {
            copy_file(&src, &dst)?;
        }
    }
    Ok(())
}

/// Moves/renames a file or directory. `dest` must not exist; its parent
/// directories are created if needed.
pub fn move_path(source: &FilePath, dest: &FilePath) -> Result<()> {
    if !exists(source) {
        return Err(Error::FileOrDirectoryNotFound(source.clone()));
    }
    if exists(dest) {
        return Err(Error::AlreadyExists(dest.clone()));
    }
    if let Some(parent) = dest.parent_dir() {
        make_path(&parent)?;
    }
    fs::rename(source, dest).map_err(|err| Error::Move {
        source_path: source.clone(),
        destination: dest.clone(),
        source: err,
    })
}

/// Removes a single file. Fails if it does not exist (like `QFile::remove()`).
pub fn remove_file(file: &FilePath) -> Result<()> {
    fs::remove_file(file).map_err(|source| Error::RemoveFile {
        path: file.clone(),
        source,
    })
}

/// Removes a directory recursively. Succeeds if it does not exist (like
/// `QDir::removeRecursively()`).
pub fn remove_dir_recursively(dir: &FilePath) -> Result<()> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::RemoveDirectory {
            path: dir.clone(),
            source,
        }),
    }
}

/// Creates a directory with all parent directories. Succeeds if it exists
/// already.
pub fn make_path(path: &FilePath) -> Result<()> {
    fs::create_dir_all(path).map_err(|source| Error::MakePath {
        path: path.clone(),
        source,
    })
}

/// Returns all files in a directory, optionally filtered by name patterns
/// (e.g. `*.txt`, case-insensitive), recursively and without hidden files.
///
/// Hidden files are files starting with `.` (on Windows: files with the
/// hidden attribute). With `skip_hidden_files`, hidden directories are not
/// searched either (like upstream).
pub fn files_in_directory(
    dir: &FilePath,
    filters: &[&str],
    recursive: bool,
    skip_hidden_files: bool,
) -> Result<Vec<FilePath>> {
    if !dir.is_existing_dir() {
        return Err(Error::DirectoryNotFound(dir.clone()));
    }
    let patterns = filters
        .iter()
        .filter_map(|f| glob::Pattern::new(f).ok())
        .collect::<Vec<_>>();
    let options = glob::MatchOptions {
        case_sensitive: false,
        require_literal_separator: false,
        require_literal_leading_dot: false,
    };
    let walker = WalkDir::new(dir)
        .min_depth(1)
        .max_depth(if recursive { usize::MAX } else { 1 })
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| !(skip_hidden_files && e.depth() > 0 && is_hidden(e)));
    let mut files = Vec::new();
    for entry in walker {
        let entry = entry.map_err(|err| walkdir_error(dir, err))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if patterns.is_empty() || patterns.iter().any(|p| p.matches_with(&name, options)) {
            files.push(to_file_path(entry.path())?);
        }
    }
    Ok(files)
}

/// Returns all directories (including hidden ones) within a directory; empty
/// if it does not exist.
pub fn find_directories(root_dir: &FilePath) -> Vec<FilePath> {
    let Ok(entries) = fs::read_dir(root_dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| FilePath::new(e.path()))
        .collect()
}

/// Returns the per-user temporary directory of LibrePCB (upstream
/// `Application::getTempDir()`).
///
/// Uses `$LIBREPCB_TEMP_DIR` if set, otherwise `$XDG_RUNTIME_DIR/LibrePCB`
/// on Unix (except macOS) or the system temporary directory + `LibrePCB`.
pub fn temp_dir() -> &'static FilePath {
    static DIR: OnceLock<FilePath> = OnceLock::new();
    DIR.get_or_init(|| {
        if let Some(fp) = std::env::var_os("LIBREPCB_TEMP_DIR").and_then(FilePath::new) {
            return fp;
        }
        let base = if cfg!(all(unix, not(target_os = "macos"))) {
            std::env::var_os("XDG_RUNTIME_DIR").and_then(FilePath::new)
        } else {
            None
        };
        base.or_else(|| FilePath::new(std::env::temp_dir()))
            .or_else(|| FilePath::new(if cfg!(windows) { "C:/" } else { "/tmp" }))
            .map(|fp| fp.path_to("LibrePCB"))
            // Invariant: the literal fallbacks above are absolute paths.
            .expect("fallback temporary directory is absolute")
    })
}

/// Returns a new, random path in [`temp_dir()`] (upstream
/// `Application::getRandomTempPath()`). The path is not created.
pub fn random_temp_path() -> FilePath {
    let millis = chrono::Utc::now().timestamp_millis();
    // Truncation intended: 32 random bits like QRandomGenerator::generate().
    let random = uuid::Uuid::new_v4().as_u128() as u32;
    temp_dir().path_to(&format!("{millis}_{random}"))
}

/// Returns whether a file or directory exists at `path`.
fn exists(path: &FilePath) -> bool {
    path.is_existing_file() || path.is_existing_dir()
}

fn to_file_path(path: &Path) -> Result<FilePath> {
    FilePath::try_from(path)
}

fn walkdir_error(dir: &FilePath, err: walkdir::Error) -> Error {
    let path = err
        .path()
        .and_then(FilePath::new)
        .unwrap_or_else(|| dir.clone());
    Error::ReadDirectory {
        path,
        source: err.into(),
    }
}

/// Returns whether a directory entry is hidden (like `QFileInfo::isHidden()`).
fn is_hidden(entry: &walkdir::DirEntry) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        entry
            .metadata()
            .is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
    }
    #[cfg(not(windows))]
    {
        entry.file_name().to_string_lossy().starts_with('.')
    }
}
