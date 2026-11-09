//! Runs the EAGLE and KiCad library import wizards
//! ([`crate::dialogs::library_import`]) of a library: opens them from the
//! library actions and runs the import in a worker thread (upstream
//! `MainWindow::triggerLibraryElement()` and the result pages of the
//! wizards, which start the import and a library rescan afterwards).

use librepcb_core::fileio::FilePath;
use librepcb_core::utils::message_logger::{LogMessage, MessageLogger};
use librepcb_import::{ImportSummary, Progress};

use super::{State, with_current_state};
use crate::dialogs::library_import::{Importer, LibraryImportWizard};

impl State {
    fn library_import_wizard(&mut self) -> Option<&mut LibraryImportWizard> {
        self.form_dialog
            .as_mut()?
            .dialog
            .as_any_mut()?
            .downcast_mut::<LibraryImportWizard>()
    }

    /// Opens the EAGLE (`kicad == false`) or KiCad library import wizard
    /// for an open library.
    pub fn show_library_import_wizard(&mut self, library: &FilePath, kicad: bool) {
        if !self.libraries.iter().any(|l| l.path() == library) {
            return;
        }
        let db = self.workspace.lock().shared_library_db();
        let wizard = if kicad {
            LibraryImportWizard::kicad(library.clone(), db)
        } else {
            LibraryImportWizard::eagle(library.clone(), db)
        };
        self.show_app_dialog(Box::new(wizard));
    }

    /// Starts the import of the open wizard in a worker thread.
    pub(crate) fn run_library_import(&mut self) {
        let progress = Progress::with_listener(|event| {
            let event = event.clone();
            let _ = slint::invoke_from_event_loop(move || {
                with_current_state(|s| {
                    if let Some(w) = s.library_import_wizard() {
                        w.progress_event(&event);
                    }
                });
            });
        });
        let Some((mut importer, db)) = self
            .library_import_wizard()
            .and_then(|w| w.take_importer(&progress))
        else {
            return;
        };
        let spawned = std::thread::Builder::new()
            .name("library-import".into())
            .spawn(move || {
                let log = MessageLogger::new();
                let summary = importer.run(&db, &log, &progress);
                let messages = log.messages();
                let _ = slint::invoke_from_event_loop(move || {
                    with_current_state(|s| s.library_import_finished(importer, summary, messages));
                });
            });
        if let Err(e) = spawned {
            log::error!("Failed to start the library import: {e}");
        }
    }

    /// The import finished (upstream `importFinished()`): the wizard shows
    /// the messages, the libraries are rescanned.
    fn library_import_finished(
        &mut self,
        importer: Importer,
        summary: Option<ImportSummary>,
        messages: Vec<LogMessage>,
    ) {
        if let Some(w) = self.library_import_wizard() {
            w.import_finished(importer, summary, &messages);
            self.refresh_form_dialog(true);
        }
        self.start_library_scan();
    }
}
