//! What the tabs of library elements without element metadata share
//! (component and package categories, organizations): the element with a
//! snapshot undo stack over its metadata and the kind specific content,
//! the metadata as edited in the UI, the checks with approvals and the
//! automatic fixes of the base element check, and saving into the library.
//!
//! Port of the common parts of upstream `ComponentCategoryTab`,
//! `PackageCategoryTab` and `OrganizationTab` (`refreshUiData()`,
//! `commitUiData()`, `save()`, `autoFix()`, `messageApprovalChanged()`).
//! The [`LibraryElementEditor`](librepcb_editor::library_editor) of the
//! other element kinds needs [`LibraryElement`](librepcb_core::library::LibraryElement)s;
//! these elements have [`BaseMetadata`] only.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use librepcb_app_ui as ui;
use librepcb_core::library::cat::{ComponentCategory, PackageCategory};
use librepcb_core::library::org::Organization;
use librepcb_core::library::{
    BaseMetadata, LibraryBaseElement, LibraryBaseElementCheckMessage, LibraryCheckMessage,
    title_case_fixed_name,
};
use librepcb_core::types::{ElementName, Uuid, Version};
use librepcb_core::workspace::{ElementKind, LibraryDb};
use librepcb_editor::library_editor::ElementUndoStack;
use librepcb_i18n::tr;

use super::element_core::OpenMode;
use super::{TabRequest, TabUpdate, editing::error_notification, feature};
use crate::open_library::OpenLibrary;
use crate::rule_check::{CheckMessage, RowAction, RuleCheckMessages};
use crate::validation;

/// The kind specific part of an element with base metadata only.
pub trait BaseElementKind: LibraryBaseElement + std::fmt::Debug {
    /// Everything besides the metadata which is edited through the undo
    /// stack.
    type Extra: Clone + PartialEq + std::fmt::Debug;
    /// The element kind in the library database.
    const KIND: ElementKind;
    /// The tab type.
    const TAB_TYPE: ui::TabType;
    /// The check type.
    const CHECK_TYPE: ui::RuleCheckType;
    /// The name of new elements (upstream `MainWindow::openNew*Tab()`).
    const NEW_NAME: &'static str;

    /// A new element.
    fn new_element(metadata: BaseMetadata) -> librepcb_core::library::Result<Self>;
    /// Copies the content of `other` except its UUID.
    fn duplicate_from(&mut self, other: &Self) -> librepcb_core::library::Result<()>;
    /// The kind specific content.
    fn extra(&self) -> Self::Extra;
    /// Replaces the kind specific content.
    fn set_extra(&mut self, extra: Self::Extra);
}

impl BaseElementKind for ComponentCategory {
    type Extra = Option<Uuid>;
    const KIND: ElementKind = ElementKind::ComponentCategory;
    const TAB_TYPE: ui::TabType = ui::TabType::ComponentCategory;
    const CHECK_TYPE: ui::RuleCheckType = ui::RuleCheckType::ComponentCategoryCheck;
    const NEW_NAME: &'static str = "New Component Category";

    fn new_element(metadata: BaseMetadata) -> librepcb_core::library::Result<Self> {
        Self::new(metadata)
    }
    fn duplicate_from(&mut self, other: &Self) -> librepcb_core::library::Result<()> {
        ComponentCategory::duplicate_from(self, other)
    }
    fn extra(&self) -> Option<Uuid> {
        self.parent_uuid()
    }
    fn set_extra(&mut self, extra: Option<Uuid>) {
        self.set_parent_uuid(extra);
    }
}

impl BaseElementKind for PackageCategory {
    type Extra = Option<Uuid>;
    const KIND: ElementKind = ElementKind::PackageCategory;
    const TAB_TYPE: ui::TabType = ui::TabType::PackageCategory;
    const CHECK_TYPE: ui::RuleCheckType = ui::RuleCheckType::PackageCategoryCheck;
    const NEW_NAME: &'static str = "New Package Category";

    fn new_element(metadata: BaseMetadata) -> librepcb_core::library::Result<Self> {
        Self::new(metadata)
    }
    fn duplicate_from(&mut self, other: &Self) -> librepcb_core::library::Result<()> {
        PackageCategory::duplicate_from(self, other)
    }
    fn extra(&self) -> Option<Uuid> {
        self.parent_uuid()
    }
    fn set_extra(&mut self, extra: Option<Uuid>) {
        self.set_parent_uuid(extra);
    }
}

/// The editable content of an organization (besides the metadata).
#[derive(Debug, Clone, PartialEq)]
pub struct OrganizationContent {
    /// Logo (PNG).
    pub logo: Vec<u8>,
    /// URL.
    pub url: String,
    /// Priority.
    pub priority: i32,
    /// PCB design rules.
    pub pcb_design_rules: Vec<librepcb_core::library::org::OrganizationPcbDesignRules>,
}

impl BaseElementKind for Organization {
    type Extra = OrganizationContent;
    const KIND: ElementKind = ElementKind::Organization;
    const TAB_TYPE: ui::TabType = ui::TabType::Organization;
    const CHECK_TYPE: ui::RuleCheckType = ui::RuleCheckType::OrganizationCheck;
    const NEW_NAME: &'static str = "New Organization";

    fn new_element(metadata: BaseMetadata) -> librepcb_core::library::Result<Self> {
        Self::new(metadata)
    }
    fn duplicate_from(&mut self, other: &Self) -> librepcb_core::library::Result<()> {
        Organization::duplicate_from(self, other)
    }
    fn extra(&self) -> OrganizationContent {
        OrganizationContent {
            logo: self.logo_png().clone(),
            url: self.url().clone(),
            priority: self.priority(),
            pcb_design_rules: self.pcb_design_rules().clone(),
        }
    }
    fn set_extra(&mut self, extra: OrganizationContent) {
        self.set_logo_png(extra.logo);
        self.set_url(extra.url);
        self.set_priority(extra.priority);
        self.set_pcb_design_rules(extra.pcb_design_rules);
    }
}

/// The snapshot of the undo stack.
#[derive(Debug, Clone, PartialEq)]
pub struct BaseContent<X> {
    /// Metadata (the message approvals are ignored on restore).
    pub metadata: BaseMetadata,
    /// The kind specific content.
    pub extra: X,
}

/// The shared state of a base element tab.
pub struct BaseElementCore<E: BaseElementKind> {
    /// The library of the element.
    pub library: Rc<OpenLibrary>,
    element: E,
    undo: ElementUndoStack<BaseContent<E::Extra>>,
    manual_modifications: bool,
    is_new: bool,
    // Metadata as edited in the UI.
    /// Name.
    pub name: String,
    /// Name error.
    pub name_error: String,
    name_parsed: Option<ElementName>,
    /// Description.
    pub description: String,
    /// Keywords.
    pub keywords: String,
    /// Author.
    pub author: String,
    /// Version.
    pub version: String,
    /// Version error.
    pub version_error: String,
    version_parsed: Option<Version>,
    /// Deprecated.
    pub deprecated: bool,
    // Checks.
    /// The check messages.
    pub checks: Rc<RefCell<RuleCheckMessages>>,
    check_error: String,
    check_messages: Vec<LibraryCheckMessage>,
    checked_state: Option<u64>,
    user_name: String,
    /// The workspace library database.
    pub db: Arc<LibraryDb>,
    /// The library locale order.
    pub locales: Vec<String>,
}

impl<E: BaseElementKind> BaseElementCore<E> {
    /// Opens the element in `dir` (relative to the library) or creates a
    /// new one or a copy.
    pub fn open(
        library: Rc<OpenLibrary>,
        relative_dir: Option<&str>,
        mode: OpenMode,
        db: Arc<LibraryDb>,
        locales: Vec<String>,
        user_name: String,
        on_check_row: impl Fn(usize, ui::RuleCheckMessageData) + 'static,
    ) -> Result<Self, String> {
        let element = match (mode, relative_dir) {
            (OpenMode::Open, Some(dir)) => {
                E::open(library.directory(dir)).map_err(|e| e.to_string())?
            }
            (mode, dir) => {
                let metadata = BaseMetadata::new(
                    Uuid::new_random(),
                    "0.1".parse::<Version>().map_err(|e| e.to_string())?,
                    user_name.clone(),
                    chrono::Utc::now(),
                    ElementName::new(E::NEW_NAME).map_err(|e| e.to_string())?,
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
                element
            }
        };
        let mut core = Self {
            library,
            element,
            undo: ElementUndoStack::new(),
            manual_modifications: mode != OpenMode::Open,
            is_new: mode != OpenMode::Open,
            name: String::new(),
            name_error: String::new(),
            name_parsed: None,
            description: String::new(),
            keywords: String::new(),
            author: String::new(),
            version: String::new(),
            version_error: String::new(),
            version_parsed: None,
            deprecated: false,
            checks: Rc::new(RefCell::new(RuleCheckMessages::new(on_check_row))),
            check_error: String::new(),
            check_messages: Vec::new(),
            checked_state: None,
            user_name,
            db,
            locales,
        };
        core.refresh();
        if mode == OpenMode::New {
            // The user can just start typing.
            core.name.clear();
            validation::element_name(&core.name, &mut core.name_error);
            core.name_parsed = None;
        }
        core.run_checks_if_modified();
        Ok(core)
    }

    /// The element.
    pub fn element(&self) -> &E {
        &self.element
    }

    /// The element directory relative to the library.
    pub fn relative_dir(&self) -> String {
        format!(
            "{}/{}",
            E::SHORT_ELEMENT_NAME,
            self.element.metadata().uuid()
        )
    }

    /// The element directory (in the library once saved).
    pub fn directory_path(&self) -> librepcb_core::fileio::FilePath {
        self.library.path().path_to(&self.relative_dir())
    }

    /// Whether the element can be modified.
    pub fn is_writable(&self) -> bool {
        self.is_new || self.library.is_writable()
    }

    /// Whether there are unsaved changes.
    pub fn is_dirty(&self) -> bool {
        !self.undo.is_clean() || self.manual_modifications
    }

    /// Increased with every modification.
    pub fn state_id(&self) -> u64 {
        self.undo.state_id()
    }

    fn content(&self) -> BaseContent<E::Extra> {
        BaseContent {
            metadata: self.element.metadata().clone(),
            extra: self.element.extra(),
        }
    }

    fn restore(&mut self, mut content: BaseContent<E::Extra>) {
        let approvals = self.element.metadata().message_approvals().clone();
        content.metadata.set_message_approvals(approvals);
        *self.element.metadata_mut() = content.metadata;
        self.element.set_extra(content.extra);
    }

    /// Modifies the content as one undo step (upstream
    /// `CmdLibraryCategoryEdit`, `CmdOrganizationEdit`); returns whether
    /// something changed.
    pub fn edit(
        &mut self,
        text: &str,
        f: impl FnOnce(&mut BaseContent<E::Extra>),
    ) -> Result<bool, String> {
        if !self.is_writable() {
            return Err(tr!(
                "librepcb::editor::LibraryEditorTab",
                "The library element is read-only."
            ));
        }
        let before = self.content();
        let mut after = before.clone();
        f(&mut after);
        if after == before {
            return Ok(false);
        }
        self.undo
            .begin_group(text, before)
            .map_err(|e| e.to_string())?;
        self.restore(after.clone());
        self.undo.commit_group(after).map_err(|e| e.to_string())?;
        Ok(true)
    }

    /// Updates the metadata UI data from the element.
    pub fn refresh(&mut self) {
        let m = self.element.metadata();
        self.name = m.name().to_string();
        self.name_error.clear();
        self.name_parsed = Some(m.name().clone());
        self.description = m.descriptions().default_value().clone();
        self.keywords = m.keywords().default_value().clone();
        self.author = m.author().clone();
        self.version = m.version().to_string();
        self.version_error.clear();
        self.version_parsed = Some(m.version().clone());
        self.deprecated = m.is_deprecated();
    }

    /// Applies the metadata inputs of the UI.
    #[allow(clippy::too_many_arguments)]
    pub fn set_metadata_from_ui(
        &mut self,
        name: &str,
        description: &str,
        keywords: &str,
        author: &str,
        version: &str,
        deprecated: bool,
    ) {
        self.name = name.to_owned();
        if let Some(n) = validation::element_name(&self.name, &mut self.name_error) {
            self.name_parsed = Some(n);
        }
        self.description = description.to_owned();
        self.keywords = keywords.to_owned();
        self.author = author.to_owned();
        self.version = version.to_owned();
        if let Some(v) = validation::version(&self.version, &mut self.version_error) {
            self.version_parsed = Some(v);
        }
        self.deprecated = deprecated;
    }

    /// Applies the edited metadata and `extra` (upstream
    /// `commitUiData()`).
    pub fn commit(&mut self, extra: impl FnOnce(&mut E::Extra)) -> Option<TabRequest> {
        let name = self.name_parsed.clone();
        let description = self.description.clone();
        let keywords = self.keywords.clone();
        let author = self.author.clone();
        let version = self.version_parsed.clone();
        let deprecated = self.deprecated;
        let text = if E::TAB_TYPE == ui::TabType::Organization {
            tr!(
                "librepcb::editor::CmdOrganizationEdit",
                "Edit Organization Properties"
            )
        } else {
            tr!(
                "librepcb::editor::CmdLibraryCategoryEdit",
                "Edit category metadata"
            )
        };
        let result = self.edit(&text, |c| {
            let m = &mut c.metadata;
            if let Some(name) = name {
                let mut names = m.names().clone();
                names.set_default_value(name);
                m.set_names(names);
            }
            if description != *m.descriptions().default_value() {
                let mut d = m.descriptions().clone();
                d.set_default_value(description.trim().to_owned());
                m.set_descriptions(d);
            }
            if keywords != *m.keywords().default_value() {
                let mut k = m.keywords().clone();
                k.set_default_value(validation::clean_keywords(&keywords));
                m.set_keywords(k);
            }
            if author != *m.author() {
                m.set_author(author.trim().to_owned());
            }
            if let Some(v) = version {
                m.set_version(v);
            }
            m.set_deprecated(deprecated);
            extra(&mut c.extra);
        });
        self.refresh();
        self.run_checks_if_modified();
        result.err().map(error_notification)
    }

    /// Undo (`true`) or redo.
    pub fn undo_redo(&mut self, undo: bool) {
        let content = if undo {
            self.undo.undo()
        } else {
            self.undo.redo()
        };
        if let Some(content) = content {
            self.restore(content);
        }
        self.refresh();
        self.run_checks_if_modified();
    }

    /// Runs the checks if the element changed since the last run.
    pub fn run_checks_if_modified(&mut self) {
        let state = self.undo.state_id();
        if self.checked_state == Some(state) {
            return;
        }
        self.checked_state = Some(state);
        match self.element.run_checks() {
            Ok(messages) => {
                self.check_error.clear();
                let approvals = self.element.metadata().message_approvals().clone();
                let entries = messages
                    .iter()
                    .map(|m| CheckMessage {
                        message: m.to_message(),
                        schematic: None,
                        autofix: matches!(
                            m,
                            LibraryCheckMessage::BaseElement(
                                LibraryBaseElementCheckMessage::NameNotTitleCase { .. }
                                    | LibraryBaseElementCheckMessage::MissingAuthor
                            )
                        ),
                    })
                    .collect();
                self.checks.borrow_mut().set_messages(entries, approvals);
                self.check_messages = messages;
            }
            Err(e) => self.check_error = e.to_string(),
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

    /// A check message row was written by the UI (approval, autofix).
    pub fn check_row_written(&mut self, row: usize, data: &ui::RuleCheckMessageData) -> TabUpdate {
        let action = self.checks.borrow_mut().row_written(row, data);
        let mut update = TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        };
        match action {
            RowAction::Approve { approval, approved } => {
                if self
                    .element
                    .metadata_mut()
                    .set_message_approved(&approval, approved)
                {
                    self.manual_modifications = true;
                }
                self.checked_state = None;
                self.run_checks_if_modified();
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
                // Upstream `autoFix(MsgNameNotTitleCase)`,
                // `autoFix(MsgMissingAuthor)`.
                match msg {
                    Some(LibraryCheckMessage::BaseElement(
                        LibraryBaseElementCheckMessage::NameNotTitleCase { name },
                    )) => {
                        self.name_parsed = Some(title_case_fixed_name(&name));
                        update.requests.extend(self.commit(|_| {}));
                    }
                    Some(LibraryCheckMessage::BaseElement(
                        LibraryBaseElementCheckMessage::MissingAuthor,
                    )) => {
                        self.author = self.user_name.clone();
                        update.requests.extend(self.commit(|_| {}));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        update
    }

    /// Saves the element (into the library if it is new).
    pub fn save(&mut self) -> Result<(), String> {
        if self.is_new {
            let mut dir = self.library.directory(&self.relative_dir());
            self.element.save_to(&mut dir).map_err(|e| e.to_string())?;
        } else {
            self.element.save().map_err(|e| e.to_string())?;
        }
        self.element
            .directory()
            .file_system()
            .save()
            .map_err(|e| e.to_string())?;
        self.undo.set_clean();
        self.manual_modifications = false;
        self.is_new = false;
        self.refresh();
        self.checked_state = None;
        self.run_checks_if_modified();
        Ok(())
    }

    /// The base `TabData`.
    pub fn tab_data(&self) -> ui::TabData {
        let writable = self.is_writable();
        ui::TabData {
            r#type: E::TAB_TYPE,
            title: self.element.metadata().name().to_string().into(),
            features: ui::TabFeatures {
                save: feature(writable),
                undo: feature(self.undo.can_undo()),
                redo: feature(self.undo.can_redo()),
                ..Default::default()
            },
            read_only: !writable,
            unsaved_changes: self.is_dirty(),
            undo_text: self.undo.undo_text().unwrap_or_default().into(),
            redo_text: self.undo.redo_text().unwrap_or_default().into(),
            ..Default::default()
        }
    }

    /// Handles save, undo and redo after committing (`commit`); returns
    /// `None` for other actions.
    pub fn trigger_common(
        &mut self,
        action: ui::TabAction,
        commit: impl FnOnce(&mut Self) -> Vec<TabRequest>,
    ) -> Option<TabUpdate> {
        use ui::TabAction as A;
        if !matches!(action, A::Apply | A::Save | A::Undo | A::Redo) {
            return None;
        }
        let mut update = TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        };
        update.requests.extend(commit(self));
        match action {
            A::Save => match self.save() {
                Ok(()) => update.requests.push(TabRequest::RescanLibraries),
                Err(e) => update.requests.push(error_notification(e)),
            },
            A::Undo => self.undo_redo(true),
            A::Redo => self.undo_redo(false),
            _ => {}
        }
        Some(update)
    }
}
