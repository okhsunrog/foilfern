//! Port of libs/librepcb/core/fileio/ziparchive.{h,cpp} and
//! libs/librepcb/rust-core/src/ffi/zip_archive_ffi.rs.
//!
//! Based on the [`zip`] crate (see the [module docs](super) for the crate
//! choice). Entry names are validated like upstream (`enclosed_name()`: no
//! NUL bytes, not absolute, never leaving the archive root via `..`), but
//! returned unmodified.

use std::fs::File;
use std::io::{Cursor, Read, Seek};

use super::error::{Error, Result};
use super::file_path::FilePath;

/// Reader types backing a [`ZipArchive`].
trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

/// ZIP file reader.
pub struct ZipArchive {
    zip: zip::ZipArchive<Box<dyn ReadSeek>>,
}

impl ZipArchive {
    /// Opens an in-memory ZIP archive.
    pub fn from_bytes(data: impl Into<Vec<u8>>) -> Result<Self> {
        let reader: Box<dyn ReadSeek> = Box::new(Cursor::new(data.into()));
        zip::ZipArchive::new(reader)
            .map(|zip| Self { zip })
            .map_err(|e| Error::ZipOpen(e.to_string()))
    }

    /// Opens a ZIP file.
    pub fn open(fp: &FilePath) -> Result<Self> {
        let err = |message: String| Error::ZipOpenFile {
            path: fp.clone(),
            message,
        };
        let file = File::open(fp).map_err(|e| err(e.to_string()))?;
        let reader: Box<dyn ReadSeek> = Box::new(file);
        zip::ZipArchive::new(reader)
            .map(|zip| Self { zip })
            .map_err(|e| err(e.to_string()))
    }

    /// Returns the number of entries (files and directories).
    pub fn len(&self) -> usize {
        self.zip.len()
    }

    /// Returns whether the archive has no entries.
    pub fn is_empty(&self) -> bool {
        self.zip.is_empty()
    }

    /// Returns the name of an entry (a directory if it ends with `/` or `\`).
    ///
    /// Fails for invalid indices and invalid or unsafe names (NUL bytes,
    /// absolute paths, paths outside the archive).
    pub fn file_name(&mut self, index: usize) -> Result<String> {
        let file = self
            .zip
            .by_index(index)
            .map_err(|e| Error::ZipFileName(e.to_string()))?;
        match file.enclosed_name() {
            Some(_) => Ok(file.name().to_owned()),
            None => Err(Error::ZipFileName(
                "Zip contains invalid or unsafe paths.".into(),
            )),
        }
    }

    /// Reads the content of an entry.
    pub fn read_file(&mut self, index: usize) -> Result<Vec<u8>> {
        let mut file = self
            .zip
            .by_index(index)
            .map_err(|e| Error::ZipRead(e.to_string()))?;
        // The declared size is only a capacity hint (untrusted input).
        let mut buf = Vec::with_capacity(file.size().min(64 << 20) as usize);
        file.read_to_end(&mut buf)
            .map_err(|e| Error::ZipRead(e.to_string()))?;
        Ok(buf)
    }

    /// Finds a file by name and reads it; `None` if there is no such file.
    pub fn read_file_by_name(&mut self, file_name: &str) -> Result<Option<Vec<u8>>> {
        for i in 0..self.len() {
            if self.file_name(i)? == file_name {
                return self.read_file(i).map(Some);
            }
        }
        Ok(None)
    }

    /// Extracts the whole archive into `dir` (overwriting existing files).
    ///
    /// Fails on unsafe entries or I/O errors, in which case the extraction
    /// may be incomplete.
    pub fn extract_to(&mut self, dir: &FilePath) -> Result<()> {
        self.zip.extract(dir).map_err(|e| Error::ZipExtract {
            path: dir.clone(),
            message: e.to_string(),
        })
    }
}

impl std::fmt::Debug for ZipArchive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZipArchive")
            .field("len", &self.zip.len())
            .finish()
    }
}
