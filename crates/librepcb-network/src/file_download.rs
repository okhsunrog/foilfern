//! Port of libs/librepcb/core/network/filedownload.{h,cpp}.
//!
//! Streams a download into a temporary file next to the destination while
//! computing its checksum incrementally, then atomically replaces the
//! destination (like upstream `QSaveFile`). Optionally, the downloaded ZIP
//! is extracted (on a blocking thread, with the synchronous
//! [`librepcb_core::fileio`] code) into a directory, replacing it
//! atomically: the archive is extracted into `<dir>.tmp`, the old directory
//! is moved to `<dir>.backup`, and restored if moving the new one fails.

use std::sync::Arc;

use librepcb_core::fileio::{self, FilePath, ZipArchive, file_utils};
use librepcb_i18n::tr;
use reqwest::Url;
use sha2::Digest;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Semaphore, watch};
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::network_access_manager::NetworkAccessManager;
use crate::network_request::{NetworkRequest, Progress, ProgressReporter, ReplySink};

/// Checksum algorithm for [`FileDownload::expected_checksum()`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChecksumAlgorithm {
    /// SHA-1.
    Sha1,
    /// SHA-256.
    Sha256,
}

enum Hasher {
    Sha1(sha1::Sha1),
    Sha256(sha2::Sha256),
}

impl Hasher {
    fn new(algorithm: ChecksumAlgorithm) -> Self {
        match algorithm {
            ChecksumAlgorithm::Sha1 => Self::Sha1(sha1::Sha1::new()),
            ChecksumAlgorithm::Sha256 => Self::Sha256(sha2::Sha256::new()),
        }
    }

    fn update(&mut self, data: &[u8]) {
        match self {
            Self::Sha1(h) => h.update(data),
            Self::Sha256(h) => h.update(data),
        }
    }

    fn finalize(self) -> Vec<u8> {
        match self {
            Self::Sha1(h) => h.finalize().to_vec(),
            Self::Sha256(h) => h.finalize().to_vec(),
        }
    }
}

/// Finds the directory of interest in the extracted ZIP (e.g. to strip a
/// root folder). Must return the passed directory or a subdirectory of it.
pub type ZipDiscoveryCallback = Box<dyn FnOnce(&FilePath) -> Result<FilePath> + Send + 'static>;

/// Called with the extraction directory after a successful extraction.
pub type ZipCleanupCallback = Box<dyn FnOnce(&FilePath) + Send + 'static>;

struct ZipExtraction {
    dir: FilePath,
    discovery: Option<ZipDiscoveryCallback>,
    cleanup: Option<ZipCleanupCallback>,
}

/// Result of a successful [`FileDownload`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Downloaded {
    /// The downloaded file (upstream `fileDownloaded()`). If the file was
    /// extracted, it has been removed already.
    pub file: FilePath,
    /// The extraction directory, if any (upstream `zipFileExtracted()`).
    pub extracted_to: Option<FilePath>,
}

/// Downloads a file (see the [module docs](self)).
pub struct FileDownload {
    request: NetworkRequest,
    destination: FilePath,
    semaphore: Arc<Semaphore>,
    checksum: Option<(ChecksumAlgorithm, Vec<u8>)>,
    extraction: Option<ZipExtraction>,
}

impl FileDownload {
    /// Creates a download of `url` into the file `destination` (overwritten
    /// if it exists).
    ///
    /// `semaphore` limits the number of downloads performing file operations
    /// (creating, writing, extracting) at the same time; pass the same
    /// semaphore to all parallel downloads, or `Arc::new(Semaphore::new(1))`.
    pub fn new(url: Url, destination: FilePath, semaphore: Arc<Semaphore>) -> Self {
        Self {
            request: NetworkRequest::get(url),
            destination,
            semaphore,
            checksum: None,
            extraction: None,
        }
    }

    /// See [`NetworkRequest::header()`].
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.request = self.request.header(name, value);
        self
    }

    /// See [`NetworkRequest::expected_reply_content_size()`].
    pub fn expected_reply_content_size(mut self, bytes: u64) -> Self {
        self.request = self.request.expected_reply_content_size(bytes);
        self
    }

    /// See [`NetworkRequest::browser_user_agent()`].
    pub fn browser_user_agent(mut self) -> Self {
        self.request = self.request.browser_user_agent();
        self
    }

    /// See [`NetworkRequest::cancellation_token()`].
    pub fn cancellation_token(mut self, token: CancellationToken) -> Self {
        self.request = self.request.cancellation_token(token);
        self
    }

    /// Sets the expected checksum (raw bytes) of the downloaded file. On
    /// mismatch, the download fails and the destination is not written.
    pub fn expected_checksum(mut self, algorithm: ChecksumAlgorithm, checksum: Vec<u8>) -> Self {
        self.checksum = Some((algorithm, checksum));
        self
    }

    /// Extracts the downloaded file (a ZIP) into `dir` and removes it
    /// afterwards. `discovery` may select a subdirectory of the extracted
    /// archive to be moved to `dir`.
    pub fn zip_extraction_directory(
        mut self,
        dir: FilePath,
        discovery: Option<ZipDiscoveryCallback>,
    ) -> Self {
        let cleanup = self.extraction.take().and_then(|e| e.cleanup);
        self.extraction = Some(ZipExtraction {
            dir,
            discovery,
            cleanup,
        });
        self
    }

    /// Sets a callback to be called (on a blocking thread) with the
    /// extraction directory after a successful extraction. Only used
    /// together with [`zip_extraction_directory()`](Self::zip_extraction_directory).
    pub fn zip_cleanup_callback(mut self, callback: ZipCleanupCallback) -> Self {
        if let Some(extraction) = &mut self.extraction {
            extraction.cleanup = Some(callback);
        }
        self
    }

    /// Returns a receiver for progress updates.
    pub fn subscribe_progress(&self) -> watch::Receiver<Progress> {
        self.request.subscribe_progress()
    }

    /// Performs the download (and extraction).
    pub async fn download(self, nam: &NetworkAccessManager) -> Result<Downloaded> {
        let Self {
            request,
            destination,
            semaphore,
            checksum,
            extraction,
        } = self;
        let reporter = request.reporter().clone();
        let result = request
            .run(async {
                let mut sink = FileSink {
                    destination: destination.clone(),
                    semaphore: Arc::clone(&semaphore),
                    algorithm: checksum.as_ref().map(|(a, _)| *a),
                    file: None,
                    hasher: None,
                };
                request.execute(nam, &mut sink).await?;
                sink.finalize(checksum.map(|(_, c)| c), &reporter).await?;
                let extracted_to = match extraction {
                    Some(extraction) => {
                        let _permit = acquire(&semaphore).await?;
                        let destination = destination.clone();
                        let reporter = reporter.clone();
                        tokio::task::spawn_blocking(move || {
                            extract(&destination, extraction, &reporter)
                        })
                        .await
                        .map_err(|e| Error::Task(e.to_string()))??
                    }
                    None => None,
                };
                Ok(Downloaded {
                    file: destination.clone(),
                    extracted_to,
                })
            })
            .await;
        request.finish(result)
    }
}

impl std::fmt::Debug for FileDownload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileDownload")
            .field("url", self.request.url())
            .field("destination", &self.destination)
            .finish_non_exhaustive()
    }
}

async fn acquire(semaphore: &Semaphore) -> Result<tokio::sync::SemaphorePermit<'_>> {
    semaphore
        .acquire()
        .await
        .map_err(|e| Error::Task(e.to_string()))
}

/// Writes the reply into a temporary file next to the destination.
struct FileSink {
    destination: FilePath,
    semaphore: Arc<Semaphore>,
    algorithm: Option<ChecksumAlgorithm>,
    file: Option<(tempfile::NamedTempFile, tokio::fs::File)>,
    hasher: Option<Hasher>,
}

impl FileSink {
    fn open_err(&self, message: impl ToString) -> Error {
        Error::OpenDestination {
            path: self.destination.to_native(),
            message: message.to_string(),
        }
    }

    fn write_err(&self, message: impl ToString) -> Error {
        Error::WriteDestination {
            path: self.destination.to_native(),
            message: message.to_string(),
        }
    }

    /// Verifies the checksum and moves the temporary file to the destination
    /// (upstream `finalizeRequest()`, without the extraction).
    async fn finalize(
        mut self,
        expected: Option<Vec<u8>>,
        reporter: &ProgressReporter,
    ) -> Result<()> {
        if let (Some(hasher), Some(expected)) = (self.hasher.take(), expected) {
            reporter.state(tr!("librepcb::FileDownload", "Verify checksum..."));
            let result = hasher.finalize();
            if result != expected {
                log::debug!(
                    "Expected checksum {} but got {}.",
                    hex::encode(&expected),
                    hex::encode(&result)
                );
                return Err(Error::ChecksumMismatch);
            }
            log::debug!("Checksum verification of downloaded file was successful.");
        }

        let _permit = acquire(&self.semaphore).await?;
        reporter.state(tr!("librepcb::FileDownload", "Write file..."));
        let (tmp, mut file) = self.file.take().ok_or_else(|| self.write_err("no data"))?;
        file.flush().await.map_err(|e| self.write_err(e))?;
        file.sync_all().await.map_err(|e| self.write_err(e))?;
        drop(file);
        let destination = self.destination.clone();
        tokio::task::spawn_blocking(move || tmp.persist(&destination))
            .await
            .map_err(|e| Error::Task(e.to_string()))?
            .map_err(|e| self.write_err(e.error))?;
        Ok(())
    }
}

impl ReplySink for FileSink {
    async fn begin(&mut self) -> Result<()> {
        // Limit number of parallel file access tasks.
        let _permit = acquire(&self.semaphore).await?;
        self.file = None; // Removes a previous temporary file.
        self.hasher = self.algorithm.map(Hasher::new);
        let destination = self.destination.clone();
        let tmp = tokio::task::spawn_blocking(move || create_temp_file(&destination))
            .await
            .map_err(|e| Error::Task(e.to_string()))?;
        let tmp = match tmp {
            Ok(tmp) => tmp,
            Err(Error::FileIo(e)) => return Err(Error::FileIo(e)),
            Err(e) => return Err(self.open_err(e)),
        };
        let std_file = tmp.as_file().try_clone().map_err(|e| self.open_err(e))?;
        self.file = Some((tmp, tokio::fs::File::from_std(std_file)));
        Ok(())
    }

    async fn write(&mut self, chunk: &[u8]) -> Result<()> {
        if let Some(hasher) = &mut self.hasher {
            hasher.update(chunk);
        }
        let result = match &mut self.file {
            Some((_, file)) => file.write_all(chunk).await,
            None => return Err(self.write_err("file not open")),
        };
        result.map_err(|e| self.write_err(e))
    }
}

/// Creates the destination directory and a temporary file in it.
fn create_temp_file(destination: &FilePath) -> Result<tempfile::NamedTempFile> {
    let dir = destination
        .parent_dir()
        .unwrap_or_else(|| destination.clone());
    file_utils::make_path(&dir)?;
    let mut builder = tempfile::Builder::new();
    builder.prefix(".").suffix(".download");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Like QSaveFile: default permissions (umask applies), not 0600.
        builder.permissions(std::fs::Permissions::from_mode(0o666));
    }
    builder
        .tempfile_in(dir.as_path())
        .map_err(|e| Error::OpenDestination {
            path: destination.to_native(),
            message: e.to_string(),
        })
}

/// Extracts the downloaded ZIP (blocking; upstream part of
/// `finalizeRequest()`).
fn extract(
    zip_file: &FilePath,
    extraction: ZipExtraction,
    reporter: &ProgressReporter,
) -> Result<Option<FilePath>> {
    let ZipExtraction {
        dir,
        discovery,
        cleanup,
    } = extraction;
    let sibling = |suffix: &str| {
        FilePath::new(format!("{dir}{suffix}"))
            .ok_or_else(|| fileio::Error::InvalidPath(format!("{dir}{suffix}")))
    };
    let tmp_dir = sibling(".tmp")?;
    let backup_dir = sibling(".backup")?;

    // Clean up temporary files and folders in any case.
    reporter.state(tr!("librepcb::FileDownload", "Remove temporary files..."));
    let _remove_zip = scopeguard::guard(zip_file.clone(), |fp| {
        let _ = std::fs::remove_file(&fp);
    });
    file_utils::remove_dir_recursively(&tmp_dir)?;
    let _remove_tmp = scopeguard::guard(tmp_dir.clone(), |fp| {
        let _ = std::fs::remove_dir_all(&fp);
    });
    file_utils::remove_dir_recursively(&backup_dir)?;
    let _remove_backup = scopeguard::guard(backup_dir.clone(), |fp| {
        let _ = std::fs::remove_dir_all(&fp);
    });

    // Extract ZIP to temporary directory.
    reporter.state(tr!("librepcb::FileDownload", "Extract ZIP..."));
    ZipArchive::open(zip_file)?.extract_to(&tmp_dir)?;

    // Find the directory of interest in the temporary directory.
    let src_dir = match discovery {
        Some(discovery) => discovery(&tmp_dir)?,
        None => tmp_dir.clone(),
    };
    if src_dir != tmp_dir && !src_dir.is_located_in_dir(&tmp_dir) {
        return Err(fileio::Error::InvalidPath(src_dir.to_string()).into());
    }

    // Create a backup of the destination directory. This is better than
    // removing the destination directory recursively, as this would not be
    // atomic.
    if dir.is_existing_dir() {
        file_utils::move_path(&dir, &backup_dir)?;
    }

    // Move the downloaded directory to the destination. If this fails,
    // restore the backup.
    if let Err(e) = file_utils::move_path(&src_dir, &dir) {
        let _ = std::fs::rename(&backup_dir, &dir);
        return Err(e.into());
    }

    if let Some(cleanup) = cleanup {
        cleanup(&dir);
    }
    Ok(Some(dir))
}
