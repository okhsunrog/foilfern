//! The project lifecycle (milestone M3c): opening projects with the
//! directory lock and autosave restore prompts, the file format upgrade
//! notification, the new project wizard, autosave, and the requests of
//! the application level dialogs.
//!
//! Port of `GuiApplication::createProject()` / `openProject()`
//! (libs/librepcb/editor/guiapplication.cpp) and of the autosave timer and
//! the migration notification of `ProjectEditor`
//! (libs/librepcb/editor/project/projecteditor.cpp).

use std::rc::Rc;
use std::time::Duration;

use librepcb_app_ui as ui;
use librepcb_core::fileio::FilePath;
use librepcb_i18n::{tr, trn};
use slint::ComponentHandle;

use super::State;
use crate::color_schemes::{ColorSchemes, SchemeKind};
use crate::dialogs::initialize_workspace::InitializeWorkspaceWizard;
use crate::dialogs::new_project::{NewProjectMode, NewProjectWizard};
use crate::dialogs::open_prompts::{DirectoryLockDialog, RestoreAutosaveDialog};
use crate::dialogs::project_library_updater::ProjectLibraryUpdaterDialog;
use crate::dialogs::workspace_settings::WorkspaceSettingsDialog;
use crate::dialogs::{AppRequest, FormDialog};
use crate::notifications::{Notification, NotificationButton};
use crate::project::{AppProject, OpenOutcome, OpenRequest};
use crate::tabs::{ProjectLibraryTab, Tab};

const PE: &str = "librepcb::editor::ProjectEditor";

impl State {
    /// Shows a dialog which belongs neither to a project nor to a tab
    /// (wizards, prompts, workspace settings).
    pub fn show_app_dialog(&mut self, dialog: Box<dyn FormDialog>) {
        self.open_library_dialog(None, dialog);
    }

    /// Handles a request of an application level dialog.
    pub fn handle_app_request(&mut self, request: AppRequest) {
        match request {
            AppRequest::OpenProject {
                path,
                import_messages,
            } => {
                self.open_project(&path);
                if !import_messages.is_empty() {
                    self.notifications.borrow_mut().push(Notification {
                        auto_popup: true,
                        ..Notification::new(
                            ui::NotificationType::Info,
                            tr!(
                                "librepcb::editor::NewProjectWizardPage_EagleImport",
                                "EAGLE Project Import"
                            ),
                            import_messages.join("\n"),
                        )
                    });
                }
            }
            AppRequest::ContinueOpening(request) => {
                self.open_project_with(request);
            }
            AppRequest::WorkspaceSettingsChanged => self.workspace_settings_changed(),
            AppRequest::ShowDialog(kind) => self.show_dialog_kind(kind),
            AppRequest::RescanLibraries => self.start_library_scan(),
            AppRequest::UpdateProjectLibrary(fp) => self.update_project_library(&fp),
            AppRequest::WorkspaceChosen(fp) => self.workspace_chosen(&fp),
            AppRequest::RunLibraryImport => self.run_library_import(),
        }
    }

    /// Opens the new project wizard (upstream
    /// `GuiApplication::createProject()`), in EAGLE import mode or for a
    /// new project in `location` (default: the workspace's projects
    /// directory).
    pub fn show_new_project_wizard(&mut self, eagle_import: bool, location: Option<FilePath>) {
        let (projects, author, locales, norms) = {
            let ws = self.workspace.lock();
            (
                ws.projects_path().clone(),
                ws.settings().user_name.get().clone(),
                ws.settings().library_locale_order.get().clone(),
                ws.settings().library_norm_order.get().clone(),
            )
        };
        let mode = if eagle_import {
            NewProjectMode::EagleImport
        } else {
            NewProjectMode::NewProject
        };
        let wizard =
            NewProjectWizard::new(mode, location.unwrap_or(projects), &author, locales, norms);
        self.show_app_dialog(Box::new(wizard));
    }

    /// Opens a project and its first schematic and board (upstream
    /// `GuiApplication::openProject()`); returns its index, `None` if it
    /// could not be opened or a prompt (directory lock, autosave restore)
    /// is shown first.
    pub(crate) fn open_project(&mut self, fp: &FilePath) -> Option<usize> {
        self.open_project_with(OpenRequest::new(fp.clone()))
    }

    /// Opens a project with the answers of the prompts so far.
    pub(crate) fn open_project_with(&mut self, request: OpenRequest) -> Option<usize> {
        let fp = request.path.clone();
        if let Some(i) = self.projects.iter().position(|p| *p.path() == fp) {
            self.switch_to_project(i);
            return Some(i);
        }
        let source = self.workspace.lock().shared_library_db();
        let project = match AppProject::open_with(&request, source) {
            Ok(OpenOutcome::Opened(p)) => Rc::new(*p),
            Ok(OpenOutcome::Locked {
                directory,
                user,
                can_override,
            }) => {
                let dialog = DirectoryLockDialog::new(request, &directory, &user, can_override);
                self.show_app_dialog(Box::new(dialog));
                return None;
            }
            Ok(OpenOutcome::AutosaveDetected) => {
                self.show_app_dialog(Box::new(RestoreAutosaveDialog::new(request)));
                return None;
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
                return None;
            }
        };
        if !project.is_writable() {
            self.notifications.borrow_mut().push(Notification::new(
                ui::NotificationType::Warning,
                tr!("librepcb::editor::GuiApplication", "Read-Only Mode"),
                fp.to_native(),
            ));
        }
        self.show_migration_notification(&project);
        let index = self.projects.len();
        self.projects_model.push(project.ui_data());
        self.projects.push(project);
        self.switch_to_project(index);
        let (has_schematic, has_board) = {
            let p = self.projects[index].shared().lock();
            (
                !p.project().schematics().is_empty(),
                !p.project().boards().is_empty(),
            )
        };
        if has_schematic {
            self.open_schematic_tab(index as i32, 0, true);
        }
        if has_board {
            self.open_board_tab(index as i32, 0, !has_schematic);
        }
        self.quick_access.borrow_mut().push_recent(&fp);
        Some(index)
    }

    /// Upstream `ProjectEditor` constructor: the notification about a file
    /// format upgrade, with a button to show the migration log (written
    /// into the project directory, like upstream).
    fn show_migration_notification(&mut self, project: &Rc<AppProject>) {
        let (log, name) = {
            let p = project.shared().lock();
            let Some(log) = p.migration_log.clone() else {
                return;
            };
            let m = p.project().metadata();
            (log, format!("{} {}", m.name.as_str(), m.version))
        };
        let msg1 = tr!(
            PE,
            "The project '{0}' has been migrated to a new file format. After saving, it will not be possible anymore to open it with an older LibrePCB version!",
            name
        );
        let count = log.messages.len();
        let mut msg = msg1.clone();
        if count > 0 {
            msg.push_str("\n\n");
            msg.push_str(&trn!(
                PE,
                "The migration produced {n} message(s), please review before proceeding.",
                "The migration produced {n} message(s), please review before proceeding.",
                count
            ));
        }
        let button = (count > 0).then(|| {
            let dir = project.path().parent_dir();
            let notifications = Rc::downgrade(&self.notifications);
            let weak_project = Rc::downgrade(project);
            NotificationButton {
                text: trn!(PE, "Show {n} Message(s)", "Show {n} Message(s)", count),
                action: Rc::new(move || {
                    let Some(dir) = &dir else { return };
                    let fp = dir.path_to(&log.relative_file_path(true));
                    let html = log.to_html(true, super::APP_VERSION);
                    let result =
                        librepcb_core::fileio::file_utils::write_file(&fp, html.as_bytes())
                            .map_err(|e| e.to_string())
                            .and_then(|()| {
                                open::that_detached(fp.as_path()).map_err(|e| e.to_string())
                            });
                    if let Err(e) = result {
                        log::error!("Failed to open the migration log: {e}");
                    }
                    // Append the file path to the description, in case the
                    // log failed to open in the web browser.
                    if let (Some(n), Some(p)) = (notifications.upgrade(), weak_project.upgrade())
                        && let Some(id) = p.migration_notification.get()
                    {
                        let text = format!(
                            "{msg1}\n\n{}",
                            tr!(PE, "Migration log saved to '{0}'.", fp.to_native())
                        );
                        n.borrow_mut()
                            .update(id, |d| d.description = text.as_str().into());
                    }
                }),
            }
        });
        let id = self.notifications.borrow_mut().push_with_id(Notification {
            auto_popup: true,
            button,
            ..Notification::new(
                ui::NotificationType::Warning,
                tr!(PE, "ATTENTION: Project File Format Upgraded"),
                msg,
            )
        });
        project.migration_notification.set(id);
    }

    /// Dismisses the migration notification of a saved project (upstream:
    /// connected to `projectSavedToDisk()`).
    pub(crate) fn project_saved(&mut self, project: &AppProject) {
        if let Some(id) = project.migration_notification.take() {
            self.notifications.borrow_mut().dismiss(id);
        }
    }

    /// (Re)starts the autosave timer with the interval of the workspace
    /// settings (upstream `setupAutoSaveTimer` of `ProjectEditor`; one
    /// timer for all projects here); an interval of 0 disables autosave.
    pub(crate) fn setup_autosave_timer(&self) {
        let secs = *self
            .workspace
            .lock()
            .settings()
            .project_autosave_interval_seconds
            .get();
        if secs == 0 {
            self.autosave_timer.stop();
            return;
        }
        let weak = self.this.clone();
        self.autosave_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_secs(u64::from(secs)),
            move || super::deferred(&weak, State::autosave_projects),
        );
    }

    /// Autosaves all open projects (upstream
    /// `ProjectEditor::autosaveProject()`).
    pub fn autosave_projects(&mut self) {
        for project in &self.projects {
            match project.autosave() {
                Ok(true) => log::debug!("Successfully autosaved project."),
                Ok(false) => {}
                Err(e) => log::warn!("Project autosave failed: {e}"),
            }
        }
    }

    /// Opens the initialize workspace wizard to choose another workspace
    /// (upstream `GuiApplication::switchWorkspace()`).
    pub fn show_switch_workspace_wizard(&mut self) {
        let path = self.workspace.lock().path().clone();
        let wizard = InitializeWorkspaceWizard::new(Some(path), true);
        self.show_app_dialog(Box::new(wizard));
    }

    /// Upstream `switchWorkspace()` after the wizard: the chosen workspace
    /// is used after restarting the application.
    fn workspace_chosen(&mut self, fp: &FilePath) {
        if let Err(e) = crate::startup::set_most_recently_used_workspace(fp) {
            log::warn!("Failed to store the workspace path: {e}");
        }
        if fp != self.workspace.lock().path() {
            self.notifications.borrow_mut().push(Notification {
                auto_popup: true,
                ..Notification::new(
                    ui::NotificationType::Info,
                    tr!("librepcb::editor::GuiApplication", "Workspace changed"),
                    tr!(
                        "librepcb::editor::GuiApplication",
                        "The chosen workspace will be used after restarting the application."
                    ),
                )
            });
        }
    }

    /// Opens the workspace settings dialog (upstream
    /// `GuiApplication::execWorkspaceSettingsDialog()`).
    pub fn show_workspace_settings(&mut self) {
        let settings = self.workspace.lock().settings().clone();
        let dialog = WorkspaceSettingsDialog::new(&settings);
        self.show_app_dialog(Box::new(dialog));
    }

    /// Applies the workspace settings to the application (at startup and
    /// after the settings dialog): autosave interval, keyboard shortcuts,
    /// theme, language, grid styles and color schemes.
    pub(crate) fn workspace_settings_changed(&mut self) {
        self.apply_workspace_settings(true);
    }

    /// Applies the workspace settings; the language only if `language`
    /// (at startup, the command line option may override it).
    pub(crate) fn apply_workspace_settings(&mut self, language: bool) {
        self.setup_autosave_timer();
        let (overrides, theme, locale, sch_grid, brd_grid, sch_schemes, brd_schemes) = {
            let ws = self.workspace.lock();
            let s = ws.settings();
            (
                s.keyboard_shortcuts.get().overrides().clone(),
                s.ui_theme.get().clone(),
                s.application_locale.get().clone(),
                *s.schematic_grid_style.get(),
                *s.board_grid_style.get(),
                ColorSchemes::load(SchemeKind::Schematic, s.schematic_color_schemes.get()),
                ColorSchemes::load(SchemeKind::Board, s.board_color_schemes.get()),
            )
        };
        crate::shortcuts::set_overrides(&overrides);
        let theme = crate::theme::UiTheme::from_setting(&theme);
        if theme != self.theme {
            self.theme = theme;
            if let Some(w) = self.window() {
                w.global::<ui::Data>().set_theme(theme.to_ui());
            }
        }
        if language && !locale.is_empty() && locale != librepcb_i18n::current_language() {
            match librepcb_i18n::set_language(&locale) {
                Ok(lang) => {
                    if let Err(e) = slint::select_bundled_translation(lang) {
                        log::warn!("Failed to select the UI language {lang}: {e}");
                    }
                }
                Err(e) => log::warn!("{e}"),
            }
        }
        self.apply_grid_styles(sch_grid, brd_grid);
        self.color_schemes = (sch_schemes.scene_scheme(), brd_schemes.scene_scheme());
        for si in 0..self.sections.len() {
            for ti in 0..self.sections[si].tabs().len() {
                let update = self.sections[si].tabs_mut()[ti]
                    .set_color_schemes(&self.color_schemes.0, &self.color_schemes.1);
                if update != crate::tabs::TabUpdate::default() {
                    self.apply_update(si, ti, update);
                }
            }
        }
    }

    /// Shows the project library updater (upstream
    /// `GuiApplication::openProjectLibraryUpdater()`).
    pub fn show_project_library_updater(&mut self, project: FilePath, log: &[String]) {
        let dialog = ProjectLibraryUpdaterDialog::new(project, log);
        self.show_app_dialog(Box::new(dialog));
    }

    /// Runs the project library updater: closes the project if it is open
    /// (not if it has unsaved changes), updates its library and reopens
    /// it with its library tab (upstream `btnUpdateClicked()` with the
    /// close callback and `finishedUpdate()`).
    pub fn update_project_library(&mut self, fp: &FilePath) {
        const CTX: &str = "librepcb::editor::ProjectLibraryUpdater";
        let mut log = Vec::new();
        let open = self.projects.iter().find(|p| p.path() == fp).cloned();
        if let Some(project) = &open {
            log.push(tr!(CTX, "Ask to close project (confirm message box!)"));
            if project.shared().lock().has_unsaved_changes() {
                log.push(tr!(
                    "librepcb::editor::ProjectEditor",
                    "The project contains unsaved changes. Please save it first."
                ));
                log.push(tr!(CTX, "Abort."));
                self.show_project_library_updater(fp.clone(), &log);
                return;
            }
            self.close_project(project);
        }
        // Release the directory lock (the last reference to the project).
        let was_open = open.is_some();
        drop(open);
        let db = self.workspace.lock().shared_library_db();
        crate::dialogs::project_library_updater::update_project_library(&db, fp, &mut log);
        if was_open && let Some(index) = self.open_project(fp) {
            self.open_project_library_tab(index);
        }
        self.show_project_library_updater(fp.clone(), &log);
    }

    /// Opens (or switches to) the library tab of a project (upstream
    /// `MainWindow::openProjectLibraryTab()`).
    pub fn open_project_library_tab(&mut self, index: usize) {
        let Some(project) = self.projects.get(index).cloned() else {
            return;
        };
        let existing = self.find_tab_where(
            |t| matches!(t, Tab::ProjectLibrary(t) if Rc::ptr_eq(t.project(), &project)),
        );
        if let Some((sec, index)) = existing {
            self.set_current_tab(sec, index as i32, true);
            return;
        }
        let mut tab = ProjectLibraryTab::new(project);
        tab.refresh(self.workspace.lock().library_db());
        let id = tab.id();
        let w = self.this.clone();
        tab.items_model().set_handler(move |row, data| {
            super::deferred(&w, move |s| {
                if let Some((sec, index)) = s.find_tab(id)
                    && let Some(Tab::ProjectLibrary(t)) = s.sections[sec].tab_mut(index)
                {
                    let update = t.item_written(row, &data);
                    s.apply_update(sec, index, update);
                }
            });
        });
        self.add_tab(Tab::ProjectLibrary(Box::new(tab)));
    }

    /// Refreshes the project library tabs (after project modifications,
    /// or `reset` after a library rescan).
    pub(crate) fn refresh_project_library_tabs(&mut self, reset: bool) {
        let db = self.workspace.lock().shared_library_db();
        for si in 0..self.sections.len() {
            for ti in 0..self.sections[si].tabs().len() {
                if let Tab::ProjectLibrary(t) = &mut self.sections[si].tabs_mut()[ti] {
                    if t.project().shared().try_lock().is_none() {
                        continue;
                    }
                    let changed = if reset {
                        t.reset(&db);
                        true
                    } else {
                        t.refresh_if_modified(&db)
                    };
                    if changed {
                        self.sections[si].refresh_tab(ti, &self.projects);
                    }
                }
            }
        }
    }

    fn show_dialog_kind(&mut self, kind: crate::dialogs::DialogKind) {
        match kind {
            crate::dialogs::DialogKind::ColorScheme(kind) => {
                self.show_color_scheme_editor(kind);
            }
        }
    }
}
