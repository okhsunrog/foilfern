//! Session state of the MCP server (see `docs/mcp-design.md`, "Process and
//! state"): at most one workspace and one open project, both shareable
//! with a host application.
//!
//! Between tool calls, the server keeps only the shared handles
//! ([`SharedWorkspace`], [`SharedProject`]) in [`SessionSlots`]. For one
//! tool call, it locks them (workspace first, then project) into a
//! [`Session`], whose fields are the lock guards, runs the synchronous tool
//! in `spawn_blocking` and releases the locks when the tool returns (never
//! across `.await`). Tools which open, create or close a project or
//! workspace replace the guards; the server writes the new handles back
//! into the slots.

use std::sync::Arc;

use librepcb_core::fileio::{FilePath, TransactionalFileSystem};
use librepcb_core::project::Project;
use librepcb_core::workspace::Workspace;
use librepcb_editor::{LibraryElementSource, NoLibrarySource};
pub use librepcb_editor::{OpenProject, SharedProject, override_stale_locks};
use parking_lot::{ArcMutexGuard, Mutex, RawMutex};

use crate::error::{ErrorKind, ToolError, ToolResult};

/// A workspace shared between the host application and the MCP server.
pub type SharedWorkspace = Arc<Mutex<Workspace>>;

/// The locked workspace of a [`Session`].
pub type WorkspaceGuard = ArcMutexGuard<RawMutex, Workspace>;

/// The locked project of a [`Session`].
pub type ProjectGuard = ArcMutexGuard<RawMutex, OpenProject>;

/// The state the server keeps between tool calls: the shared handles of
/// the session's workspace and project (unlocked).
#[derive(Debug, Default, Clone)]
pub struct SessionSlots {
    /// The workspace, if any.
    pub workspace: Option<SharedWorkspace>,
    /// The project the MCP tools work on, if any.
    pub project: Option<SharedProject>,
    /// See [`Session::resources_dir`].
    pub resources_dir: Option<FilePath>,
}

impl SessionSlots {
    /// Locks the workspace, then the project (the lock order every host
    /// must follow as well), for one tool call.
    pub fn lock(&self) -> Session {
        Session {
            workspace: self.workspace.as_ref().map(Mutex::lock_arc),
            project: self.project.as_ref().map(Mutex::lock_arc),
            resources_dir: self.resources_dir.clone(),
        }
    }
}

/// The session during one tool call: the locked workspace and project.
///
/// Tests and the standalone setup use a `Session` directly (it is then
/// simply always locked); the server converts it with
/// [`into_slots()`](Self::into_slots).
#[derive(Default)]
pub struct Session {
    /// The open workspace (locked), if any.
    pub workspace: Option<WorkspaceGuard>,
    /// The open project (locked), if any.
    pub project: Option<ProjectGuard>,
    /// Directory with the application resources (`fontobene/*.bene`), used
    /// for the stroke fonts of new projects; `None`: the fonts embedded
    /// into the binary are used (see [`librepcb_scene::resources`]).
    pub resources_dir: Option<FilePath>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field(
                "workspace",
                &self.workspace.as_ref().map(|ws| ws.path().to_native()),
            )
            .field("project", &self.project.as_ref().map(|p| p.file_path()))
            .field("resources_dir", &self.resources_dir)
            .finish()
    }
}

impl Session {
    /// Creates an empty session; the resources directory is taken from the
    /// environment variable `LIBREPCB_SHARE` or `../share/librepcb` next to
    /// the executable, if it exists (else the embedded resources are used,
    /// see [`librepcb_scene::resources`]).
    pub fn new() -> Self {
        let resources_dir = librepcb_scene::resources::resources_dir().and_then(FilePath::new);
        Self {
            workspace: None,
            project: None,
            resources_dir,
        }
    }

    /// Releases the locks and returns the shared handles.
    pub fn into_slots(self) -> SessionSlots {
        SessionSlots {
            workspace: self.shared_workspace(),
            project: self.shared_project(),
            resources_dir: self.resources_dir,
        }
    }

    /// The shared handle of the locked workspace.
    pub fn shared_workspace(&self) -> Option<SharedWorkspace> {
        self.workspace
            .as_ref()
            .map(|g| Arc::clone(ArcMutexGuard::mutex(g)))
    }

    /// The shared handle of the locked project.
    pub fn shared_project(&self) -> Option<SharedProject> {
        self.project
            .as_ref()
            .map(|g| Arc::clone(ArcMutexGuard::mutex(g)))
    }

    /// Makes `project` (e.g. opened by the host application) the session's
    /// project, locking it. Fails with `conflict` if a project is open.
    pub fn attach_project(&mut self, project: SharedProject) -> ToolResult<&mut OpenProject> {
        self.ensure_no_project()?;
        Ok(self.project.insert(project.lock_arc()))
    }

    /// Makes `workspace` (e.g. opened by the host application) the
    /// session's workspace, replacing the current one.
    pub fn attach_workspace(&mut self, workspace: SharedWorkspace) -> &Workspace {
        self.workspace = None;
        let ws = self.workspace.insert(workspace.lock_arc());
        if let Some(open) = &mut self.project {
            open.editor.set_source(ws.shared_library_db());
        }
        ws
    }

    /// Returns the open workspace or a `no_workspace` error.
    pub fn workspace(&self) -> ToolResult<&Workspace> {
        self.workspace
            .as_deref()
            .ok_or_else(ToolError::no_workspace)
    }

    /// Returns the open project or a `no_project` error.
    pub fn project(&self) -> ToolResult<&OpenProject> {
        self.project.as_deref().ok_or_else(ToolError::no_project)
    }

    /// Returns the open project for modification or a `no_project` error.
    pub fn project_mut(&mut self) -> ToolResult<&mut OpenProject> {
        self.project
            .as_deref_mut()
            .ok_or_else(ToolError::no_project)
    }

    /// Opens (or creates, if `create`) the workspace at `path`, replacing
    /// the current one.
    pub fn open_workspace(&mut self, path: &str, create: bool) -> ToolResult<&Workspace> {
        let fp = absolute_path(path)?;
        let mut handler = override_stale_locks;
        let ws = if create {
            if fp.is_existing_dir()
                && !fp.is_empty_dir()
                && Workspace::check_compatibility(&fp).is_err()
            {
                return Err(ToolError::new(
                    ErrorKind::Conflict,
                    format!(
                        "The directory \"{}\" is not empty and not a workspace.",
                        fp.to_native()
                    ),
                ));
            }
            Workspace::open_or_create(&fp, Some(&mut handler))?
        } else {
            Workspace::check_compatibility(&fp)?;
            Workspace::open_or_create(&fp, Some(&mut handler))?
        };
        Ok(self.attach_workspace(Arc::new(Mutex::new(ws))))
    }

    /// Opens the project `lpp` (a `*.lpp` file, or a directory containing
    /// exactly one) with a directory lock. Fails if a project is open.
    pub fn open_project(&mut self, lpp: &str) -> ToolResult<&mut OpenProject> {
        self.ensure_no_project()?;
        let path = absolute_path(lpp)?;
        let (dir, file_name) = if path.is_existing_dir() {
            let lpps: Vec<String> = std::fs::read_dir(path.as_path())
                .map_err(|e| ToolError::new(ErrorKind::Io, e.to_string()))?
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".lpp"))
                .collect();
            match lpps.as_slice() {
                [name] => (path.clone(), name.clone()),
                [] => {
                    return Err(ToolError::not_found(format!(
                        "No *.lpp file in \"{}\".",
                        path.to_native()
                    )));
                }
                _ => {
                    return Err(ToolError::invalid(format!(
                        "Several *.lpp files in \"{}\", pass the file.",
                        path.to_native()
                    )));
                }
            }
        } else {
            let dir = path
                .parent_dir()
                .ok_or_else(|| ToolError::invalid("the project file has no parent directory"))?;
            (dir, path.file_name().to_owned())
        };
        if !path.is_existing_dir() && !path.is_existing_file() {
            return Err(ToolError::not_found(format!(
                "The file \"{}\" does not exist.",
                path.to_native()
            )));
        }
        let open = OpenProject::open(&dir, &file_name, self.library_source())?;
        Ok(self.project.insert(open.into_shared().lock_arc()))
    }

    /// Installs a newly created project (see the `project_create` tool).
    pub fn set_project(&mut self, project: Project, fs: Arc<TransactionalFileSystem>) {
        let source = self.library_source();
        self.project = Some(
            OpenProject::new(project, fs, false, source)
                .into_shared()
                .lock_arc(),
        );
    }

    /// The source of library elements added to the project: the workspace
    /// library database, if a workspace is open (elements already in the
    /// project library are always available).
    pub fn library_source(&self) -> Arc<dyn LibraryElementSource> {
        match &self.workspace {
            Some(ws) => ws.shared_library_db(),
            None => Arc::new(NoLibrarySource),
        }
    }

    /// Fails with `conflict` if a project is open.
    pub fn ensure_no_project(&self) -> ToolResult<()> {
        match &self.project {
            Some(p) => Err(ToolError::new(
                ErrorKind::Conflict,
                format!(
                    "The project \"{}\" is still open (use project_close first).",
                    p.file_path()
                ),
            )),
            None => Ok(()),
        }
    }

    /// Fails with `conflict` if the project has unsaved changes and not
    /// `discard`.
    pub fn check_close_project(&self, discard: bool) -> ToolResult<()> {
        let open = self.project()?;
        if open.has_unsaved_changes() && !discard {
            return Err(ToolError::new(
                ErrorKind::Conflict,
                "The project has unsaved changes (save it with project_save, or pass \
                 discard_changes=true).",
            ));
        }
        Ok(())
    }

    /// Closes the project (discarding unsaved changes if `discard`): the
    /// session drops its handle; the project (and with it the directory
    /// lock) is released when no one else (e.g. the host application)
    /// holds it any more.
    pub fn close_project(&mut self, discard: bool) -> ToolResult<()> {
        self.check_close_project(discard)?;
        self.project = None;
        Ok(())
    }
}

/// Converts a path argument into an absolute [`FilePath`] (relative paths
/// are relative to the server's working directory).
pub fn absolute_path(path: &str) -> ToolResult<FilePath> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(ToolError::invalid("empty path"));
    }
    let abs = std::path::absolute(trimmed)
        .map_err(|e| ToolError::invalid(format!("invalid path \"{trimmed}\": {e}")))?;
    FilePath::new(&abs).ok_or_else(|| ToolError::invalid(format!("invalid path \"{trimmed}\"")))
}
