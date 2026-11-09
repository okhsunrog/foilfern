//! The libraries panel on the application side: the local and remote
//! library models, their actions (check for updates, apply, cancel, toggle
//! all, uninstall), the filter and the library actions of the panel
//! (open, create, download).
//!
//! Port of the library parts of libs/librepcb/editor/mainwindow.cpp
//! (`trigger()`, `triggerLibrary()`) and libs/librepcb/editor/guiapplication.cpp
//! (library models, filter).

use std::rc::Weak;

use librepcb_app_ui as ui;
use librepcb_core::fileio::{FilePath, file_utils};
use librepcb_i18n::tr;
use slint::ComponentHandle;

use super::{State, deferred, with_current_state};
use crate::library_manager;
use crate::notifications::Notification;

/// Binds the handlers of the library models (rows written by the UI).
pub fn bind(weak: &Weak<std::cell::RefCell<State>>) {
    let Some(state) = weak.upgrade() else { return };
    let s = state.borrow();
    let w = weak.clone();
    s.remote_libraries
        .model()
        .set_handler(move |row, data: ui::LibraryInfoData| {
            deferred(&w, move |s| {
                s.remote_libraries.row_written(row, &data);
                s.update_libraries_data();
            });
        });
    let w = weak.clone();
    s.local_libraries
        .model()
        .set_handler(move |row, data: ui::LibraryInfoData| {
            deferred(&w, move |s| {
                s.local_libraries.row_written(row, &data);
                s.update_libraries_data();
            });
        });
}

impl State {
    /// Reloads the installed libraries from the library database (after a
    /// scan).
    pub(crate) fn refresh_libraries(&mut self) {
        {
            let ws = self.workspace.lock();
            let db = ws.library_db();
            let remote_dir = ws.remote_libraries_path();
            let locales = ws.settings().library_locale_order.get().clone();
            self.local_libraries
                .update_installed(db, &remote_dir, &locales);
            self.remote_libraries
                .update_installed(db, &remote_dir, &locales);
        }
        self.update_libraries_data();
    }

    /// Updates `Data.local-libraries-data` and `Data.remote-libraries-data`.
    pub(crate) fn update_libraries_data(&self) {
        if let Some(w) = self.window() {
            let d = w.global::<ui::Data>();
            d.set_local_libraries_data(self.local_libraries.ui_data());
            d.set_remote_libraries_data(self.remote_libraries.ui_data());
        }
    }

    /// The library panel actions of `Backend.trigger()`.
    pub(crate) fn trigger_libraries_action(&mut self, action: ui::Action) {
        match action {
            ui::Action::LibraryPanelEnsurePopulated => {
                if !self.remote_libraries.is_fetching()
                    && !self.remote_libraries.has_online_libraries()
                    && self.libraries_auto_update_enabled()
                {
                    self.fetch_online_libraries();
                }
            }
            ui::Action::LibraryPanelCheckForUpdates => self.fetch_online_libraries(),
            ui::Action::LibraryPanelCancelUpdateCheck => {
                self.remote_libraries.cancel_fetch();
                self.update_libraries_data();
            }
            ui::Action::LibraryPanelToggleAll => {
                self.remote_libraries.toggle_all();
                self.update_libraries_data();
            }
            ui::Action::LibraryPanelApply => self.apply_library_changes(),
            ui::Action::LibraryPanelCancel => {
                self.remote_libraries.cancel();
                self.update_libraries_data();
            }
            ui::Action::LibraryCreate => self.open_create_library_tab(),
            ui::Action::LibraryDownload => self.open_download_library_tab(),
            _ => {}
        }
    }

    /// Whether the online libraries are fetched automatically (upstream:
    /// not if the library update mode is "disabled").
    fn libraries_auto_update_enabled(&self) -> bool {
        use librepcb_core::types::AutoUpdateMode;
        *self
            .workspace
            .lock()
            .settings()
            .libraries_auto_update_mode
            .get()
            != AutoUpdateMode::Disabled
    }

    /// Requests the library lists of the API servers (upstream
    /// `requestOnlineLibraries()`).
    pub(crate) fn fetch_online_libraries(&mut self) {
        let endpoints: Vec<String> = self
            .workspace
            .lock()
            .settings()
            .api_endpoints
            .get()
            .iter()
            .filter(|e| e.use_for_libraries && !e.url.is_empty())
            .map(|e| e.url.clone())
            .collect();
        if endpoints.is_empty() {
            return;
        }
        let generation = self.remote_libraries.start_fetch();
        self.update_libraries_data();
        crate::network::fetch_library_lists(endpoints, move |libraries, errors| {
            with_current_state(|s| s.online_libraries_received(generation, libraries, errors));
        });
    }

    /// The library lists of the API servers were received.
    pub(crate) fn online_libraries_received(
        &mut self,
        generation: u64,
        libraries: Vec<librepcb_network::Library>,
        errors: Vec<String>,
    ) {
        let versions = libraries
            .iter()
            .map(|l| (l.uuid, l.version.clone()))
            .collect();
        if !self
            .remote_libraries
            .set_online(generation, libraries, errors)
        {
            return;
        }
        self.local_libraries.set_online_versions(&versions);
        self.update_libraries_data();
        if let Some(msg) = library_manager::update_check_message(&self.remote_libraries.ui_data()) {
            self.show_status(&msg, 2500);
        }
        for (uuid, url) in self.remote_libraries.missing_icons() {
            crate::network::fetch_data(url, move |data| {
                with_current_state(|s| s.remote_libraries.set_icon(uuid, &data));
            });
        }
    }

    /// Uninstalls and installs the libraries of the pending changes
    /// (upstream `LibrariesModel::applyChanges()`).
    pub(crate) fn apply_library_changes(&mut self) {
        let changes = self.remote_libraries.take_pending_changes();
        let mut uninstalled = 0;
        for dir in &changes.uninstall {
            self.close_library(dir);
            if let Err(e) = file_utils::remove_dir_recursively(dir) {
                self.notifications.borrow_mut().push(Notification {
                    auto_popup: true,
                    ..Notification::new(
                        ui::NotificationType::Critical,
                        tr!(
                            "librepcb::editor::LibrariesModel",
                            "Failed to Uninstall Library"
                        ),
                        tr!(
                            "librepcb::editor::LibrariesModel",
                            "The directory '{0}' could not be removed: {1}",
                            dir.to_native(),
                            e
                        ),
                    )
                });
            }
            uninstalled += 1;
        }
        let remote_dir = self.workspace.lock().remote_libraries_path();
        for library in changes.install.iter().cloned() {
            let uuid = library.uuid;
            let existing = changes.existing.get(&uuid).cloned().unwrap_or_default();
            crate::network::install_library(library, remote_dir.clone(), existing, move |result| {
                with_current_state(|s| {
                    if let Err(e) = &result {
                        log::error!("Failed to install library {uuid}: {e}");
                        s.notifications.borrow_mut().push(Notification::new(
                            ui::NotificationType::Critical,
                            tr!(
                                "librepcb::editor::LibrariesModel",
                                "Failed to Install Library"
                            ),
                            e.clone(),
                        ));
                    }
                    if s.remote_libraries.install_finished(uuid) {
                        s.start_library_scan();
                    }
                    s.update_libraries_data();
                });
            });
        }
        if changes.install.is_empty() && uninstalled > 0 {
            self.start_library_scan();
        }
        self.update_libraries_data();
    }

    /// `Backend.trigger-library()` (upstream `MainWindow::triggerLibrary()`).
    pub(crate) fn trigger_library(&mut self, path: &str, action: ui::LibraryAction) {
        let libraries_path = self.workspace.lock().libraries_path().clone();
        let Some(fp) = FilePath::new(path).filter(|fp| fp.is_located_in_dir(&libraries_path))
        else {
            log::warn!("Invalid path in trigger_library(): {path}");
            return;
        };
        match action {
            ui::LibraryAction::Open => self.open_library_tab(&fp, false),
            ui::LibraryAction::Uninstall => {
                self.close_library(&fp);
                if let Err(e) = file_utils::remove_dir_recursively(&fp) {
                    self.notifications.borrow_mut().push(Notification {
                        auto_popup: true,
                        ..Notification::new(
                            ui::NotificationType::Critical,
                            tr!("librepcb::editor::MainWindow", "Error"),
                            e.to_string(),
                        )
                    });
                }
                self.start_library_scan();
            }
            other => self.trigger_library_new_element(&fp, other),
        }
    }

    /// `Backend.libraries-key-pressed()`: typing filters the libraries
    /// panel (upstream `SlintKeyEventTextBuilder`).
    pub(crate) fn libraries_filter_key(&mut self, event: &slint::language::KeyEvent) -> bool {
        let Some(w) = self.window() else {
            return false;
        };
        let d = w.global::<ui::Data>();
        let filter = d.get_libraries_panel_filter();
        let Some(new) =
            library_manager::filter_key_pressed(&filter, &event.text, event.modifiers.control)
        else {
            return false;
        };
        d.set_libraries_panel_filter(new.as_str().into());
        self.local_libraries.set_filter(&new);
        self.remote_libraries.set_filter(&new);
        true
    }
}
