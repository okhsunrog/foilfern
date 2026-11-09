//! The prompts when opening a project: the directory lock handler dialog
//! and the question whether to restore an autosave backup.
//!
//! Port of libs/librepcb/editor/dialogs/directorylockhandlerdialog.{h,cpp,ui}
//! and of the `askForRestoringBackup` callback of
//! `GuiApplication::openProject()`. Upstream asks from within the
//! (blocking) opening of the file system; here opening stops with
//! [`OpenOutcome::Locked`](crate::project::OpenOutcome::Locked) or
//! [`OpenOutcome::AutosaveDetected`](crate::project::OpenOutcome::AutosaveDetected),
//! the prompt is shown, and its answer continues opening with an
//! [`OpenRequest`] containing the decision.

use librepcb_core::fileio::FilePath;
use librepcb_core::types::LengthUnit;
use librepcb_i18n::tr;

use super::{AppRequest, Applied, ButtonResult, DialogContext, DialogOptions, Form, FormDialog};
use crate::project::{LockDecision, OpenRequest};

const LOCK: &str = "librepcb::editor::DirectoryLockHandlerDialog";
const APP: &str = "librepcb::editor::GuiApplication";

/// Upstream `DirectoryLockHandlerDialog`: the directory is locked by
/// another application. "Open anyway" (only if the lock may be stale:
/// another user or an unknown application) needs "I accept the risk.";
/// in addition to upstream, the project can be opened read-only.
pub struct DirectoryLockDialog {
    form: Form,
    request: OpenRequest,
    can_override: bool,
}

impl DirectoryLockDialog {
    /// A dialog for the locked `directory` of the project to open.
    pub fn new(request: OpenRequest, directory: &FilePath, user: &str, can_override: bool) -> Self {
        let mut form = Form::new(LengthUnit::Millimeters);
        form.note(
            "description",
            tr!(
                LOCK,
                "Could not open the directory \"{0}\" because it is already opened by \"{1}\". Close any application accessing this directory and try again.",
                directory.to_native(),
                user
            ),
        );
        if can_override {
            form.note(
                "disclaimer",
                tr!(
                    LOCK,
                    "If you're absolutely sure that the directory is not accessed by any other application instance anymore, you could discard the current lock and open this directory anyway. But if the directory is still accessed by another application instance, this could lead in corrupt files, so use this option very carefully!"
                ),
            );
            form.checkbox("accept_risk", "", tr!(LOCK, "I accept the risk."), false);
        }
        Self {
            form,
            request,
            can_override,
        }
    }

    /// The request continued by the answers (tests).
    pub fn request(&self) -> &OpenRequest {
        &self.request
    }
}

impl FormDialog for DirectoryLockDialog {
    fn title(&self) -> String {
        tr!(LOCK, "Directory is locked")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        DialogOptions {
            apply: false,
            ok_text: Some(tr!(LOCK, "Open Read-Only")),
            extra_buttons: if self.can_override {
                vec![tr!(LOCK, "Open anyway")]
            } else {
                Vec::new()
            },
            width: 550.0,
            label_width: 0.0,
            ..DialogOptions::default()
        }
    }

    fn button(&mut self, _ctx: &DialogContext<'_>, _index: usize) -> Result<ButtonResult, String> {
        if !self.form.get_checked("accept_risk") {
            return Err(tr!(LOCK, "I accept the risk."));
        }
        Ok(ButtonResult::App(AppRequest::ContinueOpening(
            OpenRequest {
                lock: LockDecision::Override,
                ..self.request.clone()
            },
        )))
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        Ok(Applied::App(AppRequest::ContinueOpening(OpenRequest {
            lock: LockDecision::ReadOnly,
            ..self.request.clone()
        })))
    }
}

/// Upstream `askForRestoringBackup` of `GuiApplication::openProject()`:
/// "Yes" restores the autosave backup, "No" opens the last saved state,
/// "Cancel" does not open the project.
pub struct RestoreAutosaveDialog {
    form: Form,
    request: OpenRequest,
}

impl RestoreAutosaveDialog {
    /// A dialog for the project to open.
    pub fn new(request: OpenRequest) -> Self {
        let mut form = Form::new(LengthUnit::Millimeters);
        form.note(
            "question",
            tr!(
                APP,
                "It seems that the application crashed the last time you opened this project. Do you want to restore the last autosave backup?"
            ),
        );
        Self { form, request }
    }
}

impl FormDialog for RestoreAutosaveDialog {
    fn title(&self) -> String {
        tr!(APP, "Restore autosave backup?")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        DialogOptions {
            apply: false,
            ok_text: Some(tr!("QDialogButtonBox", "&Yes").replace('&', "")),
            extra_buttons: vec![tr!("QDialogButtonBox", "&No").replace('&', "")],
            width: 450.0,
            label_width: 0.0,
            ..DialogOptions::default()
        }
    }

    fn button(&mut self, _ctx: &DialogContext<'_>, _index: usize) -> Result<ButtonResult, String> {
        Ok(ButtonResult::App(AppRequest::ContinueOpening(
            OpenRequest {
                restore: Some(false),
                ..self.request.clone()
            },
        )))
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        Ok(Applied::App(AppRequest::ContinueOpening(OpenRequest {
            restore: Some(true),
            ..self.request.clone()
        })))
    }
}
