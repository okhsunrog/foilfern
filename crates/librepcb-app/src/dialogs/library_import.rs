//! The EAGLE and KiCad library import wizards.
//!
//! Port of libs/librepcb/editor/library/eaglelibraryimportwizard/ and
//! libs/librepcb/editor/library/kicadlibraryimportwizard/ (the wizards,
//! their contexts and the pages start, choose library, select elements, set
//! options and result) on the UI independent imports of `librepcb-import`.
//!
//! The import itself runs in a worker thread (like upstream) with progress
//! and cancellation; the application moves the [`Importer`] out of the
//! wizard ([`LibraryImportWizard::take_importer()`]), runs it and hands it
//! back with the messages ([`LibraryImportWizard::import_finished()`]).
//!
//! Differences to upstream (see COMPAT.md): the element tree is shown as
//! one checkable list per element kind (a partially checked element, i.e.
//! a dependency of a checked one, is marked with an orange swatch), KiCad
//! symbols have one row per imported element (symbols, component, device);
//! the EAGLE library is parsed and the KiCad libraries are scanned and
//! parsed synchronously when going to the next page; the additional
//! categories are chosen from a combo box of all workspace categories.

use std::collections::BTreeSet;
use std::sync::Arc;

use librepcb_core::fileio::FilePath;
use librepcb_core::types::{LengthUnit, Uuid};
use librepcb_core::utils::message_logger::{LogLevel, LogMessage, MessageLogger};
use librepcb_core::workspace::{ElementKind as DbKind, LibraryDb};
use librepcb_i18n::{tr, trn};
use librepcb_import::eagle::EagleLibraryImport;
use librepcb_import::kicad::{ImportState, KiCadLibraryImport};
use librepcb_import::{CheckState, ImportSummary, Progress, ProgressEvent};

use super::{
    AppRequest, Applied, ButtonResult, DialogContext, DialogOptions, FieldEvent, Form, FormDialog,
    ListAction, ListButtons, ListItem,
};
use crate::file_dialog;

const EAGLE_START: &str = "librepcb::editor::EagleLibraryImportWizardPage_Start";
const EAGLE_CHOOSE: &str = "librepcb::editor::EagleLibraryImportWizardPage_ChooseLibrary";
const EAGLE_SELECT: &str = "librepcb::editor::EagleLibraryImportWizardPage_SelectElements";
const EAGLE_OPTIONS: &str = "librepcb::editor::EagleLibraryImportWizardPage_SetOptions";
const EAGLE_RESULT: &str = "librepcb::editor::EagleLibraryImportWizardPage_Result";
const EAGLE_CONTEXT: &str = "librepcb::editor::EagleLibraryImportWizardContext";
const KICAD_START: &str = "librepcb::editor::KiCadLibraryImportWizardPage_Start";
const KICAD_CHOOSE: &str = "librepcb::editor::KiCadLibraryImportWizardPage_ChooseLibrary";
const KICAD_SELECT: &str = "librepcb::editor::KiCadLibraryImportWizardPage_SelectElements";
const KICAD_RESULT: &str = "librepcb::editor::KiCadLibraryImportWizardPage_Result";
const KICAD_CONTEXT: &str = "librepcb::editor::KiCadLibraryImportWizardContext";

/// Color of partially checked elements (dependencies).
const PARTIAL_COLOR: slint::Color = slint::Color::from_rgb_u8(0xff, 0xa5, 0x00);

/// The import of a wizard.
pub enum Importer {
    /// EAGLE `*.lbr` import.
    Eagle(Box<EagleLibraryImport>),
    /// KiCad library import.
    KiCad(Box<KiCadLibraryImport>),
}

impl std::fmt::Debug for Importer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Eagle(_) => f.write_str("Importer::Eagle"),
            Self::KiCad(_) => f.write_str("Importer::KiCad"),
        }
    }
}

impl Importer {
    /// Runs the import (blocking, in a worker thread).
    pub fn run(
        &mut self,
        db: &LibraryDb,
        log: &MessageLogger<'_>,
        progress: &Progress,
    ) -> Option<ImportSummary> {
        match self {
            Self::Eagle(i) => Some(i.run(log, progress)),
            Self::KiCad(i) => i.import(db, log, progress),
        }
    }

    fn set_add_name_prefix(&mut self, add: bool) {
        match self {
            Self::Eagle(i) => i.set_add_name_prefix(add),
            Self::KiCad(i) => i.set_add_name_prefix(add),
        }
    }

    fn set_add_to_category(&mut self, add: bool) {
        match self {
            Self::Eagle(i) => i.set_add_to_category(add),
            Self::KiCad(i) => i.set_add_to_category(add),
        }
    }

    fn set_component_category(&mut self, uuid: Option<Uuid>) {
        match self {
            Self::Eagle(i) => i.set_component_category(uuid),
            Self::KiCad(i) => i.set_component_category(uuid),
        }
    }

    fn set_package_category(&mut self, uuid: Option<Uuid>) {
        match self {
            Self::Eagle(i) => i.set_package_category(uuid),
            Self::KiCad(i) => i.set_package_category(uuid),
        }
    }

    fn set_all_checked(&mut self, checked: bool) {
        match self {
            Self::Eagle(i) => i.set_all_checked(checked),
            Self::KiCad(i) => i.set_all_checked(checked),
        }
    }
}

/// The pages of the wizards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Page {
    /// Introduction.
    Start,
    /// Choose the EAGLE library file or the KiCad libraries directory.
    ChooseLibrary,
    /// Select the elements to import.
    SelectElements,
    /// Import options.
    SetOptions,
    /// Import progress and messages.
    Result,
}

/// Categories (UUID, name).
type Categories = Vec<(Uuid, String)>;

/// A row of the element lists: kind and name (and KiCad library name).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    list: &'static str,
    library: String,
    name: String,
    /// KiCad symbol rows: 0 symbols, 1 component, 2 device.
    part: u8,
}

/// Formats log messages for the message field (upstream
/// `MessageLogger::Message::toRichText()`, as plain text).
pub fn format_messages(messages: &[LogMessage]) -> String {
    messages
        .iter()
        .filter(|m| m.level >= LogLevel::Info)
        .map(|m| match m.level {
            LogLevel::Warning => format!("⚠ {}", m.message),
            LogLevel::Critical => format!("✖ {}", m.message),
            _ => m.message.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The EAGLE or KiCad library import wizard.
pub struct LibraryImportWizard {
    form: Form,
    kicad: bool,
    library: FilePath,
    db: Arc<LibraryDb>,
    importer: Option<Importer>,
    page: Page,
    source: String,
    shapes_3d: String,
    parse_messages: String,
    rows: Vec<Row>,
    add_name_prefix: bool,
    add_to_category: bool,
    /// Component and package categories of the workspace.
    categories: (Categories, Categories),
    component_category: Option<Uuid>,
    package_category: Option<Uuid>,
    progress: Option<Progress>,
    import_messages: String,
    status: String,
    status_text: String,
    percent: u32,
    finished: bool,
    request: Option<AppRequest>,
}

impl LibraryImportWizard {
    /// The EAGLE library import wizard for the library `library`.
    pub fn eagle(library: FilePath, db: Arc<LibraryDb>) -> Self {
        let importer = Importer::Eagle(Box::new(EagleLibraryImport::new(library.clone())));
        Self::new(false, library, db, importer)
    }

    /// The KiCad library import wizard for the library `library`.
    pub fn kicad(library: FilePath, db: Arc<LibraryDb>) -> Self {
        let importer = Importer::KiCad(Box::new(KiCadLibraryImport::new(library.clone())));
        Self::new(true, library, db, importer)
    }

    fn new(kicad: bool, library: FilePath, db: Arc<LibraryDb>, importer: Importer) -> Self {
        let categories = (
            categories(&db, DbKind::ComponentCategory),
            categories(&db, DbKind::PackageCategory),
        );
        let mut wizard = Self {
            form: Form::new(LengthUnit::Millimeters),
            kicad,
            library,
            db,
            importer: Some(importer),
            page: Page::Start,
            source: String::new(),
            shapes_3d: String::new(),
            parse_messages: String::new(),
            rows: Vec::new(),
            add_name_prefix: false,
            add_to_category: true,
            categories,
            component_category: None,
            package_category: None,
            progress: None,
            import_messages: String::new(),
            status: String::new(),
            status_text: String::new(),
            percent: 0,
            finished: false,
            request: None,
        };
        wizard.apply_options();
        wizard.build();
        wizard
    }

    /// The current page.
    pub fn page(&self) -> Page {
        self.page
    }

    /// The destination library.
    pub fn library(&self) -> &FilePath {
        &self.library
    }

    /// Whether the import finished.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    fn ctx(&self, eagle: &'static str, kicad: &'static str) -> &'static str {
        if self.kicad { kicad } else { eagle }
    }

    fn build(&mut self) {
        self.form.clear();
        match self.page {
            Page::Start => {
                if self.kicad {
                    self.form
                        .header(tr!(KICAD_START, "KiCad Library Import"))
                        .note(
                            "intro",
                            tr!(
                                KICAD_START,
                                "Kick-start the migration from KiCad to LibrePCB by importing your KiCad library elements into the currently opened LibrePCB library."
                            ),
                        );
                } else {
                    self.form
                        .header(tr!(EAGLE_START, "EAGLE Library Import"))
                        .note(
                            "intro",
                            tr!(
                                EAGLE_START,
                                "Kick-start the migration from EAGLE to LibrePCB by importing your EAGLE library elements into the currently opened LibrePCB library."
                            ),
                        );
                }
                self.form.label("destination", "", self.library.to_native());
            }
            Page::ChooseLibrary => {
                if self.kicad {
                    self.form
                        .header(tr!(KICAD_CHOOSE, "Select Libraries"))
                        .note(
                            "subtitle",
                            tr!(
                                KICAD_CHOOSE,
                                "Choose the directory containing KiCad libraries to import."
                            ),
                        )
                        .note(
                            "hint",
                            tr!(
                                KICAD_CHOOSE,
                                "Select the directory where all *.kicad_sym files, *.pretty folders and *.3dshapes folders are located. They may be located in a subdirectory (maximum 1 nesting level)."
                            ),
                        )
                        .text("source", "", &self.source)
                        .button(
                            "browse",
                            "",
                            tr!(KICAD_CHOOSE, "Select root directory of libraries"),
                        )
                        .note(
                            "version_note",
                            tr!(
                                KICAD_CHOOSE,
                                "Note: Currently this feature supports KiCad {0} libraries. Other versions may not work as intended.",
                                librepcb_import::kicad::COMPATIBLE_KICAD_VERSION
                            ),
                        );
                } else {
                    self.form
                        .header(tr!(EAGLE_CHOOSE, "Select Library"))
                        .note(
                            "subtitle",
                            tr!(EAGLE_CHOOSE, "Choose the EAGLE library file to import."),
                        )
                        .text("source", "", &self.source)
                        .button("browse", "", tr!(EAGLE_CHOOSE, "Select file to import"))
                        .note(
                            "version_note",
                            tr!(
                                EAGLE_CHOOSE,
                                "Note: Only EAGLE 6 (or later) *.lbr files are supported (XML based file format)."
                            ),
                        );
                }
                self.form.note("messages", &self.parse_messages);
            }
            Page::SelectElements => {
                let ctx = self.ctx(EAGLE_SELECT, KICAD_SELECT);
                self.form.header(tr!(ctx, "Select Elements To Import")).note(
                    "subtitle",
                    tr!(
                        ctx,
                        "Select the library elements to import. Dependent elements of devices and components will be selected automatically."
                    ),
                );
                for (id, _) in self.lists() {
                    self.form
                        .checkbox(&format!("{id}_all"), "", "", false)
                        .list(id, "", &[], &[], 5, ListButtons::default());
                }
                self.update_lists();
            }
            Page::SetOptions => {
                let (prefix, component) = if self.kicad {
                    (
                        librepcb_import::kicad::NAME_PREFIX,
                        librepcb_import::kicad::CATEGORY_NAME,
                    )
                } else {
                    (
                        librepcb_import::eagle::NAME_PREFIX,
                        librepcb_import::eagle::CATEGORY_NAME,
                    )
                };
                let cmp_names: Vec<String> = std::iter::once(tr!(
                    "librepcb::editor::InitializeWorkspaceWizard_ChooseSettings",
                    "None"
                ))
                .chain(self.categories.0.iter().map(|(_, n)| n.clone()))
                .collect();
                let pkg_names: Vec<String> = std::iter::once(tr!(
                    "librepcb::editor::InitializeWorkspaceWizard_ChooseSettings",
                    "None"
                ))
                .chain(self.categories.1.iter().map(|(_, n)| n.clone()))
                .collect();
                let index = |list: &[(Uuid, String)], uuid: Option<Uuid>| {
                    uuid.and_then(|u| list.iter().position(|(c, _)| *c == u))
                        .map_or(0, |i| i + 1)
                };
                self.form
                    .header(tr!(EAGLE_OPTIONS, "Set Options"))
                    .note(
                        "subtitle",
                        tr!(
                            EAGLE_OPTIONS,
                            "Choose the import options and click on the import button below to continue. If you are unsure about the options, just keep the default values."
                        ),
                    )
                    .checkbox(
                        "add_prefix",
                        "",
                        tr!(
                            EAGLE_OPTIONS,
                            "Add prefix \"{0}\" to all library element names",
                            prefix
                        ),
                        self.add_name_prefix,
                    )
                    .note(
                        "add_prefix_hint",
                        tr!(
                            EAGLE_OPTIONS,
                            "Helps to distinguish between manually created elements and imported elements."
                        ),
                    )
                    .checkbox(
                        "add_category",
                        "",
                        tr!(
                            EAGLE_OPTIONS,
                            "Add all elements to \"{0}\" category (recommended)",
                            component
                        ),
                        self.add_to_category,
                    )
                    .note(
                        "add_category_hint",
                        tr!(
                            EAGLE_OPTIONS,
                            "Will list all imported elements under a dedicated category, to help finding them."
                        ),
                    )
                    .choice(
                        "component_category",
                        tr!(EAGLE_OPTIONS, "Additional Component Category"),
                        &cmp_names,
                        Some(index(&self.categories.0, self.component_category)),
                    )
                    .choice(
                        "package_category",
                        tr!(EAGLE_OPTIONS, "Additional Package Category"),
                        &pkg_names,
                        Some(index(&self.categories.1, self.package_category)),
                    )
                    .note(
                        "attention",
                        tr!(
                            EAGLE_OPTIONS,
                            "<b>Attention:</b> The import cannot be undone (except by manually removing the imported elements)!"
                        )
                        .replace("<b>", "")
                        .replace("</b>", ""),
                    );
            }
            Page::Result => {
                let ctx = self.ctx(EAGLE_RESULT, KICAD_RESULT);
                self.form
                    .header(tr!(ctx, "Import Progress"))
                    .note(
                        "subtitle",
                        tr!(
                            ctx,
                            "The selected elements will be imported now. For large or complex libraries this can take a few minutes."
                        ),
                    )
                    .label("status", "", &self.status)
                    .multiline("messages", tr!(EAGLE_RESULT, "Import messages:"), &self.import_messages, 12);
                self.form.set_enabled("messages", true);
            }
        }
    }

    /// The element lists of the selection page: (field id, title).
    fn lists(&self) -> Vec<(&'static str, String)> {
        let ctx = self.ctx(EAGLE_SELECT, KICAD_SELECT);
        vec![
            ("devices", tr!(ctx, "Devices")),
            ("components", tr!(ctx, "Components")),
            ("symbols", tr!(ctx, "Symbols")),
            ("packages", tr!(ctx, "Packages")),
        ]
    }

    /// All elements: (row, check state, already imported).
    fn elements(&self) -> Vec<(Row, CheckState, bool)> {
        let mut out = Vec::new();
        let row = |list, library: &str, name: &str, part| Row {
            list,
            library: library.to_owned(),
            name: name.to_owned(),
            part,
        };
        match &self.importer {
            Some(Importer::Eagle(i)) => {
                for d in i.devices() {
                    out.push((row("devices", "", &d.display_name, 0), d.check_state, false));
                }
                for c in i.components() {
                    out.push((
                        row("components", "", &c.display_name, 0),
                        c.check_state,
                        false,
                    ));
                }
                for s in i.symbols() {
                    out.push((row("symbols", "", &s.display_name, 0), s.check_state, false));
                }
                for p in i.packages() {
                    out.push((
                        row("packages", "", &p.display_name, 0),
                        p.check_state,
                        false,
                    ));
                }
            }
            Some(Importer::KiCad(i)) => {
                for lib in &i.result().symbol_libs {
                    let lib_name = lib.file.complete_basename();
                    for s in &lib.symbols {
                        if !s.pkg_generated_by.is_empty() {
                            out.push((
                                row("devices", lib_name, &s.name, 2),
                                s.dev_checked,
                                s.dev_already_imported,
                            ));
                        }
                        out.push((
                            row("components", lib_name, &s.name, 1),
                            s.cmp_checked,
                            s.cmp_already_imported,
                        ));
                        out.push((
                            row("symbols", lib_name, &s.name, 0),
                            s.sym_checked,
                            s.sym_already_imported,
                        ));
                    }
                }
                for lib in &i.result().footprint_libs {
                    let lib_name = lib.dir.complete_basename();
                    for f in &lib.footprints {
                        out.push((
                            row("packages", lib_name, &f.name, 0),
                            f.checked,
                            f.already_imported,
                        ));
                    }
                }
            }
            None => {}
        }
        out
    }

    /// Updates the element lists and their "all" check boxes (upstream
    /// `updateRootNodes()`: "Devices (checked/total)").
    fn update_lists(&mut self) {
        let elements = self.elements();
        self.rows.clear();
        let lists = self.lists();
        for (id, title) in lists {
            let mut items = Vec::new();
            let mut checked = 0;
            let mut total = 0;
            for (row, state, already) in elements.iter().filter(|(r, _, _)| r.list == id) {
                let mut text = if row.library.is_empty() {
                    row.name.clone()
                } else {
                    format!("{}: {}", row.library, row.name)
                };
                if *already {
                    text = format!(
                        "{text} ({})",
                        tr!(
                            "librepcb::editor::KiCadLibraryImportWizardPage_SelectElements",
                            "Already imported"
                        )
                    );
                }
                let mut item = ListItem::check(text, state.is_imported());
                if *state == CheckState::PartiallyChecked {
                    item.color = Some(PARTIAL_COLOR);
                }
                items.push(item);
                if state.is_imported() {
                    checked += 1;
                }
                total += 1;
                self.rows.push(row.clone());
            }
            self.form.set_items(id, &items, None);
            self.form.update(&format!("{id}_all"), |f| {
                f.text = format!("{title} ({checked}/{total})").into();
                f.checked = total > 0 && checked == total;
                f.enabled = total > 0;
            });
        }
    }

    /// Checks or unchecks an element (a row of the list `list`).
    pub fn set_checked(&mut self, list: &str, index: usize, checked: bool) {
        let Some(row) = self
            .rows
            .iter()
            .filter(|r| r.list == list)
            .nth(index)
            .cloned()
        else {
            return;
        };
        match &mut self.importer {
            Some(Importer::Eagle(i)) => {
                let _ = match list {
                    "devices" => i.set_device_checked(&row.name, checked),
                    "components" => i.set_component_checked(&row.name, checked),
                    "symbols" => i.set_symbol_checked(&row.name, checked),
                    _ => i.set_package_checked(&row.name, checked),
                };
            }
            Some(Importer::KiCad(i)) => {
                let _ = match (list, row.part) {
                    ("packages", _) => i.set_package_checked(&row.library, &row.name, checked),
                    (_, 2) => i.set_device_checked(&row.library, &row.name, checked),
                    (_, 1) => i.set_component_checked(&row.library, &row.name, checked),
                    _ => i.set_symbol_checked(&row.library, &row.name, checked),
                };
            }
            None => {}
        }
        self.update_lists();
    }

    fn checked_count(&self) -> usize {
        self.elements()
            .iter()
            .filter(|(_, s, _)| s.is_imported())
            .count()
    }

    /// Parses the chosen EAGLE library, or scans and parses the KiCad
    /// libraries (upstream `setLbrFilePath()` / `startScan()` and the
    /// parse page); returns whether elements were found.
    fn parse_source(&mut self) -> bool {
        let log = MessageLogger::new();
        let progress = Progress::new();
        let source = FilePath::new(self.source.trim());
        let found = match (&mut self.importer, source) {
            (_, None) => {
                log.info(&if self.kicad {
                    tr!(KICAD_CONTEXT, "No file or directory selected.")
                } else {
                    tr!(EAGLE_CONTEXT, "No file selected.")
                });
                false
            }
            (Some(Importer::Eagle(i)), Some(fp)) => {
                i.reset();
                match i.open(&fp) {
                    Ok(warnings) => {
                        for w in warnings {
                            log.warning(&w);
                        }
                        let count = i.total_elements_count();
                        log.info(&trn!(
                            EAGLE_CONTEXT,
                            "Found {0} element(s) in the selected library.",
                            "Found {0} element(s) in the selected library.",
                            count,
                            count
                        ));
                        count > 0
                    }
                    Err(e) => {
                        log.critical(&e.to_string());
                        false
                    }
                }
            }
            (Some(Importer::KiCad(i)), Some(fp)) => {
                i.reset();
                let shapes = FilePath::new(self.shapes_3d.trim());
                i.scan(&fp, shapes.as_ref(), &log, &progress)
                    && i.parse(self.db.as_ref(), &log, &progress)
                    && i.state() == ImportState::Parsed
                    && !i.result().symbol_libs.is_empty() | !i.result().footprint_libs.is_empty()
            }
            (None, _) => false,
        };
        self.parse_messages = format_messages(&log.messages());
        found
    }

    fn apply_options(&mut self) {
        let (prefix, category, cmp, pkg) = (
            self.add_name_prefix,
            self.add_to_category,
            self.component_category,
            self.package_category,
        );
        if let Some(i) = &mut self.importer {
            i.set_add_name_prefix(prefix);
            i.set_add_to_category(category);
            i.set_component_category(cmp);
            i.set_package_category(pkg);
        }
    }

    /// "Next".
    pub fn next_page(&mut self) -> Result<(), String> {
        let next = match self.page {
            Page::Start => Page::ChooseLibrary,
            Page::ChooseLibrary => {
                self.source = self.form.get_text("source");
                if !self.parse_source() {
                    self.form.set_text("messages", &self.parse_messages);
                    return Err(self.parse_messages.clone());
                }
                if let Some(i) = &mut self.importer {
                    // Upstream: all elements are checked initially.
                    i.set_all_checked(true);
                }
                Page::SelectElements
            }
            Page::SelectElements => {
                if self.checked_count() == 0 {
                    return Err(tr!(EAGLE_SELECT, "Select Elements To Import"));
                }
                Page::SetOptions
            }
            Page::SetOptions => {
                self.store_options();
                self.page = Page::Result;
                self.status = String::new();
                self.import_messages.clear();
                self.build();
                return Ok(());
            }
            Page::Result => return Ok(()),
        };
        self.page = next;
        self.build();
        Ok(())
    }

    /// "Back".
    pub fn previous_page(&mut self) {
        self.page = match self.page {
            Page::Start | Page::ChooseLibrary => Page::Start,
            Page::SelectElements => Page::ChooseLibrary,
            Page::SetOptions => Page::SelectElements,
            Page::Result => return,
        };
        self.build();
    }

    fn store_options(&mut self) {
        self.add_name_prefix = self.form.get_checked("add_prefix");
        self.add_to_category = self.form.get_checked("add_category");
        self.component_category = self
            .form
            .get_index("component_category")
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| self.categories.0.get(i))
            .map(|(u, _)| *u);
        self.package_category = self
            .form
            .get_index("package_category")
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| self.categories.1.get(i))
            .map(|(u, _)| *u);
        self.apply_options();
    }

    /// Takes the importer to run it in a worker thread (on the result
    /// page) with `progress` (kept for canceling); `None` if it is running
    /// already.
    pub fn take_importer(&mut self, progress: &Progress) -> Option<(Importer, Arc<LibraryDb>)> {
        if self.page != Page::Result || self.finished {
            return None;
        }
        let importer = self.importer.take()?;
        self.progress = Some(progress.clone());
        Some((importer, Arc::clone(&self.db)))
    }

    /// Updates the progress (events of the worker thread).
    pub fn progress_event(&mut self, event: &ProgressEvent) {
        match event {
            ProgressEvent::Status(s) => self.status_text = s.clone(),
            ProgressEvent::Percent(p) => self.percent = *p,
        }
        self.status = format!("{} ({}%)", self.status_text, self.percent);
        self.form.set_text("status", &self.status);
    }

    /// The import finished: gives the importer back and shows the messages
    /// (upstream `importFinished()`).
    pub fn import_finished(
        &mut self,
        importer: Importer,
        summary: Option<ImportSummary>,
        messages: &[LogMessage],
    ) {
        self.importer = Some(importer);
        self.progress = None;
        self.finished = true;
        let mut text = format_messages(messages);
        if self.kicad && summary.is_some_and(|s| !s.canceled) {
            let ctx = KICAD_RESULT;
            text.push_str(&format!(
                "\n\n{}\n • {}\n • {}\n • {}\n • {}",
                tr!(
                    ctx,
                    "It is highly recommended to review and rework the imported elements:"
                ),
                tr!(ctx, "Assign reasonable categories"),
                tr!(ctx, "Review/correct pinouts of devices"),
                tr!(ctx, "Review/rework geometry of symbols and footprints"),
                tr!(ctx, "Fix remaining warnings shown in the library editor"),
            ));
        }
        self.import_messages = text;
        self.status = match summary {
            Some(s) if s.canceled => tr!("librepcb::editor::EagleLibraryImportWizard", "Abort"),
            _ => tr!(EAGLE_RESULT, "Finished!"),
        };
        self.build();
    }

    /// Requests canceling a running import.
    pub fn cancel_import(&self) {
        if let Some(p) = &self.progress {
            p.cancel();
        }
    }

    /// Whether the import is running.
    pub fn is_running(&self) -> bool {
        self.progress.is_some()
    }
}

/// The categories of a kind in the workspace libraries (latest versions),
/// sorted by name.
fn categories(db: &LibraryDb, kind: DbKind) -> Vec<(Uuid, String)> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let Ok(all) = db.all(kind, None, None) else {
        return out;
    };
    for (_, dir) in all.into_iter().rev() {
        let Ok(Some(info)) = db.metadata(kind, &dir) else {
            continue;
        };
        if !seen.insert(info.uuid) {
            continue;
        }
        let name = db
            .translations(kind, &dir, &[] as &[&str])
            .ok()
            .flatten()
            .map(|t| t.name)
            .unwrap_or_default();
        out.push((info.uuid, name));
    }
    out.sort_by(|a, b| crate::dialogs::natural_cmp(&a.1.to_lowercase(), &b.1.to_lowercase()));
    out
}

impl FormDialog for LibraryImportWizard {
    fn title(&self) -> String {
        if self.kicad {
            tr!(
                "librepcb::editor::KiCadLibraryImportWizard",
                "KiCad Library Import"
            )
        } else {
            tr!(
                "librepcb::editor::EagleLibraryImportWizard",
                "EAGLE Library Import"
            )
        }
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        let mut extra_buttons = Vec::new();
        let ok_text = match self.page {
            Page::SetOptions => tr!(EAGLE_OPTIONS, "&Import!").replace('&', ""),
            Page::Result => tr!("ProjectLibraryTab", "Close"),
            _ => tr!("DeviceContentTab", "Next"),
        };
        if matches!(
            self.page,
            Page::ChooseLibrary | Page::SelectElements | Page::SetOptions
        ) {
            extra_buttons.push(tr!("DeviceContentTab", "Back"));
        }
        if self.page == Page::Result && self.finished {
            extra_buttons.push(
                tr!("librepcb::editor::EagleLibraryImportWizard", "&Restart").replace('&', ""),
            );
        }
        DialogOptions {
            apply: false,
            cancel: !(self.page == Page::Result && self.finished),
            ok_text: Some(ok_text),
            extra_buttons,
            width: 650.0,
            label_width: 200.0,
            ..DialogOptions::default()
        }
    }

    fn field_event(&mut self, _ctx: &DialogContext<'_>, id: &str, event: FieldEvent) {
        match (id, event) {
            ("browse", FieldEvent::Clicked) => {
                let chosen = if self.kicad {
                    file_dialog::choose_directory(&tr!(KICAD_CHOOSE, "Choose directory"), None)
                } else {
                    let filter = file_dialog::Filter {
                        name: "*.lbr".into(),
                        extensions: vec!["lbr"],
                    };
                    file_dialog::open_file(&tr!(EAGLE_CHOOSE, "Choose file"), &[filter], None)
                };
                if let Some(path) = chosen {
                    self.source = path.to_string_lossy().into_owned();
                    self.form.set_text("source", &self.source);
                }
            }
            (list, FieldEvent::List(ListAction::Toggle(row))) => {
                let checked = self
                    .form
                    .get_list_checked(list)
                    .get(row)
                    .copied()
                    .unwrap_or(false);
                self.set_checked(list, row, checked);
            }
            (all, FieldEvent::Edited) if all.ends_with("_all") => {
                let list = all.trim_end_matches("_all").to_owned();
                let checked = self.form.get_checked(all);
                let count = self.rows.iter().filter(|r| r.list == list).count();
                for i in 0..count {
                    self.set_checked(&list, i, checked);
                }
            }
            _ => {}
        }
    }

    fn button(&mut self, _ctx: &DialogContext<'_>, index: usize) -> Result<ButtonResult, String> {
        if self.page == Page::Result {
            // "Restart": import another library.
            let library = self.library.clone();
            let db = Arc::clone(&self.db);
            *self = if self.kicad {
                Self::kicad(library, db)
            } else {
                Self::eagle(library, db)
            };
            self.page = Page::ChooseLibrary;
            self.build();
        } else {
            let _ = index;
            self.previous_page();
        }
        Ok(ButtonResult::Keep)
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        match self.page {
            Page::Result if self.finished => Ok(Applied::Nothing),
            Page::Result => Err(String::new()),
            Page::SetOptions => {
                self.next_page()?;
                self.request = Some(AppRequest::RunLibraryImport);
                Err(String::new())
            }
            _ => {
                self.next_page()?;
                Err(String::new())
            }
        }
    }

    fn take_request(&mut self) -> Option<AppRequest> {
        self.request.take()
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn closing(&mut self) {
        // Upstream asks whether to abort a running import.
        self.cancel_import();
    }
}
