//! The embedded LibrePCB MCP server ("watch the agent", see
//! `docs/ui-design.md`, decision 4 and milestone M2c, and
//! `docs/mcp-design.md`, "Embedded in the application"). No upstream
//! counterpart.
//!
//! The application starts `librepcb-mcp`'s streamable HTTP server
//! (`http://127.0.0.1:8766/mcp` by default,
//! [`transport::EMBEDDED_PORT`]) on a tokio runtime with two worker
//! threads, either from the command line (`--mcp[=ADDR]`) or with the
//! toggle in the status bar (the popup of the "AI" button next to the
//! notifications button: upstream has no MCP server, and the status bar is
//! where upstream shows global, non-modal state such as background
//! progress). The server shares the application's workspace and the
//! project of the active tab ([`McpState::set_project()`] whenever the
//! current tab changes), so the agent's edits land in the same
//! `ProjectEditor` and undo stack as the user's.
//!
//! [`AppHost`] is the [`McpHost`]:
//!
//! - `changed()` posts the [`ChangeNotice`] to the UI thread
//!   (`slint::invoke_from_event_loop`), which updates the scenes of the
//!   project's tabs from the change journal right away (the 250 ms polling
//!   of [`App`](crate::App) stays as a fallback), refreshes the tab and
//!   project data (undo/redo/save state), shows the agent indicator in the
//!   status bar and, if "Show what the agent edits" is checked, switches to
//!   (or opens) the tab of the schematic or board the agent touched.
//! - `project_open` is delegated: the UI opens the project in tabs and
//!   hands its `SharedProject` to the session; `project_close` closes the
//!   project's tabs; `project_create` is done by the server and the UI
//!   adopts the new project when the notice arrives.
//! - Workspace switches are refused (the application keeps its workspace).
//!
//! Requests wait for the UI thread for at most one minute; if the UI does
//! not answer (e.g. a modal dialog), the tool fails with `refused`.

use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use librepcb_app_ui as ui;
use librepcb_core::fileio::FilePath;
use librepcb_i18n::tr;
use librepcb_mcp::transport::{self, HttpServerHandle};
use librepcb_mcp::{
    ChangeNotice, HostDecision, McpHost, McpState, ProjectCreateRequest, SharedProject,
};
use slint::ComponentHandle;

use crate::app::{State, with_current_state};
use crate::notifications::Notification;
use crate::project::AppProject;
use crate::tabs::Tab;

pub use librepcb_mcp::SharedWorkspace;

/// How long the agent indicator stays on after an edit.
const AGENT_ACTIVE_DURATION: Duration = Duration::from_secs(3);

/// How long the server waits for the UI thread to answer a request.
const UI_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// The default address of the embedded server.
pub fn default_address() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], transport::EMBEDDED_PORT))
}

/// Errors when starting the server.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    /// The tokio runtime could not be created.
    #[error("Failed to start the MCP server runtime: {0}")]
    Runtime(std::io::Error),
    /// Binding the address failed (e.g. the port is in use).
    #[error("Failed to start the MCP server on {0}: {1}")]
    Bind(SocketAddr, std::io::Error),
}

/// A running server.
struct Running {
    /// Dropped last (after the server handle).
    runtime: tokio::runtime::Runtime,
    handle: HttpServerHandle,
    state: McpState,
}

/// The embedded MCP server of the application (UI thread side).
#[derive(Default)]
pub struct McpController {
    running: Option<Running>,
    /// Turns the agent indicator off again.
    agent_timer: slint::Timer,
    /// Whether the "agent is editing" notification was shown since the
    /// server started.
    announced: bool,
}

impl McpController {
    /// Whether the server runs.
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// The endpoint URL, if the server runs.
    pub fn url(&self) -> Option<String> {
        self.running.as_ref().map(|r| r.handle.url())
    }

    /// The server state (shared with all MCP sessions), if it runs.
    pub fn state(&self) -> Option<&McpState> {
        self.running.as_ref().map(|r| &r.state)
    }

    /// Sets the project the agent works on (no-op if the server is not
    /// running). Must not be called while holding a workspace lock.
    pub fn set_project(&self, project: Option<SharedProject>) {
        if let Some(r) = &self.running {
            let same = match (r.state.project(), &project) {
                (Some(a), Some(b)) => Arc::ptr_eq(&a, b),
                (None, None) => true,
                _ => false,
            };
            if !same {
                r.state.set_project(project);
            }
        }
    }

    fn stop(&mut self) {
        if let Some(r) = self.running.take() {
            r.handle.stop();
            drop(r.handle);
            r.runtime.shutdown_background();
            log::info!("MCP server stopped.");
        }
    }
}

impl Drop for McpController {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The [`McpHost`] of the application: forwards requests and notices to
/// the UI thread.
struct AppHost {
    workspace: SharedWorkspace,
}

/// Runs `f` on the UI thread with the application state and waits for its
/// result; `None` if the UI did not answer.
fn ask_ui<T: Send + 'static>(f: impl FnOnce(&mut State) -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = mpsc::sync_channel(1);
    slint::invoke_from_event_loop(move || {
        with_current_state(move |s| {
            let _ = tx.send(f(s));
        });
    })
    .ok()?;
    rx.recv_timeout(UI_REQUEST_TIMEOUT).ok()
}

fn busy<T>() -> HostDecision<T> {
    HostDecision::Refuse(
        "The LibrePCB application did not respond (busy or showing a dialog); try again."
            .to_owned(),
    )
}

impl McpHost for AppHost {
    fn open_workspace(&self, path: &FilePath, _create: bool) -> HostDecision<SharedWorkspace> {
        let current = self.workspace.lock().path().clone();
        if *path == current {
            HostDecision::Delegated(Arc::clone(&self.workspace))
        } else {
            HostDecision::Refuse(format!(
                "LibrePCB is running with the workspace {}; the embedded MCP server always uses \
                 it (switching workspaces is not possible while the application runs).",
                current.to_native()
            ))
        }
    }

    fn open_project(&self, path: &FilePath) -> HostDecision<SharedProject> {
        let path = path.clone();
        ask_ui(move |s| s.mcp_open_project(&path)).unwrap_or_else(busy)
    }

    fn create_project(&self, _request: &ProjectCreateRequest) -> HostDecision<SharedProject> {
        // The server creates it; the UI adopts it with the change notice.
        HostDecision::Allow
    }

    fn close_project(&self, project: &SharedProject, _discard: bool) -> HostDecision<()> {
        let project = Arc::clone(project);
        ask_ui(move |s| s.mcp_close_project(&project)).unwrap_or_else(busy)
    }

    fn changed(&self, notice: &ChangeNotice) {
        let notice = notice.clone();
        if let Err(e) = slint::invoke_from_event_loop(move || {
            with_current_state(move |s| s.mcp_changed(&notice));
        }) {
            log::warn!("Failed to notify the UI about MCP changes: {e}");
        }
    }
}

/// Finds the project file of a path given by the agent (the `*.lpp` file
/// or the project directory).
fn project_file(path: &FilePath) -> Option<FilePath> {
    if path.is_existing_file() {
        return Some(path.clone());
    }
    let entries = std::fs::read_dir(path.as_path()).ok()?;
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "lpp"))
        .collect();
    files.sort();
    files.into_iter().next().and_then(FilePath::new)
}

impl State {
    /// Starts the embedded MCP server on `addr` (port 0: any free port);
    /// returns its URL.
    pub fn start_mcp_server(&mut self, addr: SocketAddr) -> Result<String, McpError> {
        if let Some(url) = self.mcp.url() {
            return Ok(url);
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("librepcb-mcp")
            .enable_all()
            .build()
            .map_err(McpError::Runtime)?;
        let host = Arc::new(AppHost {
            workspace: Arc::clone(&self.workspace),
        });
        let state = McpState::new(host);
        state.set_workspace(Some(Arc::clone(&self.workspace)));
        state.set_project(self.active_project().map(|p| Arc::clone(p.shared())));
        let handle = transport::start_http(state.clone(), addr, runtime.handle())
            .map_err(|e| McpError::Bind(addr, e))?;
        let url = handle.url();
        self.mcp.running = Some(Running {
            runtime,
            handle,
            state,
        });
        self.mcp.announced = false;
        self.update_mcp_ui();
        self.show_status(
            &tr!(
                "librepcb::editor::MainWindow",
                "MCP server running at {0}",
                url
            ),
            5000,
        );
        Ok(url)
    }

    /// Stops the embedded MCP server.
    pub fn stop_mcp_server(&mut self) {
        self.mcp.stop();
        self.mcp.agent_timer.stop();
        if let Some(w) = self.window() {
            w.global::<ui::Data>().set_mcp_agent_active(false);
        }
        self.update_mcp_ui();
    }

    /// `Backend.toggle-mcp-server()`.
    pub(crate) fn toggle_mcp_server(&mut self) {
        if self.mcp.is_running() {
            self.stop_mcp_server();
        } else if let Err(e) = self.start_mcp_server(default_address()) {
            log::error!("{e}");
            self.notifications.borrow_mut().push(Notification {
                auto_popup: true,
                ..Notification::new(
                    ui::NotificationType::Critical,
                    tr!("librepcb::editor::MainWindow", "MCP Server"),
                    e.to_string(),
                )
            });
            self.update_mcp_ui();
        }
    }

    fn update_mcp_ui(&self) {
        if let Some(w) = self.window() {
            let d = w.global::<ui::Data>();
            d.set_mcp_server_running(self.mcp.is_running());
            d.set_mcp_server_url(self.mcp.url().unwrap_or_default().into());
        }
    }

    /// The project of the current tab (if it shows one), else the only
    /// open project.
    pub(crate) fn active_project(&self) -> Option<&Rc<AppProject>> {
        let section = self
            .window()
            .map_or(0, |w| w.global::<ui::Data>().get_current_section_index());
        let current = usize::try_from(section)
            .ok()
            .and_then(|i| self.sections.get(i))
            .and_then(|s| {
                usize::try_from(s.current_index())
                    .ok()
                    .and_then(|t| s.tabs().get(t))
            })
            .and_then(Tab::project);
        current.or(match self.projects.as_slice() {
            [only] => Some(only),
            _ => None,
        })
    }

    /// Hands the project of the active tab to the MCP server (called when
    /// the current tab or the open projects change).
    pub(crate) fn mcp_active_project_changed(&self) {
        if !self.mcp.is_running() {
            return;
        }
        let active = self.active_project().map(|p| Arc::clone(p.shared()));
        // Keep the agent's project while the home tab is shown, unless it
        // was closed.
        let keep = active.is_none()
            && self
                .mcp
                .state()
                .and_then(McpState::project)
                .is_some_and(|p| self.projects.iter().any(|a| Arc::ptr_eq(a.shared(), &p)));
        if !keep {
            self.mcp.set_project(active);
        }
    }

    /// `project_open` of the agent: opens the project in tabs.
    fn mcp_open_project(&mut self, path: &FilePath) -> HostDecision<SharedProject> {
        let Some(lpp) = project_file(path) else {
            return HostDecision::Refuse(format!(
                "There is no LibrePCB project (*.lpp) at {}.",
                path.to_native()
            ));
        };
        match self.open_project(&lpp) {
            Some(index) => {
                let shared = Arc::clone(self.projects[index].shared());
                // Make it the agent's project right away (the server stores
                // it as well).
                self.mcp.set_project(Some(Arc::clone(&shared)));
                HostDecision::Delegated(shared)
            }
            None => HostDecision::Refuse(format!(
                "LibrePCB could not open {} (see the notifications in the application).",
                lpp.to_native()
            )),
        }
    }

    /// `project_close` of the agent: closes the project's tabs.
    fn mcp_close_project(&mut self, project: &SharedProject) -> HostDecision<()> {
        match self
            .projects
            .iter()
            .find(|p| Arc::ptr_eq(p.shared(), project))
            .cloned()
        {
            Some(p) => {
                self.close_project(&p);
                // Closing the tabs handed another project (or none) to the
                // server; the server detaches the session from the closed
                // project itself after this answer.
                self.mcp.set_project(Some(Arc::clone(project)));
                HostDecision::Delegated(())
            }
            None => HostDecision::Allow,
        }
    }

    /// Handles a change notice of the MCP server (on the UI thread).
    pub(crate) fn mcp_changed(&mut self, notice: &ChangeNotice) {
        let Some(shared) = &notice.project else {
            return;
        };
        let project = match self
            .projects
            .iter()
            .find(|p| Arc::ptr_eq(p.shared(), shared))
            .cloned()
        {
            Some(p) => p,
            None if notice.project_replaced => {
                // Created (or opened without asking) by the server: show it.
                let Some(index) = self.adopt_project(Arc::clone(shared)) else {
                    return;
                };
                Rc::clone(&self.projects[index])
            }
            None => return,
        };
        self.sync_project_tabs(&project);
        if !notice.focus.is_empty()
            && self
                .window()
                .is_some_and(|w| w.global::<ui::Data>().get_mcp_follow_agent())
        {
            self.follow_agent(&project, notice);
        }
        self.show_agent_activity(&project, &notice.tool);
    }

    /// Opens a project which is already loaded (e.g. created through MCP)
    /// in tabs; returns its index.
    fn adopt_project(&mut self, shared: SharedProject) -> Option<usize> {
        let project = Rc::new(AppProject::from_shared(shared)?);
        let path = project.path().clone();
        let index = self.projects.len();
        self.projects_model.push(project.ui_data());
        self.projects.push(project);
        self.quick_access.borrow_mut().push_recent(&path);
        let (has_schematic, has_board) = {
            let p = self.projects[index].shared().lock();
            (
                !p.project().schematics().is_empty(),
                !p.project().boards().is_empty(),
            )
        };
        if has_schematic {
            self.open_schematic_tab(index as i32, 0, true);
        }
        if has_board {
            self.open_board_tab(index as i32, 0, !has_schematic);
        }
        Some(index)
    }

    /// Updates the scenes and the UI data of all tabs of a project after a
    /// change (incrementally from the change journal).
    pub(crate) fn sync_project_tabs(&mut self, project: &Rc<AppProject>) {
        for si in 0..self.sections.len() {
            for ti in 0..self.sections[si].tabs().len() {
                let tab = &mut self.sections[si].tabs_mut()[ti];
                if !tab.project().is_some_and(|p| Rc::ptr_eq(p, project)) {
                    continue;
                }
                let update = tab.project_modified();
                if update != Default::default() {
                    self.apply_update(si, ti, update);
                }
            }
        }
        // Undo/redo/save state, names, documents.
        self.refresh_project(project);
    }

    /// Switches to the tab of the schematic or board the agent touched
    /// (unless the current tab shows one of them).
    fn follow_agent(&mut self, project: &Rc<AppProject>, notice: &ChangeNotice) {
        let focus = &notice.focus;
        let current_shown = self
            .sections
            .iter()
            .filter_map(|s| {
                usize::try_from(s.current_index())
                    .ok()
                    .and_then(|t| s.tabs().get(t))
            })
            .any(|t| match t {
                Tab::Schematic(t) => {
                    Rc::ptr_eq(t.project(), project) && focus.schematics.contains(&t.schematic())
                }
                Tab::Board2d(t) => {
                    Rc::ptr_eq(t.project(), project) && focus.boards.contains(&t.board())
                }
                _ => false,
            });
        if current_shown {
            return;
        }
        let Some(pidx) = self.projects.iter().position(|p| Rc::ptr_eq(p, project)) else {
            return;
        };
        let (schematic, board) = {
            let p = project.shared().lock();
            (
                focus
                    .schematics
                    .first()
                    .and_then(|id| p.project().schematic_index(*id)),
                focus
                    .boards
                    .first()
                    .and_then(|id| p.project().board_index(*id)),
            )
        };
        if let Some(i) = schematic {
            self.open_schematic_tab(pidx as i32, i as i32, true);
        } else if let Some(i) = board {
            self.open_board_tab(pidx as i32, i as i32, true);
        }
    }

    /// Shows that the agent is editing (status bar indicator, status
    /// message, and once per server run a notification).
    fn show_agent_activity(&mut self, project: &Rc<AppProject>, tool: &str) {
        let status = tr!("librepcb::editor::MainWindow", "AI agent: {0}", tool);
        if let Some(w) = self.window() {
            let d = w.global::<ui::Data>();
            d.set_mcp_agent_active(true);
            d.set_mcp_agent_status(status.as_str().into());
            let win = w.as_weak();
            self.mcp.agent_timer.start(
                slint::TimerMode::SingleShot,
                AGENT_ACTIVE_DURATION,
                move || {
                    if let Some(w) = win.upgrade() {
                        w.global::<ui::Data>().set_mcp_agent_active(false);
                    }
                },
            );
        }
        self.show_status(&status, AGENT_ACTIVE_DURATION.as_millis() as u64);
        if !self.mcp.announced {
            self.mcp.announced = true;
            let name = project
                .shared()
                .lock()
                .project()
                .metadata()
                .name
                .to_string();
            self.notifications.borrow_mut().push(Notification::new(
                ui::NotificationType::Info,
                tr!("librepcb::editor::MainWindow", "AI Agent Connected"),
                tr!(
                    "librepcb::editor::MainWindow",
                    "An AI agent is editing the project \"{0}\" through the MCP server. Its \
                     changes can be undone like your own.",
                    name
                ),
            ));
        }
    }
}
