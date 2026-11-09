//! Embedding the MCP server in a host application (the desktop app, see
//! `docs/ui-design.md`, decision 4, and `docs/mcp-design.md`, "Embedded
//! in the application"). No upstream counterpart.
//!
//! - [`McpState`] is the server state shared by all MCP client sessions
//!   (stdio or HTTP): the session's workspace and project as shared handles
//!   ([`SharedWorkspace`], [`SharedProject`]) which the host uses itself,
//!   the [`McpHost`] and a revision channel.
//! - [`McpHost`] is asked before `workspace_open`/`workspace_create`,
//!   `project_open`, `project_create` and `project_close` (allow, refuse
//!   with a message, or delegate: the host does it itself, e.g. opens the
//!   project in a tab and hands the shared project over) and receives a
//!   [`ChangeNotice`] after every tool call which changed something
//!   (write tools, undo/redo, save, derived data, open/close).
//! - [`McpState::subscribe()`] gives a `tokio::sync::watch` channel whose
//!   value increases with every notice, for hosts which prefer polling
//!   (the UI pulls `Project::changes_since()` and repaints).
//!
//! Locking: tool calls are serialized; one call locks the workspace, then
//! the project (hosts must use the same order and never lock the workspace
//! while holding a project lock), and releases both before the host is
//! notified. Host callbacks run on a blocking thread without any lock held
//! and must not wait for a thread which is waiting for a project lock.

use std::sync::Arc;

use librepcb_core::fileio::FilePath;
use librepcb_core::project::{BoardId, Change, ChangesSince, SchematicId};
use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::watch;

use crate::session::{Session, SessionSlots, SharedProject, SharedWorkspace};

/// The answer of the host to a workspace or project request.
#[derive(Debug, Clone)]
pub enum HostDecision<T> {
    /// The MCP server does it itself (the host learns about it from the
    /// [`ChangeNotice`], e.g. to open a tab for the new project).
    Allow,
    /// The operation is refused; the tool fails with kind `refused` and
    /// this message (shown to the agent, so say why and what to do).
    Refuse(String),
    /// The host did it itself (e.g. opened the project in a tab); for
    /// open/create requests the value is the shared project (workspace)
    /// the session uses from now on.
    Delegated(T),
}

/// The arguments of a `project_create` call, for [`McpHost::create_project`].
#[derive(Debug, Clone)]
pub struct ProjectCreateRequest {
    /// Project name.
    pub name: String,
    /// Project directory as given by the agent (`None`: default location in
    /// the workspace).
    pub directory: Option<String>,
    /// EAGLE schematic to import (as given by the agent); a host which
    /// delegates must import it itself (`librepcb_import::eagle`).
    pub eagle_schematic: Option<String>,
    /// EAGLE board to import with the schematic.
    pub eagle_board: Option<String>,
}

/// The host application embedding the MCP server. All methods have
/// defaults (allow everything, ignore notices), which is the behavior of
/// the standalone server ([`StandaloneHost`]).
///
/// The request methods run on a blocking thread without any lock held;
/// they may block (e.g. wait for the UI thread to open a tab), but must not
/// wait for anything which waits for the session's project lock.
pub trait McpHost: Send + Sync + 'static {
    /// `workspace_open` (`create == false`) or `workspace_create`.
    fn open_workspace(&self, path: &FilePath, create: bool) -> HostDecision<SharedWorkspace> {
        let _ = (path, create);
        HostDecision::Allow
    }

    /// `project_open` (`path`: the `*.lpp` file or the project directory,
    /// absolute).
    fn open_project(&self, path: &FilePath) -> HostDecision<SharedProject> {
        let _ = path;
        HostDecision::Allow
    }

    /// `project_create`.
    fn create_project(&self, request: &ProjectCreateRequest) -> HostDecision<SharedProject> {
        let _ = request;
        HostDecision::Allow
    }

    /// `project_close` of the session's project. With `Allow` or
    /// `Delegated`, the session drops its handle (the project stays open
    /// as long as the host holds it).
    fn close_project(&self, project: &SharedProject, discard_changes: bool) -> HostDecision<()> {
        let _ = (project, discard_changes);
        HostDecision::Allow
    }

    /// Called after every tool call which changed the session's project
    /// (revision, undo stack, save state) or replaced the workspace or
    /// project, with no lock held. Keep it short (e.g. post an event to
    /// the UI thread, which then pulls the change journal).
    fn changed(&self, notice: &ChangeNotice) {
        let _ = notice;
    }
}

/// The host of the standalone `librepcb-mcp` binary: allows everything.
#[derive(Debug, Default, Clone, Copy)]
pub struct StandaloneHost;

impl McpHost for StandaloneHost {}

/// Where a tool made changes, so the host can show them (e.g. switch to
/// the tab of the schematic or board).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Focus {
    /// Schematic pages which were added, changed or had items changed.
    pub schematics: Vec<SchematicId>,
    /// Boards which were added, changed or had items changed.
    pub boards: Vec<BoardId>,
}

impl Focus {
    /// The schematics and boards touched by `changes` (in order of first
    /// appearance; removed ones are left out).
    pub fn from_changes(changes: &[Change]) -> Self {
        let mut focus = Self::default();
        for change in changes {
            match change {
                Change::SchematicAdded(id)
                | Change::SchematicChanged(id)
                | Change::Schematic { id, .. } => {
                    if !focus.schematics.contains(id) {
                        focus.schematics.push(*id);
                    }
                }
                Change::BoardAdded(id) | Change::BoardChanged(id) | Change::Board { id, .. } => {
                    if !focus.boards.contains(id) {
                        focus.boards.push(*id);
                    }
                }
                Change::SchematicRemoved(id) => focus.schematics.retain(|s| s != id),
                Change::BoardRemoved(id) => focus.boards.retain(|b| b != id),
                _ => {}
            }
        }
        focus
    }

    /// Whether no schematic or board was touched.
    pub fn is_empty(&self) -> bool {
        self.schematics.is_empty() && self.boards.is_empty()
    }
}

/// What a tool call changed (see [`McpHost::changed`]).
#[derive(Debug, Clone)]
pub struct ChangeNotice {
    /// Sequence number of the notice (the value of the
    /// [`McpState::subscribe()`] channel after it).
    pub sequence: u64,
    /// The tool (e.g. `"connect"`); its undo group is `"AI: <tool>"`.
    pub tool: String,
    /// The session's project after the call.
    pub project: Option<SharedProject>,
    /// Whether the call opened, created, attached or closed the project
    /// (then `revision_before` refers to the previous project, if any).
    pub project_replaced: bool,
    /// Whether the call opened or created a workspace.
    pub workspace_replaced: bool,
    /// Project revision before the call.
    pub revision_before: Option<u64>,
    /// Project revision after the call.
    pub revision: Option<u64>,
    /// Schematics and boards touched by the call (empty if the project was
    /// replaced or the journal did not reach back far enough).
    pub focus: Focus,
}

/// The observable state of the session's project, to detect changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProjectState {
    ptr: usize,
    revision: u64,
    undo_state: u64,
    unsaved: bool,
}

impl ProjectState {
    fn of(session: &Session) -> Option<Self> {
        let guard = session.project.as_ref()?;
        Some(Self {
            ptr: Arc::as_ptr(parking_lot::ArcMutexGuard::mutex(guard)) as usize,
            revision: guard.project().revision(),
            undo_state: guard.editor.undo_stack().state_id(),
            unsaved: guard.has_unsaved_changes(),
        })
    }
}

fn same<T>(a: &Option<Arc<T>>, b: &Option<Arc<T>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

struct Inner {
    /// The shared handles; locked only briefly (never while locking a
    /// project or workspace).
    slots: Mutex<SessionSlots>,
    /// Serializes tool calls.
    calls: Mutex<()>,
    host: Arc<dyn McpHost>,
    sequence: watch::Sender<u64>,
}

/// The state of an MCP server shared by all its client sessions and the
/// host (cheap to clone).
#[derive(Clone)]
pub struct McpState {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for McpState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpState")
            .field("slots", &*self.inner.slots.lock())
            .field("sequence", &*self.inner.sequence.borrow())
            .finish_non_exhaustive()
    }
}

impl McpState {
    /// Creates a state without workspace and project (resources directory
    /// as in [`Session::new()`]).
    pub fn new(host: Arc<dyn McpHost>) -> Self {
        Self::from_session(Session::new(), host)
    }

    /// Creates a state from a prepared session (e.g. workspace and project
    /// opened from the command line).
    pub fn from_session(session: Session, host: Arc<dyn McpHost>) -> Self {
        Self::from_slots(session.into_slots(), host)
    }

    /// Creates a state from shared handles.
    pub fn from_slots(slots: SessionSlots, host: Arc<dyn McpHost>) -> Self {
        Self {
            inner: Arc::new(Inner {
                slots: Mutex::new(slots),
                calls: Mutex::new(()),
                host,
                sequence: watch::Sender::new(0),
            }),
        }
    }

    /// The host.
    pub fn host(&self) -> &Arc<dyn McpHost> {
        &self.inner.host
    }

    /// The session's workspace.
    pub fn workspace(&self) -> Option<SharedWorkspace> {
        self.inner.slots.lock().workspace.clone()
    }

    /// Sets (or clears) the session's workspace, e.g. the workspace of the
    /// application. Takes effect with the next tool call. Must not be
    /// called while holding a lock of the current workspace.
    pub fn set_workspace(&self, workspace: Option<SharedWorkspace>) {
        let source = workspace.as_ref().map(|ws| ws.lock().shared_library_db());
        let project = {
            let mut slots = self.inner.slots.lock();
            slots.workspace = workspace;
            slots.project.clone()
        };
        if let (Some(project), Some(source)) = (project, source) {
            project.lock().editor.set_source(source);
        }
    }

    /// The project the MCP tools work on.
    pub fn project(&self) -> Option<SharedProject> {
        self.inner.slots.lock().project.clone()
    }

    /// Sets (or clears) the project the MCP tools work on, e.g. the
    /// project of the active tab. Takes effect with the next tool call.
    pub fn set_project(&self, project: Option<SharedProject>) {
        self.inner.slots.lock().project = project;
    }

    /// Sets the resources directory (stroke fonts of new projects).
    pub fn set_resources_dir(&self, dir: Option<FilePath>) {
        self.inner.slots.lock().resources_dir = dir;
    }

    /// A channel whose value (the sequence number of the last
    /// [`ChangeNotice`]) increases after every tool call which changed
    /// something.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.inner.sequence.subscribe()
    }

    /// Runs `f` on the locked session (synchronously: call it in a
    /// blocking thread), writes back replaced workspace/project handles,
    /// releases the locks and notifies the host if something changed.
    pub fn call<T>(&self, tool: &str, f: impl FnOnce(&mut Session) -> T) -> T {
        let (value, notice) = {
            let _calls = self.inner.calls.lock();
            let before = self.inner.slots.lock().clone();
            let mut session = before.lock();
            let state_before = ProjectState::of(&session);
            let value = f(&mut session);
            let state_after = ProjectState::of(&session);
            let workspace = session.shared_workspace();
            let project = session.shared_project();
            let project_replaced = !same(&before.project, &project);
            let workspace_replaced = !same(&before.workspace, &workspace);
            let notice = (project_replaced || workspace_replaced || state_before != state_after)
                .then(|| {
                    let revision_before = state_before.map(|s| s.revision);
                    let revision = state_after.map(|s| s.revision);
                    let focus = match (&session.project, revision_before) {
                        (Some(open), Some(rev)) if !project_replaced => {
                            match open.project().changes_since(rev) {
                                ChangesSince::Changes(changes) => Focus::from_changes(changes),
                                ChangesSince::Resync => Focus::default(),
                            }
                        }
                        _ => Focus::default(),
                    };
                    ChangeNotice {
                        sequence: 0,
                        tool: tool.to_owned(),
                        project: project.clone(),
                        project_replaced,
                        workspace_replaced,
                        revision_before,
                        revision,
                        focus,
                    }
                });
            // Release the project and workspace locks before touching the
            // slots again.
            drop(session);
            if project_replaced || workspace_replaced {
                let mut slots = self.inner.slots.lock();
                if project_replaced {
                    slots.project = project;
                }
                if workspace_replaced {
                    slots.workspace = workspace;
                }
            }
            (value, notice)
        };
        if let Some(notice) = notice {
            self.notify(notice);
        }
        value
    }

    /// Assigns the next sequence number, publishes it and calls the host.
    fn notify(&self, mut notice: ChangeNotice) {
        self.inner.sequence.send_modify(|s| {
            *s += 1;
            notice.sequence = *s;
        });
        self.inner.host.changed(&notice);
    }
}
