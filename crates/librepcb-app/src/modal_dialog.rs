//! Shows a form dialog in the main window before the application exists
//! (the initialize workspace wizard at startup, upstream `QDialog::exec()`
//! in `main.cpp`).
//!
//! A minimal version of the application's dialog host (`app::dialog_host`)
//! without application state: it routes the UI's edits and button clicks to
//! the dialog and reports the result.

use std::cell::RefCell;
use std::rc::Rc;

use librepcb_app_ui as ui;
use slint::{ComponentHandle, SharedString};

use crate::dialogs::{AppRequest, Applied, ButtonResult, DialogContext, FormDialog};
use crate::models::{defer, vec_model};

/// Replaces the preview data of the `.slint` files in the main window by
/// empty data (the window shows only the dialog before the application
/// exists).
fn clear_window(window: &ui::AppWindow) {
    let d = window.global::<ui::Data>();
    d.set_preview_mode(false);
    d.set_sections(vec_model(Vec::new()));
    d.set_projects(vec_model(Vec::new()));
    d.set_libraries(vec_model(Vec::new()));
    d.set_quick_access_items(vec_model(Vec::new()));
    d.set_workspace_folder_tree(vec_model(Vec::new()));
    d.set_notifications(vec_model(Vec::new()));
    d.set_notifications_unread(0);
    d.set_notifications_progress_index(-1);
    d.set_workspace_path(SharedString::new());
    d.set_status_bar_message(SharedString::new());
    d.set_cursor_coordinates(SharedString::new());
    d.set_panel_page(ui::PanelPage::None);
    d.set_window_title("LibrePCB".into());
}

/// The result of a modal form dialog.
#[derive(Debug, Clone, PartialEq)]
pub enum ModalResult {
    /// Still open.
    Open,
    /// Canceled.
    Canceled,
    /// Accepted with a request (e.g. the chosen workspace).
    Accepted(Option<AppRequest>),
}

struct Inner {
    window: slint::Weak<ui::AppWindow>,
    dialog: Box<dyn FormDialog>,
    result: ModalResult,
    on_finished: Option<Box<dyn Fn()>>,
}

/// A form dialog shown without application (see the module docs).
#[derive(Clone)]
pub struct ModalDialog(Rc<RefCell<Inner>>);

impl ModalDialog {
    /// Shows `dialog` in `window`; `on_finished` is called (deferred) when
    /// it is accepted or canceled.
    pub fn show(
        window: &ui::AppWindow,
        dialog: Box<dyn FormDialog>,
        on_finished: impl Fn() + 'static,
    ) -> Self {
        let this = Self(Rc::new(RefCell::new(Inner {
            window: window.as_weak(),
            dialog,
            result: ModalResult::Open,
            on_finished: Some(Box::new(on_finished)),
        })));
        let weak = Rc::downgrade(&this.0);
        this.0
            .borrow()
            .dialog
            .form()
            .model()
            .set_handler(move |row, data| {
                let weak = weak.clone();
                defer(move || {
                    if let Some(inner) = weak.upgrade() {
                        Self(inner).row_written(row, &data);
                    }
                });
            });
        let weak = Rc::downgrade(&this.0);
        window
            .global::<ui::Dialogs>()
            .on_form_button(move |button| {
                let weak = weak.clone();
                defer(move || {
                    if let Some(inner) = weak.upgrade() {
                        Self(inner).button(button);
                    }
                });
            });
        clear_window(window);
        let d = window.global::<ui::Dialogs>();
        d.set_form_page(0);
        d.set_form_error(SharedString::new());
        d.set_form_busy(false);
        this.refresh();
        d.set_form_shown(true);
        this
    }

    /// The result so far.
    pub fn result(&self) -> ModalResult {
        self.0.borrow().result.clone()
    }

    /// Runs `f` with the dialog (tests).
    pub fn with_dialog<R>(&self, f: impl FnOnce(&mut dyn FormDialog) -> R) -> R {
        f(self.0.borrow_mut().dialog.as_mut())
    }

    fn refresh(&self) {
        let inner = self.0.borrow();
        let Some(w) = inner.window.upgrade() else {
            return;
        };
        let d = w.global::<ui::Dialogs>();
        let options = inner.dialog.options();
        let form = inner.dialog.form();
        d.set_form_title(inner.dialog.title().into());
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
        d.set_form_preview_shown(false);
    }

    fn set_error(&self, message: &str) {
        if let Some(w) = self.0.borrow().window.upgrade() {
            w.global::<ui::Dialogs>().set_form_error(message.into());
        }
    }

    fn row_written(&self, row: usize, data: &ui::FormField) {
        {
            let mut inner = self.0.borrow_mut();
            let Some((id, event)) = inner.dialog.form_mut().ui_written(row, data) else {
                return;
            };
            inner
                .dialog
                .field_event(&DialogContext::without_project(), &id, event);
        }
        self.refresh();
    }

    fn button(&self, button: i32) {
        let ctx = DialogContext::without_project();
        let result = match button {
            -2 => Some(ModalResult::Canceled),
            -1 | 0 => {
                let applied = self.0.borrow_mut().dialog.apply(&ctx);
                match applied {
                    Ok(Applied::App(request)) => Some(ModalResult::Accepted(Some(request))),
                    Ok(_) => Some(ModalResult::Accepted(None)),
                    Err(e) => {
                        self.refresh();
                        self.set_error(&e);
                        None
                    }
                }
            }
            n => {
                let r = self.0.borrow_mut().dialog.button(&ctx, (n - 1) as usize);
                match r {
                    Ok(ButtonResult::App(request)) => Some(ModalResult::Accepted(Some(request))),
                    Ok(ButtonResult::Close) => Some(ModalResult::Canceled),
                    Ok(_) => {
                        self.refresh();
                        self.set_error("");
                        None
                    }
                    Err(e) => {
                        self.refresh();
                        self.set_error(&e);
                        None
                    }
                }
            }
        };
        if let Some(result) = result {
            let on_finished = {
                let mut inner = self.0.borrow_mut();
                inner.result = result;
                if let Some(w) = inner.window.upgrade() {
                    let d = w.global::<ui::Dialogs>();
                    d.set_form_shown(false);
                    d.set_form_fields(vec_model(Vec::new()));
                }
                inner.on_finished.take()
            };
            if let Some(f) = on_finished {
                f();
            }
        }
    }
}
