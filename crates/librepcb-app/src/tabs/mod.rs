//! Window tabs.
//!
//! Port of libs/librepcb/editor/windowtab.{h,cpp}: the base behavior of all
//! tabs (UI data, actions, scene rendering and events). Upstream uses a
//! class hierarchy; here [`Tab`] is an enum over the tab kinds, which keeps
//! the per-kind data (`SchematicTabData`, `Board2dTabData`, ...) in the tab
//! types.
//!
//! Ported tab kinds: [`HomeTab`](home), [`SchematicTab`] and
//! [`Board2dTab`]. Library editor tabs, the 3D view and the project library
//! tab follow in later milestones (their `.slint` files are already
//! there).

pub mod base_element_core;
pub mod board_2d;
pub mod board_view;
pub mod category;
pub mod component;
pub mod create_library;
pub mod device;
pub mod download_library;
pub mod editing;
pub mod element_core;
pub mod element_metadata;
pub mod home;
pub mod library;
pub mod organization;
pub mod package;
pub mod project_library;
pub mod schematic;
pub mod schematic_view;
pub mod symbol;

use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use librepcb_app_ui as ui;
use librepcb_canvas::kurbo::Point;
use librepcb_canvas::{Modifiers, PointerButton, PointerKind};
use librepcb_core::project::{BoardId, SchematicId, SymbolId};
use librepcb_core::types::{GridStyle, Length, LengthUnit, UnsignedLength, Uuid};
use librepcb_editor::fsm::board::BoardItemRef;
use librepcb_editor::fsm::schematic::{ComponentChoice, SchematicTool};
use librepcb_i18n::tr;
use slint::language::{PointerEvent, PointerEventButton, PointerEventKind};

pub use board_2d::Board2dTab;
pub use category::CategoryTab;
pub use component::{ComponentRowEvent, ComponentRowSink, ComponentTab};
pub use create_library::CreateLibraryTab;
pub use device::{DeviceRowEvent, DeviceRowSink, DeviceTab};
pub use download_library::DownloadLibraryTab;
pub use library::LibraryTab;
use librepcb_core::library::cat::{ComponentCategoryKind, PackageCategoryKind};
pub use organization::OrganizationTab;
pub use package::{PackageRowEvent, PackageRowSink, PackageTab};
pub use project_library::ProjectLibraryTab;
pub use schematic::SchematicTab;
pub use symbol::{LibraryItemRef, SymbolTab};

use crate::notifications::Notification;
use crate::project::AppProject;

/// A unique identifier of a tab (stable while tabs move between sections).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TabId(u64);

impl TabId {
    /// A new unique identifier.
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for TabId {
    fn default() -> Self {
        Self::new()
    }
}

/// A window tab.
pub enum Tab {
    /// The home tab (upstream `HomeTab`).
    Home(TabId),
    /// A schematic page.
    Schematic(Box<SchematicTab>),
    /// A board (2D view).
    Board2d(Box<Board2dTab>),
    /// The "create library" tab.
    CreateLibrary(Box<CreateLibraryTab>),
    /// The "download library" tab.
    DownloadLibrary(Box<DownloadLibraryTab>),
    /// A library (overview, metadata, checks).
    Library(Box<LibraryTab>),
    /// A symbol editor.
    Symbol(Box<SymbolTab>),
    /// A package editor.
    Package(Box<PackageTab>),
    /// A component editor.
    Component(Box<ComponentTab>),
    /// A device editor.
    Device(Box<DeviceTab>),
    /// A component category editor.
    ComponentCategory(Box<CategoryTab<ComponentCategoryKind>>),
    /// A package category editor.
    PackageCategory(Box<CategoryTab<PackageCategoryKind>>),
    /// An organization editor.
    Organization(Box<OrganizationTab>),
    /// The library manager of a project.
    ProjectLibrary(Box<ProjectLibraryTab>),
}

/// Per-kind tab data written by the UI (short-lived, moved once: the size
/// difference of the variants does not matter).
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum DerivedWrite {
    /// `CreateLibraryTabData`.
    CreateLibrary(ui::CreateLibraryTabData),
    /// `DownloadLibraryTabData`.
    DownloadLibrary(ui::DownloadLibraryTabData),
    /// `LibraryTabData`.
    Library(ui::LibraryTabData),
    /// `SymbolTabData`.
    Symbol(ui::SymbolTabData),
    /// `PackageTabData`.
    Package(ui::PackageTabData),
    /// `ComponentTabData`.
    Component(ui::ComponentTabData),
    /// `DeviceTabData`.
    Device(ui::DeviceTabData),
    /// `CategoryTabData` of a component category.
    ComponentCategory(ui::CategoryTabData),
    /// `CategoryTabData` of a package category.
    PackageCategory(ui::CategoryTabData),
    /// `OrganizationTabData`.
    Organization(ui::OrganizationTabData),
}

/// What an event or action changed in a tab.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TabUpdate {
    /// The scene must be repainted (the tab's frame counter is bumped).
    pub repaint: bool,
    /// The tab's UI data changed.
    pub data_changed: bool,
    /// New cursor position (world coordinates) with the unit to show it.
    pub cursor: Option<(Point, LengthUnit)>,
    /// A status bar message (empty to clear it).
    pub status: Option<String>,
    /// The tab wants to be closed.
    pub close: bool,
    /// Requests to the application (notifications, menus, dialogs).
    pub requests: Vec<TabRequest>,
    /// The project was modified: other tabs of the project must update
    /// their scenes.
    pub project_modified: bool,
}

/// The editor importing a DXF file (upstream: the arguments of
/// `DxfImportDialog` which differ between the editors).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DxfImportKind {
    /// Board editor (default layer: board outlines, circles as drills).
    Board,
    /// Symbol editor (default layer: symbol outlines, no drills).
    Symbol,
    /// Package editor (default layer: top documentation, circles as
    /// drills).
    Package,
}

/// An entry of a scene context menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMenuEntry {
    /// The (translated) text; empty for a separator.
    pub text: String,
    /// Whether the entry can be chosen.
    pub enabled: bool,
    /// Check state of checkable entries.
    pub checked: Option<bool>,
    /// Whether this is the default action (shown bold upstream).
    pub is_default: bool,
}

impl ContextMenuEntry {
    /// An enabled entry.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            enabled: true,
            checked: None,
            is_default: false,
        }
    }

    /// A separator.
    pub fn separator() -> Self {
        Self {
            text: String::new(),
            enabled: false,
            checked: None,
            is_default: false,
        }
    }

    /// Whether this is a separator.
    pub fn is_separator(&self) -> bool {
        self.text.is_empty()
    }
}

/// A request of a tab to the application (upstream: dialogs, message boxes
/// and menus the tabs open themselves).
#[derive(Debug, Clone, PartialEq)]
pub enum TabRequest {
    /// Show a notification.
    Notify(Notification),
    /// Show a context menu at a position of the scene (logical pixels,
    /// relative to the scene); the chosen entry is passed back with
    /// [`Tab::context_menu_action()`].
    ContextMenu {
        /// Position in the scene.
        pos: Point,
        /// The entries.
        entries: Vec<ContextMenuEntry>,
    },
    /// Open the "add component" dialog; the choice is passed back with
    /// [`Tab::add_component()`].
    AddComponent {
        /// Preselected search term.
        search_term: String,
    },
    /// Open the properties dialog of an item.
    Properties(PropertiesTarget),
    /// Rebuild all planes of the board (upstream
    /// `BoardEditor::startPlanesRebuild(true)`).
    RebuildPlanes(librepcb_core::project::BoardId),
    /// Ask for a line width (board "Set Width" dialog); the answer is
    /// passed back with [`Tab::set_line_width()`].
    LineWidth {
        /// The current width.
        current: UnsignedLength,
    },
    /// Choose an image file for the schematic or symbol image tool (upstream
    /// `ImageHelpers::execImageChooserDialog()`); the file is passed back
    /// with [`Tab::add_image()`].
    ChooseImageFile,
    /// Choose a DXF file and the import options (upstream
    /// `DxfImportDialog`); the choice is passed back with
    /// [`Tab::import_dxf()`].
    ImportDxf {
        /// The layers the polygons can be imported to.
        layers: Vec<librepcb_core::types::Layer>,
        /// The editor importing the file.
        kind: DxfImportKind,
    },
    /// Open the library tab of a library (e.g. a created one; `wizard`:
    /// in wizard mode).
    OpenLibrary {
        /// The library directory.
        path: librepcb_core::fileio::FilePath,
        /// Wizard mode.
        wizard: bool,
    },
    /// Start downloading a library (download library tab).
    DownloadLibrary {
        /// The ZIP URL.
        url: librepcb_network::Url,
        /// The destination directory.
        dir: librepcb_core::fileio::FilePath,
    },
    /// A library was downloaded (highlight it after the rescan).
    LibraryDownloaded(librepcb_core::fileio::FilePath),
    /// The library of the tab was modified (update its other tabs and the
    /// documents panel).
    LibraryModified,
    /// Rescan the workspace libraries (after saving).
    RescanLibraries,
    /// Open (or duplicate) a library element in its editor tab.
    OpenLibraryElement {
        /// The library directory.
        library: librepcb_core::fileio::FilePath,
        /// The element kind.
        kind: ui::LibraryTreeViewItemType,
        /// The element directory.
        path: librepcb_core::fileio::FilePath,
        /// Open a copy as new element.
        duplicate: bool,
    },
    /// Remove library elements (directory, name) after asking the user.
    RemoveLibraryElements(Vec<(librepcb_core::fileio::FilePath, String)>),
    /// Choose a new library icon (PNG file).
    ChooseLibraryIcon,
    /// Open the properties dialog of an item of a library element.
    LibraryItemProperties(LibraryItemRef),
    /// Open the "import pins" dialog of the symbol editor.
    ImportPinsDialog,
    /// Open a copy of the tab's element as new element ("save as").
    DuplicateLibraryElement,
    /// Choose a STEP file for a new 3D model (`None`) or to replace the
    /// file of a model (package editor); the application calls
    /// [`Tab::step_file_chosen()`].
    ChooseStepFile {
        /// The model to replace.
        model: Option<Uuid>,
    },
    /// Ask for the courtyard excess (package editor); the application calls
    /// [`Tab::generate_courtyard()`].
    CourtyardOffsetDialog,
    /// Open the project library updater for a project file.
    ProjectLibraryUpdater(librepcb_core::fileio::FilePath),
    /// Open the "move/align" dialog for these positions; the application
    /// calls [`Tab::move_align()`].
    MoveAlign {
        /// Positions of the selected items.
        positions: Vec<librepcb_core::types::Point>,
    },
    /// Choose a library element (chooser dialog); the application calls
    /// [`Tab::element_chosen()`].
    ChooseElement(crate::dialogs::chooser::ChooserPurpose),
    /// Open a URL (e.g. a datasheet) in the browser.
    OpenUrl(String),
    /// Choose a pinout CSV file (device editor); the application calls
    /// [`Tab::pinout_file_chosen()`].
    ChoosePinoutFile,
    /// Ask whether to save the unsaved changes before closing the tab
    /// ("Yes" saves and closes, "No" discards and closes).
    ConfirmClose {
        /// Dialog title.
        title: String,
        /// Question.
        text: String,
        /// Whether saving is possible ("Yes" is shown).
        can_save: bool,
    },
    /// Ask for the name of organization PCB design rules; the application
    /// calls [`Tab::design_rules_named()`].
    DesignRulesName {
        /// What the name is for.
        purpose: organization::DesignRulesNamePurpose,
        /// The proposed name.
        name: String,
    },
}

/// The item of a properties dialog request.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertiesTarget {
    /// A symbol (and its component).
    Symbol(SymbolId),
    /// A schematic net segment (net label rename dialog).
    NetSegment(SchematicId, Uuid),
    /// A schematic bus segment (bus label rename dialog).
    BusSegment(SchematicId, Uuid),
    /// A text of a symbol.
    SymbolText(SymbolId, Uuid),
    /// A schematic polygon.
    SchematicPolygon(SchematicId, Uuid),
    /// A schematic text.
    SchematicText(SchematicId, Uuid),
    /// A board item.
    Board(BoardId, BoardItemRef),
}

/// Cross-probing: what the current tab has selected, highlighted in the
/// other tabs of the project (upstream `ProjectEditor::getCrossProbe()`):
/// components, nets, component signals (pins and pads) and buses.
pub use librepcb_editor::fsm::CrossProbe;

impl TabUpdate {
    /// Only a repaint.
    pub fn repaint() -> Self {
        Self {
            repaint: true,
            ..Self::default()
        }
    }
}

impl Tab {
    /// The unique identifier.
    pub fn id(&self) -> TabId {
        match self {
            Self::Home(id) => *id,
            Self::Schematic(t) => t.id(),
            Self::Board2d(t) => t.id(),
            Self::CreateLibrary(t) => t.id(),
            Self::DownloadLibrary(t) => t.id(),
            Self::Library(t) => t.id(),
            Self::Symbol(t) => t.id(),
            Self::Package(t) => t.id(),
            Self::Component(t) => t.id(),
            Self::Device(t) => t.id(),
            Self::ComponentCategory(t) => t.id(),
            Self::PackageCategory(t) => t.id(),
            Self::Organization(t) => t.id(),
            Self::ProjectLibrary(t) => t.id(),
        }
    }

    /// A check message row of a library element tab was written by the UI.
    pub fn element_check_row(&mut self, row: usize, data: &ui::RuleCheckMessageData) -> TabUpdate {
        match self {
            Self::Symbol(t) => t.check_row_written(row, data),
            Self::Package(t) => t.check_row_written(row, data),
            Self::Component(t) => t.check_row_written(row, data),
            Self::Device(t) => t.check_row_written(row, data),
            Self::ComponentCategory(t) => t.check_row_written(row, data),
            Self::PackageCategory(t) => t.check_row_written(row, data),
            Self::Organization(t) => t.check_row_written(row, data),
            _ => TabUpdate::default(),
        }
    }

    /// A category row of a library element tab was written by the UI.
    pub fn element_category_row(
        &mut self,
        row: usize,
        data: &ui::LibraryElementCategoryData,
    ) -> TabUpdate {
        match self {
            Self::Symbol(t) => t.category_row_written(row, data),
            Self::Package(t) => t.category_row_written(row, data),
            Self::Component(t) => t.category_row_written(row, data),
            Self::Device(t) => t.category_row_written(row, data),
            _ => TabUpdate::default(),
        }
    }

    /// Applies an object modified in a library item properties dialog.
    pub fn update_library_object(
        &mut self,
        object: crate::dialogs::library_items::LibraryObject,
    ) -> TabUpdate {
        use crate::dialogs::library_items::LibraryObject;
        match (self, object) {
            (Self::Symbol(t), LibraryObject::Symbol(o)) => t.update_object(o),
            (Self::Package(t), LibraryObject::Footprint(fpt, o)) => t.update_object(fpt, o),
            _ => TabUpdate::default(),
        }
    }

    /// Sets the index of the tab's library in `Data.libraries`.
    pub fn set_library_index(&mut self, index: i32) {
        match self {
            Self::Library(t) => t.set_library_index(index),
            Self::Symbol(t) => t.set_library_index(index),
            Self::Package(t) => t.set_library_index(index),
            Self::Component(t) => t.set_library_index(index),
            Self::Device(t) => t.set_library_index(index),
            Self::ComponentCategory(t) => t.set_library_index(index),
            Self::PackageCategory(t) => t.set_library_index(index),
            Self::Organization(t) => t.set_library_index(index),
            _ => {}
        }
    }

    /// The directory of the library or library element shown by the tab
    /// (upstream `LibraryEditorTab::getDirectoryPath()`).
    pub fn directory_path(&self) -> Option<librepcb_core::fileio::FilePath> {
        match self {
            Self::Library(t) => Some(t.library().path().clone()),
            Self::Symbol(t) => Some(t.core().directory_path()),
            Self::Package(t) => Some(t.core().directory_path()),
            Self::Component(t) => Some(t.core().directory_path()),
            Self::Device(t) => Some(t.core().directory_path()),
            Self::ComponentCategory(t) => Some(t.core().directory_path()),
            Self::PackageCategory(t) => Some(t.core().directory_path()),
            Self::Organization(t) => Some(t.core().directory_path()),
            _ => None,
        }
    }

    /// The library shown by the tab, if any.
    pub fn library(&self) -> Option<&Rc<crate::open_library::OpenLibrary>> {
        match self {
            Self::Library(t) => Some(t.library()),
            Self::Symbol(t) => Some(&t.core().library),
            Self::Package(t) => Some(&t.core().library),
            Self::Component(t) => Some(&t.core().library),
            Self::Device(t) => Some(&t.core().library),
            Self::ComponentCategory(t) => Some(&t.core().library),
            Self::PackageCategory(t) => Some(&t.core().library),
            Self::Organization(t) => Some(&t.core().library),
            _ => None,
        }
    }

    /// Applies per-kind data written by the UI.
    pub fn set_derived(&mut self, data: DerivedWrite) -> TabUpdate {
        match (self, data) {
            (Self::CreateLibrary(t), DerivedWrite::CreateLibrary(d)) => t.set_derived_ui_data(&d),
            (Self::DownloadLibrary(t), DerivedWrite::DownloadLibrary(d)) => {
                t.set_derived_ui_data(&d)
            }
            (Self::Library(t), DerivedWrite::Library(d)) => t.set_derived_ui_data(&d),
            (Self::Symbol(t), DerivedWrite::Symbol(d)) => t.set_derived_ui_data(&d),
            (Self::Package(t), DerivedWrite::Package(d)) => t.set_derived_ui_data(&d),
            (Self::Component(t), DerivedWrite::Component(d)) => t.set_derived_ui_data(&d),
            (Self::Device(t), DerivedWrite::Device(d)) => t.set_derived_ui_data(&d),
            (Self::ComponentCategory(t), DerivedWrite::ComponentCategory(d)) => {
                t.set_derived_ui_data(&d)
            }
            (Self::PackageCategory(t), DerivedWrite::PackageCategory(d)) => {
                t.set_derived_ui_data(&d)
            }
            (Self::Organization(t), DerivedWrite::Organization(d)) => t.set_derived_ui_data(&d),
            _ => TabUpdate::default(),
        }
    }

    /// Changes the color schemes (schematic and board) of the scene tabs.
    pub fn set_color_schemes(
        &mut self,
        schematic: &librepcb_scene::ColorScheme,
        board: &librepcb_scene::ColorScheme,
    ) -> TabUpdate {
        match self {
            Self::Schematic(t) => t.set_color_scheme(schematic),
            Self::Board2d(t) => t.set_color_scheme(board),
            _ => TabUpdate::default(),
        }
    }

    /// The project shown by the tab, if any.
    pub fn project(&self) -> Option<&Rc<AppProject>> {
        match self {
            Self::Schematic(t) => Some(t.project()),
            Self::Board2d(t) => Some(t.project()),
            Self::ProjectLibrary(t) => Some(t.project()),
            _ => None,
        }
    }

    /// The base UI data (`TabData`).
    pub fn ui_data(&self) -> ui::TabData {
        let mut data = match self {
            Self::Home(_) => return home::ui_data(),
            Self::Schematic(t) => t.ui_data(),
            Self::Board2d(t) => t.ui_data(),
            Self::CreateLibrary(t) => return t.ui_data(),
            Self::DownloadLibrary(t) => return t.ui_data(),
            Self::Library(t) => return t.ui_data(),
            Self::Symbol(t) => return t.ui_data(),
            Self::Package(t) => return t.ui_data(),
            Self::Component(t) => return t.ui_data(),
            Self::Device(t) => return t.ui_data(),
            Self::ComponentCategory(t) => return t.ui_data(),
            Self::PackageCategory(t) => return t.ui_data(),
            Self::Organization(t) => return t.ui_data(),
            Self::ProjectLibrary(t) => return t.ui_data(),
        };
        // The graphics export (PDF) is handled by the application (see
        // `outputs.rs`).
        data.features.export_graphics = ui::FeatureState::Enabled;
        data
    }

    /// Applies base UI data written by the UI (the find term); returns
    /// whether it changed.
    pub fn set_ui_data(&mut self, data: &ui::TabData) -> bool {
        match self {
            Self::Schematic(t) => t.set_find_term(&data.find_term),
            Self::Board2d(t) => t.set_find_term(&data.find_term),
            Self::Library(t) => t.set_find_term(&data.find_term),
            _ => false,
        }
    }

    /// The schematic tab data (default for other tabs).
    pub fn schematic_data(&self, projects: &[Rc<AppProject>]) -> ui::SchematicTabData {
        match self {
            Self::Schematic(t) => t.derived_ui_data(projects),
            _ => ui::SchematicTabData::default(),
        }
    }

    /// The board tab data (default for other tabs).
    pub fn board_data(&self, projects: &[Rc<AppProject>]) -> ui::Board2dTabData {
        match self {
            Self::Board2d(t) => t.derived_ui_data(projects),
            _ => ui::Board2dTabData::default(),
        }
    }

    /// Handles a tab action.
    pub fn trigger(&mut self, action: ui::TabAction) -> TabUpdate {
        if action == ui::TabAction::Close
            && let Some(question) = self.close_question()
        {
            return TabUpdate {
                requests: vec![question],
                ..TabUpdate::default()
            };
        }
        let update = match self {
            Self::Home(_) => TabUpdate::default(),
            Self::Schematic(t) => t.trigger(action),
            Self::Board2d(t) => t.trigger(action),
            Self::CreateLibrary(t) => t.trigger(action),
            Self::DownloadLibrary(t) => t.trigger(action),
            Self::Library(t) => t.trigger(action),
            Self::Symbol(t) => t.trigger(action),
            Self::Package(t) => t.trigger(action),
            Self::Component(t) => t.trigger(action),
            Self::Device(t) => t.trigger(action),
            Self::ComponentCategory(t) => t.trigger(action),
            Self::PackageCategory(t) => t.trigger(action),
            Self::Organization(t) => t.trigger(action),
            Self::ProjectLibrary(t) => t.trigger(action),
        };
        if action == ui::TabAction::Close && !matches!(self, Self::Home(_)) {
            return TabUpdate {
                close: true,
                ..update
            };
        }
        update
    }

    /// The "Save Changes?" question before closing a library element tab
    /// with unsaved changes (upstream `requestClose()` of the element
    /// tabs); commits pending UI data first.
    fn close_question(&mut self) -> Option<TabRequest> {
        if !matches!(
            self,
            Self::Symbol(_)
                | Self::Package(_)
                | Self::Component(_)
                | Self::Device(_)
                | Self::ComponentCategory(_)
                | Self::PackageCategory(_)
                | Self::Organization(_)
        ) {
            return None;
        }
        let _ = self.trigger(ui::TabAction::Apply);
        let data = self.ui_data();
        if !data.unsaved_changes {
            return None;
        }
        let name = data.title.to_string();
        let (title, text) = match self {
            Self::Symbol(_) => (
                tr!("librepcb::editor::SymbolTab", "Save Changes?"),
                tr!(
                    "librepcb::editor::SymbolTab",
                    "The symbol '{0}' contains unsaved changes.\nDo you want to save them before closing it?",
                    name
                ),
            ),
            Self::Package(_) => (
                tr!("librepcb::editor::PackageTab", "Save Changes?"),
                tr!(
                    "librepcb::editor::PackageTab",
                    "The package '{0}' contains unsaved changes.\nDo you want to save them before closing it?",
                    name
                ),
            ),
            Self::Component(_) => (
                tr!("librepcb::editor::ComponentTab", "Save Changes?"),
                tr!(
                    "librepcb::editor::ComponentTab",
                    "The component '{0}' contains unsaved changes.\nDo you want to save them before closing it?",
                    name
                ),
            ),
            Self::Device(_) => (
                tr!("librepcb::editor::DeviceTab", "Save Changes?"),
                tr!(
                    "librepcb::editor::DeviceTab",
                    "The device '{0}' contains unsaved changes.\nDo you want to save them before closing it?",
                    name
                ),
            ),
            Self::ComponentCategory(_) => (
                tr!("librepcb::editor::ComponentCategoryTab", "Save Changes?"),
                tr!(
                    "librepcb::editor::ComponentCategoryTab",
                    "The component category '{0}' contains unsaved changes.\nDo you want to save them before closing it?",
                    name
                ),
            ),
            Self::PackageCategory(_) => (
                tr!("librepcb::editor::PackageCategoryTab", "Save Changes?"),
                tr!(
                    "librepcb::editor::PackageCategoryTab",
                    "The package category '{0}' contains unsaved changes.\nDo you want to save them before closing it?",
                    name
                ),
            ),
            Self::Organization(_) => (
                tr!("librepcb::editor::OrganizationTab", "Save Changes?"),
                tr!(
                    "librepcb::editor::OrganizationTab",
                    "The organization '{0}' contains unsaved changes.\nDo you want to save them before closing it?",
                    name
                ),
            ),
            _ => return None,
        };
        Some(TabRequest::ConfirmClose {
            title,
            text,
            can_save: !data.read_only,
        })
    }

    /// Renders the scene for `Backend.render-scene`.
    pub fn render_scene(
        &mut self,
        width: f32,
        height: f32,
        scale_factor: f32,
        scene: i32,
    ) -> slint::Image {
        match self {
            Self::Component(t) => t.render_scene(width, height, scale_factor, scene),
            Self::Device(t) => t.render_scene(width, height, scale_factor, scene),
            Self::Home(_)
            | Self::CreateLibrary(_)
            | Self::DownloadLibrary(_)
            | Self::Library(_)
            | Self::ComponentCategory(_)
            | Self::PackageCategory(_)
            | Self::Organization(_)
            | Self::ProjectLibrary(_) => slint::Image::default(),
            Self::Schematic(t) => t.render_scene(width, height, scale_factor),
            Self::Symbol(t) => t.render_scene(width, height, scale_factor),
            Self::Package(t) => t.render_scene(width, height, scale_factor),
            Self::Board2d(t) => t.render_scene(width, height, scale_factor),
        }
    }

    /// Handles `Backend.scene-pointer-event`.
    pub fn pointer_event(&mut self, pos: Point, event: &PointerEvent) -> TabUpdate {
        let (kind, button, modifiers) = convert_pointer_event(event);
        match self {
            Self::Schematic(t) => t.pointer_event(kind, button, pos, modifiers),
            Self::Board2d(t) => t.pointer_event(kind, button, pos, modifiers),
            Self::Symbol(t) => t.pointer_event(kind, button, pos, modifiers),
            Self::Package(t) => t.pointer_event(kind, button, pos, modifiers),
            _ => TabUpdate::default(),
        }
    }

    /// Handles `Backend.scene-key-pressed`; returns whether the key was
    /// handled.
    pub fn key_pressed(&mut self, event: &slint::language::KeyEvent) -> (bool, TabUpdate) {
        match self {
            Self::Home(_)
            | Self::CreateLibrary(_)
            | Self::DownloadLibrary(_)
            | Self::Library(_)
            | Self::Component(_)
            | Self::Device(_)
            | Self::ComponentCategory(_)
            | Self::PackageCategory(_)
            | Self::Organization(_)
            | Self::ProjectLibrary(_) => (false, TabUpdate::default()),
            Self::Schematic(t) => t.key_event(event, true),
            Self::Board2d(t) => t.key_event(event, true),
            Self::Symbol(t) => t.key_event(event, true),
            Self::Package(t) => t.key_event(event, true),
        }
    }

    /// Handles `Backend.scene-key-released`; returns whether the key was
    /// handled.
    pub fn key_released(&mut self, event: &slint::language::KeyEvent) -> (bool, TabUpdate) {
        match self {
            Self::Home(_)
            | Self::CreateLibrary(_)
            | Self::DownloadLibrary(_)
            | Self::Library(_)
            | Self::Component(_)
            | Self::Device(_)
            | Self::ComponentCategory(_)
            | Self::PackageCategory(_)
            | Self::Organization(_)
            | Self::ProjectLibrary(_) => (false, TabUpdate::default()),
            Self::Schematic(t) => t.key_event(event, false),
            Self::Board2d(t) => t.key_event(event, false),
            Self::Symbol(t) => t.key_event(event, false),
            Self::Package(t) => t.key_event(event, false),
        }
    }

    /// An entry of the last requested context menu was chosen.
    pub fn context_menu_action(&mut self, index: usize) -> TabUpdate {
        match self {
            Self::Home(_)
            | Self::CreateLibrary(_)
            | Self::DownloadLibrary(_)
            | Self::Library(_)
            | Self::Component(_)
            | Self::Device(_)
            | Self::ComponentCategory(_)
            | Self::PackageCategory(_)
            | Self::Organization(_)
            | Self::ProjectLibrary(_) => TabUpdate::default(),
            Self::Schematic(t) => t.context_menu_action(index),
            Self::Board2d(t) => t.context_menu_action(index),
            Self::Symbol(t) => t.context_menu_action(index),
            Self::Package(t) => t.context_menu_action(index),
        }
    }

    /// Whether the tab's FSM is adding a component.
    pub fn is_adding_component(&self) -> bool {
        matches!(self, Self::Schematic(t) if t.fsm().tool() == SchematicTool::Component)
    }

    /// Whether the tab's FSM is in its select tool.
    pub fn is_select_tool(&self) -> bool {
        matches!(self, Self::Schematic(t) if t.fsm().tool() == SchematicTool::Select)
    }

    /// A component was chosen in the "add component" dialog (`None`:
    /// canceled).
    pub fn add_component(&mut self, choice: Option<ComponentChoice>) -> TabUpdate {
        match self {
            Self::Schematic(t) => t.add_component(choice),
            _ => TabUpdate::default(),
        }
    }

    /// The answer of [`TabRequest::ChooseImageFile`].
    pub fn add_image(&mut self, data: librepcb_editor::fsm::schematic::ImageData) -> TabUpdate {
        match self {
            Self::Schematic(t) => t.add_image(data),
            Self::Symbol(t) => t.add_image(data),
            _ => TabUpdate::default(),
        }
    }

    /// The answer of [`TabRequest::ChooseStepFile`].
    pub fn step_file_chosen(
        &mut self,
        model: Option<Uuid>,
        file_stem: &str,
        content: Vec<u8>,
    ) -> TabUpdate {
        match self {
            Self::Package(t) => t.step_file_chosen(model, file_stem, content),
            _ => TabUpdate::default(),
        }
    }

    /// The answer of [`TabRequest::CourtyardOffsetDialog`].
    pub fn generate_courtyard(
        &mut self,
        offset: librepcb_core::types::PositiveLength,
    ) -> TabUpdate {
        match self {
            Self::Package(t) => t.generate_courtyard(offset),
            _ => TabUpdate::default(),
        }
    }

    /// The answer of [`TabRequest::MoveAlign`].
    pub fn move_align(&mut self, positions: &[librepcb_core::types::Point]) -> TabUpdate {
        match self {
            Self::Package(t) => t.move_align(positions),
            _ => TabUpdate::default(),
        }
    }

    /// The answer of [`TabRequest::ChooseElement`].
    pub fn element_chosen(
        &mut self,
        purpose: crate::dialogs::chooser::ChooserPurpose,
        uuid: Uuid,
    ) -> TabUpdate {
        match self {
            Self::Component(t) => t.element_chosen(purpose, uuid),
            Self::Device(t) => t.element_chosen(purpose, uuid),
            _ => TabUpdate::default(),
        }
    }

    /// The answer of [`TabRequest::DesignRulesName`].
    pub fn design_rules_named(
        &mut self,
        purpose: organization::DesignRulesNamePurpose,
        name: &str,
    ) -> TabUpdate {
        match self {
            Self::Organization(t) => t.design_rules_named(purpose, name),
            _ => TabUpdate::default(),
        }
    }

    /// A design rules row of an organization tab was written by the UI.
    pub fn organization_rules_row(
        &mut self,
        row: usize,
        data: &ui::OrganizationPcbDesignRulesData,
    ) -> TabUpdate {
        match self {
            Self::Organization(t) => t.rules_row_written(row, data),
            _ => TabUpdate::default(),
        }
    }

    /// A row of a device tab's list models was written by the UI.
    pub fn device_row_written(&mut self, event: DeviceRowEvent) -> TabUpdate {
        match self {
            Self::Device(t) => t.row_written(event),
            _ => TabUpdate::default(),
        }
    }

    /// The answer of [`TabRequest::ChoosePinoutFile`].
    pub fn pinout_file_chosen(&mut self, content: &str) -> TabUpdate {
        match self {
            Self::Device(t) => t.pinout_file_chosen(content),
            _ => TabUpdate::default(),
        }
    }

    /// A row of a component tab's list models was written by the UI.
    pub fn component_row_written(&mut self, event: ComponentRowEvent) -> TabUpdate {
        match self {
            Self::Component(t) => t.row_written(event),
            _ => TabUpdate::default(),
        }
    }

    /// A row of a package tab's list models was written by the UI.
    pub fn package_row_written(&mut self, event: PackageRowEvent) -> TabUpdate {
        match self {
            Self::Package(t) => t.row_written(event),
            _ => TabUpdate::default(),
        }
    }

    /// The answer of [`TabRequest::ImportDxf`].
    pub fn import_dxf(
        &mut self,
        settings: librepcb_editor::fsm::board::DxfImportSettings,
    ) -> TabUpdate {
        match self {
            Self::Board2d(t) => t.import_dxf(settings),
            Self::Symbol(t) => t.import_dxf(settings),
            Self::Package(t) => t.import_dxf(settings),
            _ => TabUpdate::default(),
        }
    }

    /// The answer of [`TabRequest::LineWidth`].
    pub fn set_line_width(&mut self, width: UnsignedLength) -> TabUpdate {
        match self {
            Self::Board2d(t) => t.set_line_width(width),
            _ => TabUpdate::default(),
        }
    }

    /// The length unit for dialogs opened by the tab (the grid unit).
    pub fn length_unit(&self) -> Option<LengthUnit> {
        match self {
            Self::Home(_)
            | Self::CreateLibrary(_)
            | Self::DownloadLibrary(_)
            | Self::Library(_)
            | Self::Component(_)
            | Self::Device(_)
            | Self::ComponentCategory(_)
            | Self::PackageCategory(_)
            | Self::Organization(_)
            | Self::ProjectLibrary(_) => None,
            Self::Schematic(t) => Some(t.length_unit()),
            Self::Board2d(t) => Some(t.length_unit()),
            Self::Symbol(t) => Some(t.length_unit()),
            Self::Package(t) => Some(t.length_unit()),
        }
    }

    /// Aborts a tool which keeps an undo group of the project open (before
    /// undo/redo or when another tab of the project becomes current,
    /// upstream `abortBlockingToolsInOtherEditors()`).
    pub fn abort_blocking_tool(&mut self) -> TabUpdate {
        match self {
            Self::Home(_)
            | Self::CreateLibrary(_)
            | Self::DownloadLibrary(_)
            | Self::Library(_)
            | Self::Component(_)
            | Self::Device(_)
            | Self::ComponentCategory(_)
            | Self::PackageCategory(_)
            | Self::Organization(_)
            | Self::ProjectLibrary(_) => TabUpdate::default(),
            Self::Schematic(t) => t.abort_blocking_tool(),
            Self::Board2d(t) => t.abort_blocking_tool(),
            Self::Symbol(t) => t.abort_tool(),
            Self::Package(t) => t.abort_tool(),
        }
    }

    /// What this tab cross-probes to the other tabs of its project.
    pub fn cross_probe(&self) -> Option<CrossProbe> {
        match self {
            Self::Home(_)
            | Self::CreateLibrary(_)
            | Self::DownloadLibrary(_)
            | Self::Library(_)
            | Self::Symbol(_)
            | Self::Package(_)
            | Self::Component(_)
            | Self::Device(_)
            | Self::ComponentCategory(_)
            | Self::PackageCategory(_)
            | Self::Organization(_)
            | Self::ProjectLibrary(_) => None,
            Self::Schematic(t) => Some(t.cross_probe()),
            Self::Board2d(t) => Some(t.cross_probe()),
        }
    }

    /// Highlights what another tab of the project cross-probes.
    pub fn set_cross_probe(&mut self, probe: &CrossProbe) -> TabUpdate {
        match self {
            Self::Home(_)
            | Self::CreateLibrary(_)
            | Self::DownloadLibrary(_)
            | Self::Library(_)
            | Self::Symbol(_)
            | Self::Package(_)
            | Self::Component(_)
            | Self::Device(_)
            | Self::ComponentCategory(_)
            | Self::PackageCategory(_)
            | Self::Organization(_)
            | Self::ProjectLibrary(_) => TabUpdate::default(),
            Self::Schematic(t) => t.set_cross_probe(probe),
            Self::Board2d(t) => t.set_cross_probe(probe),
        }
    }

    /// Handles `Backend.scene-scrolled`; returns whether it was handled.
    pub fn scrolled(&mut self, pos: Point, delta: (f64, f64), modifiers: Modifiers) -> bool {
        match self {
            Self::Home(_)
            | Self::CreateLibrary(_)
            | Self::DownloadLibrary(_)
            | Self::Library(_)
            | Self::Component(_)
            | Self::Device(_)
            | Self::ComponentCategory(_)
            | Self::PackageCategory(_)
            | Self::Organization(_)
            | Self::ProjectLibrary(_) => false,
            Self::Schematic(t) => t.scrolled(pos, delta.into(), modifiers),
            Self::Board2d(t) => t.scrolled(pos, delta.into(), modifiers),
            Self::Symbol(t) => t.scrolled(pos, delta.into(), modifiers),
            Self::Package(t) => t.scrolled(pos, delta.into(), modifiers),
        }
    }

    /// Applies derived schematic data written by the UI.
    pub fn set_schematic_data(&mut self, data: &ui::SchematicTabData) -> TabUpdate {
        match self {
            Self::Schematic(t) => t.set_derived_ui_data(data),
            _ => TabUpdate::default(),
        }
    }

    /// Applies derived board data written by the UI.
    pub fn set_board_data(&mut self, data: &ui::Board2dTabData) -> TabUpdate {
        match self {
            Self::Board2d(t) => t.set_derived_ui_data(data),
            _ => TabUpdate::default(),
        }
    }

    /// Applies layer data written by the UI (layers panel).
    pub fn set_layer_data(&mut self, row: usize, data: &ui::GraphicsLayerData) -> TabUpdate {
        match self {
            Self::Board2d(t) => t.set_layer_visible(row, data.visible),
            _ => TabUpdate::default(),
        }
    }

    /// Bumps the frame counter of scene tabs (the UI then requests a new
    /// image).
    pub fn bump_frame(&mut self) {
        match self {
            Self::Home(_)
            | Self::CreateLibrary(_)
            | Self::DownloadLibrary(_)
            | Self::Library(_)
            | Self::Component(_)
            | Self::Device(_)
            | Self::ComponentCategory(_)
            | Self::PackageCategory(_)
            | Self::Organization(_)
            | Self::ProjectLibrary(_) => {}
            Self::Schematic(t) => t.bump_frame(),
            Self::Board2d(t) => t.bump_frame(),
            Self::Symbol(t) => t.bump_frame(),
            Self::Package(t) => t.bump_frame(),
        }
    }

    /// Applies the schematic or board grid style (workspace settings).
    pub fn set_grid_styles(&mut self, schematic: GridStyle, board: GridStyle) -> TabUpdate {
        match self {
            Self::Home(_)
            | Self::CreateLibrary(_)
            | Self::DownloadLibrary(_)
            | Self::Library(_)
            | Self::Component(_)
            | Self::Device(_)
            | Self::ComponentCategory(_)
            | Self::PackageCategory(_)
            | Self::Organization(_)
            | Self::ProjectLibrary(_) => TabUpdate::default(),
            Self::Schematic(t) => t.set_grid_style(schematic),
            Self::Board2d(t) => t.set_grid_style(board),
            Self::Symbol(t) => t.set_grid_style(schematic),
            Self::Package(t) => t.set_grid_style(board),
        }
    }

    /// Called when the project changed (e.g. through MCP): rebuild scenes.
    pub fn project_modified(&mut self) -> TabUpdate {
        match self {
            Self::Home(_)
            | Self::CreateLibrary(_)
            | Self::DownloadLibrary(_)
            | Self::Library(_)
            | Self::Symbol(_)
            | Self::Package(_)
            | Self::Component(_)
            | Self::Device(_)
            | Self::ComponentCategory(_)
            | Self::PackageCategory(_)
            | Self::Organization(_)
            | Self::ProjectLibrary(_) => TabUpdate::default(),
            Self::Schematic(t) => t.rebuild_if_modified(),
            Self::Board2d(t) => t.rebuild_if_modified(),
        }
    }
}

/// Converts Slint's pointer event.
pub fn convert_pointer_event(e: &PointerEvent) -> (PointerKind, PointerButton, Modifiers) {
    let kind = match e.kind {
        PointerEventKind::Down => PointerKind::Down,
        PointerEventKind::Up => PointerKind::Up,
        PointerEventKind::Cancel => PointerKind::Cancel,
        _ => PointerKind::Move,
    };
    let button = match e.button {
        PointerEventButton::Left => PointerButton::Left,
        PointerEventButton::Right => PointerButton::Right,
        PointerEventButton::Middle => PointerButton::Middle,
        _ => PointerButton::Other,
    };
    (kind, button, convert_modifiers(&e.modifiers))
}

/// Converts Slint's keyboard modifiers.
pub fn convert_modifiers(m: &slint::language::KeyboardModifiers) -> Modifiers {
    Modifiers {
        shift: m.shift,
        control: m.control,
        alt: m.alt,
        meta: m.meta,
    }
}

/// The index of a project in the application's project list (for the UI
/// data), -1 if not found.
pub fn project_index(projects: &[Rc<AppProject>], project: &Rc<AppProject>) -> i32 {
    projects
        .iter()
        .position(|p| Rc::ptr_eq(p, project))
        .map_or(-1, |i| i as i32)
}

/// Formats a cursor position like upstream `MainWindow` (both coordinates
/// in the grid unit with its reasonable number of decimals).
pub fn format_cursor(pos: Point, unit: LengthUnit) -> String {
    let conv = |mm: f64| {
        Length::from_mm(mm)
            .map(|l| unit.convert_to_unit(l))
            .unwrap_or(0.0)
    };
    let decimals = unit.reasonable_number_of_decimals();
    format!(
        "{:.decimals$}, {:.decimals$}",
        conv(pos.x),
        conv(pos.y),
        decimals = decimals
    )
}

/// Upstream `toFs()`: a feature that is supported and enabled or disabled.
pub fn feature(enabled: bool) -> ui::FeatureState {
    if enabled {
        ui::FeatureState::Enabled
    } else {
        ui::FeatureState::Disabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_format() {
        assert_eq!(
            format_cursor(Point::new(2.54, -1.0), LengthUnit::Millimeters),
            "2.540, -1.000"
        );
    }
}
