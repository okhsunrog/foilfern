//! Port of libs/librepcb/core/fileio/directorylock.{h,cpp}.
//!
//! A directory is locked by creating the file `<dir>/.lock`, a UTF-8 text
//! file with 6 lines (separated by `\n`, no trailing newline):
//!
//! 1. full name of the user holding the lock
//! 2. username (login name)
//! 3. hostname
//! 4. process ID
//! 5. process name
//! 6. creation date/time (UTC, ISO 8601, e.g. `2013-04-13T12:43:52Z`)
//!
//! The format and the status detection are identical to upstream, since C++
//! and Rust LibrePCB instances may open the same directories concurrently:
//! a lock with another user/host is [`LockStatus::LockedByOtherUser`]; one
//! with the same PID is [`LockStatus::LockedByThisApp`] only if this process
//! (i.e. any [`DirectoryLock`] in it) created it, otherwise
//! [`LockStatus::LockedByUnknownApp`] (namespaced PIDs, e.g. Flatpak); a lock
//! of another PID is [`LockStatus::LockedByOtherApp`] if a process with that
//! PID and name is running, otherwise [`LockStatus::StaleLock`].
//!
//! OS-level advisory locks (`fs4`, `fd-lock`) are deliberately not used:
//! upstream does not take them, so they would neither protect against C++
//! instances nor allow the stale lock detection (which relies on the PID and
//! host stored in the file), and they are unreliable on network drives.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock, PoisonError};

use chrono::{SecondsFormat, Utc};

use super::error::{Error, Result};
use super::file_path::FilePath;
use super::file_utils;
use crate::system_info;

/// Lock status of a directory (see [`DirectoryLock::status()`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LockStatus {
    /// The directory is not locked (lock file does not exist).
    Unlocked,
    /// The directory is locked by a crashed application instance.
    StaleLock,
    /// The directory is locked by this application instance.
    LockedByThisApp,
    /// The directory is locked by another application instance on this
    /// machine.
    LockedByOtherApp,
    /// The directory is locked by another user or machine.
    LockedByOtherUser,
    /// The directory is locked by an unknown application (may be stale).
    LockedByUnknownApp,
}

/// Callback deciding whether an existing lock shall be overridden (upstream
/// `LockHandlerCallback`).
///
/// Arguments: the directory, its [`LockStatus`] and `user@host` of the lock
/// owner. Returns `Ok(true)` to override the lock, `Ok(false)` to fail with
/// [`Error::AlreadyLocked`], or an error (e.g. [`Error::UserCanceled`]) to
/// abort.
pub type LockHandler<'a> = &'a mut dyn FnMut(&FilePath, LockStatus, &str) -> Result<bool>;

/// Directories locked by this process (upstream
/// `dirsLockedByThisAppInstance()`).
fn dirs_locked_by_this_app() -> &'static Mutex<HashSet<FilePath>> {
    static SET: OnceLock<Mutex<HashSet<FilePath>>> = OnceLock::new();
    SET.get_or_init(Default::default)
}

/// File based directory lock (RAII: the lock is released on drop if it was
/// acquired by this object).
#[derive(Debug, Default)]
pub struct DirectoryLock {
    dir_to_lock: Option<FilePath>,
    lock_file_path: Option<FilePath>,
    locked_by_this_object: bool,
}

impl DirectoryLock {
    /// Creates a lock object for `dir` (does not lock it yet).
    pub fn new(dir: &FilePath) -> Self {
        let mut lock = Self::default();
        lock.set_dir_to_lock(dir);
        lock
    }

    /// Sets the directory to lock.
    ///
    /// Must not be called while this object holds a lock.
    pub fn set_dir_to_lock(&mut self, dir: &FilePath) {
        debug_assert!(!self.locked_by_this_object);
        self.dir_to_lock = Some(dir.clone());
        self.lock_file_path = Some(dir.path_to(".lock"));
    }

    /// Returns the directory to lock, if set.
    pub fn dir_to_lock(&self) -> Option<&FilePath> {
        self.dir_to_lock.as_ref()
    }

    /// Returns the path of the lock file (`<dir>/.lock`), if set.
    pub fn lock_file_path(&self) -> Option<&FilePath> {
        self.lock_file_path.as_ref()
    }

    /// Returns the directory and the lock file path, or an error if the
    /// directory is not set or does not exist.
    fn paths(&self) -> Result<(&FilePath, &FilePath)> {
        match (&self.dir_to_lock, &self.lock_file_path) {
            (Some(dir), Some(file)) if dir.is_existing_dir() => Ok((dir, file)),
            (Some(dir), _) => Err(Error::LockDirectoryNotFound(dir.clone())),
            _ => Err(Error::NoDirectoryToLock),
        }
    }

    /// Returns the lock status of the directory.
    ///
    /// Fails if the directory does not exist or the lock file is invalid.
    pub fn status(&self) -> Result<LockStatus> {
        self.status_with_user().map(|(status, _)| status)
    }

    /// Same as [`status()`](Self::status), but also returns `user@host` of
    /// the lock owner (empty if unlocked).
    pub fn status_with_user(&self) -> Result<(LockStatus, String)> {
        let (dir, lock_file) = self.paths()?;
        if !lock_file.is_existing_file() {
            return Ok((LockStatus::Unlocked, String::new()));
        }

        // Read the content of the lock file.
        let content = file_utils::read_file(lock_file)?;
        let content = String::from_utf8_lossy(&content);
        let lines: Vec<&str> = content.split('\n').collect();
        if lines.len() < 6 {
            return Err(Error::LockFileTooFewLines(lock_file.clone()));
        }
        let lock_user = lines[1];
        let lock_host = lines[2];
        // Like QString::toLongLong(): 0 if invalid.
        let lock_pid: i64 = lines[3].parse().unwrap_or(0);
        let lock_app_name = lines[4];
        let user = format!("{lock_user}@{lock_host}");

        // Check if the lock file was created by another computer or user.
        if lock_user != system_info::username() || lock_host != system_info::hostname() {
            return Ok((LockStatus::LockedByOtherUser, user));
        }

        // Check if the lock was created by this application instance.
        if lock_pid == system_info::current_pid() {
            let locked_by_us = dirs_locked_by_this_app()
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(dir);
            // If the PID is ours but we did not create the lock, LibrePCB is
            // probably running with namespaced PIDs (e.g. Flatpak), so we
            // cannot know whether the lock is stale.
            let status = if locked_by_us {
                LockStatus::LockedByThisApp
            } else {
                LockStatus::LockedByUnknownApp
            };
            return Ok((status, user));
        }

        // Another instance on this computer: the lock is stale if that process
        // is no longer running.
        let status = if system_info::is_process_running(lock_pid)?
            && system_info::process_name_by_pid(lock_pid)? == lock_app_name
        {
            LockStatus::LockedByOtherApp
        } else {
            LockStatus::StaleLock
        };
        Ok((status, user))
    }

    /// Locks the directory if it is unlocked or the lock is stale.
    ///
    /// If it is locked, `lock_handler` (if given) decides whether to override
    /// the lock; otherwise [`Error::AlreadyLocked`] is returned.
    pub fn try_lock(&mut self, lock_handler: Option<LockHandler<'_>>) -> Result<()> {
        let (status, user) = self.status_with_user()?;
        match status {
            LockStatus::StaleLock | LockStatus::Unlocked => {
                if status == LockStatus::StaleLock {
                    log::warn!("Override stale lock on directory {}...", self.dir_native());
                }
                self.lock()
            }
            _ => {
                if let (Some(handler), Some(dir)) = (lock_handler, self.dir_to_lock.clone())
                    && handler(&dir, status, &user)?
                {
                    log::info!("Override lock on directory {}...", dir.to_native());
                    return self.lock();
                }
                Err(Error::AlreadyLocked {
                    directory: self.paths()?.0.clone(),
                    user,
                })
            }
        }
    }

    /// Releases the lock if it was acquired by this object.
    ///
    /// Returns whether the lock has been released.
    pub fn unlock_if_locked(&mut self) -> Result<bool> {
        if self.locked_by_this_object {
            self.unlock()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Creates/overwrites the lock file, even if the directory is locked by
    /// someone else (use [`try_lock()`](Self::try_lock) instead).
    pub fn lock(&mut self) -> Result<()> {
        let (dir, lock_file) = self.paths()?;
        let pid = system_info::current_pid();
        let lines = [
            system_info::full_username().to_owned(),
            system_info::username().to_owned(),
            system_info::hostname().to_owned(),
            pid.to_string(),
            system_info::process_name_by_pid(pid)?,
            Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        ];
        file_utils::write_file(lock_file, lines.join("\n").as_bytes())?;
        dirs_locked_by_this_app()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(dir.clone());
        self.locked_by_this_object = true;
        Ok(())
    }

    /// Removes the lock file, even if it was created by someone else (use
    /// [`unlock_if_locked()`](Self::unlock_if_locked) instead).
    pub fn unlock(&mut self) -> Result<()> {
        let lock_file = self
            .lock_file_path
            .as_ref()
            .ok_or(Error::NoDirectoryToLock)?;
        file_utils::remove_file(lock_file)?;
        self.locked_by_this_object = false;
        if let Some(dir) = &self.dir_to_lock {
            dirs_locked_by_this_app()
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(dir);
        }
        Ok(())
    }

    fn dir_native(&self) -> String {
        self.dir_to_lock
            .as_ref()
            .map(FilePath::to_native)
            .unwrap_or_default()
    }
}

impl Drop for DirectoryLock {
    fn drop(&mut self) {
        if let Err(e) = self.unlock_if_locked() {
            log::error!(
                "Failed to remove lock file {}: {e}",
                self.lock_file_path
                    .as_ref()
                    .map(FilePath::to_native)
                    .unwrap_or_default()
            );
        }
    }
}
