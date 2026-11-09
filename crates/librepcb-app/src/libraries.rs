//! The background library scan (port of the library rescan of upstream
//! `WorkspaceLibraryDb`). The libraries panel is in
//! [`crate::library_manager`].

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use librepcb_core::workspace::LibraryScanner;

/// Scans the workspace libraries on a new thread and calls `done` on the
/// UI thread afterwards (upstream `WorkspaceLibraryDb::startLibraryRescan()`).
pub fn start_rescan(scanner: LibraryScanner, done: impl FnOnce(bool) + Send + 'static) {
    let abort = Arc::new(AtomicBool::new(false));
    let spawned = std::thread::Builder::new()
        .name("library-scan".into())
        .spawn(move || {
            let result = scanner.scan(&abort, &mut |_| {});
            let ok = match result {
                Ok(outcome) => {
                    log::info!("Workspace library scan finished: {outcome:?}");
                    true
                }
                Err(e) => {
                    log::error!("Workspace library scan failed: {e}");
                    false
                }
            };
            if let Err(e) = slint::invoke_from_event_loop(move || done(ok)) {
                log::debug!("Library scan result not delivered: {e}");
            }
        });
    if let Err(e) = spawned {
        log::error!("Failed to start the library scan: {e}");
    }
}
