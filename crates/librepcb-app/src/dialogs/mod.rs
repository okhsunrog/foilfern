//! Dialogs of the project editors (milestone M3b).
//!
//! Upstream uses Qt Widgets dialogs (`*.ui` files) for the properties of
//! schematic and board items, the board and project setup, the output jobs
//! and more. Here they are Slint dialogs (`ui/dialogs/*.slint`):
//!
//! - Most dialogs are **form dialogs** ([`FormDialog`]): the dialog
//!   describes its fields in a [`form::Form`] with the labels of the
//!   upstream dialog (translated with the upstream Qt context, so the
//!   existing translations apply), the generic `FormDialog` of
//!   `ui/dialogs/formdialog.slint` renders them with upstream's widgets,
//!   and the dialog applies the values as **one undoable command group**
//!   when the user clicks "OK" or "Apply".
//! - The "add component" dialog has its own UI
//!   (`ui/dialogs/addcomponentdialog.slint`, [`add_component`]).
//!
//! The dialogs are UI-toolkit independent enough to be tested without a
//! window: open them from a tab request, edit fields with
//! [`form::Form::edit()`] and [`FormDialog::field_event()`], and apply.

/// Implements [`FormDialog::form()`] and [`FormDialog::form_mut()`] for a
/// dialog with a `form` field.
macro_rules! form_accessors {
    () => {
        fn form(&self) -> &Form {
            &self.form
        }

        fn form_mut(&mut self) -> &mut Form {
            &mut self.form
        }
    };
}

pub mod add_component;
pub mod attributes;
pub mod board;
pub mod chooser;
pub mod form;
pub mod geometry;
pub mod initialize_workspace;
pub mod library;
pub mod library_import;
pub mod library_items;
pub mod move_align;
pub mod new_project;
pub mod open_prompts;
pub mod output;
pub mod project_library_updater;
pub mod review;
pub mod schematic;
pub mod setup;
pub mod workspace_settings;

use std::rc::Rc;

use librepcb_core::types::LengthUnit;
use librepcb_editor::ProjectEditor;
use librepcb_i18n::tr;

use crate::mcp::SharedWorkspace;
use crate::project::AppProject;
use crate::tabs::PropertiesTarget;
pub use form::{FieldEvent, Form, ListAction, ListButtons, ListItem};

/// What a dialog works on.
pub struct DialogContext<'a> {
    /// The project (`None` for dialogs of the library editors).
    pub project: Option<&'a Rc<AppProject>>,
    /// The workspace (library database, settings), if available.
    pub workspace: Option<&'a SharedWorkspace>,
}

impl<'a> DialogContext<'a> {
    /// A context without workspace (tests, dialogs which do not need it).
    pub fn new(project: &'a Rc<AppProject>) -> Self {
        Self {
            project: Some(project),
            workspace: None,
        }
    }

    /// A context without project (library editor dialogs).
    pub fn without_project() -> Self {
        Self {
            project: None,
            workspace: None,
        }
    }

    /// The project; an error for dialogs opened without project.
    pub fn project(&self) -> Result<&'a Rc<AppProject>, String> {
        self.project.ok_or_else(|| "No project".to_owned())
    }
}

/// Opens the properties dialog requested by a tab; `None` if the item does
/// not exist (anymore).
pub fn open_properties(
    project: &AppProject,
    target: &PropertiesTarget,
    unit: LengthUnit,
) -> Option<Box<dyn FormDialog>> {
    Some(match target {
        PropertiesTarget::Symbol(symbol) => Box::new(schematic::SymbolPropertiesDialog::new(
            project, *symbol, unit,
        )?),
        PropertiesTarget::NetSegment(sch, segment) => Box::new(
            schematic::RenameSegmentDialog::for_net(project, *sch, *segment)?,
        ),
        PropertiesTarget::BusSegment(sch, segment) => Box::new(
            schematic::RenameSegmentDialog::for_bus(project, *sch, *segment)?,
        ),
        PropertiesTarget::SymbolText(symbol, uuid) => Box::new(
            schematic::SchematicTextDialog::for_symbol(project, *symbol, *uuid, unit)?,
        ),
        PropertiesTarget::SchematicPolygon(sch, uuid) => Box::new(
            schematic::SchematicPolygonDialog::new(project, *sch, *uuid, unit)?,
        ),
        PropertiesTarget::SchematicText(sch, uuid) => Box::new(
            schematic::SchematicTextDialog::new(project, *sch, *uuid, unit)?,
        ),
        PropertiesTarget::Board(board, item) => board::open(project, *board, *item, unit)?,
    })
}

/// The result of applying a dialog.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Applied {
    /// Nothing changed.
    #[default]
    Nothing,
    /// The project was modified (one undo group).
    Project,
    /// A value for the tab which opened the dialog (e.g. the line width
    /// for the FSM).
    Tab(TabDialogResult),
    /// Run output jobs (in a worker thread, reported in notifications).
    RunJobs {
        /// Title of the notifications.
        title: String,
        /// The jobs.
        jobs: Vec<librepcb_core::job::OutputJob>,
    },
    /// A request to the application (dialogs of the project lifecycle and
    /// the workspace).
    App(AppRequest),
}

/// Requests of application level dialogs (wizards, prompts, settings) to
/// the application.
#[derive(Debug, Clone, PartialEq)]
pub enum AppRequest {
    /// Open a project (e.g. created by the new project wizard) and show
    /// the messages of an import (if any).
    OpenProject {
        /// The project file.
        path: librepcb_core::fileio::FilePath,
        /// Messages of the EAGLE project import.
        import_messages: Vec<String>,
    },
    /// Continue opening a project with the answers of a prompt (directory
    /// lock, autosave restore).
    ContinueOpening(crate::project::OpenRequest),
    /// The workspace settings were modified (apply them to the UI).
    WorkspaceSettingsChanged,
    /// Show another dialog.
    ShowDialog(DialogKind),
    /// Rescan the workspace libraries.
    RescanLibraries,
    /// Update the library of a project (project library updater).
    UpdateProjectLibrary(librepcb_core::fileio::FilePath),
    /// A workspace was chosen (and initialized) in the initialize
    /// workspace wizard.
    WorkspaceChosen(librepcb_core::fileio::FilePath),
    /// Run the import of the open library import wizard in a worker
    /// thread.
    RunLibraryImport,
}

/// Dialogs which a dialog can open (see [`AppRequest::ShowDialog`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogKind {
    /// The color scheme editor of the workspace settings dialog (for its
    /// active scheme of the kind).
    ColorScheme(crate::color_schemes::SchemeKind),
}

/// Results of dialogs which answer a request of a tab's FSM.
#[derive(Debug, Clone, PartialEq)]
pub enum TabDialogResult {
    /// The line width of the "Set Width" dialog.
    LineWidth(librepcb_core::types::UnsignedLength),
    /// The new positions of the "Move/Align Elements" dialog.
    Positions(Vec<librepcb_core::types::Point>),
    /// The choices of the DXF import dialog.
    ImportDxf(librepcb_editor::fsm::board::DxfImportSettings),
    /// Remove these library elements (confirmed).
    RemoveLibraryElements(Vec<librepcb_core::fileio::FilePath>),
    /// A modified object of a library element (properties dialogs).
    LibraryObject(library_items::LibraryObject),
    /// The pin names of the "import pins" dialog.
    ImportPins(Vec<librepcb_core::types::CircuitIdentifier>),
    /// The excess of the "generate courtyard" dialog.
    CourtyardOffset(librepcb_core::types::PositiveLength),
    /// A library element chosen in a chooser dialog.
    ElementChosen(chooser::ChooserPurpose, librepcb_core::types::Uuid),
    /// The name of organization PCB design rules.
    DesignRulesName(crate::tabs::organization::DesignRulesNamePurpose, String),
    /// Close the tab, saving it first if `save`.
    CloseTab {
        /// Save before closing.
        save: bool,
    },
}

/// Buttons and size of a form dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct DialogOptions {
    /// Show an "Apply" button (upstream properties dialogs).
    pub apply: bool,
    /// Show a "Cancel" button.
    pub cancel: bool,
    /// Text of the OK button (default: "OK").
    pub ok_text: Option<String>,
    /// Additional buttons on the left (reported to
    /// [`FormDialog::button()`]).
    pub extra_buttons: Vec<String>,
    /// Width of the main column (logical pixels).
    pub width: f32,
    /// Width of the label column.
    pub label_width: f32,
    /// Width of the side column (if fields are in it).
    pub side_width: f32,
}

impl Default for DialogOptions {
    fn default() -> Self {
        Self {
            apply: true,
            cancel: true,
            ok_text: None,
            extra_buttons: Vec::new(),
            width: 550.0,
            label_width: 150.0,
            side_width: 250.0,
        }
    }
}

/// What a click on an extra button did.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ButtonResult {
    /// Keep the dialog open.
    #[default]
    Keep,
    /// The project was modified; keep the dialog open.
    Modified,
    /// Close the dialog.
    Close,
    /// Close the dialog with a result for the tab.
    TabResult(TabDialogResult),
    /// Run output jobs; keep the dialog open.
    RunJobs {
        /// Title of the notifications.
        title: String,
        /// The jobs.
        jobs: Vec<librepcb_core::job::OutputJob>,
    },
    /// Close the dialog with a request to the application.
    App(AppRequest),
}

/// A dialog described by a form.
pub trait FormDialog {
    /// The (translated) title.
    fn title(&self) -> String;

    /// The fields.
    fn form(&self) -> &Form;

    /// The fields (mutable).
    fn form_mut(&mut self) -> &mut Form;

    /// Buttons and size.
    fn options(&self) -> DialogOptions {
        DialogOptions::default()
    }

    /// A field was edited or a list/button action happened (after the
    /// form applied step requests).
    fn field_event(&mut self, _ctx: &DialogContext<'_>, _id: &str, _event: FieldEvent) {}

    /// Applies the values ("OK" and "Apply"); `Err` with a (translated)
    /// message keeps the dialog open and shows the message.
    fn apply(&mut self, ctx: &DialogContext<'_>) -> Result<Applied, String>;

    /// An extra button was clicked (index into
    /// [`DialogOptions::extra_buttons`]).
    fn button(&mut self, _ctx: &DialogContext<'_>, _index: usize) -> Result<ButtonResult, String> {
        Ok(ButtonResult::Keep)
    }

    /// A request to the application after a field event or a button (e.g.
    /// to open another dialog), polled by the application.
    fn take_request(&mut self) -> Option<AppRequest> {
        None
    }

    /// The dialog is closed (e.g. canceled); stop running operations.
    fn closing(&mut self) {}

    /// The dialog as `Any` (to access its type from the application, e.g.
    /// the workspace settings dialog from the color scheme editor).
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        None
    }

    /// The preview image shown next to the fields (if any).
    fn preview(&mut self, _ctx: &DialogContext<'_>) -> Option<(slint::Image, String)> {
        None
    }
}

/// Runs `f` on the project's editor as one undo group labeled `text`
/// (upstream `UndoStackTransaction`): commits it if `f` succeeds and
/// modified something, aborts it (rolling back) otherwise.
pub fn transaction<R>(
    project: &AppProject,
    text: impl Into<String>,
    f: impl FnOnce(&mut ProjectEditor) -> librepcb_editor::Result<R>,
) -> Result<R, String> {
    let mut p = project.shared().lock();
    if p.editor.undo_stack().is_group_active() {
        return Err(tr!(
            "librepcb::editor::UndoStack",
            "Another command is active at the moment. Please finish that command to continue."
        ));
    }
    let editor = &mut p.editor;
    editor.begin_group(text).map_err(|e| e.to_string())?;
    match f(editor) {
        Ok(r) => {
            editor.commit_group().map_err(|e| e.to_string())?;
            Ok(r)
        }
        Err(e) => {
            if let Err(e2) = editor.abort_group() {
                log::error!("Failed to abort the undo group: {e2}");
            }
            Err(e.to_string())
        }
    }
}

/// The length unit for dialogs opened without a tab (the first board's
/// grid unit, like upstream's default).
pub fn default_unit(project: &AppProject) -> LengthUnit {
    let p = project.shared().lock();
    p.project()
        .boards()
        .first()
        .map_or(LengthUnit::Millimeters, |b| b.settings().grid_unit)
}

/// Upstream `Toolbox::sortNumeric()` for names (natural order,
/// case-insensitive).
pub fn sort_numeric(names: &mut [String]) {
    names.sort_by(|a, b| natural_cmp(&a.to_lowercase(), &b.to_lowercase()));
}

/// Compares strings with embedded numbers in natural order.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = a.peek().copied().filter(char::is_ascii_digit) {
                    na.push(c);
                    a.next();
                }
                let mut nb = String::new();
                while let Some(c) = b.peek().copied().filter(char::is_ascii_digit) {
                    nb.push(c);
                    b.next();
                }
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["R10".to_owned(), "r2".to_owned(), "R1".to_owned()];
        sort_numeric(&mut v);
        assert_eq!(v, vec!["R1", "r2", "R10"]);
    }
}
