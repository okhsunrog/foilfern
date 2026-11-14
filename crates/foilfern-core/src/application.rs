//! Port of the file format part of libs/librepcb/core/application.{h,cpp}.
//!
//! Only the file format version is ported; the application version, build
//! information and resource paths belong to the application crates.

use crate::types::Version;

/// Upstream `LIBREPCB_FILE_FORMAT_VERSION`.
const FILE_FORMAT_VERSION: &str = "2";

/// Upstream `LIBREPCB_FILE_FORMAT_STABLE`.
const FILE_FORMAT_STABLE: bool = true;

/// Returns the version of the file format written by this implementation
/// (upstream `Application::getFileFormatVersion()`).
pub fn file_format_version() -> Version {
    // Invariant: the constant is a valid version number (checked by a test).
    FILE_FORMAT_VERSION
        .parse()
        .expect("FILE_FORMAT_VERSION is a valid version")
}

/// Returns whether the file format is stable, i.e. whether files written by
/// this implementation will be readable by future releases (upstream
/// `Application::isFileFormatStable()`).
pub fn is_file_format_stable() -> bool {
    FILE_FORMAT_STABLE
}

#[cfg(test)]
mod tests {
    #[test]
    fn file_format_version() {
        assert_eq!(super::file_format_version().to_string(), "2");
    }
}
