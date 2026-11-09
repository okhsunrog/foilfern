//! The application and its main window.
//!
//! Port of libs/librepcb/editor/guiapplication.{h,cpp} (workspace, open
//! projects, notifications, quick access, the global `Data` properties and
//! the `Backend` callbacks) and libs/librepcb/editor/mainwindow.{h,cpp}
//! (window sections, tabs, actions). Upstream supports several windows;
//! for now there is one main window, so both are combined in [`App`].
//!
//! # Threading and re-entrancy
//!
//! All state lives on the UI thread in an `Rc<RefCell<State>>`; projects
//! are [`SharedProject`](librepcb_editor::SharedProject)s which other
//! threads (MCP server, workers) can lock. Like upstream (which queues most
//! callbacks with `Qt::QueuedConnection`), actions triggered from the UI run
//! deferred ([`defer`]), so they never run while Slint evaluates bindings.
//! The synchronous callbacks (`render-scene`, `scene-scrolled`) only use
//! `try_borrow_mut()` and do nothing if the state is busy.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use librepcb_app_ui as ui;
use librepcb_canvas::kurbo::Point;
use librepcb_core::attribute::AttributeType;
use librepcb_core::fileio::FilePath;
use librepcb_core::types::{GridStyle, Length};
use librepcb_core::workspace::Workspace;
use librepcb_i18n::tr;
use slint::{ComponentHandle, Model, ModelRc, SharedString};

use crate::helpers;
use crate::libraries;
use crate::mcp::{McpController, SharedWorkspace};
use crate::models::{UiModel, defer, model_rc, vec_model};
use crate::notifications::{Notification, Notifications};
use crate::project::AppProject;
use crate::section::WindowSection;
use crate::tabs::{
    Board2dTab, DerivedWrite, SchematicTab, Tab, TabId, TabUpdate, convert_modifiers, format_cursor,
};
use crate::theme::UiTheme;
use crate::workspace_models::{FileSystemTree, QuickAccess};

mod add_component_host;
mod color_scheme_host;
mod dialog_host;
mod libraries_panel;
mod library_elements;
mod library_host;
mod library_import_host;
mod lifecycle;
mod tab_editing;

pub use add_component_host::OpenAddComponent;
pub use dialog_host::OpenDialog;
pub use tab_editing::load_image_file;

/// The application version.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The Slint version the UI is built with (see `ui/PROVENANCE.md`).
const SLINT_VERSION: &str = "1.18";

/// How often open tabs check whether their project was modified (e.g. by
/// the embedded MCP server) to rebuild their scenes.
const PROJECT_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Which tab to show after opening a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitialTab {
    /// The home tab.
    Home,
    /// The first schematic page.
    Schematic,
    /// The first board.
    Board,
}

/// The running application with its main window.
pub struct App {
    window: ui::AppWindow,
    state: Rc<RefCell<State>>,
    poll_timer: slint::Timer,
}

/// Application and main window state.
pub struct State {
    pub(crate) window: slint::Weak<ui::AppWindow>,
    pub(crate) this: Weak<RefCell<State>>,
    pub(crate) workspace: SharedWorkspace,
    pub(crate) theme: UiTheme,
    pub(crate) projects: Vec<Rc<AppProject>>,
    pub(crate) projects_model: Rc<UiModel<ui::ProjectData>>,
    pub(crate) notifications: Rc<RefCell<Notifications>>,
    pub(crate) quick_access: Rc<RefCell<QuickAccess>>,
    pub(crate) file_tree: Rc<RefCell<FileSystemTree>>,
    pub(crate) sections: Vec<WindowSection>,
    pub(crate) sections_model: Rc<UiModel<ui::WindowSectionData>>,
    pub(crate) status_timer: slint::Timer,
    /// Autosave of the open projects (see [`lifecycle`]).
    pub(crate) autosave_timer: slint::Timer,
    /// The embedded MCP server (see [`crate::mcp`]).
    pub(crate) mcp: McpController,
    /// Editing in the scene tabs (see [`tab_editing`]).
    pub(crate) editing: tab_editing::EditingState,
    /// The open form dialog (see [`crate::dialogs`]).
    pub(crate) form_dialog: Option<OpenDialog>,
    /// The color scheme being edited (see [`color_scheme_host`]).
    pub(crate) color_scheme_edit: Option<color_scheme_host::ColorSchemeEdit>,
    /// The active color schemes of the schematic and board scenes.
    pub(crate) color_schemes: (librepcb_scene::ColorScheme, librepcb_scene::ColorScheme),
    /// The open "add component" dialog.
    pub(crate) add_component: Option<OpenAddComponent>,
    /// The local libraries of the libraries panel.
    pub(crate) local_libraries: crate::library_manager::LibrariesModel,
    /// The remote libraries of the libraries panel.
    pub(crate) remote_libraries: crate::library_manager::LibrariesModel,
    /// The opened libraries (`Data.libraries`).
    pub(crate) libraries: Vec<Rc<crate::open_library::OpenLibrary>>,
    pub(crate) libraries_model: Rc<UiModel<ui::LibraryData>>,
    /// The plane rebuilds of the boards (see [`crate::planes`]).
    pub(crate) planes: crate::planes::PlaneRebuilds,
}

thread_local! {
    /// The state of the application running on this (UI) thread, for
    /// results delivered from worker threads with
    /// `slint::invoke_from_event_loop()` (closures there must be `Send`).
    static CURRENT: RefCell<Weak<RefCell<State>>> = const { RefCell::new(Weak::new()) };
}

/// Runs `f` with the state of the application on the calling UI thread, if
/// it is not busy.
pub(crate) fn with_current_state(f: impl FnOnce(&mut State)) {
    let state = CURRENT.with(|c| c.borrow().upgrade());
    if let Some(s) = state
        && let Ok(mut s) = s.try_borrow_mut()
    {
        f(&mut s);
    }
}

/// Runs `f` with the state, deferred (see the module documentation).
pub(crate) fn deferred(state: &Weak<RefCell<State>>, f: impl FnOnce(&mut State) + 'static) {
    let state = state.clone();
    defer(move || {
        if let Some(s) = state.upgrade() {
            // Deferred calls run from the event loop, never nested in
            // another borrow; skip (with a log) if that ever happens.
            match s.try_borrow_mut() {
                Ok(mut s) => f(&mut s),
                Err(_) => log::error!("Application state busy, dropped a deferred action."),
            }
        }
    });
}

impl App {
    /// Creates the application for a window and an opened workspace, binds
    /// the `Data` and `Backend` globals and shows the home tab.
    pub fn new(window: ui::AppWindow, workspace: Workspace) -> Self {
        let theme = UiTheme::from_setting(workspace.settings().ui_theme.get());
        let dismissed: Vec<String> = workspace
            .settings()
            .dismissed_messages
            .get()
            .iter()
            .cloned()
            .collect();
        let quick_access = QuickAccess::new(workspace.path(), workspace.data_path());
        let file_tree = FileSystemTree::new(workspace.projects_path(), &quick_access);
        let state = Rc::new_cyclic(|this: &Weak<RefCell<State>>| {
            RefCell::new(State {
                window: window.as_weak(),
                this: this.clone(),
                workspace: Arc::new(parking_lot::Mutex::new(workspace)),
                theme,
                projects: Vec::new(),
                projects_model: UiModel::shared(Vec::new()),
                notifications: Notifications::new(dismissed),
                quick_access,
                file_tree,
                sections: Vec::new(),
                sections_model: UiModel::shared(Vec::new()),
                status_timer: slint::Timer::default(),
                autosave_timer: slint::Timer::default(),
                mcp: McpController::default(),
                editing: tab_editing::EditingState::default(),
                form_dialog: None,
                color_scheme_edit: None,
                color_schemes: (
                    librepcb_scene::ColorScheme::SCHEMATIC_LIGHT,
                    librepcb_scene::ColorScheme::BOARD_DARK,
                ),
                add_component: None,
                local_libraries: crate::library_manager::LibrariesModel::new(false),
                remote_libraries: crate::library_manager::LibrariesModel::new(true),
                libraries: Vec::new(),
                libraries_model: UiModel::shared(Vec::new()),
                planes: Default::default(),
            })
        });
        let app = Self {
            window,
            state,
            poll_timer: slint::Timer::default(),
        };
        app.init();
        app
    }

    /// The main window.
    pub fn window(&self) -> &ui::AppWindow {
        &self.window
    }

    /// The state (for tests and the screenshot mode).
    pub fn state(&self) -> &Rc<RefCell<State>> {
        &self.state
    }

    fn init(&self) {
        let weak = Rc::downgrade(&self.state);
        let win = &self.window;
        let d = win.global::<ui::Data>();
        let s = self.state.borrow();

        // Global data (upstream `GuiApplication::createNewWindow()`).
        d.set_preview_mode(false);
        d.set_window_id(1);
        d.set_window_title(format!("LibrePCB {APP_VERSION}").into());
        d.set_window_ids(vec_model(vec![1]));
        d.set_about_librepcb_details(about_details().into());
        d.set_workspace_path(s.workspace.lock().path().to_native().into());
        d.set_theme(s.theme.to_ui());
        d.set_notifications(model_rc(s.notifications.borrow().model()));
        d.set_notifications_unread(0);
        d.set_notifications_progress_index(-1);
        d.set_notifications_shown(false);
        d.set_quick_access_items(model_rc(s.quick_access.borrow().model()));
        d.set_workspace_folder_tree(model_rc(s.file_tree.borrow().model()));
        d.set_local_libraries(model_rc(s.local_libraries.model()));
        d.set_local_libraries_data(s.local_libraries.ui_data());
        d.set_remote_libraries(model_rc(s.remote_libraries.model()));
        d.set_remote_libraries_data(s.remote_libraries.ui_data());
        d.set_libraries_panel_filter(SharedString::new());
        d.set_libraries_rescan_in_progress(false);
        d.set_libraries(model_rc(&s.libraries_model));
        d.set_projects(model_rc(&s.projects_model));
        d.set_min_length(helpers::length_to_ui(Length::MIN));
        d.set_norms(vec_model(vec!["IEC 60617".into(), "IEEE 315".into()]));
        d.set_attribute_types(vec_model(
            AttributeType::ALL
                .iter()
                .map(|t| SharedString::from(t.name_tr()))
                .collect(),
        ));
        d.set_attribute_units(vec_model(
            AttributeType::ALL
                .iter()
                .map(|t| {
                    vec_model(
                        t.available_units()
                            .iter()
                            .map(|u| SharedString::from(u.symbol()))
                            .collect(),
                    )
                })
                .collect::<Vec<ModelRc<SharedString>>>(),
        ));
        d.set_project_preview_rendering(false);
        d.set_status_bar_message(SharedString::new());
        d.set_cursor_coordinates(SharedString::new());
        d.set_panel_page(ui::PanelPage::Home);
        d.set_sections(model_rc(&s.sections_model));
        d.set_current_section_index(0);
        drop(s);

        self.connect_models(&weak);
        self.bind_backend(&weak);

        CURRENT.with(|c| *c.borrow_mut() = weak.clone());

        // Installed libraries, then rescan them in the background (upstream
        // `GuiApplication` constructor).
        {
            let mut s = self.state.borrow_mut();
            s.refresh_libraries();
            s.start_library_scan();
        }

        // One section with the home tab (upstream `MainWindow` constructor).
        {
            let mut s = self.state.borrow_mut();
            s.add_section(0);
            s.current_tab_changed();
        }

        self.state.borrow_mut().apply_workspace_settings(false);

        // Rebuild scenes when projects are modified from elsewhere.
        let poll_weak = weak.clone();
        self.poll_timer.start(
            slint::TimerMode::Repeated,
            PROJECT_POLL_INTERVAL,
            move || {
                if let Some(s) = poll_weak.upgrade()
                    && let Ok(mut s) = s.try_borrow_mut()
                {
                    s.poll_projects();
                }
            },
        );
    }

    fn connect_models(&self, weak: &Weak<RefCell<State>>) {
        let s = self.state.borrow();
        let open_weak = weak.clone();
        let open: Rc<dyn Fn(FilePath)> = Rc::new(move |fp| {
            deferred(&open_weak, move |s| {
                s.open_file(&fp);
            });
        });
        QuickAccess::connect(&s.quick_access, open.clone());
        let new_weak = weak.clone();
        let new_project: Rc<dyn Fn(FilePath)> = Rc::new(move |fp| {
            deferred(&new_weak, move |s| {
                s.show_new_project_wizard(false, Some(fp));
            });
        });
        FileSystemTree::connect(&s.file_tree, open, new_project);
        let tree = Rc::downgrade(&s.file_tree);
        s.quick_access
            .borrow_mut()
            .set_on_favorite_changed(move |fp, favorite| {
                if let Some(tree) = tree.upgrade() {
                    tree.borrow().favorite_changed(fp, favorite);
                }
            });
        let win = self.window.as_weak();
        s.notifications
            .borrow_mut()
            .set_on_changed(move |state, popup| {
                if let Some(w) = win.upgrade() {
                    let d = w.global::<ui::Data>();
                    d.set_notifications_unread(state.unread);
                    d.set_notifications_progress_index(state.progress_index);
                    if popup {
                        d.set_notifications_shown(true);
                    }
                }
            });
        let dismiss_weak = weak.clone();
        s.notifications
            .borrow_mut()
            .set_on_dont_show_again(move |key| {
                let key = key.to_owned();
                deferred(&dismiss_weak, move |s| {
                    let mut keys = s
                        .workspace
                        .lock()
                        .settings()
                        .dismissed_messages
                        .get()
                        .clone();
                    keys.insert(key);
                    s.workspace
                        .lock()
                        .settings_mut()
                        .dismissed_messages
                        .set(keys);
                    if let Err(e) = s.workspace.lock().save_settings() {
                        log::error!("Failed to save the workspace settings: {e}");
                    }
                });
            });
        let sections_weak = weak.clone();
        s.sections_model
            .set_handler(move |row, data: ui::WindowSectionData| {
                deferred(&sections_weak, move |s| {
                    s.set_current_tab(row, data.current_tab_index, false);
                });
            });
    }

    fn bind_backend(&self, weak: &Weak<RefCell<State>>) {
        let b = self.window.global::<ui::Backend>();

        let w = weak.clone();
        b.on_trigger(move |action| deferred(&w, move |s| s.trigger(action)));
        let w = weak.clone();
        b.on_trigger_section(move |section, action| {
            deferred(&w, move |s| s.trigger_section(section, action));
        });
        let w = weak.clone();
        b.on_trigger_tab(move |section, tab, action| {
            deferred(&w, move |s| s.trigger_tab(section, tab, action));
        });
        let w = weak.clone();
        b.on_trigger_project(move |project, action| {
            deferred(&w, move |s| s.trigger_project(project, action));
        });
        let w = weak.clone();
        b.on_trigger_schematic(move |project, schematic, action| {
            deferred(&w, move |s| {
                if action == ui::SchematicAction::Open {
                    s.open_schematic_tab(project, schematic, true);
                } else {
                    s.not_implemented(&format!("{action:?}"));
                }
            });
        });
        let w = weak.clone();
        b.on_trigger_board(move |project, board, action| {
            deferred(&w, move |s| {
                if action == ui::BoardAction::Open2d {
                    s.open_board_tab(project, board, true);
                } else {
                    s.trigger_board_action(project, board, action);
                }
            });
        });
        let w = weak.clone();
        b.on_trigger_library(move |path, action| {
            deferred(&w, move |s| s.trigger_library(&path, action));
        });
        let w = weak.clone();
        b.on_trigger_library_element(move |path, action| {
            deferred(&w, move |s| s.trigger_library_element(&path, action));
        });

        // Scene rendering and events.
        let w = weak.clone();
        let win = self.window.as_weak();
        b.on_render_scene(move |section, width, height, scene, _frame| {
            let scale = win.upgrade().map_or(1.0, |w| w.window().scale_factor());
            let Some(s) = w.upgrade() else {
                return slint::Image::default();
            };
            let Ok(mut s) = s.try_borrow_mut() else {
                return slint::Image::default();
            };
            usize::try_from(section)
                .ok()
                .and_then(|i| s.sections.get_mut(i))
                .and_then(WindowSection::current_tab_mut)
                .map(|t| t.render_scene(width, height, scale, scene))
                .unwrap_or_default()
        });
        let w = weak.clone();
        b.on_scene_pointer_event(move |section, x, y, event, _scene| {
            // Processed asynchronously like upstream (keeps the order of
            // all pointer events, see LibrePCB issue #1740).
            deferred(&w, move |s| {
                s.scene_pointer_event(section, Point::new(f64::from(x), f64::from(y)), &event);
            });
        });
        let w = weak.clone();
        b.on_scene_scrolled(move |section, x, y, event, _scene| {
            let Some(s) = w.upgrade() else { return false };
            let Ok(mut s) = s.try_borrow_mut() else {
                return false;
            };
            s.scene_scrolled(
                section,
                Point::new(f64::from(x), f64::from(y)),
                (f64::from(event.delta_x), f64::from(event.delta_y)),
                convert_modifiers(&event.modifiers),
            )
        });
        tab_editing::bind(&self.window, weak);
        libraries_panel::bind(weak);
        dialog_host::bind(&self.window, weak);
        add_component_host::bind(&self.window, weak);
        color_scheme_host::bind(&self.window, weak);

        // Pure helpers.
        b.on_is_shortcut(|event, command| crate::shortcuts::is_shortcut(&event, &command));
        b.on_format_length(helpers::format_length);
        b.on_parse_length_input(|text, unit, minimum| {
            helpers::parse_length_input(&text, unit, minimum)
        });
        b.on_format_angle(helpers::format_angle);
        b.on_parse_angle_input(|text| helpers::parse_angle_input(&text));
        b.on_format_ratio(helpers::format_ratio);
        b.on_parse_ratio_input(|text, min, max| helpers::parse_ratio_input(&text, min, max));
        b.on_open_url(|url| match open::that_detached(url.as_str()) {
            Ok(()) => true,
            Err(e) => {
                log::warn!("Failed to open {url}: {e}");
                false
            }
        });
        b.on_copy_to_clipboard(|_| false);
        b.on_request_project_preview(|_, _, _| true);
        b.on_get_organizations_with_design_rules(|| vec_model(Vec::new()));
        b.on_can_drop_tab(|_, _| false);
        let w = weak.clone();
        b.on_libraries_key_pressed(move |event| {
            use slint::private_unstable_api::re_exports::EventResult;
            let handled = w
                .upgrade()
                .and_then(|s| {
                    s.try_borrow_mut()
                        .ok()
                        .map(|mut s| s.libraries_filter_key(&event))
                })
                .unwrap_or(false);
            if handled {
                EventResult::Accept
            } else {
                EventResult::Reject
            }
        });
        let w = weak.clone();
        b.on_toggle_theme(move || deferred(&w, |s| s.set_theme(s.theme.next())));
        let w = weak.clone();
        b.on_toggle_mcp_server(move || deferred(&w, State::toggle_mcp_server));
    }

    /// Opens a project file (like the command line argument or the open
    /// action); returns its index.
    pub fn open_project(&self, path: &std::path::Path) -> Option<usize> {
        let fp = absolute_file_path(path)?;
        self.state.borrow_mut().open_project(&fp)
    }

    /// Shows a tab of the first project (e.g. for screenshots).
    pub fn show_tab(&self, tab: InitialTab) {
        let mut s = self.state.borrow_mut();
        match tab {
            InitialTab::Home => {
                s.set_current_tab(0, 0, true);
            }
            InitialTab::Schematic => {
                s.open_schematic_tab(0, 0, true);
            }
            InitialTab::Board => {
                s.open_board_tab(0, 0, true);
            }
        }
    }

    /// Shows a side panel page.
    pub fn show_panel(&self, page: ui::PanelPage) {
        self.window.global::<ui::Data>().invoke_set_panel_page(page);
    }

    /// Pushes a notification.
    pub fn notify(&self, n: Notification) {
        let s = self.state.borrow();
        s.notifications.borrow_mut().push(n);
    }

    /// Starts the embedded LibrePCB MCP server (see [`crate::mcp`]) on
    /// `addr` (e.g. [`crate::mcp::default_address()`]); returns its URL.
    pub fn start_mcp_server(
        &self,
        addr: std::net::SocketAddr,
    ) -> Result<String, crate::mcp::McpError> {
        self.state.borrow_mut().start_mcp_server(addr)
    }
}

/// Makes a path absolute (relative to the working directory).
pub fn absolute_file_path(path: &std::path::Path) -> Option<FilePath> {
    let abs: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    FilePath::new(abs)
}

/// Version details for the about panel (upstream
/// `Application::buildFullVersionDetails()`, always English).
fn about_details() -> String {
    let mut details = vec![
        format!("LibrePCB Version: {APP_VERSION} (Rust port)"),
        format!("Slint Version:    {SLINT_VERSION}"),
        format!(
            "Operating System: {} ({})",
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    ];
    let runtime = librepcb_core::system_info::detect_runtime();
    if !runtime.is_empty() {
        details.push(format!("Runtime:          {runtime}"));
    }
    details.join("\n")
}

impl State {
    pub(crate) fn window(&self) -> Option<ui::AppWindow> {
        self.window.upgrade()
    }

    /// Open projects.
    pub fn projects(&self) -> &[Rc<AppProject>] {
        &self.projects
    }

    /// Window sections.
    pub fn sections(&self) -> &[WindowSection] {
        &self.sections
    }

    /// Window sections, mutably (tests).
    pub fn sections_mut(&mut self) -> &mut [WindowSection] {
        &mut self.sections
    }

    /// The workspace (shared with the embedded MCP server; lock order:
    /// workspace before project, never lock it while holding a project
    /// lock).
    pub fn workspace(&self) -> &SharedWorkspace {
        &self.workspace
    }

    /// Adds a section at `index` (upstream `MainWindow::addSection()`).
    pub(crate) fn add_section(&mut self, index: usize) {
        let index = index.min(self.sections.len());
        let section = WindowSection::new();
        self.connect_section(&section);
        self.sections.insert(index, section);
        self.sections_model
            .insert(index, self.sections[index].ui_data());
        self.update_home_tab_section();
        if let Some(w) = self.window() {
            w.global::<ui::Data>()
                .set_current_section_index(index as i32);
        }
    }

    /// Connects the handlers of a section's models; they find the section
    /// by its tab models (sections may move).
    pub(crate) fn connect_section(&self, section: &WindowSection) {
        let (tabs, derived) = section.models();
        let (schematic, board) = (&derived.schematic, &derived.board);
        let tabs_ptr = Rc::as_ptr(tabs) as usize;
        let find = move |s: &State| {
            s.sections
                .iter()
                .position(|sec| Rc::as_ptr(sec.models().0) as usize == tabs_ptr)
        };
        let w = self.this.clone();
        tabs.set_handler(move |row, data: ui::TabData| {
            deferred(&w, move |s| {
                if let Some(i) = find(s)
                    && let Some(tab) = s.sections[i].tab_mut(row)
                    && tab.set_ui_data(&data)
                {
                    s.sections[i].refresh_derived(row, &s.projects);
                }
            });
        });
        macro_rules! connect {
            ($model:ident, $variant:ident) => {
                let w = self.this.clone();
                derived.$model.set_handler(move |row, data| {
                    deferred(&w, move |s| {
                        if let Some(i) = find(s)
                            && let Some(tab) = s.sections[i].tab_mut(row)
                        {
                            let update = tab.set_derived(DerivedWrite::$variant(data));
                            s.apply_update(i, row, update);
                        }
                    });
                });
            };
        }
        connect!(create_library, CreateLibrary);
        connect!(download_library, DownloadLibrary);
        connect!(library, Library);
        connect!(symbol, Symbol);
        connect!(package, Package);
        connect!(component, Component);
        connect!(device, Device);
        connect!(component_category, ComponentCategory);
        connect!(package_category, PackageCategory);
        connect!(organization, Organization);
        let w = self.this.clone();
        derived
            .project_library
            .set_handler(move |row, data: ui::ProjectLibraryTabData| {
                deferred(&w, move |s| {
                    let local = s.workspace.lock().local_libraries_path().clone();
                    if let Some(i) = find(s)
                        && let Some(crate::tabs::Tab::ProjectLibrary(t)) =
                            s.sections[i].tab_mut(row)
                    {
                        let update = t.set_derived_ui_data(&data, &local);
                        s.apply_update(i, row, update);
                    }
                });
            });
        let w = self.this.clone();
        schematic.set_handler(move |row, data: ui::SchematicTabData| {
            deferred(&w, move |s| {
                if let Some(i) = find(s)
                    && let Some(tab) = s.sections[i].tab_mut(row)
                {
                    let update = tab.set_schematic_data(&data);
                    s.apply_update(i, row, update);
                    let style = helpers::grid_style_from_ui(data.grid_style);
                    s.set_grid_style_setting(true, style);
                }
            });
        });
        let w = self.this.clone();
        board.set_handler(move |row, data: ui::Board2dTabData| {
            deferred(&w, move |s| {
                if let Some(i) = find(s)
                    && let Some(tab) = s.sections[i].tab_mut(row)
                {
                    let update = tab.set_board_data(&data);
                    s.apply_update(i, row, update);
                    let style = helpers::grid_style_from_ui(data.grid_style);
                    s.set_grid_style_setting(false, style);
                }
            });
        });
    }

    /// Keeps the home tab in the first section only (upstream
    /// `MainWindow::updateHomeTabSection()`).
    pub(crate) fn update_home_tab_section(&mut self) {
        for i in 0..self.sections.len() {
            let has = self.sections[i].has_home_tab();
            if i == 0 && !has {
                self.sections[0].add_tab(Tab::Home(TabId::new()), Some(0), false, &self.projects);
            } else if i > 0 && has {
                self.sections[i].remove_tab(0);
            }
            self.update_section_row(i);
        }
    }

    pub(crate) fn update_section_row(&self, index: usize) {
        if let Some(section) = self.sections.get(index) {
            self.sections_model.set(index, section.ui_data());
        }
    }

    /// Upstream `Data.current-tab-changed()`.
    pub(crate) fn current_tab_changed(&self) {
        if let Some(w) = self.window() {
            w.global::<ui::Data>().invoke_current_tab_changed();
        }
        // The agent works on the project of the active tab.
        self.mcp_active_project_changed();
    }

    /// Makes a tab current (from the UI or the backend).
    pub(crate) fn set_current_tab(&mut self, section: usize, tab: i32, make_section_current: bool) {
        let Some(sec) = self.sections.get_mut(section) else {
            return;
        };
        let changed = sec.set_current_tab(tab);
        if changed {
            self.update_section_row(section);
            self.abort_blocking_tools_except(section, tab);
        }
        if make_section_current && let Some(w) = self.window() {
            w.global::<ui::Data>()
                .set_current_section_index(section as i32);
        }
        if changed || make_section_current {
            self.current_tab_changed();
        }
    }

    /// Adds a tab to the current section and makes it current.
    pub(crate) fn add_tab(&mut self, mut tab: Tab) {
        tab.set_color_schemes(&self.color_schemes.0, &self.color_schemes.1);
        let section = self
            .window()
            .map_or(0, |w| w.global::<ui::Data>().get_current_section_index())
            .clamp(0, self.sections.len().saturating_sub(1) as i32) as usize;
        if let Tab::Board2d(board) = &tab {
            self.connect_layers(board);
        }
        let Some(sec) = self.sections.get_mut(section) else {
            return;
        };
        sec.add_tab(tab, None, true, &self.projects);
        self.update_section_row(section);
        if let Some(w) = self.window() {
            w.global::<ui::Data>()
                .set_current_section_index(section as i32);
        }
        self.current_tab_changed();
    }

    pub(crate) fn connect_layers(&self, tab: &Board2dTab) {
        let id = tab.id();
        let w = self.this.clone();
        tab.layers_model()
            .set_handler(move |row, data: ui::GraphicsLayerData| {
                deferred(&w, move |s| {
                    if let Some((sec, index)) = s.find_tab(id)
                        && let Some(tab) = s.sections[sec].tab_mut(index)
                    {
                        let update = tab.set_layer_data(row, &data);
                        s.apply_update(sec, index, update);
                    }
                });
            });
    }

    pub(crate) fn find_tab(&self, id: TabId) -> Option<(usize, usize)> {
        self.sections
            .iter()
            .enumerate()
            .find_map(|(i, s)| s.tab_index(id).map(|t| (i, t)))
    }

    /// Applies what a tab reported after an event or action.
    pub(crate) fn apply_update(&mut self, section: usize, tab: usize, update: TabUpdate) {
        if update.close {
            self.close_tab(section, tab);
            return;
        }
        if update.repaint
            && let Some(t) = self.sections.get_mut(section).and_then(|s| s.tab_mut(tab))
        {
            t.bump_frame();
        }
        if let Some(sec) = self.sections.get(section) {
            if update.data_changed {
                sec.refresh_tab(tab, &self.projects);
            } else if update.repaint {
                sec.refresh_derived(tab, &self.projects);
            }
        }
        if let Some(w) = self.window() {
            let d = w.global::<ui::Data>();
            if let Some((pos, unit)) = update.cursor {
                d.set_cursor_coordinates(format_cursor(pos, unit).into());
            }
            if let Some(status) = update.status {
                d.set_status_bar_message(status.into());
            }
        }
        if !update.requests.is_empty() || update.project_modified {
            self.apply_tab_requests(section, tab, update.requests, update.project_modified);
        }
    }

    pub(crate) fn close_tab(&mut self, section: usize, tab: usize) {
        let Some(sec) = self.sections.get_mut(section) else {
            return;
        };
        if matches!(sec.tabs().get(tab), Some(Tab::Home(_))) {
            return;
        }
        if let Some(t) = sec.tab_mut(tab) {
            t.abort_blocking_tool();
        }
        sec.remove_tab(tab);
        self.update_section_row(section);
        self.current_tab_changed();
    }

    /// Opens a file from the home tab or the command line (upstream
    /// `GuiApplication::openFile()`).
    pub(crate) fn open_file(&mut self, fp: &FilePath) {
        if matches!(fp.suffix(), "lpp" | "lppz") {
            self.open_project(fp);
        } else if let Err(e) = open::that_detached(fp.as_path()) {
            log::warn!("Failed to open {}: {e}", fp.to_native());
        }
    }

    pub(crate) fn switch_to_project(&self, index: usize) {
        if let Some(w) = self.window() {
            let d = w.global::<ui::Data>();
            d.invoke_set_current_project(index as i32);
            d.invoke_set_panel_page(ui::PanelPage::Documents);
        }
    }

    /// Opens (or switches to) the tab of a schematic.
    pub(crate) fn open_schematic_tab(&mut self, project: i32, schematic: i32, switch_to: bool) {
        let Some(prj) = usize::try_from(project)
            .ok()
            .and_then(|i| self.projects.get(i))
            .cloned()
        else {
            return;
        };
        let Some((id, _)) = usize::try_from(schematic)
            .ok()
            .and_then(|i| prj.schematic_at(i))
        else {
            return;
        };
        let existing = self.find_tab_where(|t| {
            matches!(t, Tab::Schematic(s) if Rc::ptr_eq(s.project(), &prj) && s.schematic() == id)
        });
        if let Some((sec, index)) = existing {
            if switch_to {
                self.set_current_tab(sec, index as i32, true);
            }
            return;
        }
        let style = *self.workspace.lock().settings().schematic_grid_style.get();
        let tab = Tab::Schematic(Box::new(SchematicTab::new(prj, id, style)));
        self.add_tab_maybe_current(tab, switch_to);
    }

    /// Opens (or switches to) the 2D tab of a board.
    pub(crate) fn open_board_tab(&mut self, project: i32, board: i32, switch_to: bool) {
        let Some(prj) = usize::try_from(project)
            .ok()
            .and_then(|i| self.projects.get(i))
            .cloned()
        else {
            return;
        };
        let Some((id, _)) = usize::try_from(board).ok().and_then(|i| prj.board_at(i)) else {
            return;
        };
        let existing = self.find_tab_where(
            |t| matches!(t, Tab::Board2d(b) if Rc::ptr_eq(b.project(), &prj) && b.board() == id),
        );
        if let Some((sec, index)) = existing {
            if switch_to {
                self.set_current_tab(sec, index as i32, true);
            }
            return;
        }
        let style = *self.workspace.lock().settings().board_grid_style.get();
        let tab = Tab::Board2d(Box::new(Board2dTab::new(prj, id, style)));
        self.add_tab_maybe_current(tab, switch_to);
    }

    pub(crate) fn add_tab_maybe_current(&mut self, mut tab: Tab, switch_to: bool) {
        tab.set_color_schemes(&self.color_schemes.0, &self.color_schemes.1);
        if switch_to {
            self.add_tab(tab);
        } else {
            if let Tab::Board2d(board) = &tab {
                self.connect_layers(board);
            }
            let section = self
                .window()
                .map_or(0, |w| w.global::<ui::Data>().get_current_section_index())
                .clamp(0, self.sections.len().saturating_sub(1) as i32)
                as usize;
            if let Some(sec) = self.sections.get_mut(section) {
                sec.add_tab(tab, None, false, &self.projects);
                self.update_section_row(section);
            }
        }
    }

    pub(crate) fn find_tab_where(&self, f: impl Fn(&Tab) -> bool) -> Option<(usize, usize)> {
        self.sections
            .iter()
            .enumerate()
            .find_map(|(i, s)| s.tabs().iter().position(&f).map(|t| (i, t)))
    }

    /// `Backend.trigger()` (upstream `MainWindow::trigger()`).
    pub(crate) fn trigger(&mut self, action: ui::Action) {
        match action {
            ui::Action::Quit | ui::Action::WindowClose => {
                if let Err(e) = slint::quit_event_loop() {
                    log::error!("Failed to quit: {e}");
                }
            }
            ui::Action::ProjectOpen => self.open_project_dialog(),
            ui::Action::ProjectNew => self.show_new_project_wizard(false, None),
            ui::Action::ProjectImportEagle => self.show_new_project_wizard(true, None),
            ui::Action::WorkspaceSettings => self.show_workspace_settings(),
            ui::Action::WorkspaceSwitch => self.show_switch_workspace_wizard(),
            ui::Action::WorkspaceLibrariesRescan => self.start_library_scan(),
            ui::Action::LibraryPanelEnsurePopulated
            | ui::Action::LibraryPanelCheckForUpdates
            | ui::Action::LibraryPanelCancelUpdateCheck
            | ui::Action::LibraryPanelToggleAll
            | ui::Action::LibraryPanelApply
            | ui::Action::LibraryPanelCancel
            | ui::Action::LibraryCreate
            | ui::Action::LibraryDownload => self.trigger_libraries_action(action),
            other => self.not_implemented(&format!("{other:?}")),
        }
    }

    pub(crate) fn open_project_dialog(&mut self) {
        let dialog = rfd::FileDialog::new()
            .set_title(tr!("librepcb::editor::GuiApplication", "Open Project"))
            .add_filter(
                tr!(
                    "librepcb::editor::GuiApplication",
                    "LibrePCB project files ({0})",
                    "*.lpp *.lppz"
                ),
                &["lpp", "lppz"],
            )
            .set_directory(self.workspace.lock().projects_path().as_path());
        if let Some(path) = dialog.pick_file()
            && let Some(fp) = absolute_file_path(&path)
        {
            self.open_project(&fp);
        }
    }

    /// `Backend.trigger-section()`.
    pub(crate) fn trigger_section(&mut self, section: i32, action: ui::WindowSectionAction) {
        let Ok(section) = usize::try_from(section) else {
            return;
        };
        match action {
            ui::WindowSectionAction::Split => self.add_section(section + 1),
            ui::WindowSectionAction::Close => {
                if self.sections.len() > 1 && section < self.sections.len() {
                    self.sections.remove(section);
                    self.sections_model.remove(section);
                    self.update_home_tab_section();
                    if let Some(w) = self.window() {
                        let d = w.global::<ui::Data>();
                        let current = d
                            .get_current_section_index()
                            .clamp(0, self.sections.len() as i32 - 1);
                        d.set_current_section_index(current);
                    }
                    self.current_tab_changed();
                }
            }
        }
    }

    /// `Backend.trigger-tab()`.
    pub(crate) fn trigger_tab(&mut self, section: i32, tab: i32, action: ui::TabAction) {
        let (Ok(section), Ok(tab)) = (usize::try_from(section), usize::try_from(tab)) else {
            return;
        };
        if matches!(
            action,
            ui::TabAction::Undo | ui::TabAction::Redo | ui::TabAction::Save
        ) {
            // A running tool keeps an undo group open: abort it first.
            self.abort_all_blocking_tools(section, tab);
        } else if !tab_editing::is_view_action(action) {
            self.abort_blocking_tools_except(section, tab as i32);
        }
        // Undo/redo/save and outputs (see `history.rs`, `outputs.rs`).
        if self.trigger_tab_history(section, tab, action)
            || self.trigger_tab_output(section, tab, action)
        {
            self.after_tab_event(section, tab);
            return;
        }
        let update = match self.sections.get_mut(section).and_then(|s| s.tab_mut(tab)) {
            Some(t) => t.trigger(action),
            None => return,
        };
        self.apply_update(section, tab, update);
        self.after_tab_event(section, tab);
    }

    /// `Backend.trigger-project()`.
    pub(crate) fn trigger_project(&mut self, index: i32, action: ui::ProjectAction) {
        let Some(project) = usize::try_from(index)
            .ok()
            .and_then(|i| self.projects.get(i))
            .cloned()
        else {
            return;
        };
        match action {
            ui::ProjectAction::Close => self.close_project(&project),
            ui::ProjectAction::Save => {
                self.abort_project_tools(&project);
                let result = project.save();
                match result {
                    Ok(()) => {
                        self.project_saved(&project);
                        self.show_status(
                            &tr!("librepcb::editor::ProjectEditor", "Project saved!"),
                            2000,
                        );
                    }
                    Err(e) => self.notifications.borrow_mut().push(Notification::new(
                        ui::NotificationType::Critical,
                        tr!("librepcb::editor::ProjectEditor", "Error"),
                        e.to_string(),
                    )),
                }
                self.refresh_project(&project);
            }
            ui::ProjectAction::OpenLibraryManager => {
                if let Ok(index) = usize::try_from(index) {
                    self.open_project_library_tab(index);
                }
            }
            ui::ProjectAction::OpenSetupDialog => {
                let dialog = crate::dialogs::setup::ProjectSetupDialog::new(&project);
                self.show_form_dialog(project, Box::new(dialog));
            }
            ui::ProjectAction::OpenFolder => {
                if let Some(dir) = project.path().parent_dir()
                    && let Err(e) = open::that_detached(dir.as_path())
                {
                    log::warn!("Failed to open {}: {e}", dir.to_native());
                }
            }
            other => {
                if !self.trigger_project_output(&project, other) {
                    self.not_implemented(&format!("{other:?}"));
                }
            }
        }
    }

    pub(crate) fn refresh_project(&mut self, project: &Rc<AppProject>) {
        if let Some(i) = self.projects.iter().position(|p| Rc::ptr_eq(p, project)) {
            self.projects_model.set(i, project.ui_data());
        }
        for sec in &self.sections {
            sec.refresh_all(&self.projects);
        }
    }

    /// Closes a project and its tabs (upstream `ProjectEditor::requestClose()`;
    /// unsaved changes are discarded, the viewer does not modify projects).
    pub fn close_project(&mut self, project: &Rc<AppProject>) {
        for si in 0..self.sections.len() {
            while let Some(ti) = self.sections[si]
                .tabs()
                .iter()
                .position(|t| t.project().is_some_and(|p| Rc::ptr_eq(p, project)))
            {
                self.sections[si].remove_tab(ti);
            }
            self.update_section_row(si);
        }
        if let Some(i) = self.projects.iter().position(|p| Rc::ptr_eq(p, project)) {
            self.projects.remove(i);
            self.projects_model.remove(i);
        }
        for sec in &self.sections {
            sec.refresh_all(&self.projects);
        }
        self.current_tab_changed();
    }

    pub(crate) fn scene_pointer_event(
        &mut self,
        section: i32,
        pos: Point,
        event: &slint::language::PointerEvent,
    ) {
        let Ok(section) = usize::try_from(section) else {
            return;
        };
        let Some(tab) = self.sections.get(section).map(WindowSection::current_index) else {
            return;
        };
        if event.kind == slint::language::PointerEventKind::Down {
            self.abort_blocking_tools_except(section, tab);
        }
        let Some(sec) = self.sections.get_mut(section) else {
            return;
        };
        let update = match sec.current_tab_mut() {
            Some(t) => t.pointer_event(pos, event),
            None => return,
        };
        if let Ok(tab) = usize::try_from(tab) {
            self.apply_update(section, tab, update);
            self.after_tab_event(section, tab);
        }
    }

    pub(crate) fn scene_scrolled(
        &mut self,
        section: i32,
        pos: Point,
        delta: (f64, f64),
        modifiers: librepcb_canvas::Modifiers,
    ) -> bool {
        let Ok(section) = usize::try_from(section) else {
            return false;
        };
        let Some(sec) = self.sections.get_mut(section) else {
            return false;
        };
        let tab = sec.current_index();
        let handled = sec
            .current_tab_mut()
            .is_some_and(|t| t.scrolled(pos, delta, modifiers));
        if handled && let Ok(tab) = usize::try_from(tab) {
            // Repaint later: this callback runs inside Slint's event
            // dispatch, where changing the models is fine, but keep it
            // consistent with the other updates.
            self.apply_update(section, tab, TabUpdate::repaint());
        }
        handled
    }

    /// Rebuilds scenes of projects modified from elsewhere (a fallback: the
    /// embedded MCP server notifies about its changes right away) and
    /// schedules the ERC of modified projects.
    pub(crate) fn poll_projects(&mut self) {
        for si in 0..self.sections.len() {
            for ti in 0..self.sections[si].tabs().len() {
                // Do not wait for projects locked by a worker (exports).
                let busy = self.sections[si].tabs()[ti]
                    .project()
                    .is_some_and(|p| p.shared().try_lock().is_none());
                if busy {
                    continue;
                }
                let update = self.sections[si].tabs_mut()[ti].project_modified();
                if update != TabUpdate::default() {
                    self.apply_update(si, ti, update);
                }
            }
        }
        self.schedule_rule_checks();
        self.schedule_plane_rebuilds();
        self.refresh_project_library_tabs(false);
    }

    pub(crate) fn set_theme(&mut self, theme: UiTheme) {
        self.theme = theme;
        {
            let mut ws = self.workspace.lock();
            ws.settings_mut().ui_theme.set(theme.id.to_owned());
            if let Err(e) = ws.save_settings() {
                log::warn!("Failed to save the workspace settings: {e}");
            }
        }
        if let Some(w) = self.window() {
            w.global::<ui::Data>().set_theme(theme.to_ui());
        }
    }

    /// Starts a background rescan of the workspace libraries.
    pub(crate) fn start_library_scan(&mut self) {
        if let Some(w) = self.window() {
            w.global::<ui::Data>()
                .set_libraries_rescan_in_progress(true);
        }
        libraries::start_rescan(self.workspace.lock().library_db().scanner(), |ok| {
            with_current_state(|s| {
                if let Some(w) = s.window() {
                    w.global::<ui::Data>()
                        .set_libraries_rescan_in_progress(false);
                }
                if ok {
                    s.refresh_libraries();
                    s.refresh_library_tabs();
                    s.refresh_project_library_tabs(true);
                }
            });
        });
    }

    /// Changes the schematic or board grid style of the workspace settings
    /// (like upstream, a grid style change applies to all tabs and is
    /// stored in the workspace settings).
    pub(crate) fn set_grid_style_setting(&mut self, schematic: bool, style: GridStyle) {
        let (sch, brd) = {
            let mut ws = self.workspace.lock();
            let settings = ws.settings_mut();
            let item = if schematic {
                &mut settings.schematic_grid_style
            } else {
                &mut settings.board_grid_style
            };
            if *item.get() == style {
                return;
            }
            item.set(style);
            if let Err(e) = ws.save_settings() {
                log::warn!("Failed to save the workspace settings: {e}");
            }
            (
                *ws.settings().schematic_grid_style.get(),
                *ws.settings().board_grid_style.get(),
            )
        };
        self.apply_grid_styles(sch, brd);
    }

    /// Applies the grid styles of the workspace settings to all tabs.
    pub fn apply_grid_styles(&mut self, schematic: GridStyle, board: GridStyle) {
        for si in 0..self.sections.len() {
            for ti in 0..self.sections[si].tabs().len() {
                let update = self.sections[si].tabs_mut()[ti].set_grid_styles(schematic, board);
                self.apply_update(si, ti, update);
            }
        }
    }

    /// Shows a status bar message, cleared after `timeout_ms` (0: never).
    pub(crate) fn show_status(&self, message: &str, timeout_ms: u64) {
        let Some(w) = self.window() else { return };
        w.global::<ui::Data>()
            .set_status_bar_message(message.into());
        if timeout_ms > 0 {
            let win = w.as_weak();
            let message = SharedString::from(message);
            self.status_timer.start(
                slint::TimerMode::SingleShot,
                Duration::from_millis(timeout_ms),
                move || {
                    if let Some(w) = win.upgrade() {
                        let d = w.global::<ui::Data>();
                        if d.get_status_bar_message() == message {
                            d.set_status_bar_message(SharedString::new());
                        }
                    }
                },
            );
        }
    }

    pub(crate) fn not_implemented(&self, what: &str) {
        log::info!("Not implemented yet: {what}");
        self.show_status(
            &tr!(
                "librepcb::editor::MainWindow",
                "Not available yet in this version: {0}",
                what
            ),
            4000,
        );
    }

    /// The number of rows in the sections model (for tests).
    pub fn section_count(&self) -> usize {
        self.sections_model.row_count()
    }
}
