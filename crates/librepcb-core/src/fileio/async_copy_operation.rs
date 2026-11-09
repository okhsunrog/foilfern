//! Port of libs/librepcb/core/fileio/asynccopyoperation.{h,cpp}.
//!
//! Upstream is a `QThread` emitting signals. Here, [`AsyncCopyOperation::run()`]
//! performs the (blocking) copy on the caller's thread and reports progress
//! through a callback; [`AsyncCopyOperation::start()`] is a convenience which
//! runs it on a new thread. The operation can be aborted from any thread via
//! an [`AbortHandle`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use librepcb_i18n::tr;

use super::error::{Error, Result};
use super::file_path::FilePath;
use super::file_utils;

/// Progress report of an [`AsyncCopyOperation`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyProgress {
    /// Human readable status (upstream `progressStatus()`).
    Status(String),
    /// Progress in percent, 0..=100 (upstream `progressPercent()`).
    Percent(i32),
}

/// Handle to abort a running operation from another thread.
#[derive(Debug, Clone, Default)]
pub struct AbortHandle(Arc<AtomicBool>);

impl AbortHandle {
    /// Requests the operation to abort.
    pub fn abort(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Returns whether abort was requested.
    pub fn is_aborted(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Recursive copy of a directory into a new directory.
///
/// The files are first copied into `<destination>~`, which is then renamed
/// to the destination, so a failed or aborted operation leaves no
/// half-copied destination behind.
#[derive(Debug)]
pub struct AsyncCopyOperation {
    source: FilePath,
    destination: FilePath,
    abort: AbortHandle,
}

impl AsyncCopyOperation {
    /// Creates a copy operation (the destination must not exist).
    pub fn new(source: &FilePath, destination: &FilePath) -> Self {
        Self {
            source: source.clone(),
            destination: destination.clone(),
            abort: AbortHandle::default(),
        }
    }

    /// Returns the source directory.
    pub fn source(&self) -> &FilePath {
        &self.source
    }

    /// Returns the destination directory.
    pub fn destination(&self) -> &FilePath {
        &self.destination
    }

    /// Returns a handle to abort the operation.
    pub fn abort_handle(&self) -> AbortHandle {
        self.abort.clone()
    }

    /// Performs the copy on the current thread.
    ///
    /// Returns [`Error::UserCanceled`] if aborted. On failure, a final
    /// status "Failed to copy files: ..." is reported as well.
    pub fn run(&self, on_progress: &mut dyn FnMut(CopyProgress)) -> Result<()> {
        let result = self.copy(on_progress);
        if let Err(e) = &result {
            on_progress(CopyProgress::Status(format!(
                "{} {e}",
                tr!("librepcb::AsyncCopyOperation", "Failed to copy files:")
            )));
        }
        result
    }

    /// Runs the operation on a new thread.
    pub fn start(
        self,
        mut on_progress: impl FnMut(CopyProgress) + Send + 'static,
    ) -> RunningCopyOperation {
        let abort = self.abort_handle();
        let thread = std::thread::spawn(move || self.run(&mut on_progress));
        RunningCopyOperation {
            abort,
            thread: Some(thread),
        }
    }

    fn copy(&self, on_progress: &mut dyn FnMut(CopyProgress)) -> Result<()> {
        let status = |s: String| CopyProgress::Status(s);
        // Abort if destination already exists. Otherwise it would be deleted
        // during cleanup, which might not be intended.
        if self.destination.is_existing_file() || self.destination.is_existing_dir() {
            return Err(Error::AlreadyExists(self.destination.clone()));
        }

        // First copy to a temporary directory and rename it afterwards.
        on_progress(status(tr!(
            "librepcb::AsyncCopyOperation",
            "Removing temporary directory..."
        )));
        let tmp_dst = FilePath::new(format!("{}~", self.destination))
            .ok_or_else(|| Error::InvalidPath(format!("{}~", self.destination)))?;
        file_utils::remove_dir_recursively(&tmp_dst)?;
        file_utils::make_path(&tmp_dst)?;

        on_progress(status(tr!(
            "librepcb::AsyncCopyOperation",
            "Looking for files to copy..."
        )));
        let files = file_utils::files_in_directory(&self.source, &[], true, false)?;

        let result = (|| {
            let count = files.len();
            for (i, src) in files.iter().enumerate() {
                if self.abort.is_aborted() {
                    return Err(Error::UserCanceled);
                }
                let dst = tmp_dst.path_to(&src.to_relative(&self.source));
                if i % (count / 100 + 1) == 0 {
                    on_progress(status(tr!(
                        "librepcb::AsyncCopyOperation",
                        "Copy file {0} of {1}...",
                        i + 1,
                        count
                    )));
                    // Always < 100, so the conversion cannot fail.
                    let percent = i32::try_from((95 * (i + 1)) / count).unwrap_or(95);
                    on_progress(CopyProgress::Percent(percent));
                }
                if let Some(parent) = dst.parent_dir() {
                    file_utils::make_path(&parent)?;
                }
                file_utils::copy_file(src, &dst)?;
            }

            on_progress(status(tr!(
                "librepcb::AsyncCopyOperation",
                "Renaming temporary directory..."
            )));
            on_progress(CopyProgress::Percent(98));
            file_utils::move_path(&tmp_dst, &self.destination)?;

            on_progress(status(tr!(
                "librepcb::AsyncCopyOperation",
                "Successfully finished!"
            )));
            on_progress(CopyProgress::Percent(100));
            Ok(())
        })();
        if result.is_err() {
            // Clean up, but ignore failures to avoid misleading error messages.
            let _ = std::fs::remove_dir_all(&self.destination);
            let _ = std::fs::remove_dir_all(&tmp_dst);
        }
        result
    }
}

/// A copy operation running on its own thread (see
/// [`AsyncCopyOperation::start()`]).
///
/// Dropping it aborts the operation and waits until the thread finished
/// (like the upstream destructor).
#[derive(Debug)]
pub struct RunningCopyOperation {
    abort: AbortHandle,
    thread: Option<JoinHandle<Result<()>>>,
}

impl RunningCopyOperation {
    /// Returns a handle to abort the operation.
    pub fn abort_handle(&self) -> AbortHandle {
        self.abort.clone()
    }

    /// Returns whether the operation has finished.
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Waits until the operation finished and returns its result.
    pub fn wait(mut self) -> Result<()> {
        self.join()
    }

    /// Aborts the operation and waits until it finished (upstream
    /// `abort()`). The operation is completely reverted if it was not
    /// finished yet.
    pub fn abort(mut self) -> Result<()> {
        self.abort.abort();
        self.join()
    }

    fn join(&mut self) -> Result<()> {
        match self.thread.take().map(JoinHandle::join) {
            Some(Ok(result)) => result,
            Some(Err(panic)) => std::panic::resume_unwind(panic),
            None => Ok(()),
        }
    }
}

impl Drop for RunningCopyOperation {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.abort.abort();
            let _ = thread.join();
        }
    }
}
