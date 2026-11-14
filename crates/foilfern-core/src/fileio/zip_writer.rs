//! Port of libs/librepcb/core/fileio/zipwriter.{h,cpp} and
//! libs/librepcb/rust-core/src/ffi/zip_writer_ffi.rs.
//!
//! Based on the [`zip`] crate (see the [module docs](super) for the crate
//! choice). Like upstream, entries are deflate-compressed and get the current
//! UTC time as modification time.

use std::fs::File;
use std::io::{Cursor, Seek, Write};

use chrono::{Datelike, Timelike, Utc};
use zip::write::SimpleFileOptions;

use super::error::{Error, Result};
use super::file_path::FilePath;

/// ZIP file writer, either in memory ([`ZipWriter::new_in_memory()`]) or to
/// a file ([`ZipWriter::create()`]).
///
/// [`finish()`](Self::finish) must be called after all files were written.
pub struct ZipWriter<W: Write + Seek> {
    zip: zip::ZipWriter<W>,
}

impl ZipWriter<Cursor<Vec<u8>>> {
    /// Creates an in-memory writer.
    pub fn new_in_memory() -> Self {
        Self {
            zip: zip::ZipWriter::new(Cursor::new(Vec::new())),
        }
    }

    /// Finishes writing and returns the ZIP content.
    pub fn finish(self) -> Result<Vec<u8>> {
        self.zip
            .finish()
            .map(Cursor::into_inner)
            .map_err(|e| Error::ZipFinish(e.to_string()))
    }
}

impl ZipWriter<File> {
    /// Creates (or overwrites) a ZIP file, creating missing parent
    /// directories.
    pub fn create(fp: &FilePath) -> Result<Self> {
        if let Some(parent) = fp.parent_dir()
            && !parent.is_existing_dir()
        {
            // Errors are reported by File::create() below (like upstream).
            let _ = std::fs::create_dir_all(&parent);
        }
        let file = File::create(fp).map_err(|e| Error::ZipCreateFile {
            path: fp.clone(),
            message: e.to_string(),
        })?;
        Ok(Self {
            zip: zip::ZipWriter::new(file),
        })
    }

    /// Finishes writing the file.
    pub fn finish(self) -> Result<()> {
        self.zip
            .finish()
            .map(|_| ())
            .map_err(|e| Error::ZipFinish(e.to_string()))
    }
}

impl<W: Write + Seek> ZipWriter<W> {
    /// Adds a file with the given path inside the archive, content and Unix
    /// permissions (e.g. `0o644`).
    pub fn write_file(&mut self, path: &str, data: &[u8], mode: u32) -> Result<()> {
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(now())
            .unix_permissions(mode)
            .large_file(data.len() as u64 >= u64::from(u32::MAX));
        self.zip
            .start_file(path, options)
            .map_err(|e| Error::ZipWrite(e.to_string()))?;
        self.zip
            .write_all(data)
            .map_err(|e| Error::ZipWrite(e.to_string()))
    }
}

/// Returns the current UTC time as ZIP timestamp (1980-01-01 if out of
/// range).
fn now() -> zip::DateTime {
    let now = Utc::now();
    let narrow = |v: u32| u8::try_from(v).unwrap_or(0);
    u16::try_from(now.year())
        .ok()
        .and_then(|year| {
            zip::DateTime::from_date_and_time(
                year,
                narrow(now.month()),
                narrow(now.day()),
                narrow(now.hour()),
                narrow(now.minute()),
                narrow(now.second()),
            )
            .ok()
        })
        .unwrap_or_default()
}

impl<W: Write + Seek> std::fmt::Debug for ZipWriter<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZipWriter").finish_non_exhaustive()
    }
}
