//! The initialize workspace wizard (first run, switching the workspace).
//!
//! Port of libs/librepcb/editor/workspace/initializeworkspacewizard/
//! (`initializeworkspacewizard.{h,cpp,ui}`, the context and the pages
//! welcome, choose workspace, upgrade and choose settings): chooses the
//! workspace directory (opening an existing one or creating a new one),
//! upgrades an outdated data directory by copying it, and initializes new
//! data directories with the most important settings (language, length
//! unit, preferred norm, user name).
//!
//! Differences to upstream (see COMPAT.md): the data directory is copied
//! synchronously (no progress bar); the example projects are not downloaded
//! when creating a workspace (they can be imported later).

use librepcb_core::fileio::{FilePath, file_utils};
use librepcb_core::types::LengthUnit;
use librepcb_core::workspace::Workspace;
use librepcb_i18n::tr;

use super::{
    AppRequest, Applied, ButtonResult, DialogContext, DialogOptions, FieldEvent, Form, FormDialog,
};
use crate::file_dialog;

const WIZARD: &str = "librepcb::editor::InitializeWorkspaceWizard";
const WELCOME: &str = "librepcb::editor::InitializeWorkspaceWizard_Welcome";
const CHOOSE: &str = "librepcb::editor::InitializeWorkspaceWizard_ChooseWorkspace";
const UPGRADE: &str = "librepcb::editor::InitializeWorkspaceWizard_Upgrade";
const SETTINGS: &str = "librepcb::editor::InitializeWorkspaceWizard_ChooseSettings";

/// The norms offered for the library norm order (upstream
/// `getAvailableNorms()`).
const NORMS: [&str; 2] = ["IEC 60617", "IEEE 315"];

/// The pages (upstream `InitializeWorkspaceWizardContext::PageId`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Page {
    /// No page needs to be shown (the workspace is ready).
    None,
    /// Welcome page (no workspace chosen yet).
    Welcome,
    /// Choose the workspace directory.
    ChooseWorkspace,
    /// Upgrade the data directory.
    Upgrade,
    /// Initialize a new data directory with settings.
    ChooseSettings,
}

/// The state of the wizard (upstream `InitializeWorkspaceWizardContext`).
#[derive(Debug, Clone, Default)]
pub struct WorkspaceStatus {
    /// The workspace path, if valid.
    pub path: Option<FilePath>,
    /// Whether the path can be used (existing workspace or empty/new
    /// directory).
    pub valid: bool,
    /// Whether a compatible workspace exists at the path.
    pub exists: bool,
    /// The data directory to open.
    pub data_dir: String,
    /// The data directories of the workspace.
    pub data_dirs: Vec<String>,
    /// The copy for upgrading or importing the data directory.
    pub upgrade: Option<(String, String)>,
}

impl WorkspaceStatus {
    /// Upstream `setWorkspacePath()`.
    pub fn of(path: Option<FilePath>) -> Self {
        let mut s = Self {
            path: path.clone(),
            ..Self::default()
        };
        let mut dirs = std::collections::BTreeMap::new();
        if let Some(fp) = &path {
            if Workspace::check_compatibility(fp).is_ok() {
                match Workspace::find_data_directories(fp) {
                    Ok(d) => {
                        dirs = d;
                        s.valid = true;
                        s.exists = true;
                    }
                    Err(e) => log::warn!("{e}"),
                }
            } else if (!fp.is_existing_dir() && !fp.is_existing_file()) || fp.is_empty_dir() {
                s.valid = true;
            }
        }
        let choice = Workspace::determine_data_directory(&dirs);
        s.data_dir = choice.dir;
        s.upgrade = choice.copy;
        s.data_dirs = dirs.into_keys().collect();
        s
    }

    /// Upstream `getNeedsInitialization()`.
    pub fn needs_initialization(&self) -> bool {
        !self.data_dirs.contains(&self.data_dir)
    }

    /// Upstream `getNeedsUpgrade()`.
    pub fn needs_upgrade(&self) -> bool {
        self.upgrade.is_some()
    }

    /// The page to start with (upstream `updateStartPage()`), `Page::None`
    /// if the workspace can be opened without the wizard.
    pub fn start_page(&self, force_choose_path: bool) -> Page {
        if self.path.is_none() && !force_choose_path {
            Page::Welcome
        } else if force_choose_path || !self.exists {
            Page::ChooseWorkspace
        } else if self.needs_upgrade() {
            Page::Upgrade
        } else if self.needs_initialization() {
            Page::ChooseSettings
        } else {
            Page::None
        }
    }

    /// The page after "choose workspace" (upstream `nextId()`).
    fn next_after_choose(&self) -> Page {
        if self.needs_upgrade() {
            Page::Upgrade
        } else if self.needs_initialization() {
            Page::ChooseSettings
        } else {
            Page::None
        }
    }
}

/// The default workspace path (upstream: `LIBREPCB_DEFAULT_WORKSPACE_PATH`
/// or `~/LibrePCB-Workspace`).
pub fn default_workspace_path() -> Option<FilePath> {
    std::env::var_os("LIBREPCB_DEFAULT_WORKSPACE_PATH")
        .and_then(FilePath::new)
        .or_else(|| std::env::home_dir().and_then(|h| FilePath::new(h.join("LibrePCB-Workspace"))))
}

/// The wizard (upstream `InitializeWorkspaceWizard`).
pub struct InitializeWorkspaceWizard {
    form: Form,
    status: WorkspaceStatus,
    force_choose_path: bool,
    page: Page,
    history: Vec<Page>,
    upgrade_error: Option<String>,
}

impl InitializeWorkspaceWizard {
    /// Creates the wizard for a workspace path (upstream constructor and
    /// `setWorkspacePath()`); `force_choose_path` starts with choosing the
    /// path (switching the workspace).
    pub fn new(path: Option<FilePath>, force_choose_path: bool) -> Self {
        let status = WorkspaceStatus::of(path);
        let page = status.start_page(force_choose_path);
        let mut wizard = Self {
            form: Form::new(LengthUnit::Millimeters),
            status,
            force_choose_path,
            page,
            history: Vec::new(),
            upgrade_error: None,
        };
        wizard.build();
        wizard
    }

    /// Whether the wizard needs to be shown (upstream
    /// `getNeedsToBeShown()`).
    pub fn needs_to_be_shown(&self) -> bool {
        self.page != Page::None
    }

    /// The current page.
    pub fn page(&self) -> Page {
        self.page
    }

    /// The chosen workspace path.
    pub fn workspace_path(&self) -> Option<&FilePath> {
        self.status.path.as_ref()
    }

    /// The workspace state.
    pub fn status(&self) -> &WorkspaceStatus {
        &self.status
    }

    fn is_last_page(&self) -> bool {
        match self.page {
            Page::Welcome => false,
            Page::ChooseWorkspace => self.status.next_after_choose() == Page::None,
            _ => true,
        }
    }

    fn build(&mut self) {
        self.form.clear();
        match self.page {
            Page::None => {}
            Page::Welcome => {
                self.form
                    .header(tr!(WELCOME, "Welcome to LibrePCB"))
                    .note(
                        "subtitle",
                        tr!(
                            WELCOME,
                            "This wizard will help you to open or create a LibrePCB workspace."
                        ),
                    )
                    .note(
                        "links",
                        "Website: https://librepcb.org\nGitHub Project: https://github.com/LibrePCB\nLibrePCB is published under the GNU GPLv3 License.",
                    );
            }
            Page::ChooseWorkspace => {
                let path = self
                    .status
                    .path
                    .clone()
                    .or_else(default_workspace_path)
                    .map(|p| p.to_native())
                    .unwrap_or_default();
                self.form
                    .header(tr!(CHOOSE, "Select Workspace Path"))
                    .note(
                        "subtitle",
                        tr!(
                            CHOOSE,
                            "Please select a directory to open or create a LibrePCB workspace."
                        ),
                    )
                    .note(
                        "path_label",
                        tr!(CHOOSE, "Choose the workspace directory to open or create:"),
                    )
                    .text("path", "", &path)
                    .button("browse", "", tr!("EditorCommandSet", "Browse"))
                    .note("status", "");
                self.path_changed();
            }
            Page::Upgrade => {
                let (src, dst) = self.status.upgrade.clone().unwrap_or_default();
                let ws = self.status.path.clone();
                let abs = |d: &str| {
                    ws.as_ref()
                        .map(|w| w.path_to(d).to_native())
                        .unwrap_or_default()
                };
                self.form
                    .header(tr!(UPGRADE, "Upgrade Workspace"))
                    .note(
                        "subtitle",
                        tr!(UPGRADE, "Upgrade the workspace to the latest file format."),
                    )
                    .note(
                        "title",
                        tr!(
                            UPGRADE,
                            "Upgrade to LibrePCB {0}",
                            format!("{}.x", librepcb_core::application::file_format_version())
                        ),
                    )
                    .note(
                        "info",
                        tr!(
                            UPGRADE,
                            "The workspace data directory will be copied to a new directory for the current file format. The old directory is kept, so older LibrePCB versions can still use it."
                        ),
                    )
                    .label("source", tr!(UPGRADE, "Source:"), abs(&src))
                    .label("destination", tr!(UPGRADE, "Destination:"), abs(&dst));
                if let Some(e) = &self.upgrade_error {
                    self.form.note(
                        "error",
                        format!(
                            "{} {e}\n\n{}",
                            tr!(UPGRADE, "Error:"),
                            tr!(
                                UPGRADE,
                                "If the error persists, you could try to copy the mentioned directory manually (e.g. with your file manager)."
                            )
                        ),
                    );
                }
            }
            Page::ChooseSettings => {
                let mut languages = vec![tr!(SETTINGS, "System Language")];
                let mut locales: Vec<String> = librepcb_i18n::available_languages()
                    .iter()
                    .map(|l| (*l).to_owned())
                    .collect();
                locales.sort();
                languages.extend(locales);
                let units: Vec<String> = LengthUnit::ALL.iter().map(|u| u.to_string_tr()).collect();
                let mut norms = vec![tr!(SETTINGS, "None")];
                norms.extend(NORMS.iter().map(|n| (*n).to_owned()));
                self.form
                    .header(tr!(SETTINGS, "Choose Settings"))
                    .note(
                        "subtitle",
                        tr!(SETTINGS, "Set the most important workspace settings."),
                    )
                    .choice("language", tr!(SETTINGS, "Language:"), &languages, Some(0))
                    .choice(
                        "length_unit",
                        tr!(SETTINGS, "Length Unit:"),
                        &units,
                        LengthUnit::ALL
                            .iter()
                            .position(|u| *u == LengthUnit::Millimeters),
                    )
                    .choice("norm", tr!(SETTINGS, "Preferred Norm:"), &norms, Some(0))
                    .text(
                        "user_name",
                        tr!(SETTINGS, "User Name:"),
                        librepcb_core::system_info::full_username(),
                    )
                    .note(
                        "user_name_hint",
                        tr!(
                            SETTINGS,
                            "This name will be used as author when creating new projects or libraries."
                        ),
                    );
            }
        }
    }

    /// Upstream `ChooseWorkspace::updateWorkspacePath()`.
    fn path_changed(&mut self) {
        let text = self.form.get_text("path");
        let path = crate::app::absolute_file_path(std::path::Path::new(text.trim()))
            .filter(|_| !text.trim().is_empty());
        self.status = WorkspaceStatus::of(path);
        let message = if self.status.path.is_none() {
            format!("⚠ {}", tr!(CHOOSE, "Please select a directory."))
        } else if self.status.exists {
            format!("✔ {}", tr!(CHOOSE, "Directory contains a valid workspace."))
        } else if self.status.valid {
            format!("➤ {}", tr!(CHOOSE, "New workspace will be created."))
        } else {
            format!("⚠ {}", tr!(CHOOSE, "Directory is not empty!"))
        };
        self.form.set_text("status", message);
    }

    /// Goes to the next page ("Next"); errors are shown by the dialog.
    pub fn next_page(&mut self) -> Result<(), String> {
        let next = match self.page {
            Page::Welcome => Page::ChooseWorkspace,
            Page::ChooseWorkspace => {
                if !self.status.valid {
                    return Err(tr!(CHOOSE, "Directory is not empty!"));
                }
                self.status.next_after_choose()
            }
            _ => Page::None,
        };
        if next != Page::None {
            self.history.push(self.page);
            self.page = next;
            self.build();
        }
        Ok(())
    }

    /// Goes back ("Back").
    pub fn previous_page(&mut self) {
        if let Some(page) = self.history.pop() {
            self.page = page;
            self.build();
        }
    }

    /// Finishes the wizard on the current page (upstream `validatePage()`
    /// of the last page and `accept()`): upgrades or initializes the
    /// workspace; returns its path.
    pub fn finish(&mut self) -> Result<FilePath, String> {
        let path = self
            .status
            .path
            .clone()
            .ok_or_else(|| tr!(CHOOSE, "Please select a directory."))?;
        match self.page {
            Page::Welcome => return Err(tr!(CHOOSE, "Please select a directory.")),
            Page::ChooseWorkspace => {
                if !self.status.valid {
                    return Err(tr!(CHOOSE, "Directory is not empty!"));
                }
                if self.status.next_after_choose() != Page::None {
                    // Continue with the upgrade or the settings.
                    self.next_page()?;
                    return Err(String::new());
                }
            }
            Page::Upgrade => {
                if let Some((src, dst)) = self.status.upgrade.clone() {
                    let result =
                        file_utils::copy_dir_recursively(&path.path_to(&src), &path.path_to(&dst));
                    if let Err(e) = result {
                        self.upgrade_error = Some(e.to_string());
                        self.build();
                        return Err(e.to_string());
                    }
                }
            }
            Page::ChooseSettings => self.initialize_workspace(&path)?,
            Page::None => {}
        }
        Ok(path)
    }

    /// Upstream `InitializeWorkspaceWizardContext::initializeEmptyWorkspace()`.
    fn initialize_workspace(&self, path: &FilePath) -> Result<(), String> {
        let f = &self.form;
        let locale = f
            .get_index("language")
            .filter(|i| *i > 0)
            .map(|i| {
                let mut locales: Vec<&str> = librepcb_i18n::available_languages().to_vec();
                locales.sort_unstable();
                locales
                    .get(i - 1)
                    .map(|l| (*l).to_owned())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let unit = f
            .get_index("length_unit")
            .and_then(|i| LengthUnit::ALL.get(i).copied())
            .unwrap_or(LengthUnit::Millimeters);
        let norms: Vec<String> = f
            .get_index("norm")
            .filter(|i| *i > 0)
            .and_then(|i| NORMS.get(i - 1))
            .map(|n| vec![(*n).to_owned()])
            .unwrap_or_default();
        let user = f.get_text("user_name").trim().to_owned();
        if !self.status.exists {
            Workspace::create(path).map_err(|e| e.to_string())?;
        }
        let mut ws =
            Workspace::open(path, &self.status.data_dir, None).map_err(|e| e.to_string())?;
        let s = ws.settings_mut();
        s.application_locale.set(locale);
        s.default_length_unit.set(unit);
        s.library_norm_order.set(norms);
        s.user_name.set(user);
        ws.save_settings().map_err(|e| e.to_string())
    }
}

impl FormDialog for InitializeWorkspaceWizard {
    fn title(&self) -> String {
        tr!(WIZARD, "LibrePCB Workspace Setup")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        let mut extra_buttons = Vec::new();
        if !self.history.is_empty() {
            extra_buttons.push(tr!("DeviceContentTab", "Back"));
        }
        if !self.is_last_page() {
            extra_buttons.push(tr!("DeviceContentTab", "Next"));
        }
        // Upstream: "Switch Workspace" if an upgrade or initialization is
        // needed (to choose a different workspace instead).
        if self.page > Page::ChooseWorkspace && self.history.is_empty() {
            extra_buttons.push(tr!(WIZARD, "Switch Workspace"));
        }
        let ok_text = if self.page == Page::Upgrade {
            tr!(UPGRADE, "Upgrade")
        } else {
            tr!("DeviceContentTab", "Finish")
        };
        DialogOptions {
            apply: false,
            ok_text: Some(ok_text),
            extra_buttons,
            width: 620.0,
            label_width: 180.0,
            ..DialogOptions::default()
        }
    }

    fn field_event(&mut self, _ctx: &DialogContext<'_>, id: &str, event: FieldEvent) {
        match (id, event) {
            ("path", FieldEvent::Edited) => self.path_changed(),
            ("browse", FieldEvent::Clicked) => {
                if let Some(dir) =
                    file_dialog::choose_directory(&tr!(CHOOSE, "Select Workspace Directory"), None)
                {
                    self.form.set_text("path", dir.to_string_lossy());
                    self.path_changed();
                }
            }
            _ => {}
        }
    }

    fn button(&mut self, _ctx: &DialogContext<'_>, index: usize) -> Result<ButtonResult, String> {
        let labels = self.options().extra_buttons;
        let label = labels.get(index).cloned().unwrap_or_default();
        if label == tr!("DeviceContentTab", "Back") {
            self.previous_page();
        } else if label == tr!(WIZARD, "Switch Workspace") {
            self.force_choose_path = true;
            self.page = Page::ChooseWorkspace;
            self.history.clear();
            self.build();
        } else {
            self.next_page()?;
        }
        Ok(ButtonResult::Keep)
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        match self.finish() {
            Ok(path) => Ok(Applied::App(AppRequest::WorkspaceChosen(path))),
            // Moved on to the next page: keep the dialog open without an
            // error message.
            Err(e) if e.is_empty() => Err(String::new()),
            Err(e) => Err(e),
        }
    }
}
