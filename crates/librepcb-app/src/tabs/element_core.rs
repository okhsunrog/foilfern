//! What all library element editor tabs share: the element editor (undo
//! stack, dirty state, interface check), the metadata, the checks with
//! approvals and automatic fixes, the wizard mode of new elements and
//! saving into the library.
//!
//! Port of the common parts of upstream `LibraryEditorTab` and the element
//! tabs (`SymbolTab`, `PackageTab`, `ComponentTab`, `DeviceTab`):
//! `commitUiData()`, `save()`, `runChecks()`, `autoFix()`,
//! `messageApprovalChanged()`, the "interface broken" and "element
//! duplicated" messages.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use librepcb_app_ui as ui;
use librepcb_core::library::cmp::{Component, ComponentSymbolVariant};
use librepcb_core::library::dev::Device;
use librepcb_core::library::pkg::{AssemblyType, Footprint, Package};
use librepcb_core::library::sym::Symbol;
use librepcb_core::library::{BaseMetadata, LibraryCheckMessage};
use librepcb_core::types::{ElementName, Uuid, Version};
use librepcb_core::workspace::{ElementKind, LibraryDb};
use librepcb_editor::library_editor::checks::{FixOutcome, FixParams, FixRequest};
use librepcb_editor::library_editor::{EditableElement, LibraryElementEditor};
use librepcb_i18n::tr;

use super::element_metadata::ElementMetadataUi;
use super::{TabRequest, TabUpdate, editing::error_notification, feature};
use crate::open_library::OpenLibrary;
use crate::rule_check::{CheckMessage, RowAction, RuleCheckMessages};

/// The element kind specific functions used by [`ElementCore`].
pub trait ElementKindInfo: EditableElement + Sized {
    /// The element kind in the library database.
    const KIND: ElementKind;
    /// The kind of the categories.
    const CATEGORY_KIND: ElementKind;
    /// The tab type.
    const TAB_TYPE: ui::TabType;
    /// The check type.
    const CHECK_TYPE: ui::RuleCheckType;

    /// A new, empty element.
    fn new_element(metadata: BaseMetadata) -> librepcb_core::library::Result<Self>;
    /// Copies the content of `other` except its UUID (upstream
    /// `duplicateFrom()`).
    fn duplicate_from(&mut self, other: &Self) -> librepcb_core::library::Result<()>;
    /// Whether a message has an automatic fix.
    fn can_auto_fix(msg: &LibraryCheckMessage) -> bool;
    /// Applies the fix of a message.
    fn auto_fix(
        editor: &mut LibraryElementEditor<Self>,
        msg: &LibraryCheckMessage,
        params: &FixParams,
    ) -> librepcb_editor::Result<FixOutcome>;
    /// The "save changes?" text (upstream per tab).
    fn save_title() -> String;
    /// The page of the editor (after the wizard pages).
    const EDITOR_PAGE: i32;
    /// Whether the editor page is still part of the wizard (components:
    /// adding gates creates signals).
    const WIZARD_ON_EDITOR_PAGE: bool = false;
    /// The name of new elements (upstream `MainWindow::openNew*Tab()`, not
    /// translated).
    const NEW_NAME: &'static str;
}

macro_rules! kind_info {
    ($ty:ty, $kind:ident, $cat:ident, $tab:ident, $check:ident, $page:expr, $save_title:expr, $name:literal, $new:expr) => {
        impl ElementKindInfo for $ty {
            const EDITOR_PAGE: i32 = $page;
            const WIZARD_ON_EDITOR_PAGE: bool =
                matches!(ElementKind::$kind, ElementKind::Component);
            const NEW_NAME: &'static str = $name;
            const KIND: ElementKind = ElementKind::$kind;
            const CATEGORY_KIND: ElementKind = ElementKind::$cat;
            const TAB_TYPE: ui::TabType = ui::TabType::$tab;
            const CHECK_TYPE: ui::RuleCheckType = ui::RuleCheckType::$check;

            fn new_element(metadata: BaseMetadata) -> librepcb_core::library::Result<Self> {
                #[allow(clippy::redundant_closure_call)]
                ($new)(metadata)
            }

            fn duplicate_from(&mut self, other: &Self) -> librepcb_core::library::Result<()> {
                <$ty>::duplicate_from(self, other)
            }

            fn can_auto_fix(msg: &LibraryCheckMessage) -> bool {
                LibraryElementEditor::<$ty>::can_auto_fix(msg)
            }

            fn auto_fix(
                editor: &mut LibraryElementEditor<Self>,
                msg: &LibraryCheckMessage,
                params: &FixParams,
            ) -> librepcb_editor::Result<FixOutcome> {
                editor.auto_fix(msg, params)
            }

            fn save_title() -> String {
                #[allow(clippy::redundant_closure_call)]
                ($save_title)()
            }
        }
    };
}

kind_info!(
    Symbol,
    Symbol,
    ComponentCategory,
    Symbol,
    SymbolCheck,
    1,
    || tr!("librepcb::editor::SymbolTab", "Save Changes?"),
    "New Symbol",
    Symbol::new
);
kind_info!(
    Package,
    Package,
    PackageCategory,
    Package,
    PackageCheck,
    2,
    || tr!("librepcb::editor::PackageTab", "Save Changes?"),
    "New Package",
    |m| {
        let mut p = Package::new(m, AssemblyType::Auto)?;
        p.footprints_mut().push(Footprint::new(
            Uuid::new_random(),
            default_name(),
            String::new(),
        ));
        Ok(p)
    }
);
kind_info!(
    Component,
    Component,
    ComponentCategory,
    Component,
    ComponentCheck,
    2,
    || tr!("librepcb::editor::ComponentTab", "Save Changes?"),
    "New Component",
    |m| {
        let mut c = Component::new(m)?;
        c.symbol_variants_mut().push(ComponentSymbolVariant::new(
            Uuid::new_random(),
            "",
            default_name(),
            "",
        ));
        Ok(c)
    }
);
kind_info!(
    Device,
    Device,
    ComponentCategory,
    Device,
    DeviceCheck,
    1,
    || tr!("librepcb::editor::DeviceTab", "Save Changes?"),
    "New Device",
    |m| Device::new(m, Uuid::new_random(), Uuid::new_random())
);

/// The name "default" of the first footprint or symbol variant of new
/// packages and components.
fn default_name() -> ElementName {
    // A valid constant name.
    ElementName::new("default").expect("valid element name")
}

/// How an element tab was opened (upstream `Mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    /// An existing element.
    Open,
    /// A new element.
    New,
    /// A copy of an existing element.
    Duplicate,
}

/// The shared state of an element tab.
pub struct ElementCore<E: ElementKindInfo> {
    /// The library of the element.
    pub library: Rc<OpenLibrary>,
    /// The element editor.
    pub editor: LibraryElementEditor<E>,
    /// The metadata as edited in the UI.
    pub meta: ElementMetadataUi,
    /// The check messages.
    pub checks: Rc<RefCell<RuleCheckMessages>>,
    /// Error of the last check run.
    pub check_error: String,
    check_messages: Vec<LibraryCheckMessage>,
    checked_state: Option<u64>,
    /// Wizard mode (new elements: metadata first).
    pub wizard_mode: bool,
    /// The page of the tab.
    pub page_index: i32,
    /// The element was just duplicated (message).
    pub element_duplicated: bool,
    /// The user allowed breaking the interface (upstream "unlock").
    pub allow_breaking_changes: bool,
    /// Opened as a new (or duplicated) element: its interface can't be
    /// broken while the tab is open (upstream `mIsNewElement`).
    is_new: bool,
    /// How the tab was opened.
    pub mode: OpenMode,
    user_name: String,
    /// The database (for new elements' categories etc.).
    pub db: Arc<LibraryDb>,
}

impl<E: ElementKindInfo> ElementCore<E> {
    /// Opens the element in `dir` (relative to the library) or creates a
    /// new one (`OpenMode::New`) or a copy of the element in `dir`
    /// (`OpenMode::Duplicate`).
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        library: Rc<OpenLibrary>,
        relative_dir: Option<&str>,
        mode: OpenMode,
        db: Arc<LibraryDb>,
        locales: Vec<String>,
        user_name: String,
        on_check_row: impl Fn(usize, ui::RuleCheckMessageData) + 'static,
        on_category_row: impl Fn(usize, ui::LibraryElementCategoryData) + 'static,
    ) -> Result<Self, String> {
        let editor = match (mode, relative_dir) {
            (OpenMode::Open, Some(dir)) => {
                LibraryElementEditor::open(library.directory(dir)).map_err(|e| e.to_string())?
            }
            (mode, dir) => {
                let name = ElementName::new(E::NEW_NAME).map_err(|e| e.to_string())?;
                let metadata = BaseMetadata::new(
                    Uuid::new_random(),
                    "0.1".parse::<Version>().map_err(|e| e.to_string())?,
                    user_name.clone(),
                    chrono::Utc::now(),
                    name,
                    "",
                    "",
                );
                let mut element = E::new_element(metadata).map_err(|e| e.to_string())?;
                if mode == OpenMode::Duplicate
                    && let Some(dir) = dir
                {
                    let src = E::open(library.directory(dir)).map_err(|e| e.to_string())?;
                    element.duplicate_from(&src).map_err(|e| e.to_string())?;
                }
                LibraryElementEditor::new_element(element)
            }
        };
        let meta = ElementMetadataUi::new(db.clone(), locales, E::CATEGORY_KIND, on_category_row);
        let mut core = Self {
            library,
            editor,
            meta,
            checks: Rc::new(RefCell::new(RuleCheckMessages::new(on_check_row))),
            check_error: String::new(),
            check_messages: Vec::new(),
            checked_state: None,
            wizard_mode: mode != OpenMode::Open,
            page_index: if mode == OpenMode::Open {
                E::EDITOR_PAGE
            } else {
                0
            },
            element_duplicated: false,
            allow_breaking_changes: false,
            is_new: mode != OpenMode::Open,
            mode,
            user_name,
            db,
        };
        core.refresh();
        if mode == OpenMode::New {
            core.meta.clear_name();
        }
        core.run_checks_if_modified();
        Ok(core)
    }

    /// The element directory (in the library once saved).
    pub fn directory_path(&self) -> librepcb_core::fileio::FilePath {
        self.library.path().path_to(&self.relative_dir())
    }

    /// The element directory relative to the library.
    pub fn relative_dir(&self) -> String {
        format!(
            "{}/{}",
            E::SHORT_ELEMENT_NAME,
            self.editor.element().metadata().uuid()
        )
    }

    /// Whether the element can be modified (upstream `fsmIsWritable()`).
    pub fn is_writable(&self) -> bool {
        self.is_new
            || (self.library.is_writable()
                && (!self.is_interface_broken() || self.allow_breaking_changes))
    }

    /// Whether the interface was broken (the "not backward-compatible"
    /// message; never for new elements).
    pub fn is_interface_broken(&self) -> bool {
        !self.is_new && self.editor.is_interface_broken()
    }

    /// Updates the metadata UI data from the element.
    pub fn refresh(&mut self) {
        self.meta.refresh(self.editor.element());
    }

    /// The base `TabData` (features of the tab kind are added by the tab).
    pub fn tab_data(&self) -> ui::TabData {
        let writable = self.is_writable();
        ui::TabData {
            r#type: E::TAB_TYPE,
            title: self.editor.element().metadata().name().to_string().into(),
            features: ui::TabFeatures {
                save: feature(writable),
                undo: feature(self.editor.can_undo()),
                redo: feature(self.editor.can_redo()),
                ..Default::default()
            },
            read_only: !writable,
            unsaved_changes: self.editor.is_dirty(),
            undo_text: self
                .editor
                .undo_stack()
                .undo_text()
                .unwrap_or_default()
                .into(),
            redo_text: self
                .editor
                .undo_stack()
                .redo_text()
                .unwrap_or_default()
                .into(),
            ..Default::default()
        }
    }

    /// The `RuleCheckData`.
    pub fn checks_data(&self) -> ui::RuleCheckData {
        let checks = self.checks.borrow();
        ui::RuleCheckData {
            r#type: E::CHECK_TYPE,
            state: ui::RuleCheckState::UpToDate,
            messages: crate::models::model_rc(checks.model()),
            unapproved: checks.unapproved() as i32,
            errors: checks.errors() as i32,
            execution_error: self.check_error.as_str().into(),
            read_only: !self.is_writable(),
        }
    }

    /// Applies the edited metadata (upstream `commitUiData()`).
    pub fn commit_metadata(&mut self) -> Option<TabRequest> {
        if self.editor.is_group_active() {
            return None;
        }
        let cmd = self.meta.command(self.editor.element());
        let result = self.editor.execute(cmd);
        self.refresh();
        self.run_checks_if_modified();
        result.err().map(|e| error_notification(e.to_string()))
    }

    /// Runs the checks if the element changed since the last run.
    pub fn run_checks_if_modified(&mut self) {
        // Upstream schedules the checks only after the wizard.
        if self.wizard_mode {
            return;
        }
        let state = self.editor.state_id();
        if self.checked_state == Some(state) {
            return;
        }
        self.checked_state = Some(state);
        match self.editor.run_checks() {
            Ok(messages) => {
                self.check_error.clear();
                let approvals = self.editor.element().metadata().message_approvals().clone();
                let entries = messages
                    .iter()
                    .map(|m| CheckMessage {
                        message: m.to_message(),
                        schematic: None,
                        autofix: E::can_auto_fix(m),
                    })
                    .collect();
                self.checks.borrow_mut().set_messages(entries, approvals);
                self.check_messages = messages;
            }
            Err(e) => self.check_error = e.to_string(),
        }
    }

    /// Forces the next check run.
    pub fn invalidate_checks(&mut self) {
        self.checked_state = None;
    }

    /// A check message row was written by the UI (approval, autofix);
    /// returns the fix request (if any) and an update.
    pub fn check_row_written(
        &mut self,
        row: usize,
        data: &ui::RuleCheckMessageData,
    ) -> (Option<FixRequest>, TabUpdate) {
        let action = self.checks.borrow_mut().row_written(row, data);
        let mut update = TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        };
        match action {
            RowAction::Approve { approval, approved } => {
                self.editor.set_message_approved(&approval, approved);
                self.invalidate_checks();
                self.run_checks_if_modified();
                (None, update)
            }
            RowAction::Autofix { index } => {
                let msg = {
                    let checks = self.checks.borrow();
                    checks.entries().get(index).and_then(|e| {
                        self.check_messages
                            .iter()
                            .find(|m| m.to_message().approval() == e.message.approval())
                            .cloned()
                    })
                };
                let Some(msg) = msg else {
                    return (None, update);
                };
                let params = FixParams {
                    author: Some(self.user_name.clone()),
                    ..FixParams::default()
                };
                let result = E::auto_fix(&mut self.editor, &msg, &params);
                self.refresh();
                self.run_checks_if_modified();
                match result {
                    Ok(FixOutcome::Request(r)) => (Some(r), update),
                    Ok(_) => (None, update),
                    Err(e) => {
                        update.requests.push(error_notification(e.to_string()));
                        (None, update)
                    }
                }
            }
            _ => (None, update),
        }
    }

    /// Saves the element (into the library if it is new; upstream
    /// `save()`).
    pub fn save(&mut self) -> Result<(), String> {
        if self.editor.is_new_element() {
            let mut dir = self.library.directory(&self.relative_dir());
            self.editor.save_to(&mut dir).map_err(|e| e.to_string())?;
        } else {
            self.editor.save().map_err(|e| e.to_string())?;
        }
        // Upstream: saving the first wizard page goes to the next page
        // (not for devices, whose first page chooses component and package).
        if self.wizard_mode && self.page_index == 0 && E::KIND != ElementKind::Device {
            self.page_index += 1;
            if E::EDITOR_PAGE == 1 {
                self.wizard_mode = false;
            }
        }
        self.invalidate_checks();
        self.refresh();
        self.run_checks_if_modified();
        Ok(())
    }

    /// Undo (`true`) or redo.
    pub fn undo_redo(&mut self, undo: bool) -> Option<TabRequest> {
        let result = if undo {
            self.editor.undo()
        } else {
            self.editor.redo()
        };
        self.refresh();
        self.run_checks_if_modified();
        result.err().map(|e| error_notification(e.to_string()))
    }

    /// Handles the actions shared by all element tabs (next, apply, save,
    /// undo, redo, unlock); returns `None` for other actions.
    pub fn trigger_common(&mut self, action: ui::TabAction) -> Option<TabUpdate> {
        use ui::TabAction as A;
        let mut update = TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        };
        match action {
            A::Next if E::EDITOR_PAGE == 1 => {
                // Symbols: the metadata page only.
                if self.wizard_mode {
                    self.wizard_mode = false;
                    self.page_index = 1;
                    self.invalidate_checks();
                    self.run_checks_if_modified();
                }
            }
            A::Next => {
                // Packages and components: every page is saved (upstream
                // `PackageTab`/`ComponentTab::trigger(Next)`).
                update.requests.extend(self.commit_metadata());
                if self.wizard_mode {
                    if self.page_index + 1 < E::EDITOR_PAGE
                        || (E::WIZARD_ON_EDITOR_PAGE && self.page_index + 1 == E::EDITOR_PAGE)
                    {
                        self.page_index += 1;
                    } else {
                        self.wizard_mode = false;
                        self.page_index = E::EDITOR_PAGE;
                        self.invalidate_checks();
                    }
                    match self.save() {
                        Ok(()) => update.requests.push(TabRequest::RescanLibraries),
                        Err(e) => update.requests.push(error_notification(e)),
                    }
                }
            }
            A::Apply => {
                update.requests.extend(self.commit_metadata());
            }
            A::Save => {
                update.requests.extend(self.commit_metadata());
                match self.save() {
                    Ok(()) => update.requests.push(TabRequest::RescanLibraries),
                    Err(e) => update.requests.push(error_notification(e)),
                }
            }
            A::Undo | A::Redo => {
                update.requests.extend(self.commit_metadata());
                update.requests.extend(self.undo_redo(action == A::Undo));
            }
            A::Unlock => self.allow_breaking_changes = true,
            _ => return None,
        }
        Some(update)
    }
}
