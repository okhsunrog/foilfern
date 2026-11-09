//! Progress reporting and cancellation of long running imports (upstream:
//! the `progressStatus()`/`progressPercent()` signals and the `mAbort` flag
//! of the import classes).

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// A progress update.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProgressEvent {
    /// Status text (e.g. the name of the element being imported).
    Status(String),
    /// Progress in percent (0..=100).
    Percent(u32),
}

type Listener = Box<dyn Fn(&ProgressEvent) + Send + Sync>;

struct Inner {
    canceled: AtomicBool,
    percent: AtomicU32,
    status: Mutex<String>,
    listener: Option<Listener>,
}

/// Shared progress state of an operation: the operation reports status and
/// percentage, the caller (e.g. a UI thread) polls them or gets them through
/// a listener, and can request cancellation. Cloning shares the state.
#[derive(Clone)]
pub struct Progress(Arc<Inner>);

static_assertions::assert_impl_all!(Progress: Send, Sync);

impl Default for Progress {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Progress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Progress")
            .field("canceled", &self.is_canceled())
            .field("percent", &self.percent())
            .field("status", &self.status())
            .finish()
    }
}

impl Progress {
    /// Creates a progress state without listener.
    pub fn new() -> Self {
        Self(Arc::new(Inner {
            canceled: AtomicBool::new(false),
            percent: AtomicU32::new(0),
            status: Mutex::new(String::new()),
            listener: None,
        }))
    }

    /// Creates a progress state calling `listener` for each update (called
    /// from the thread running the operation).
    pub fn with_listener(listener: impl Fn(&ProgressEvent) + Send + Sync + 'static) -> Self {
        Self(Arc::new(Inner {
            canceled: AtomicBool::new(false),
            percent: AtomicU32::new(0),
            status: Mutex::new(String::new()),
            listener: Some(Box::new(listener)),
        }))
    }

    /// Requests cancellation; the operation stops as soon as possible.
    pub fn cancel(&self) {
        self.0.canceled.store(true, Ordering::SeqCst);
    }

    /// Resets the cancellation request (before starting a new operation).
    pub fn reset_cancel(&self) {
        self.0.canceled.store(false, Ordering::SeqCst);
    }

    /// Returns whether cancellation was requested.
    pub fn is_canceled(&self) -> bool {
        self.0.canceled.load(Ordering::SeqCst)
    }

    /// Returns the last reported percentage.
    pub fn percent(&self) -> u32 {
        self.0.percent.load(Ordering::SeqCst)
    }

    /// Returns the last reported status text.
    pub fn status(&self) -> String {
        self.lock_status().clone()
    }

    /// Reports a status text.
    pub fn set_status(&self, status: impl Into<String>) {
        let status = status.into();
        *self.lock_status() = status.clone();
        if let Some(listener) = &self.0.listener {
            listener(&ProgressEvent::Status(status));
        }
    }

    /// Reports the progress in percent (clamped to 100).
    pub fn set_percent(&self, percent: u32) {
        let percent = percent.min(100);
        self.0.percent.store(percent, Ordering::SeqCst);
        if let Some(listener) = &self.0.listener {
            listener(&ProgressEvent::Percent(percent));
        }
    }

    fn lock_status(&self) -> std::sync::MutexGuard<'_, String> {
        // A panic while holding the lock can't leave a string inconsistent.
        self.0
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Selection state of an element to import (upstream `Qt::CheckState`):
/// checked by the user, unchecked, or partially checked (not selected by the
/// user, but needed by a selected element).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum CheckState {
    /// Not imported.
    #[default]
    Unchecked,
    /// Imported because a checked element depends on it.
    PartiallyChecked,
    /// Imported.
    Checked,
}

impl CheckState {
    /// Returns whether the element gets imported (checked or partially
    /// checked).
    pub fn is_imported(self) -> bool {
        self != Self::Unchecked
    }

    /// Updates a dependency state (upstream `setElementDependent()`/
    /// `setDependent()`): an unchecked element becomes partially checked if
    /// `dependent`, a partially checked one unchecked if not. Returns
    /// whether the state changed.
    pub fn set_dependent(&mut self, dependent: bool) -> bool {
        if dependent && (*self == Self::Unchecked) {
            *self = Self::PartiallyChecked;
            true
        } else if !dependent && (*self == Self::PartiallyChecked) {
            *self = Self::Unchecked;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let events2 = Arc::clone(&events);
        let p = Progress::with_listener(move |e| events2.lock().unwrap().push(e.clone()));
        p.set_status("x");
        p.set_percent(150);
        assert_eq!(p.percent(), 100);
        assert_eq!(p.status(), "x");
        assert!(!p.is_canceled());
        p.clone().cancel();
        assert!(p.is_canceled());
        assert_eq!(events.lock().unwrap().len(), 2);
    }

    #[test]
    fn test_set_dependent() {
        let mut s = CheckState::Unchecked;
        assert!(s.set_dependent(true));
        assert_eq!(s, CheckState::PartiallyChecked);
        assert!(!s.set_dependent(true));
        assert!(s.set_dependent(false));
        let mut c = CheckState::Checked;
        assert!(!c.set_dependent(false));
    }
}
