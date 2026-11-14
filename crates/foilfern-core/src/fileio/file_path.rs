//! Port of libs/librepcb/core/fileio/filepath.{h,cpp}.
//!
//! # Why a wrapper type instead of plain `PathBuf`
//!
//! [`FilePath`] is a thin validated wrapper which guarantees the upstream
//! "well-formatted path" invariants the rest of the code base (and the file
//! formats) rely on:
//!
//! - always absolute (relative paths are rejected, there is no "invalid"
//!   state: APIs use `Option<FilePath>` where upstream allows invalid paths);
//! - lexically cleaned: no `.`/`..` segments, no redundant or trailing
//!   separators (like `QDir::cleanPath()`);
//! - `/` as separator on every platform ([`FilePath::as_str()`], upstream
//!   `toStr()`), native separators only on request ([`FilePath::to_native()`]);
//! - relative paths produced by [`FilePath::to_relative()`] always use `/`,
//!   as they are stored in files which must be platform independent;
//! - equality and ordering are defined on the cleaned string, and
//!   [`FilePath::is_located_in_dir()`] is a case-insensitive prefix check,
//!   like upstream.
//!
//! A plain `PathBuf` provides none of these guarantees (`Path::components()`
//! does not resolve `..`, `PathBuf` equality depends on how a path was
//! built, `Display` uses native separators), so every caller would have to
//! re-implement them. The wrapper stores a UTF-8 `String` (upstream
//! `QString`); paths which are not valid Unicode are rejected. It implements
//! [`AsRef<Path>`] so it can be passed to `std::fs` directly.
//!
//! Lexical cleaning uses [`path_clean`], relative paths [`pathdiff`], and
//! symlink resolution [`dunce`] (to avoid `\\?\` prefixes on Windows).

use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use unicode_normalization::UnicodeNormalization;

use super::error::Error;

/// Letter case handling of [`FilePath::clean_file_name()`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FileNameCase {
    /// Keep the case (upstream `KeepCase`).
    #[default]
    Keep,
    /// Convert to lower case (upstream `ToLowerCase`).
    Lower,
    /// Convert to upper case (upstream `ToUpperCase`).
    Upper,
}

/// Options of [`FilePath::clean_file_name()`] (upstream
/// `CleanFileNameOptions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CleanFileNameOptions {
    /// Replace spaces by underscores (upstream `ReplaceSpaces`).
    pub replace_spaces: bool,
    /// Letter case conversion.
    pub case: FileNameCase,
}

impl CleanFileNameOptions {
    /// Keep spaces and case (upstream `Default`).
    pub const DEFAULT: Self = Self {
        replace_spaces: false,
        case: FileNameCase::Keep,
    };

    /// Creates options from the individual flags.
    pub const fn new(replace_spaces: bool, case: FileNameCase) -> Self {
        Self {
            replace_spaces,
            case,
        }
    }
}

/// An absolute, well-formatted path to a file or directory (which may or may
/// not exist).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FilePath(String);

impl FilePath {
    /// Creates a well-formatted path from an absolute path (upstream
    /// `FilePath(QString)` / `setPath()`).
    ///
    /// `.`, `..` and redundant separators are allowed and cleaned. On Windows,
    /// both `/` and `\` are accepted as separators; on other platforms, `\`
    /// is a regular character. Returns `None` for relative, empty or
    /// non-Unicode paths.
    pub fn new(path: impl AsRef<Path>) -> Option<Self> {
        let cleaned = make_well_formatted(path.as_ref().to_str()?);
        Path::new(&cleaned).is_absolute().then_some(Self(cleaned))
    }

    /// Returns the path with `/` separators (upstream `toStr()`).
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the path as [`Path`].
    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }

    /// Returns the path with native separators (upstream `toNative()`).
    pub fn to_native(&self) -> String {
        to_native_separators(&self.0)
    }

    /// Checks whether the path points to an existing file.
    pub fn is_existing_file(&self) -> bool {
        self.as_path().is_file()
    }

    /// Checks whether the path points to an existing directory.
    pub fn is_existing_dir(&self) -> bool {
        self.as_path().is_dir()
    }

    /// Checks whether the path points to an existing, empty directory.
    pub fn is_empty_dir(&self) -> bool {
        std::fs::read_dir(self.as_path()).is_ok_and(|mut entries| entries.next().is_none())
    }

    /// Checks whether the path is the file system root (e.g. `/` or `C:/`).
    pub fn is_root(&self) -> bool {
        self.as_path().parent().is_none()
    }

    /// Checks whether the path is located inside `dir` (case-insensitive
    /// string comparison, like upstream).
    pub fn is_located_in_dir(&self, dir: &FilePath) -> bool {
        // Upstream appends "/" unconditionally, which never matches for the
        // root directory; the root is handled explicitly here.
        let prefix = if dir.0.ends_with('/') {
            dir.0.clone()
        } else {
            format!("{}/", dir.0)
        };
        self.0.to_lowercase().starts_with(&prefix.to_lowercase())
    }

    /// Resolves symbolic links if the path exists, otherwise returns the
    /// path unchanged (upstream `toUnique()`).
    pub fn to_unique(&self) -> FilePath {
        dunce::canonicalize(self.as_path())
            .ok()
            .and_then(FilePath::new)
            .unwrap_or_else(|| self.clone())
    }

    /// Converts the path to a relative path with `/` separators (upstream
    /// `toRelative()`).
    ///
    /// `base` must be a directory. The result may contain `..` and is empty
    /// if both paths are equal. If there is no relative path (different
    /// drives on Windows), the absolute path is returned.
    pub fn to_relative(&self, base: &FilePath) -> String {
        let prefix = |p: &FilePath| match p.as_path().components().next() {
            Some(Component::Prefix(prefix)) => Some(prefix.as_os_str().to_ascii_lowercase()),
            _ => None,
        };
        if prefix(self) != prefix(base) {
            return self.0.clone();
        }
        match pathdiff::diff_paths(self.as_path(), base.as_path()) {
            Some(relative) => join_components(&relative),
            None => self.0.clone(),
        }
    }

    /// Same as [`to_relative()`](Self::to_relative), but with native
    /// separators.
    pub fn to_relative_native(&self, base: &FilePath) -> String {
        to_native_separators(&self.to_relative(base))
    }

    /// Returns the file name before the first `.` (upstream `getBasename()`).
    pub fn basename(&self) -> &str {
        let name = self.file_name();
        name.split_once('.').map_or(name, |(base, _)| base)
    }

    /// Returns the file name before the last `.` (upstream
    /// `getCompleteBasename()`).
    pub fn complete_basename(&self) -> &str {
        let name = self.file_name();
        name.rsplit_once('.').map_or(name, |(base, _)| base)
    }

    /// Returns the file name after the last `.` (upstream `getSuffix()`).
    pub fn suffix(&self) -> &str {
        self.file_name().rsplit_once('.').map_or("", |(_, s)| s)
    }

    /// Returns the file name after the first `.` (upstream
    /// `getCompleteSuffix()`).
    pub fn complete_suffix(&self) -> &str {
        self.file_name().split_once('.').map_or("", |(_, s)| s)
    }

    /// Returns the file or directory name without path (empty for the root).
    pub fn file_name(&self) -> &str {
        if self.is_root() {
            ""
        } else {
            self.0.rsplit_once('/').map_or(self.0.as_str(), |(_, n)| n)
        }
    }

    /// Returns the parent directory, or `None` for the root.
    pub fn parent_dir(&self) -> Option<FilePath> {
        self.as_path().parent().and_then(FilePath::new)
    }

    /// Returns the path of `relative` inside this directory (upstream
    /// `getPathTo()` / `fromRelative()`).
    ///
    /// `relative` may contain subdirectories (`subdir/file.txt`), `.` and
    /// `..`; the result is cleaned.
    pub fn path_to(&self, relative: &str) -> FilePath {
        let joined = make_well_formatted(&format!("{}/{}", self.0, relative));
        // Invariant: cleaning an absolute path keeps it absolute (".." at the
        // root is dropped), so the result is always a valid FilePath.
        debug_assert!(Path::new(&joined).is_absolute());
        Self(joined)
    }

    /// Same as `base.path_to(relative)` (upstream `fromRelative()`).
    pub fn from_relative(base: &FilePath, relative: &str) -> FilePath {
        base.path_to(relative)
    }

    /// Cleans a string so it becomes a valid, platform independent file name
    /// (upstream `cleanFileName()`).
    ///
    /// Only `-._ 0-9A-Za-z` are kept (after NFKD decomposition), leading and
    /// trailing spaces are removed and the length is limited to `max_length`
    /// characters (upstream default: 120). The result may be empty.
    pub fn clean_file_name(
        user_input: &str,
        options: CleanFileNameOptions,
        max_length: usize,
    ) -> String {
        let filtered: String = user_input
            .nfkd()
            .filter(|&c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | ' '))
            .collect();
        let mut ret = filtered.trim_matches(' ').to_owned();
        if options.replace_spaces {
            ret = ret.replace(' ', "_");
        }
        match options.case {
            FileNameCase::Keep => {}
            FileNameCase::Lower => ret.make_ascii_lowercase(),
            FileNameCase::Upper => ret.make_ascii_uppercase(),
        }
        // Only ASCII characters are left, so byte length == char count.
        ret.truncate(max_length);
        ret
    }
}

/// Cleans a path lexically and converts it to `/` separators (upstream
/// `makeWellFormatted()`, except making it absolute).
fn make_well_formatted(path: &str) -> String {
    #[cfg(windows)]
    let path = &path.replace('\\', "/");
    let cleaned = path_clean::clean(path);
    let mut s = join_components(&cleaned);
    // Windows drive paths must end with a slash ("C:" -> "C:/").
    if s.len() == 2 && s.ends_with(':') {
        s.push('/');
    }
    s
}

/// Joins the components of `path` with `/` (`.` becomes empty).
fn join_components(path: &Path) -> String {
    let mut out = String::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                out.push_str(&prefix.as_os_str().to_string_lossy().replace('\\', "/"));
            }
            Component::RootDir => {
                if !out.ends_with('/') {
                    out.push('/');
                }
            }
            Component::CurDir => {}
            Component::ParentDir | Component::Normal(_) => {
                if !out.is_empty() && !out.ends_with('/') {
                    out.push('/');
                }
                out.push_str(&component.as_os_str().to_string_lossy());
            }
        }
    }
    out
}

/// Converts `/` to the native separator (upstream
/// `QDir::toNativeSeparators()`).
fn to_native_separators(path: &str) -> String {
    if cfg!(windows) {
        path.replace('/', "\\")
    } else {
        path.to_owned()
    }
}

impl fmt::Debug for FilePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FilePath({})", self.0)
    }
}

impl fmt::Display for FilePath {
    /// Formats the path with `/` separators (upstream `toStr()`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<Path> for FilePath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl From<FilePath> for PathBuf {
    fn from(path: FilePath) -> Self {
        PathBuf::from(path.0)
    }
}

impl FromStr for FilePath {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        Self::new(s).ok_or_else(|| Error::InvalidPath(s.to_owned()))
    }
}

impl TryFrom<&Path> for FilePath {
    type Error = Error;
    fn try_from(path: &Path) -> Result<Self, Error> {
        Self::new(path).ok_or_else(|| Error::InvalidPath(path.display().to_string()))
    }
}

impl TryFrom<PathBuf> for FilePath {
    type Error = Error;
    fn try_from(path: PathBuf) -> Result<Self, Error> {
        Self::try_from(path.as_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_parts() {
        let p = FilePath::new("/foo/.hidden.tar.gz").unwrap();
        assert_eq!(p.file_name(), ".hidden.tar.gz");
        assert_eq!(p.basename(), "");
        assert_eq!(p.complete_basename(), ".hidden.tar");
        assert_eq!(p.suffix(), "gz");
        assert_eq!(p.complete_suffix(), "hidden.tar.gz");
        let p = FilePath::new("/foo/bar").unwrap();
        assert_eq!(p.basename(), "bar");
        assert_eq!(p.suffix(), "");
        assert_eq!(FilePath::new("/").unwrap().file_name(), "");
    }

    #[test]
    fn parent_and_path_to() {
        let p = FilePath::new("/foo/bar").unwrap();
        assert_eq!(p.parent_dir().unwrap().as_str(), "/foo");
        assert_eq!(p.path_to("../x/./y/").as_str(), "/foo/x/y");
        assert_eq!(p.path_to("").as_str(), "/foo/bar");
        assert_eq!(p.path_to("../../../..").as_str(), "/");
        assert!(FilePath::new("/").unwrap().parent_dir().is_none());
    }

    #[test]
    fn located_in_dir() {
        let dir = FilePath::new("/foo/Bar").unwrap();
        assert!(FilePath::new("/foo/bar/x").unwrap().is_located_in_dir(&dir));
        assert!(!FilePath::new("/foo/bar").unwrap().is_located_in_dir(&dir));
        assert!(!FilePath::new("/foo/barx").unwrap().is_located_in_dir(&dir));
        let root = FilePath::new("/").unwrap();
        assert!(FilePath::new("/foo").unwrap().is_located_in_dir(&root));
    }
}
