//! The clipboard of the schematic and board editors.
//!
//! Upstream copies `QMimeData` with the MIME types
//! `application/x-librepcb-clipboard.schematic; version=<app version>` (and
//! `.board`) to `QApplication::clipboard()`. Here the data goes to the
//! system clipboard under the same MIME type through `clipboard-rs` (Slint
//! and `arboard` only support text and images), so copy and paste work
//! between this application and upstream LibrePCB of the same version.
//!
//! The system clipboard is opened lazily on the first copy or paste. If it
//! is not available (no display, e.g. in tests or headless sessions) or a
//! system clipboard operation fails, the data is kept in an in-memory
//! clipboard of the application instead (see `COMPAT.md`).
//!
//! Interoperability with upstream is limited to X11 and Wayland: on Windows
//! and macOS, Qt wraps custom MIME types into its own clipboard format
//! names, which are not reproduced.

use std::cell::RefCell;

use clipboard_rs::Clipboard as _;
use librepcb_editor::fsm::{Clipboard, MemoryClipboard};

/// Where the clipboard data goes.
enum Backend {
    /// Not opened yet.
    Unopened,
    /// The system clipboard.
    System(clipboard_rs::ClipboardContext),
    /// No system clipboard available.
    Unavailable,
}

/// The application clipboard: the system clipboard with an in-memory
/// fallback.
pub struct AppClipboard {
    backend: Backend,
    memory: MemoryClipboard,
    /// Whether the last `set()` reached the system clipboard (if not, the
    /// memory clipboard holds the newest data).
    system_is_current: bool,
    use_system: bool,
}

impl std::fmt::Debug for AppClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppClipboard")
            .field("system_is_current", &self.system_is_current)
            .field("use_system", &self.use_system)
            .finish_non_exhaustive()
    }
}

impl AppClipboard {
    /// A clipboard using the system clipboard (if `use_system`) with the
    /// in-memory fallback.
    pub fn new(use_system: bool) -> Self {
        Self {
            backend: Backend::Unopened,
            memory: MemoryClipboard::new(),
            system_is_current: false,
            use_system,
        }
    }

    fn system(&mut self) -> Option<&clipboard_rs::ClipboardContext> {
        if matches!(self.backend, Backend::Unopened) {
            self.backend = if self.use_system {
                match clipboard_rs::ClipboardContext::new() {
                    Ok(ctx) => Backend::System(ctx),
                    Err(e) => {
                        log::info!("System clipboard not available, using an internal one: {e}");
                        Backend::Unavailable
                    }
                }
            } else {
                Backend::Unavailable
            };
        }
        match &self.backend {
            Backend::System(ctx) => Some(ctx),
            _ => None,
        }
    }
}

impl Clipboard for AppClipboard {
    fn set(&mut self, mime_type: &str, data: Vec<u8>) {
        self.memory.set(mime_type, data.clone());
        self.system_is_current = match self.system() {
            Some(ctx) => match ctx.set_buffer(mime_type, data) {
                Ok(()) => true,
                Err(e) => {
                    log::warn!("Failed to copy to the system clipboard: {e}");
                    false
                }
            },
            None => false,
        };
    }

    fn get(&self, mime_type: &str) -> Option<Vec<u8>> {
        if let Backend::System(ctx) = &self.backend
            && (self.system_is_current || self.memory.get(mime_type).is_none())
        {
            // The system clipboard may hold newer data of other
            // applications (or another LibrePCB instance).
            return ctx.get_buffer(mime_type).ok().filter(|d| !d.is_empty());
        }
        self.memory.get(mime_type)
    }
}

thread_local! {
    static CLIPBOARD: RefCell<AppClipboard> = RefCell::new(AppClipboard::new(
        std::env::var_os("LIBREPCB_NO_SYSTEM_CLIPBOARD").is_none(),
    ));
}

/// Runs `f` with the application clipboard of the UI thread. The system
/// clipboard is not used if the environment variable
/// `LIBREPCB_NO_SYSTEM_CLIPBOARD` is set.
pub fn with_clipboard<R>(f: impl FnOnce(&mut AppClipboard) -> R) -> R {
    CLIPBOARD.with(|c| f(&mut c.borrow_mut()))
}

/// Opens the system clipboard before the first paste (so that data copied
/// by other applications is found).
pub fn ensure_opened(clipboard: &mut AppClipboard) {
    clipboard.system();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_fallback() {
        let mut c = AppClipboard::new(false);
        assert_eq!(c.get("a/b"), None);
        c.set("a/b", vec![1, 2]);
        assert_eq!(c.get("a/b"), Some(vec![1, 2]));
        assert_eq!(c.get("a/c"), None);
    }
}
