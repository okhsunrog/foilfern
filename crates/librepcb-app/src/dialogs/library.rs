//! Dialogs of the library tab: the confirmation before removing library
//! elements (upstream: a `QMessageBox::warning()` of
//! `LibraryTab::deleteElements()`).

use librepcb_core::fileio::FilePath;
use librepcb_core::types::LengthUnit;
use librepcb_i18n::tr;

use super::{Applied, DialogContext, DialogOptions, Form, FormDialog, TabDialogResult};
use crate::tabs::organization::DesignRulesNamePurpose;

const LT: &str = "librepcb::editor::LibraryTab";

/// Asks whether library elements shall really be removed (upstream
/// `LibraryTab::deleteElements()`).
pub struct RemoveElementsDialog {
    form: Form,
    paths: Vec<FilePath>,
}

impl RemoveElementsDialog {
    /// A dialog for the elements (directory, name).
    pub fn new(elements: &[(FilePath, String)]) -> Self {
        let mut names: Vec<String> = elements.iter().map(|(_, n)| n.clone()).collect();
        super::sort_numeric(&mut names);
        let mut msg = tr!(
            LT,
            "WARNING: Library elements must normally NOT be removed because this will break other elements which depend on this one! They should be just marked as deprecated instead.\n\nAre you still sure to delete the following library elements?"
        );
        msg.push_str("\n\n");
        for name in names.iter().take(10) {
            msg.push_str(&format!(" - {name}\n"));
        }
        if names.len() > 10 {
            msg.push_str(" - ...\n");
        }
        msg.push('\n');
        msg.push_str(&tr!(LT, "This cannot be undone!"));
        let mut form = Form::new(LengthUnit::Millimeters);
        form.note("message", msg);
        Self {
            form,
            paths: elements.iter().map(|(p, _)| p.clone()).collect(),
        }
    }
}

impl FormDialog for RemoveElementsDialog {
    fn title(&self) -> String {
        tr!(LT, "Remove {0} Elements", self.paths.len())
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        DialogOptions {
            apply: false,
            ok_text: Some(tr!("QDialogButtonBox", "&Yes").replace('&', "")),
            width: 500.0,
            label_width: 0.0,
            ..DialogOptions::default()
        }
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        Ok(Applied::Tab(TabDialogResult::RemoveLibraryElements(
            self.paths.clone(),
        )))
    }
}

const RULES: &str = "librepcb::editor::OrganizationPcbDesignRulesModel";

/// The name of new, renamed or copied PCB design rules of an organization
/// (upstream `OrganizationPcbDesignRulesModel::askForName()`, a
/// `QInputDialog`).
pub struct DesignRulesNameDialog {
    form: Form,
    purpose: DesignRulesNamePurpose,
}

impl DesignRulesNameDialog {
    /// A dialog with the proposed name.
    pub fn new(purpose: DesignRulesNamePurpose, name: &str) -> Self {
        let mut form = Form::new(LengthUnit::Millimeters);
        form.text("name", tr!(RULES, "Name of the PCB design rules:"), name);
        Self { form, purpose }
    }
}

impl FormDialog for DesignRulesNameDialog {
    fn title(&self) -> String {
        tr!(RULES, "PCB Design Rules Name")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        DialogOptions {
            apply: false,
            width: 450.0,
            label_width: 200.0,
            ..DialogOptions::default()
        }
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let name = self.form.get_text("name");
        if name.trim().is_empty() {
            return Ok(Applied::Nothing);
        }
        Ok(Applied::Tab(TabDialogResult::DesignRulesName(
            self.purpose,
            name,
        )))
    }
}

/// The "Save Changes?" question before closing a library element tab
/// (upstream `QMessageBox::question()` of the tabs' `requestClose()`):
/// "Yes" saves and closes, "No" closes without saving, "Cancel" keeps the
/// tab open. Without `can_save`, "Yes" is not offered.
pub struct SaveChangesDialog {
    form: Form,
    title: String,
    can_save: bool,
}

impl SaveChangesDialog {
    /// A dialog with the question.
    pub fn new(title: String, text: &str, can_save: bool) -> Self {
        let mut form = Form::new(LengthUnit::Millimeters);
        form.note("message", text);
        Self {
            form,
            title,
            can_save,
        }
    }
}

impl FormDialog for SaveChangesDialog {
    fn title(&self) -> String {
        self.title.clone()
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        let yes = tr!("QDialogButtonBox", "&Yes").replace('&', "");
        let no = tr!("QDialogButtonBox", "&No").replace('&', "");
        DialogOptions {
            apply: false,
            ok_text: Some(if self.can_save { yes } else { no.clone() }),
            extra_buttons: if self.can_save { vec![no] } else { Vec::new() },
            width: 450.0,
            label_width: 0.0,
            ..DialogOptions::default()
        }
    }

    fn button(
        &mut self,
        _ctx: &DialogContext<'_>,
        _index: usize,
    ) -> Result<super::ButtonResult, String> {
        Ok(super::ButtonResult::TabResult(TabDialogResult::CloseTab {
            save: false,
        }))
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        Ok(Applied::Tab(TabDialogResult::CloseTab {
            save: self.can_save,
        }))
    }
}
