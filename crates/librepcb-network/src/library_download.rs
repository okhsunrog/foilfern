//! Port of libs/librepcb/editor/library/librarydownload.{h,cpp}.
//!
//! Downloads a library ZIP and extracts it atomically into a library
//! directory (e.g. `<workspace>/data/libraries/remote/<uuid>.lplib`), based
//! on [`FileDownload`]: the ZIP is downloaded to `<dir>.zip`, extracted to
//! `<dir>.tmp`, and the library found in it (at the root or in the only
//! subdirectory) replaces `<dir>`.
//!
//! Differences to upstream: the download is an `async fn` returning the
//! library directory instead of emitting `finished(bool, QString)`;
//! aborting is done with a cancellation token (or by dropping the future).
//! It lives in this crate (upstream: editor) since it has no UI.

use std::collections::HashSet;
use std::sync::Arc;

use librepcb_core::fileio::{FilePath, file_utils};
use librepcb_core::library::{Library, LibraryBaseElement};
use reqwest::Url;
use tokio::sync::{Semaphore, watch};
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::file_download::{ChecksumAlgorithm, FileDownload};
use crate::network_access_manager::NetworkAccessManager;
use crate::network_request::Progress;

/// Downloads and extracts a library (see the [module docs](self)).
#[derive(Debug)]
pub struct LibraryDownload {
    download: FileDownload,
    dest_dir: FilePath,
    existing_dirs: HashSet<FilePath>,
}

impl LibraryDownload {
    /// Creates a download of the library ZIP `url_to_zip` into the library
    /// directory `dest_dir` (replaced if it exists).
    ///
    /// `semaphore` limits the number of parallel file operations (see
    /// [`FileDownload::new()`]).
    pub fn new(url_to_zip: Url, dest_dir: FilePath, semaphore: Arc<Semaphore>) -> Result<Self> {
        let zip_file = FilePath::new(format!("{dest_dir}.zip"))
            .ok_or_else(|| librepcb_core::fileio::Error::InvalidPath(format!("{dest_dir}.zip")))?;
        let download = FileDownload::new(url_to_zip, zip_file, semaphore)
            .zip_extraction_directory(dest_dir.clone(), Some(Box::new(find_library_in_zip)));
        Ok(Self {
            download,
            dest_dir,
            existing_dirs: HashSet::new(),
        })
    }

    /// Sets the expected size of the ZIP file (for the progress).
    pub fn expected_zip_file_size(mut self, bytes: u64) -> Self {
        self.download = self.download.expected_reply_content_size(bytes);
        self
    }

    /// Sets the expected checksum (raw bytes) of the ZIP file.
    pub fn expected_checksum(mut self, algorithm: ChecksumAlgorithm, checksum: Vec<u8>) -> Self {
        self.download = self.download.expected_checksum(algorithm, checksum);
        self
    }

    /// Sets other directories of the same library (e.g. older copies)
    /// which are removed after a successful download.
    pub fn existing_dirs_to_replace(mut self, dirs: HashSet<FilePath>) -> Self {
        self.existing_dirs = dirs;
        self
    }

    /// Sets a token to abort the download.
    pub fn cancellation_token(mut self, token: CancellationToken) -> Self {
        self.download = self.download.cancellation_token(token);
        self
    }

    /// Returns a receiver for progress updates.
    pub fn subscribe_progress(&self) -> watch::Receiver<Progress> {
        self.download.subscribe_progress()
    }

    /// Returns the destination library directory.
    pub fn dest_dir(&self) -> &FilePath {
        &self.dest_dir
    }

    /// Performs the download and returns the library directory.
    pub async fn download(self, nam: &NetworkAccessManager) -> Result<FilePath> {
        let Self {
            download,
            dest_dir,
            existing_dirs,
        } = self;
        let download = download.zip_cleanup_callback(Box::new(move |dst: &FilePath| {
            for fp in existing_dirs.iter().filter(|fp| *fp != dst) {
                log::debug!("Removing existing library directory: {}", fp.to_native());
                if let Err(e) = file_utils::remove_dir_recursively(fp) {
                    log::warn!("Failed to remove {}: {e}", fp.to_native());
                }
            }
        }));
        let downloaded = download.download(nam).await?;
        Ok(downloaded.extracted_to.unwrap_or(dest_dir))
    }
}

/// Returns the library directory in an extracted ZIP: the root itself or
/// its only subdirectory (upstream `findLibraryInZip()`).
pub fn find_library_in_zip(root: &FilePath) -> Result<FilePath> {
    if Library::is_valid_element_directory_path(root) {
        return Ok(root.clone());
    }
    let subdirs: Vec<FilePath> = file_utils::find_directories(root)
        .into_iter()
        .filter(|dir| !dir.file_name().starts_with('.'))
        .collect();
    if let [subdir] = subdirs.as_slice()
        && Library::is_valid_element_directory_path(subdir)
    {
        return Ok(subdir.clone());
    }
    Err(Error::NoLibraryInZip)
}
