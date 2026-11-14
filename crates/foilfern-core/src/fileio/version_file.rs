//! Port of libs/librepcb/core/fileio/versionfile.{h,cpp}.

use super::error::Result;
use crate::types::Version;

/// Content of a version file (e.g. `.librepcb-project`): the file format
/// version on the first line, followed by `\n`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionFile {
    version: Version,
}

impl VersionFile {
    /// Creates a version file.
    pub fn new(version: Version) -> Self {
        Self { version }
    }

    /// Returns the version.
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// Sets the version.
    pub fn set_version(&mut self, version: Version) {
        self.version = version;
    }

    /// Returns the file content (`"<version>\n"`).
    pub fn to_bytes(&self) -> Vec<u8> {
        format!("{}\n", self.version).into_bytes()
    }

    /// Parses file content; only the first line is used.
    pub fn from_bytes(content: &[u8]) -> Result<Self> {
        let first_line = content.split(|&b| b == b'\n').next().unwrap_or_default();
        let version = String::from_utf8_lossy(first_line).parse::<Version>()?;
        Ok(Self { version })
    }
}
