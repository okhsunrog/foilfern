//! The new project wizard.
//!
//! Port of libs/librepcb/editor/project/newprojectwizard/
//! (`newprojectwizard.{h,cpp,ui}` and its pages
//! `newprojectwizardpage_{eagleimport,metadata,initialization}`): the
//! EAGLE import page (only in EAGLE import mode), the metadata page (name,
//! author, license, path) and the initialization page (schematic and
//! board, only for new projects). [`create_project()`] is upstream
//! `NewProjectWizard::createProject()`: the project with the stroke fonts,
//! the workspace's library locale and norm order, the schematic, the board
//! with its default content (100x80 mm outline), `LICENSE.txt`,
//! `README.md`, `.gitignore` and `.gitattributes` from upstream's
//! templates (embedded at build time), or the imported EAGLE project.
//!
//! The pages are shown one after another in a form dialog with "Back",
//! "Next" and "Finish" buttons (upstream `QWizard`); the EAGLE files are
//! parsed synchronously when the paths change (upstream parses them in a
//! background thread).

use std::sync::Arc;

use librepcb_core::fileio::{
    CleanFileNameOptions, FileNameCase, FilePath, FileSystem, TransactionalDirectory,
    TransactionalFileSystem,
};
use librepcb_core::geometry::Path;
use librepcb_core::project::board::{Board, BoardItem, BoardPolygonData};
use librepcb_core::project::schematic::Schematic;
use librepcb_core::project::{Mutation, Project};
use librepcb_core::types::{ElementName, Layer, Length, LengthUnit, Point, UnsignedLength, Uuid};
use librepcb_core::utils::message_logger::{LogLevel, MessageLogger};
use librepcb_i18n::{tr, trn};
use librepcb_import::eagle::EagleProjectImport;

use super::{
    AppRequest, Applied, ButtonResult, DialogContext, DialogOptions, FieldEvent, Form, FormDialog,
};
use crate::file_dialog;

const WIZARD: &str = "librepcb::editor::NewProjectWizard";
const META: &str = "librepcb::editor::NewProjectWizardPage_Metadata";
const INIT: &str = "librepcb::editor::NewProjectWizardPage_Initialization";
const EAGLE: &str = "librepcb::editor::NewProjectWizardPage_EagleImport";

macro_rules! share_file {
    ($path:literal) => {
        include_bytes!(concat!(
            env!("LIBREPCB_UPSTREAM_DIR"),
            "/share/librepcb/",
            $path
        ))
    };
}

const README_TEMPLATE: &[u8] = share_file!("project/readme_template");
const GITIGNORE_TEMPLATE: &[u8] = share_file!("project/gitignore_template");
const GITATTRIBUTES_TEMPLATE: &[u8] = share_file!("project/gitattributes_template");

/// A license of the metadata page: the (translated) name and the license
/// text (`None`: no license).
struct License {
    name: fn() -> String,
    text: Option<&'static [u8]>,
}

/// The licenses offered by upstream (SPDX identifiers).
const LICENSES: &[License] = &[
    License {
        name: || tr!(META, "None"),
        text: None,
    },
    License {
        name: || tr!(META, "CC0-1.0 (no restrictions)"),
        text: Some(share_file!("licenses/cc0-1.0.txt")),
    },
    License {
        name: || tr!(META, "CC-BY-4.0 (requires attribution)"),
        text: Some(share_file!("licenses/cc-by-4.0.txt")),
    },
    License {
        name: || tr!(META, "CC-BY-SA-4.0 (requires attribution + share alike)"),
        text: Some(share_file!("licenses/cc-by-sa-4.0.txt")),
    },
    License {
        name: || tr!(META, "CC-BY-NC-4.0 (requires attribution + non commercial)"),
        text: Some(share_file!("licenses/cc-by-nc-4.0.txt")),
    },
    License {
        name: || {
            tr!(
                META,
                "CC-BY-NC-SA-4.0 (requires attribution + non commercial + share alike)"
            )
        },
        text: Some(share_file!("licenses/cc-by-nc-sa-4.0.txt")),
    },
    License {
        name: || {
            tr!(
                META,
                "CC-BY-NC-ND-4.0 (requires attribution + non commercial + no derivatives)"
            )
        },
        text: Some(share_file!("licenses/cc-by-nc-nd-4.0.txt")),
    },
    License {
        name: || tr!(META, "CC-BY-ND-4.0 (requires attribution + no derivatives)"),
        text: Some(share_file!("licenses/cc-by-nd-4.0.txt")),
    },
    License {
        name: || tr!(META, "TAPR-OHL-1.0"),
        text: Some(share_file!("licenses/tapr-ohl-1.0.txt")),
    },
    License {
        name: || tr!(META, "CERN-OHL-P-2.0 (permissive)"),
        text: Some(share_file!("licenses/cern-ohl-p-2.0.txt")),
    },
    License {
        name: || tr!(META, "CERN-OHL-W-2.0 (weakly reciprocal)"),
        text: Some(share_file!("licenses/cern-ohl-w-2.0.txt")),
    },
    License {
        name: || tr!(META, "CERN-OHL-S-2.0 (strongly reciprocal)"),
        text: Some(share_file!("licenses/cern-ohl-s-2.0.txt")),
    },
];

/// Upstream `NewProjectWizard::Mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewProjectMode {
    /// A new, empty project (metadata and initialization pages).
    NewProject,
    /// Import an EAGLE project (EAGLE import and metadata pages).
    EagleImport,
}

/// The pages of the wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    EagleImport,
    Metadata,
    Initialization,
}

/// Everything [`create_project()`] needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProjectOptions {
    /// The project name.
    pub name: String,
    /// The author.
    pub author: String,
    /// Index into the offered licenses (0: none).
    pub license: usize,
    /// The project file (`<dir>/<name>.lpp`).
    pub file: FilePath,
    /// The name of the schematic to add (`None`: no schematic).
    pub schematic: Option<String>,
    /// The name of the board to add (`None`: no board).
    pub board: Option<String>,
    /// Library locale order of the project (from the workspace settings).
    pub locale_order: Vec<String>,
    /// Library norm order of the project (from the workspace settings).
    pub norm_order: Vec<String>,
}

/// Directory name of a schematic or board (upstream
/// `FilePath::cleanFileName(name, ReplaceSpaces | ToLowerCase)`).
pub fn document_dir_name(name: &str) -> String {
    FilePath::clean_file_name(
        name,
        CleanFileNameOptions::new(true, FileNameCase::Lower),
        120,
    )
}

/// The project file for a name in `location` (upstream
/// `NewProjectWizardPage_Metadata::nameChanged()`):
/// `<location>/<name>/<name>.lpp` with a cleaned file name.
pub fn project_file_for(location: &FilePath, name: &str) -> FilePath {
    let mut fname = FilePath::clean_file_name(
        name,
        CleanFileNameOptions::new(true, FileNameCase::Keep),
        120,
    );
    if fname.is_empty() {
        fname = "project".to_owned();
    }
    location.path_to(&format!("{fname}/{fname}.lpp"))
}

/// Validates the project file path of the metadata page (upstream
/// `NewProjectWizardPage_Metadata::isComplete()`); returns the error
/// message.
pub fn validate_project_file(path: &str) -> Result<FilePath, String> {
    let fp = FilePath::new(path.trim())
        .filter(|fp| fp.suffix() == "lpp" && !fp.basename().is_empty())
        .ok_or_else(|| {
            tr!(
                META,
                "Please enter a valid project path with '{0}' file extension.",
                ".lpp"
            )
        })?;
    let parent = fp
        .parent_dir()
        .ok_or_else(|| tr!(META, "The selected directory is not empty."))?;
    if (parent.is_existing_dir() && !parent.is_empty_dir()) || parent.is_existing_file() {
        return Err(tr!(META, "The selected directory is not empty."));
    }
    Ok(fp)
}

/// Creates the project files (upstream `NewProjectWizard::createProject()`)
/// and returns the messages of the EAGLE import. The directory is removed
/// again if creating the project fails and it did not exist before.
pub fn create_project(
    options: &NewProjectOptions,
    eagle: Option<&EagleProjectImport>,
) -> Result<Vec<String>, String> {
    let dir = options
        .file
        .parent_dir()
        .ok_or_else(|| tr!(META, "Invalid filepath"))?;
    let existed = dir.is_existing_dir();
    let result = create_project_impl(options, &dir, eagle);
    if result.is_err()
        && !existed
        && let Err(e) = std::fs::remove_dir_all(dir.as_path())
    {
        log::warn!("Failed to remove {}: {e}", dir.to_native());
    }
    result
}

fn create_project_impl(
    options: &NewProjectOptions,
    dir: &FilePath,
    eagle: Option<&EagleProjectImport>,
) -> Result<Vec<String>, String> {
    let err = |e: &dyn std::fmt::Display| e.to_string();
    let fs = Arc::new(TransactionalFileSystem::open_rw(dir).map_err(|e| err(&e))?);
    let mut root = TransactionalDirectory::new(Arc::clone(&fs), "");
    // Stroke fonts (upstream `Project::create()` copies the application's
    // fontobene fonts into the project).
    let (fonts, _) = librepcb_scene::resources::stroke_font_files().map_err(|e| err(&e))?;
    for (file, content) in fonts {
        root.write(&format!("resources/fontobene/{file}"), &content)
            .map_err(|e| err(&e))?;
    }
    let mut project =
        Project::create(root, options.file.file_name(), Uuid::new_random).map_err(|e| err(&e))?;

    // Metadata and settings.
    let mut metadata = project.metadata().clone();
    metadata.name = ElementName::new(ElementName::clean(&options.name)).map_err(|e| err(&e))?;
    metadata.author = options.author.clone();
    project
        .apply(Mutation::SetProjectMetadata(metadata))
        .map_err(|e| err(&e))?;
    let mut settings = project.settings().clone();
    settings.locale_order = options.locale_order.clone();
    settings.norm_order = options.norm_order.clone();
    project
        .apply(Mutation::SetProjectSettings(settings))
        .map_err(|e| err(&e))?;

    // Schematic and board.
    if eagle.is_none() {
        if let Some(name) = &options.schematic {
            let name = ElementName::new(name.trim()).map_err(|e| err(&e))?;
            let dir = document_dir_name(name.as_str());
            project
                .add_schematic(Schematic::new(Uuid::new_random(), name, dir), None)
                .map_err(|e| err(&e))?;
        }
        if let Some(name) = &options.board {
            let name = ElementName::new(name.trim()).map_err(|e| err(&e))?;
            let dir = document_dir_name(name.as_str());
            let board = project
                .add_board(Board::new(Uuid::new_random(), name, dir), None)
                .map_err(|e| err(&e))?;
            // Upstream `Board::addDefaultContent()`: 100x80mm board outline
            // (1/2 Eurocard size).
            project
                .add_board_item(
                    board,
                    BoardItem::Polygon(BoardPolygonData::new(
                        Uuid::new_random(),
                        Layer::BOARD_OUTLINES,
                        UnsignedLength::new(Length::new(0)).map_err(|e| err(&e))?,
                        Path::rect(
                            Point::from_nm(0, 0),
                            Point::from_nm(100_000_000, 80_000_000),
                        ),
                        false,
                        false,
                        false,
                    )),
                )
                .map_err(|e| err(&e))?;
        }
    }

    // License, readme, .gitignore and .gitattributes.
    let license = LICENSES.get(options.license).and_then(|l| l.text);
    if let Some(text) = license
        && let Err(e) = fs.write("LICENSE.txt", text)
    {
        log::error!("Could not copy the license file: {e}");
    }
    let readme = String::from_utf8_lossy(README_TEMPLATE)
        .replace("{PROJECT_NAME}", &options.name)
        .replace(
            "{LICENSE_TEXT}",
            if license.is_some() {
                "See [LICENSE.txt](LICENSE.txt)."
            } else {
                "No license set."
            },
        );
    for (file, content) in [
        ("README.md", readme.as_bytes()),
        (".gitignore", GITIGNORE_TEMPLATE),
        (".gitattributes", GITATTRIBUTES_TEMPLATE),
    ] {
        if let Err(e) = fs.write(file, content) {
            log::error!("Could not copy the {file} file: {e}");
        }
    }

    // EAGLE import.
    let mut messages = Vec::new();
    if let Some(import) = eagle {
        let log = MessageLogger::new();
        let result = import.import(&mut project, &log);
        messages.extend(
            log.messages()
                .into_iter()
                .filter(|m| m.level >= LogLevel::Info)
                .map(|m| format!("{}: {}", m.level, m.message)),
        );
        if let Err(e) = result {
            return Err(format!("{}\n\n{e}", tr!(WIZARD, "EAGLE import failed:")));
        }
    }

    project.save().map_err(|e| err(&e))?;
    fs.save().map_err(|e| err(&e))?;
    drop(project);
    fs.release_lock().map_err(|e| err(&e))?;
    Ok(messages)
}

/// The new project wizard (upstream `NewProjectWizard`).
pub struct NewProjectWizard {
    form: Form,
    mode: NewProjectMode,
    pages: Vec<Page>,
    page: usize,
    // Metadata page.
    name: String,
    author: String,
    license: usize,
    location: FilePath,
    path: String,
    // Initialization page.
    add_schematic: bool,
    schematic_name: String,
    add_board: bool,
    board_name: String,
    // EAGLE import page.
    eagle_schematic: String,
    eagle_board: String,
    eagle_parsed: (String, String),
    eagle: Option<EagleProjectImport>,
    eagle_messages: Vec<String>,
    // Workspace settings.
    locale_order: Vec<String>,
    norm_order: Vec<String>,
}

impl NewProjectWizard {
    /// Creates the wizard: new projects are created in `location` (upstream:
    /// the workspace's projects directory, or the folder of the home tab
    /// the wizard was opened from, `setLocationOverride()`).
    pub fn new(
        mode: NewProjectMode,
        location: FilePath,
        author: &str,
        locale_order: Vec<String>,
        norm_order: Vec<String>,
    ) -> Self {
        let pages = match mode {
            NewProjectMode::NewProject => vec![Page::Metadata, Page::Initialization],
            NewProjectMode::EagleImport => vec![Page::EagleImport, Page::Metadata],
        };
        let mut wizard = Self {
            form: Form::new(LengthUnit::Millimeters),
            mode,
            pages,
            page: 0,
            name: String::new(),
            author: author.to_owned(),
            license: 0,
            location,
            path: String::new(),
            add_schematic: true,
            // Do not translate these names into other languages!
            schematic_name: "Main".to_owned(),
            add_board: true,
            board_name: "default".to_owned(),
            eagle_schematic: String::new(),
            eagle_board: String::new(),
            eagle_parsed: (String::new(), String::new()),
            eagle: None,
            eagle_messages: Vec::new(),
            locale_order,
            norm_order,
        };
        wizard.build();
        wizard
    }

    /// The wizard's mode.
    pub fn mode(&self) -> NewProjectMode {
        self.mode
    }

    /// The index of the current page.
    pub fn page_index(&self) -> usize {
        self.page
    }

    /// Goes to the next page if the current one is complete (like the
    /// "Next" button).
    pub fn next_page(&mut self) -> Result<(), String> {
        self.store();
        self.validate_page(self.pages[self.page])?;
        if self.page + 1 < self.pages.len() {
            self.page += 1;
            self.build();
        }
        Ok(())
    }

    /// Goes to the previous page (like the "Back" button).
    pub fn previous_page(&mut self) {
        self.store();
        if self.page > 0 {
            self.page -= 1;
            self.build();
        }
    }

    /// The options for [`create_project()`] if all pages are complete.
    pub fn project_options(&mut self) -> Result<NewProjectOptions, String> {
        self.store();
        for page in self.pages.clone() {
            self.validate_page(page)?;
        }
        let file = validate_project_file(&self.path)?;
        let with_init = self.pages.contains(&Page::Initialization);
        Ok(NewProjectOptions {
            name: ElementName::clean(&self.name),
            author: self.author.clone(),
            license: self.license,
            file,
            schematic: (with_init && self.add_schematic).then(|| self.schematic_name.clone()),
            board: (with_init && self.add_board).then(|| self.board_name.clone()),
            locale_order: self.locale_order.clone(),
            norm_order: self.norm_order.clone(),
        })
    }

    fn validate_page(&mut self, page: Page) -> Result<(), String> {
        match page {
            Page::EagleImport => {
                self.parse_eagle();
                if self
                    .eagle
                    .as_ref()
                    .is_some_and(EagleProjectImport::is_ready)
                {
                    Ok(())
                } else {
                    Err(tr!(EAGLE, "Invalid file path(s)."))
                }
            }
            Page::Metadata => {
                if ElementName::clean(&self.name).is_empty() {
                    return Err(tr!(META, "Please enter a project name"));
                }
                validate_project_file(&self.path).map(|_| ())
            }
            Page::Initialization => {
                if self.add_schematic && document_dir_name(&self.schematic_name).is_empty()
                    || self.add_board && document_dir_name(&self.board_name).is_empty()
                {
                    Err(tr!(INIT, "Invalid name!"))
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Reads the fields of the current page into the wizard's state.
    fn store(&mut self) {
        let f = &self.form;
        match self.pages[self.page] {
            Page::EagleImport => {
                self.eagle_schematic = f.get_text("eagle_schematic");
                self.eagle_board = f.get_text("eagle_board");
            }
            Page::Metadata => {
                self.name = f.get_text("name");
                self.author = f.get_text("author");
                self.license = f.get_index("license").unwrap_or(0);
                self.path = f.get_text("path");
            }
            Page::Initialization => {
                self.add_schematic = f.get_checked("add_schematic");
                self.schematic_name = f.get_text("schematic_name");
                self.add_board = f.get_checked("add_board");
                self.board_name = f.get_text("board_name");
            }
        }
    }

    /// Builds the form of the current page.
    fn build(&mut self) {
        self.form.clear();
        match self.pages[self.page] {
            Page::EagleImport => {
                self.form
                    .header(tr!(EAGLE, "Select EAGLE (v6 or later) project files"))
                    .note(
                        "subtitle",
                        tr!(
                            EAGLE,
                            "Please choose the EAGLE schematic and optionally board files to import."
                        ),
                    )
                    .text("eagle_schematic", "", &self.eagle_schematic)
                    .button(
                        "browse_schematic",
                        "",
                        tr!(EAGLE, "Select EAGLE schematic file"),
                    )
                    .text("eagle_board", "", &self.eagle_board)
                    .button(
                        "browse_board",
                        "",
                        tr!(EAGLE, "Select EAGLE board file (optional)"),
                    )
                    .note("messages", "")
                    .note(
                        "note",
                        tr!(
                            EAGLE,
                            "Note that due to conceptual differences between EDA tools, migrations won't be perfect and you should review the result carefully. Whenever possible, creating a new project from scratch is preferred."
                        ),
                    );
                self.update_eagle_messages();
            }
            Page::Metadata => {
                let licenses: Vec<String> = LICENSES.iter().map(|l| (l.name)()).collect();
                self.form
                    .header(tr!(META, "Project Metadata"))
                    .note(
                        "subtitle",
                        tr!(META, "Specify some metadata of the project to be created."),
                    )
                    .text("name", tr!(META, "Name:"), &self.name)
                    .text("author", tr!(META, "Author:"), &self.author)
                    .choice(
                        "license",
                        tr!(META, "License:"),
                        &licenses,
                        Some(self.license),
                    )
                    .text("path", tr!(META, "Path:"), &self.path)
                    .button(
                        "browse_location",
                        "",
                        tr!(META, "Project's parent directory"),
                    )
                    .note(
                        "hint",
                        tr!(
                            META,
                            "A LibrePCB project consists of a whole directory, not only of a single file. Just select the new project's parent directory, and the subdirectory and filename will be appended automatically."
                        ),
                    )
                    .note("status", "");
                self.name_changed();
            }
            Page::Initialization => {
                self.form
                    .header(tr!(INIT, "Initialization"))
                    .note(
                        "subtitle",
                        tr!(INIT, "Specify how the project should be initialized."),
                    )
                    .checkbox(
                        "add_schematic",
                        "",
                        tr!(INIT, "Add Sche&matic").replace('&', ""),
                        self.add_schematic,
                    )
                    .text("schematic_name", tr!(INIT, "Name:"), &self.schematic_name)
                    .label("schematic_dir", tr!(INIT, "Directory:"), "")
                    .checkbox(
                        "add_board",
                        "",
                        tr!(INIT, "Add B&oard").replace('&', ""),
                        self.add_board,
                    )
                    .text("board_name", tr!(INIT, "Name:"), &self.board_name)
                    .label("board_dir", tr!(INIT, "Directory:"), "");
                self.initialization_changed();
            }
        }
    }

    /// Upstream `NewProjectWizardPage_Metadata::nameChanged()`: proposes
    /// the project path for the name.
    fn name_changed(&mut self) {
        let name = self.form.get_text("name");
        if ElementName::clean(&name).is_empty() {
            self.path.clear();
            self.form.set_text("path", "");
            self.form
                .set_hint("path", tr!(META, "Please enter a project name"));
            self.form.set_enabled("path", false);
        } else {
            self.path = project_file_for(&self.location, &name).to_native();
            self.form.set_text("path", &self.path);
            self.form.set_hint("path", "");
            self.form.set_enabled("path", true);
        }
        self.metadata_status();
    }

    /// Upstream `pathChanged()` and `isComplete()`: the location follows a
    /// manually entered path, the status shows why the page is incomplete.
    fn path_changed(&mut self) {
        self.path = self.form.get_text("path");
        if let Some(fp) = FilePath::new(self.path.trim())
            && fp.suffix() == "lpp"
            && let Some(location) = fp.parent_dir().and_then(|d| d.parent_dir())
        {
            self.location = location;
        }
        self.metadata_status();
    }

    fn metadata_status(&mut self) {
        let status = if ElementName::clean(&self.form.get_text("name")).is_empty() {
            String::new()
        } else {
            match validate_project_file(&self.path) {
                Ok(_) => String::new(),
                Err(e) => format!("⚠ {e}"),
            }
        };
        self.form.set_text("status", status);
    }

    /// Updates the directory labels of the initialization page.
    fn initialization_changed(&mut self) {
        for (check, name, label, prefix) in [
            (
                "add_schematic",
                "schematic_name",
                "schematic_dir",
                "schematics",
            ),
            ("add_board", "board_name", "board_dir", "boards"),
        ] {
            let enabled = self.form.get_checked(check);
            let dir = document_dir_name(&self.form.get_text(name));
            let text = if dir.is_empty() {
                tr!(INIT, "Invalid name!")
            } else {
                format!("{prefix}/{dir}/")
            };
            self.form.set_text(label, text);
            self.form.set_enabled(name, enabled);
            self.form.set_enabled(label, enabled);
        }
    }

    /// Parses the EAGLE files if the paths changed (upstream
    /// `NewProjectWizardPage_EagleImport::updateStatus()`).
    fn parse_eagle(&mut self) {
        let (sch, brd) = (
            self.eagle_schematic.trim().to_owned(),
            self.eagle_board.trim().to_owned(),
        );
        if (sch.clone(), brd.clone()) == self.eagle_parsed {
            return;
        }
        self.eagle_parsed = (sch.clone(), brd.clone());
        self.eagle = None;
        self.eagle_messages.clear();
        if sch.is_empty() {
            return;
        }
        let sch_fp = FilePath::new(&sch);
        let brd_fp = FilePath::new(&brd);
        let (Some(sch_fp), true) = (sch_fp, brd_fp.is_some() || brd.is_empty()) else {
            self.eagle_messages
                .push(format!("⚠ {}", tr!(EAGLE, "Invalid file path(s).")));
            return;
        };
        let mut import = EagleProjectImport::default();
        match import.open(&sch_fp, brd_fp.as_ref()) {
            Ok(warnings) => {
                self.eagle_messages
                    .extend(warnings.into_iter().map(|w| format!("➤ {w}")));
                let msg = if import.has_board() {
                    trn!(
                        EAGLE,
                        "Ready to import {n} sheet(s) and a board.",
                        "Ready to import {n} sheet(s) and a board.",
                        import.sheet_count()
                    )
                } else {
                    trn!(
                        EAGLE,
                        "Ready to import {n} sheet(s) without board.",
                        "Ready to import {n} sheet(s) without board.",
                        import.sheet_count()
                    )
                };
                self.eagle_messages.push(format!("✔ {msg}"));
                // Preselect the project name for the metadata page.
                self.name = import.project_name().to_owned();
                self.eagle = Some(import);
            }
            Err(e) => {
                self.eagle_messages
                    .push(format!("⚠ {} {e}", tr!(EAGLE, "ERROR:")));
            }
        }
    }

    fn update_eagle_messages(&mut self) {
        self.parse_eagle();
        self.form
            .set_text("messages", self.eagle_messages.join("\n"));
        let valid = FilePath::new(self.eagle_schematic.trim()).is_some();
        self.form.set_enabled("eagle_board", valid);
        self.form.set_enabled("browse_board", valid);
    }

    fn browse(&mut self, id: &str) {
        match id {
            "browse_location" => {
                if let Some(dir) = file_dialog::choose_directory(
                    &tr!(META, "Project's parent directory"),
                    Some(self.location.as_path()),
                ) && let Some(fp) = crate::app::absolute_file_path(&dir)
                {
                    self.location = fp;
                    self.name_changed();
                }
            }
            "browse_schematic" | "browse_board" => {
                let schematic = id == "browse_schematic";
                let (title, ext) = if schematic {
                    (tr!(EAGLE, "Select EAGLE Schematic"), "sch")
                } else {
                    (tr!(EAGLE, "Select EAGLE Board"), "brd")
                };
                let filter = file_dialog::Filter {
                    name: format!("*.{ext}"),
                    extensions: vec![ext],
                };
                if let Some(path) = file_dialog::open_file(&title, &[filter], None) {
                    let path = path.to_string_lossy().into_owned();
                    if schematic {
                        // Import the board too, if there is one.
                        let brd = format!("{}.brd", path.strip_suffix(".sch").unwrap_or(&path));
                        self.eagle_board =
                            if FilePath::new(&brd).is_some_and(|f| f.is_existing_file()) {
                                brd
                            } else {
                                String::new()
                            };
                        self.eagle_schematic = path;
                        self.form.set_text("eagle_board", &self.eagle_board);
                    } else {
                        self.eagle_board = path;
                    }
                    self.form.set_text("eagle_schematic", &self.eagle_schematic);
                    self.form.set_text("eagle_board", &self.eagle_board);
                    self.update_eagle_messages();
                }
            }
            _ => {}
        }
    }
}

impl FormDialog for NewProjectWizard {
    fn title(&self) -> String {
        tr!(WIZARD, "Create New Project")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        let last = self.page + 1 == self.pages.len();
        let mut extra_buttons = Vec::new();
        if self.page > 0 {
            extra_buttons.push(tr!("DeviceContentTab", "Back"));
        }
        if !last {
            extra_buttons.push(tr!("DeviceContentTab", "Next"));
        }
        DialogOptions {
            apply: false,
            ok_text: Some(tr!("DeviceContentTab", "Finish")),
            extra_buttons,
            width: 650.0,
            label_width: 130.0,
            ..DialogOptions::default()
        }
    }

    fn field_event(&mut self, _ctx: &DialogContext<'_>, id: &str, event: FieldEvent) {
        match (id, event) {
            (_, FieldEvent::Clicked) => self.browse(id),
            ("name", _) => {
                self.name = self.form.get_text("name");
                self.name_changed();
            }
            ("path", _) => self.path_changed(),
            ("eagle_schematic" | "eagle_board", _) => {
                self.store();
                self.update_eagle_messages();
            }
            ("add_schematic" | "add_board" | "schematic_name" | "board_name", _) => {
                self.initialization_changed();
            }
            _ => {}
        }
    }

    fn button(&mut self, _ctx: &DialogContext<'_>, index: usize) -> Result<ButtonResult, String> {
        if index == 0 && self.page > 0 {
            self.previous_page();
        } else {
            self.next_page()?;
        }
        Ok(ButtonResult::Keep)
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let options = self.project_options()?;
        let messages = create_project(&options, self.eagle.as_ref()).map_err(|e| {
            format!(
                "{}\n\n{e}",
                tr!(
                    "librepcb::editor::GuiApplication",
                    "Could not create project"
                )
            )
        })?;
        Ok(Applied::App(AppRequest::OpenProject {
            path: options.file,
            import_messages: messages,
        }))
    }
}
