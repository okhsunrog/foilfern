//! Libraries on the application side: opened libraries (`Data.libraries`,
//! upstream `GuiApplication::openLibrary()`), the library, create library
//! and download library tabs and the library requests of the tabs.
//!
//! Port of the library parts of libs/librepcb/editor/guiapplication.cpp and
//! libs/librepcb/editor/mainwindow.cpp (`openLibraryTab()`,
//! `closeLibrary()`).

use std::rc::Rc;

use librepcb_app_ui as ui;
use librepcb_core::fileio::{FilePath, file_utils};
use librepcb_i18n::tr;
use slint::ComponentHandle;

use super::{State, deferred, with_current_state};
use crate::notifications::Notification;
use crate::open_library::OpenLibrary;
use crate::tabs::download_library::DownloadResult;
use crate::tabs::{CreateLibraryTab, DownloadLibraryTab, LibraryTab, Tab, TabId, TabRequest};

impl State {
    /// The opened libraries.
    pub fn libraries(&self) -> &[Rc<OpenLibrary>] {
        &self.libraries
    }

    /// Updates `Data.libraries`.
    pub(crate) fn update_libraries_model(&self) {
        let rows = self.libraries.iter().map(|l| l.ui_data()).collect();
        self.libraries_model.replace_all(rows);
    }

    /// Opens a library (or returns the opened one; upstream
    /// `GuiApplication::openLibrary()`): libraries installed from the server
    /// are read-only.
    pub(crate) fn open_library(&mut self, dir: &FilePath) -> Option<Rc<OpenLibrary>> {
        if let Some(lib) = self.libraries.iter().find(|l| l.path() == dir) {
            return Some(Rc::clone(lib));
        }
        let read_only = dir.is_located_in_dir(&self.workspace.lock().remote_libraries_path());
        match OpenLibrary::open(dir, read_only) {
            Ok(lib) => {
                let lib = Rc::new(lib);
                self.libraries.push(Rc::clone(&lib));
                self.update_libraries_model();
                Some(lib)
            }
            Err(e) => {
                self.notifications.borrow_mut().push(Notification {
                    auto_popup: true,
                    ..Notification::new(
                        ui::NotificationType::Critical,
                        tr!("librepcb::editor::GuiApplication", "Error"),
                        e.to_string(),
                    )
                });
                None
            }
        }
    }

    /// Opens (or switches to) the library tab of a library (upstream
    /// `MainWindow::openLibraryTab()`).
    pub(crate) fn open_library_tab(&mut self, dir: &FilePath, wizard: bool) {
        let existing =
            self.find_tab_where(|t| matches!(t, Tab::Library(l) if l.library().path() == dir));
        if let Some((sec, index)) = existing {
            self.set_current_tab(sec, index as i32, true);
            return;
        }
        let Some(lib) = self.open_library(dir) else {
            return;
        };
        let index = self
            .libraries
            .iter()
            .position(|l| Rc::ptr_eq(l, &lib))
            .map_or(-1, |i| i as i32);
        let id = TabId::new();
        let w = self.this.clone();
        let on_check = move |row, data| {
            deferred(&w, move |s| {
                s.library_tab_event(id, |t| t.check_row_written(row, &data))
            });
        };
        let w = self.this.clone();
        let on_dependency = move |row, data| {
            deferred(&w, move |s| {
                s.library_tab_event(id, |t| t.dependency_row_written(row, &data));
            });
        };
        let tab = LibraryTab::new(
            lib,
            std::sync::Arc::clone(&self.workspace),
            index,
            wizard,
            on_check,
            on_dependency,
        )
        .with_id(id);
        self.add_tab(Tab::Library(Box::new(tab)));
        if let Some(w) = self.window() {
            let d = w.global::<ui::Data>();
            d.invoke_set_current_library(index);
            d.invoke_set_panel_page(ui::PanelPage::Documents);
        }
    }

    /// Runs `f` on the library tab `id` and applies its update.
    fn library_tab_event(
        &mut self,
        id: TabId,
        f: impl FnOnce(&mut LibraryTab) -> crate::tabs::TabUpdate,
    ) {
        let Some((si, ti)) = self.find_tab(id) else {
            return;
        };
        let update = match self.sections[si].tab_mut(ti) {
            Some(Tab::Library(t)) => f(t),
            _ => return,
        };
        self.apply_update(si, ti, update);
    }

    /// Closes the tabs of a library and the library itself (before it is
    /// removed; upstream `GuiApplication::closeLibrary()`).
    pub(crate) fn close_library(&mut self, dir: &FilePath) {
        for si in 0..self.sections.len() {
            while let Some(ti) = self.sections[si]
                .tabs()
                .iter()
                .position(|t| t.library().is_some_and(|l| l.path() == dir))
            {
                self.sections[si].remove_tab(ti);
            }
            self.update_section_row(si);
        }
        let before = self.libraries.len();
        self.libraries.retain(|l| l.path() != dir);
        if self.libraries.len() != before {
            self.update_libraries_model();
            self.update_library_indices();
        }
    }

    /// Updates the library index of the library tabs.
    fn update_library_indices(&mut self) {
        for si in 0..self.sections.len() {
            for ti in 0..self.sections[si].tabs().len() {
                if let Tab::Library(t) = &mut self.sections[si].tabs_mut()[ti] {
                    let index = self
                        .libraries
                        .iter()
                        .position(|l| Rc::ptr_eq(l, t.library()))
                        .map_or(-1, |i| i as i32);
                    t.set_library_index(index);
                }
            }
            self.sections[si].refresh_all(&self.projects);
        }
    }

    /// `Backend.trigger-library-element()` (upstream
    /// `MainWindow::triggerLibraryElement()`).
    pub(crate) fn trigger_library_element(&mut self, path: &str, action: ui::LibraryElementAction) {
        let Some(fp) = FilePath::new(path) else {
            return;
        };
        match action {
            ui::LibraryElementAction::Open => {
                let existing = self.find_tab_where(|t| t.directory_path().as_ref() == Some(&fp));
                if let Some((sec, index)) = existing {
                    self.set_current_tab(sec, index as i32, true);
                } else if self.libraries.iter().any(|l| l.path() == &fp) {
                    self.open_library_tab(&fp, false);
                }
            }
            ui::LibraryElementAction::Close => {
                // Unsaved changes are discarded (upstream asks).
                self.close_library(&fp);
            }
            ui::LibraryElementAction::OpenFolder => {
                if let Err(e) = open::that_detached(fp.as_path()) {
                    log::warn!("Failed to open {}: {e}", fp.to_native());
                }
            }
            ui::LibraryElementAction::ImportEagleLibrary => {
                self.show_library_import_wizard(&fp, false);
            }
            ui::LibraryElementAction::ImportKicadLibrary => {
                self.show_library_import_wizard(&fp, true);
            }
        }
    }

    /// Opens (or switches to) the "create library" tab.
    pub(crate) fn open_create_library_tab(&mut self) {
        if let Some((sec, index)) = self.find_tab_where(|t| matches!(t, Tab::CreateLibrary(_))) {
            self.set_current_tab(sec, index as i32, true);
            return;
        }
        let (dir, user) = {
            let ws = self.workspace.lock();
            (
                ws.local_libraries_path(),
                ws.settings().user_name.get().clone(),
            )
        };
        self.add_tab(Tab::CreateLibrary(Box::new(CreateLibraryTab::new(
            dir, &user,
        ))));
    }

    /// Opens (or switches to) the "download library" tab.
    pub(crate) fn open_download_library_tab(&mut self) {
        if let Some((sec, index)) = self.find_tab_where(|t| matches!(t, Tab::DownloadLibrary(_))) {
            self.set_current_tab(sec, index as i32, true);
            return;
        }
        let dir = self.workspace.lock().local_libraries_path();
        self.add_tab(Tab::DownloadLibrary(Box::new(DownloadLibraryTab::new(dir))));
    }

    /// The "new element" actions of the library tab (M4b).
    pub(crate) fn trigger_library_new_element(
        &mut self,
        lib: &FilePath,
        action: ui::LibraryAction,
    ) {
        self.open_new_library_element_tab(lib, action);
    }

    /// Handles a library request of a tab.
    pub(crate) fn apply_library_request(&mut self, tab: TabId, request: TabRequest) {
        match request {
            TabRequest::OpenLibrary { path, wizard } => {
                self.start_library_scan();
                self.open_library_tab(&path, wizard);
            }
            TabRequest::DownloadLibrary { url, dir } => {
                let handle = crate::network::download_library(
                    url,
                    dir,
                    move |status, percent| {
                        with_current_state(|s| {
                            s.download_tab_event(tab, |t| t.download_progress(status, percent));
                        });
                    },
                    move |result| {
                        with_current_state(|s| {
                            let result = match result {
                                Ok(dir) => DownloadResult::Done(dir),
                                Err(e) => DownloadResult::Failed(e),
                            };
                            s.download_tab_event(tab, |t| t.download_finished(result));
                        });
                    },
                );
                if let Some((si, ti)) = self.find_tab(tab)
                    && let Some(Tab::DownloadLibrary(t)) = self.sections[si].tab_mut(ti)
                {
                    t.set_download(handle);
                }
            }
            TabRequest::LibraryDownloaded(dir) => {
                self.local_libraries.highlight_on_next_rescan(dir);
                if let Some(w) = self.window() {
                    w.global::<ui::Data>()
                        .invoke_set_panel_page(ui::PanelPage::Libraries);
                }
                self.start_library_scan();
            }
            TabRequest::LibraryModified => {
                self.update_libraries_model();
                let lib = self
                    .find_tab(tab)
                    .and_then(|(si, ti)| self.sections[si].tabs()[ti].library().cloned());
                if let Some(lib) = lib {
                    self.library_modified(tab, &lib);
                }
            }
            TabRequest::RescanLibraries => self.start_library_scan(),
            TabRequest::OpenLibraryElement {
                library,
                kind,
                path,
                duplicate,
            } => self.open_library_element_tab(&library, kind, &path, duplicate),
            TabRequest::RemoveLibraryElements(elements) => {
                let dialog = crate::dialogs::library::RemoveElementsDialog::new(&elements);
                self.open_library_dialog(Some(tab), Box::new(dialog));
            }
            TabRequest::ChooseLibraryIcon => {
                let filters = [crate::file_dialog::Filter {
                    name: tr!(
                        "librepcb::editor::LibraryTab",
                        "Portable Network Graphics (*.png)"
                    ),
                    extensions: vec!["png", "PNG"],
                }];
                let title = tr!("librepcb::editor::LibraryTab", "Choose Library Icon");
                if let Some(path) = crate::file_dialog::open_file(&title, &filters, None) {
                    match std::fs::read(&path) {
                        Ok(png) => {
                            let is_org = self.find_tab(tab).is_some_and(|(si, ti)| {
                                matches!(self.sections[si].tabs()[ti], Tab::Organization(_))
                            });
                            if is_org {
                                self.element_tab_event(tab, |t| match t {
                                    Tab::Organization(o) => o.set_logo(png),
                                    _ => crate::tabs::TabUpdate::default(),
                                });
                            } else {
                                self.library_tab_event(tab, |t| t.set_icon(png));
                            }
                        }
                        Err(e) => self.notifications.borrow_mut().push(Notification::new(
                            ui::NotificationType::Critical,
                            tr!("librepcb::editor::LibraryTab", "Could not open file"),
                            e.to_string(),
                        )),
                    }
                }
            }
            TabRequest::DuplicateLibraryElement => self.duplicate_element_tab(tab),
            TabRequest::LibraryItemProperties(item) => self.open_library_item_properties(tab, item),
            TabRequest::ImportPinsDialog => self.open_import_pins_dialog(tab),
            TabRequest::ChooseStepFile { model } => self.choose_step_file(tab, model),
            TabRequest::CourtyardOffsetDialog => self.open_courtyard_offset_dialog(tab),
            TabRequest::MoveAlign { positions } => self.open_move_align_dialog(tab, positions),
            TabRequest::ChooseElement(purpose) => self.open_element_chooser(tab, purpose),
            TabRequest::ChoosePinoutFile => self.choose_pinout_file(tab),
            TabRequest::ConfirmClose {
                title,
                text,
                can_save,
            } => {
                let dialog =
                    crate::dialogs::library::SaveChangesDialog::new(title, &text, can_save);
                self.open_library_dialog(Some(tab), Box::new(dialog));
            }
            TabRequest::DesignRulesName { purpose, name } => {
                let dialog = crate::dialogs::library::DesignRulesNameDialog::new(purpose, &name);
                self.open_library_dialog(Some(tab), Box::new(dialog));
            }
            TabRequest::OpenUrl(url) => {
                if let Err(e) = open::that_detached(&url) {
                    log::warn!("Failed to open {url}: {e}");
                }
            }
            _ => {}
        }
    }

    fn download_tab_event(
        &mut self,
        id: TabId,
        f: impl FnOnce(&mut DownloadLibraryTab) -> crate::tabs::TabUpdate,
    ) {
        let Some((si, ti)) = self.find_tab(id) else {
            return;
        };
        let update = match self.sections[si].tab_mut(ti) {
            Some(Tab::DownloadLibrary(t)) => f(t),
            _ => return,
        };
        self.apply_update(si, ti, update);
    }

    /// Updates the other tabs of a library after one of them modified it.
    fn library_modified(&mut self, origin: TabId, lib: &Rc<OpenLibrary>) {
        for si in 0..self.sections.len() {
            for ti in 0..self.sections[si].tabs().len() {
                if let Tab::Library(t) = &mut self.sections[si].tabs_mut()[ti]
                    && t.id() != origin
                    && Rc::ptr_eq(t.library(), lib)
                {
                    let update = t.library_modified();
                    self.apply_update(si, ti, update);
                }
            }
        }
    }

    /// Removes library elements (confirmed in the dialog; upstream
    /// `LibraryTab::deleteElements()`).
    pub(crate) fn remove_library_elements(&mut self, paths: &[FilePath]) {
        self.close_library_element_tabs(paths);
        for fp in paths {
            if let Err(e) = file_utils::remove_dir_recursively(fp) {
                self.notifications.borrow_mut().push(Notification::new(
                    ui::NotificationType::Critical,
                    tr!("librepcb::editor::LibraryTab", "Error"),
                    e.to_string(),
                ));
            }
        }
        self.start_library_scan();
    }

    /// After a library scan: refresh the content of the library tabs.
    pub(crate) fn refresh_library_tabs(&mut self) {
        for si in 0..self.sections.len() {
            for ti in 0..self.sections[si].tabs().len() {
                if let Tab::Library(t) = &mut self.sections[si].tabs_mut()[ti] {
                    t.refresh_elements();
                    self.sections[si].refresh_tab(ti, &self.projects);
                }
            }
        }
    }
}
