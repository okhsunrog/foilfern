//! Shows the form dialogs of [`crate::dialogs`] in the main window (the
//! `Dialogs` global of `ui/dialogs/formdialog.slint`) and routes the UI's
//! edits and button clicks to the open dialog.
//!
//! Upstream opens modal Qt dialogs (`QDialog::exec()`); here one dialog is
//! shown at a time as an overlay of the main window, and the application
//! keeps running while it is open.

use std::rc::Rc;

use librepcb_app_ui as ui;
use slint::{ComponentHandle, SharedString};

use super::{State, deferred};
use crate::dialogs::{Applied, ButtonResult, DialogContext, FormDialog, TabDialogResult};
use crate::models::vec_model;
use crate::project::AppProject;
use crate::tabs::TabId;

/// The open form dialog.
pub struct OpenDialog {
    /// The project the dialog works on (`None` for library editor
    /// dialogs).
    pub project: Option<Rc<AppProject>>,
    /// The tab which opened the dialog (for results going back to it).
    pub tab: Option<TabId>,
    /// The dialog.
    pub dialog: Box<dyn FormDialog>,
}

impl std::fmt::Debug for OpenDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenDialog")
            .field("title", &self.dialog.title())
            .finish_non_exhaustive()
    }
}

/// Binds the callbacks of the `Dialogs` global.
pub fn bind(window: &ui::AppWindow, weak: &std::rc::Weak<std::cell::RefCell<State>>) {
    let d = window.global::<ui::Dialogs>();
    let w = weak.clone();
    d.on_form_button(move |button| {
        deferred(&w, move |s| s.form_dialog_button(button));
    });
}

impl State {
    /// Shows a form dialog (replacing an open one).
    pub(crate) fn open_form_dialog(
        &mut self,
        project: Rc<AppProject>,
        tab: Option<TabId>,
        dialog: Box<dyn FormDialog>,
    ) {
        self.open_dialog(Some(project), tab, dialog);
    }

    /// Shows a form dialog of a library editor tab (without project).
    pub fn open_library_dialog(&mut self, tab: Option<TabId>, dialog: Box<dyn FormDialog>) {
        self.open_dialog(None, tab, dialog);
    }

    fn open_dialog(
        &mut self,
        project: Option<Rc<AppProject>>,
        tab: Option<TabId>,
        dialog: Box<dyn FormDialog>,
    ) {
        let w = self.this.clone();
        dialog.form().model().set_handler(move |row, data| {
            deferred(&w, move |s| s.form_dialog_row_written(row, data));
        });
        self.form_dialog = Some(OpenDialog {
            project,
            tab,
            dialog,
        });
        if let Some(w) = self.window() {
            let d = w.global::<ui::Dialogs>();
            d.set_form_page(0);
            d.set_form_error(SharedString::new());
            d.set_form_busy(false);
        }
        self.refresh_form_dialog(true);
        if let Some(w) = self.window() {
            w.global::<ui::Dialogs>().set_form_shown(true);
        }
    }

    /// Shows a form dialog which does not belong to a tab (menu actions,
    /// tests).
    pub fn show_form_dialog(&mut self, project: Rc<AppProject>, dialog: Box<dyn FormDialog>) {
        self.open_form_dialog(project, None, dialog);
    }

    /// Shows a page (tab) of the open form dialog.
    pub fn show_form_dialog_page(&mut self, page: i32) {
        if let Some(w) = self.window() {
            w.global::<ui::Dialogs>().set_form_page(page);
        }
    }

    /// The open form dialog (tests).
    pub fn form_dialog(&mut self) -> Option<&mut OpenDialog> {
        self.form_dialog.as_mut()
    }

    /// Updates the `Dialogs` properties from the open dialog.
    pub(crate) fn refresh_form_dialog(&mut self, all: bool) {
        let workspace = self.workspace.clone();
        let Some(open) = self.form_dialog.as_mut() else {
            return;
        };
        let Some(w) = self.window.upgrade() else {
            return;
        };
        let d = w.global::<ui::Dialogs>();
        if all {
            let options = open.dialog.options();
            let form = open.dialog.form();
            d.set_form_title(open.dialog.title().into());
            d.set_form_pages(vec_model(
                form.pages()
                    .iter()
                    .map(|p| SharedString::from(p.as_str()))
                    .collect(),
            ));
            d.set_form_fields(form.model_rc());
            d.set_form_has_side(form.has_side_column());
            d.set_form_has_apply(options.apply);
            d.set_form_has_cancel(options.cancel);
            d.set_form_ok_text(options.ok_text.unwrap_or_default().into());
            d.set_form_extra_buttons(vec_model(
                options
                    .extra_buttons
                    .iter()
                    .map(|b| SharedString::from(b.as_str()))
                    .collect(),
            ));
            d.set_form_width(options.width);
            d.set_form_label_width(options.label_width);
            d.set_form_side_width(options.side_width);
        }
        let ctx = DialogContext {
            project: open.project.as_ref(),
            workspace: Some(&workspace),
        };
        match open.dialog.preview(&ctx) {
            Some((image, caption)) => {
                d.set_form_preview(image);
                d.set_form_preview_caption(caption.into());
                d.set_form_preview_shown(true);
            }
            None => d.set_form_preview_shown(false),
        }
    }

    /// A row of the form was written by the UI.
    fn form_dialog_row_written(&mut self, row: usize, data: ui::FormField) {
        let workspace = self.workspace.clone();
        let Some(open) = self.form_dialog.as_mut() else {
            return;
        };
        let Some((id, event)) = open.dialog.form_mut().ui_written(row, &data) else {
            return;
        };
        let ctx = DialogContext {
            project: open.project.as_ref(),
            workspace: Some(&workspace),
        };
        open.dialog.field_event(&ctx, &id, event);
        let request = open.dialog.take_request();
        // Dialogs may rebuild their pages (e.g. output jobs).
        let pages_changed = self.window().is_some_and(|w| {
            let d = w.global::<ui::Dialogs>();
            let n = slint::Model::row_count(&d.get_form_pages());
            self.form_dialog
                .as_ref()
                .is_some_and(|o| o.dialog.form().pages().len() != n)
        });
        self.refresh_form_dialog(pages_changed);
        if let Some(request) = request {
            self.handle_app_request(request);
        }
    }

    /// `Dialogs.form-button()`.
    fn form_dialog_button(&mut self, button: i32) {
        match button {
            -2 => self.close_form_dialog(),
            -1 | 0 => self.apply_form_dialog(button == -1),
            n => self.form_dialog_extra_button((n - 1) as usize),
        }
    }

    /// Applies the open dialog ("OK" closes it on success).
    pub fn apply_form_dialog(&mut self, close: bool) {
        let workspace = self.workspace.clone();
        let Some(open) = self.form_dialog.as_mut() else {
            return;
        };
        let project = open.project.clone();
        let tab = open.tab;
        let ctx = DialogContext {
            project: project.as_ref(),
            workspace: Some(&workspace),
        };
        let result = open.dialog.apply(&ctx);
        if let Some(request) = open.dialog.take_request() {
            self.handle_app_request(request);
        }
        match result {
            Ok(applied) => {
                if close {
                    self.close_form_dialog();
                } else if let Some(w) = self.window() {
                    w.global::<ui::Dialogs>()
                        .set_form_error(SharedString::new());
                }
                match applied {
                    Applied::Nothing => {}
                    Applied::Project => {
                        if let Some(project) = &project {
                            self.project_changed_by_tab(project);
                        }
                    }
                    Applied::Tab(result) => self.form_dialog_tab_result(tab, result),
                    Applied::RunJobs { title, jobs } => {
                        if let Some(project) = &project {
                            self.run_jobs(project, title, jobs);
                        }
                    }
                    Applied::App(request) => self.handle_app_request(request),
                }
                if !close {
                    self.refresh_form_dialog(false);
                }
            }
            Err(message) => {
                // The dialog may have changed its page (wizards).
                self.refresh_form_dialog(true);
                if let Some(w) = self.window() {
                    w.global::<ui::Dialogs>().set_form_error(message.into());
                }
            }
        }
    }

    fn form_dialog_extra_button(&mut self, index: usize) {
        let workspace = self.workspace.clone();
        let Some(open) = self.form_dialog.as_mut() else {
            return;
        };
        let project = open.project.clone();
        let ctx = DialogContext {
            project: project.as_ref(),
            workspace: Some(&workspace),
        };
        let result = open.dialog.button(&ctx, index);
        let request = open.dialog.take_request();
        match result {
            Ok(ButtonResult::Keep) => self.refresh_form_dialog(true),
            Ok(ButtonResult::Modified) => {
                self.refresh_form_dialog(true);
                if let Some(project) = &project {
                    self.project_changed_by_tab(project);
                }
            }
            Ok(ButtonResult::Close) => self.close_form_dialog(),
            Ok(ButtonResult::TabResult(result)) => {
                let tab = open.tab;
                self.close_form_dialog();
                self.form_dialog_tab_result(tab, result);
            }
            Ok(ButtonResult::RunJobs { title, jobs }) => {
                self.refresh_form_dialog(true);
                if let Some(project) = &project {
                    self.run_jobs(project, title, jobs);
                }
            }
            Ok(ButtonResult::App(request)) => {
                self.close_form_dialog();
                self.handle_app_request(request);
            }
            Err(message) => {
                if let Some(w) = self.window() {
                    w.global::<ui::Dialogs>().set_form_error(message.into());
                }
            }
        }
        if let Some(request) = request {
            self.handle_app_request(request);
        }
    }

    /// Closes the open dialog without applying it.
    pub fn close_form_dialog(&mut self) {
        if let Some(open) = &mut self.form_dialog {
            open.dialog.closing();
        }
        self.form_dialog = None;
        if let Some(w) = self.window() {
            let d = w.global::<ui::Dialogs>();
            d.set_form_shown(false);
            d.set_form_fields(vec_model(Vec::new()));
        }
    }

    fn form_dialog_tab_result(&mut self, tab: Option<TabId>, result: TabDialogResult) {
        if let TabDialogResult::RemoveLibraryElements(paths) = &result {
            self.remove_library_elements(paths);
            return;
        }
        let Some((si, ti)) = tab.and_then(|t| self.find_tab(t)) else {
            return;
        };
        if let TabDialogResult::CloseTab { save } = result {
            if save {
                let update = self.sections[si].tabs_mut()[ti].trigger(ui::TabAction::Save);
                let saved = !self.sections[si].tabs()[ti].ui_data().unsaved_changes;
                self.apply_update(si, ti, update);
                if !saved {
                    return;
                }
            }
            if let Some((si, ti)) = tab.and_then(|t| self.find_tab(t)) {
                self.close_tab(si, ti);
            }
            return;
        }
        let update = match result {
            TabDialogResult::CloseTab { .. } => return,
            TabDialogResult::LineWidth(width) => {
                self.sections[si].tabs_mut()[ti].set_line_width(width)
            }
            TabDialogResult::ImportDxf(settings) => {
                self.sections[si].tabs_mut()[ti].import_dxf(settings)
            }
            TabDialogResult::RemoveLibraryElements(_) => return,
            TabDialogResult::LibraryObject(object) => {
                self.sections[si].tabs_mut()[ti].update_library_object(object)
            }
            TabDialogResult::ImportPins(names) => match &mut self.sections[si].tabs_mut()[ti] {
                crate::tabs::Tab::Symbol(t) => t.import_pins(names),
                _ => return,
            },
            TabDialogResult::DesignRulesName(purpose, name) => {
                self.sections[si].tabs_mut()[ti].design_rules_named(purpose, &name)
            }
            TabDialogResult::ElementChosen(purpose, uuid) => {
                self.sections[si].tabs_mut()[ti].element_chosen(purpose, uuid)
            }
            TabDialogResult::CourtyardOffset(offset) => {
                self.sections[si].tabs_mut()[ti].generate_courtyard(offset)
            }
            TabDialogResult::Positions(positions) => {
                self.sections[si].tabs_mut()[ti].move_align(&positions)
            }
        };
        self.apply_update(si, ti, update);
        self.after_tab_event(si, ti);
    }
}
