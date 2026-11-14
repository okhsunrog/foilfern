//! Port of libs/librepcb/core/systeminfo.{h,cpp}.
//!
//! Information about the user, the machine and running processes, mainly
//! for [`DirectoryLock`](crate::fileio::DirectoryLock). The values written to
//! lock files must be identical to what upstream LibrePCB writes, because
//! C++ and Rust instances inspect each other's lock files:
//!
//! - [`username()`]: `$USERNAME`, falling back to `$USER` (exactly like
//!   upstream; no crate, because e.g. `whoami::username()` queries the
//!   account database, which differs from upstream under `sudo`/containers).
//! - [`full_username()`]: [`whoami::realname()`] (GECOS field up to the first
//!   comma on Unix, `NetUserGetInfo` on Windows), falling back to the username.
//! - [`hostname()`]: [`whoami::hostname()`] (`gethostname()` on Unix,
//!   `GetComputerNameExW(ComputerNameDnsHostname)` on Windows, which is what
//!   `QHostInfo::localHostName()` uses).
//! - [`is_process_running()`]: `kill(pid, 0)` via [`rustix`] on Unix,
//!   [`sysinfo`] on Windows.
//! - [`process_name_by_pid()`]: file name of `/proc/<pid>/exe` on Linux
//!   (like upstream, handwritten since no crate returns the untruncated
//!   name without the " (deleted)" suffix), [`sysinfo`] elsewhere.

use std::sync::OnceLock;

use foilfern_i18n::tr;

/// Result type of the system_info module.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Error returned when querying process information fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// `kill(pid, 0)` failed with an unexpected error (e.g. `EPERM`).
    #[error("{}", tr!("SystemInfo", "Could not determine if another process is running."))]
    ProcessRunningUnknown,
    /// `/proc/version` is missing (Linux).
    #[error("{}", tr!("SystemInfo", "Could not find the file \"/proc/version\"."))]
    ProcFsMissing,
    /// The process name of a running process is empty or unknown.
    #[error("{}", tr!("SystemInfo", "Could not determine the process name of another process."))]
    ProcessNameUnknown,
}

/// Removes line breaks and surrounding whitespace.
fn clean(s: &str) -> String {
    s.replace(['\n', '\r'], "").trim().to_owned()
}

/// Returns the login name of the current user (like "homer"), or an empty
/// string if it cannot be determined.
pub fn username() -> &'static str {
    static VALUE: OnceLock<String> = OnceLock::new();
    VALUE.get_or_init(|| {
        let var = |name| clean(&std::env::var(name).unwrap_or_default());
        let mut s = var("USERNAME");
        if s.is_empty() {
            s = var("USER");
        }
        s
    })
}

/// Returns the full name of the current user (like "Homer Simpson"),
/// falling back to [`username()`].
pub fn full_username() -> &'static str {
    static VALUE: OnceLock<String> = OnceLock::new();
    VALUE.get_or_init(|| {
        let name = whoami::realname().unwrap_or_default();
        let s = clean(name.split(',').next().unwrap_or_default());
        if s.is_empty() {
            username().to_owned()
        } else {
            s
        }
    })
}

/// Returns the hostname of the computer (like "homer-workstation"), or an
/// empty string if it cannot be determined.
pub fn hostname() -> &'static str {
    static VALUE: OnceLock<String> = OnceLock::new();
    VALUE.get_or_init(|| clean(&whoami::hostname().unwrap_or_default()))
}

/// Detects the environment LibrePCB runs in, e.g. "Snap", "Flatpak" or
/// "AppImage" (comma separated), or an empty string.
pub fn detect_runtime() -> String {
    let var = |name| {
        std::env::var(name)
            .map(|v| v.trim().to_owned())
            .unwrap_or_default()
    };
    let manual = var("LIBREPCB_RUNTIME");
    if !manual.is_empty() {
        return manual;
    }
    [
        ("SNAP", "Snap"),
        ("FLATPAK_ID", "Flatpak"),
        ("APPIMAGE", "AppImage"),
    ]
    .into_iter()
    .filter(|(env, _)| !var(env).is_empty())
    .map(|(_, name)| name)
    .collect::<Vec<_>>()
    .join(", ")
}

/// Returns the process ID of this process (upstream
/// `QCoreApplication::applicationPid()`).
pub fn current_pid() -> i64 {
    i64::from(std::process::id())
}

/// Checks whether a process with the given PID is running.
///
/// Fails if this cannot be determined (e.g. `EPERM` on Unix, like upstream).
pub fn is_process_running(pid: i64) -> Result<bool> {
    imp::is_process_running(pid)
}

/// Returns the process name of the given PID (e.g. "librepcb"), or an empty
/// string if no such process is running.
///
/// On Windows, the `.exe` extension is removed.
pub fn process_name_by_pid(pid: i64) -> Result<String> {
    let name = imp::process_name_by_pid(pid)?;
    match name {
        None => Ok(String::new()),
        Some(name) if name.is_empty() => Err(Error::ProcessNameUnknown),
        Some(name) => Ok(name),
    }
}

#[cfg(unix)]
fn unix_is_process_running(pid: i64) -> Result<bool> {
    use rustix::io::Errno;
    use rustix::process::{Pid, test_kill_process};
    // PIDs <= 0 address process groups with kill(), they never denote a
    // single (lock owning) process.
    let Some(pid) = i32::try_from(pid).ok().filter(|&p| p > 0) else {
        return Ok(false);
    };
    let Some(pid) = Pid::from_raw(pid) else {
        return Ok(false);
    };
    match test_kill_process(pid) {
        Ok(()) => Ok(true),
        Err(Errno::SRCH) => Ok(false),
        Err(_) => Err(Error::ProcessRunningUnknown),
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;

    pub fn is_process_running(pid: i64) -> Result<bool> {
        unix_is_process_running(pid)
    }

    pub fn process_name_by_pid(pid: i64) -> Result<Option<String>> {
        if !std::path::Path::new("/proc/version").is_file() {
            return Err(Error::ProcFsMissing);
        }
        let Ok(target) = std::fs::read_link(format!("/proc/{pid}/exe")) else {
            return Ok(None); // Process not running.
        };
        let target = target.to_string_lossy();
        let name = target.rsplit('/').next().unwrap_or_default();
        // If the executable does no longer exist, " (deleted)" is appended.
        let name = name.strip_suffix(" (deleted)").unwrap_or(name);
        Ok(Some(name.to_owned()))
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::*;
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    fn with_process<T>(pid: i64, f: impl FnOnce(Option<&sysinfo::Process>) -> T) -> T {
        let Some(pid) = usize::try_from(pid).ok().map(Pid::from) else {
            return f(None);
        };
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
        );
        f(system.process(pid))
    }

    pub fn is_process_running(pid: i64) -> Result<bool> {
        #[cfg(unix)]
        {
            unix_is_process_running(pid)
        }
        #[cfg(not(unix))]
        {
            Ok(with_process(pid, |p| p.is_some()))
        }
    }

    pub fn process_name_by_pid(pid: i64) -> Result<Option<String>> {
        Ok(with_process(pid, |process| {
            process.map(|p| {
                let name = p
                    .exe()
                    .and_then(|exe| exe.file_name())
                    .unwrap_or_else(|| p.name())
                    .to_string_lossy()
                    .into_owned();
                if cfg!(windows) {
                    // Upstream strips everything after the last '.'.
                    name.rsplit_once('.')
                        .map_or(name.clone(), |(stem, _)| stem.to_owned())
                } else {
                    name
                }
            })
        }))
    }
}
