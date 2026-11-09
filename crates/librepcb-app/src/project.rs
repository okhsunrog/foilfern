//! A project opened in the application.
//!
//! Port of the project handling of libs/librepcb/editor/project/projecteditor.{h,cpp}
//! and `GuiApplication::openProject()`: opening a `*.lpp` file with a
//! directory lock (read-only if the project is locked by another
//! application or not writable) or a `*.lppz` archive (extracted, read-only),
//! and the project's `ProjectData` for the
//! documents panel.
//!
//! The project lives in a [`SharedProject`] (`Arc<Mutex<OpenProject>>`
//! holding the [`ProjectEditor`](librepcb_editor::ProjectEditor) and the
//! directory lock) so that background jobs and the embedded MCP server can
//! share it (see `docs/ui-design.md`, decisions 3 and 4).

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use librepcb_app_ui as ui;
use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::project::{BoardId, ProjectLoader, SchematicId};
use librepcb_editor::{LibraryElementSource, OpenProject, SharedProject};

use crate::models::vec_model;
use crate::rule_check::ProjectChecks;

/// Errors when opening a project.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// Not a project file.
    #[error("Not a LibrePCB project file: {0}")]
    NotAProject(String),
    /// File system error (locks, I/O).
    #[error(transparent)]
    FileIo(#[from] librepcb_core::fileio::Error),
    /// Loading failed (boxed: large).
    #[error(transparent)]
    Project(Box<librepcb_core::project::Error>),
}

impl From<librepcb_core::project::Error> for OpenError {
    fn from(e: librepcb_core::project::Error) -> Self {
        Self::Project(Box::new(e))
    }
}

/// A project open in the application.
pub struct AppProject {
    shared: SharedProject,
    path: FilePath,
    writable: bool,
    /// ERC and DRC results (UI thread only).
    checks: RefCell<ProjectChecks>,
    /// The undo stack state at the last (auto)save (upstream
    /// `mLastAutosaveStateId`).
    autosave_state: Cell<u64>,
    /// The "file format upgraded" notification (dismissed on save).
    pub(crate) migration_notification: Cell<Option<crate::notifications::NotificationId>>,
}

impl std::fmt::Debug for AppProject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppProject")
            .field("path", &self.path)
            .field("writable", &self.writable)
            .finish_non_exhaustive()
    }
}

/// How an existing directory lock is handled when opening a project
/// (the answer of the directory lock prompt).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LockDecision {
    /// Report the lock ([`OpenOutcome::Locked`]) to ask the user.
    #[default]
    Ask,
    /// Override the lock (upstream "Open anyway").
    Override,
    /// Open the project read-only if it is locked.
    ReadOnly,
}

/// A request to open a project, with the answers of the prompts shown so
/// far (see [`AppProject::open_with()`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRequest {
    /// The project file (`*.lpp`).
    pub path: FilePath,
    /// How to handle an existing directory lock.
    pub lock: LockDecision,
    /// Whether to restore an autosave backup (`None`: ask).
    pub restore: Option<bool>,
}

impl OpenRequest {
    /// A request without answers yet.
    pub fn new(path: FilePath) -> Self {
        Self {
            path,
            lock: LockDecision::Ask,
            restore: None,
        }
    }
}

/// The result of [`AppProject::open_with()`].
#[derive(Debug)]
pub enum OpenOutcome {
    /// The project was opened.
    Opened(Box<AppProject>),
    /// The directory is locked by another application: ask the user
    /// (upstream `DirectoryLockHandlerDialog`).
    Locked {
        /// The locked directory.
        directory: FilePath,
        /// `user@host` of the lock owner.
        user: String,
        /// Whether overriding the lock is offered (the lock belongs to
        /// another user or an unknown application; upstream
        /// `allowOverrideLock`).
        can_override: bool,
    },
    /// An autosave backup exists (the application crashed): ask whether
    /// to restore it.
    AutosaveDetected,
}

impl AppProject {
    /// Opens a project file (`*.lpp`) without asking: stale locks are
    /// overridden, the project is opened read-only if the directory cannot
    /// be locked (e.g. the project is open in another application), and
    /// autosave backups are not restored.
    pub fn open(lpp: &FilePath, source: Arc<dyn LibraryElementSource>) -> Result<Self, OpenError> {
        let request = OpenRequest {
            path: lpp.clone(),
            lock: LockDecision::ReadOnly,
            restore: Some(false),
        };
        match Self::open_with(&request, source)? {
            OpenOutcome::Opened(p) => Ok(*p),
            _ => Err(OpenError::NotAProject(lpp.to_native())),
        }
    }

    /// Opens a project file (`*.lpp`) like upstream
    /// `GuiApplication::openProject()`: a lock of another application and
    /// an autosave backup are reported to the caller (to ask the user)
    /// unless `request` already contains the answer. Other errors while
    /// locking the directory (e.g. no write permission) open the project
    /// read-only.
    pub fn open_with(
        request: &OpenRequest,
        source: Arc<dyn LibraryElementSource>,
    ) -> Result<OpenOutcome, OpenError> {
        use librepcb_core::fileio::{Error as FileError, LockStatus, RestoreMode};
        let lpp = &request.path;
        if lpp.suffix() == "lppz" && lpp.is_existing_file() {
            return Self::open_archive(lpp, source);
        }
        if lpp.suffix() != "lpp" || !lpp.is_existing_file() {
            return Err(OpenError::NotAProject(lpp.to_native()));
        }
        let dir = lpp
            .parent_dir()
            .ok_or_else(|| OpenError::NotAProject(lpp.to_native()))?;
        let mut lock_status = None;
        let read_only = || -> Result<OpenOutcome, OpenError> {
            let fs = Arc::new(TransactionalFileSystem::open_ro(&dir)?);
            let mut loader = ProjectLoader::new();
            let project = loader.open(
                TransactionalDirectory::new(Arc::clone(&fs), ""),
                lpp.file_name(),
            )?;
            let mut open = OpenProject::new(project, fs, false, source.clone());
            open.migration_log = loader.migration_log().cloned();
            open.upgraded = open.migration_log.is_some();
            Ok(OpenOutcome::Opened(Box::new(Self::new(open, lpp, false))))
        };
        let override_lock = request.lock == LockDecision::Override;
        let mut handler = |_: &FilePath, status: LockStatus, user: &str| {
            lock_status = Some((status, user.to_owned()));
            Ok(override_lock)
        };
        let restore = match request.restore {
            None => RestoreMode::Abort,
            Some(true) => RestoreMode::Yes,
            Some(false) => RestoreMode::No,
        };
        let result = OpenProject::open_with(
            &dir,
            lpp.file_name(),
            source.clone(),
            restore,
            Some(&mut handler),
        );
        match result {
            Ok(p) => Ok(OpenOutcome::Opened(Box::new(Self::new(p, lpp, true)))),
            Err(librepcb_editor::Error::FileIo(FileError::AutosaveDetected(_))) => {
                Ok(OpenOutcome::AutosaveDetected)
            }
            Err(librepcb_editor::Error::FileIo(FileError::AlreadyLocked { directory, user })) => {
                if request.lock == LockDecision::ReadOnly {
                    return read_only();
                }
                let can_override = matches!(
                    lock_status.as_ref().map(|(s, _)| *s),
                    Some(LockStatus::LockedByOtherUser | LockStatus::LockedByUnknownApp)
                );
                Ok(OpenOutcome::Locked {
                    directory,
                    user,
                    can_override,
                })
            }
            Err(librepcb_editor::Error::FileIo(e)) => {
                log::warn!("Opening {} read-only: {e}", dir.to_native());
                read_only()
            }
            Err(librepcb_editor::Error::Project(e)) => Err(OpenError::Project(e)),
            Err(e) => Err(OpenError::NotAProject(format!("{}: {e}", lpp.to_native()))),
        }
    }

    /// Opens a project archive (`*.lppz`) read-only like upstream
    /// `GuiApplication::openProject()`: the archive is extracted into a new
    /// temporary directory (without lock) and the project file found there
    /// is opened.
    fn open_archive(
        lppz: &FilePath,
        source: Arc<dyn LibraryElementSource>,
    ) -> Result<OpenOutcome, OpenError> {
        let tmp = FilePath::new(
            std::env::temp_dir()
                .join("librepcb")
                .join(librepcb_core::types::Uuid::new_random().to_string()),
        )
        .ok_or_else(|| OpenError::NotAProject(lppz.to_native()))?;
        let fs = Arc::new(TransactionalFileSystem::open_ro(&tmp)?);
        fs.remove_dir_recursively("")?;
        fs.load_from_zip(lppz)?;
        let Some(file_name) = fs.files("").into_iter().rfind(|f| f.ends_with(".lpp")) else {
            return Err(OpenError::NotAProject(lppz.to_native()));
        };
        let mut loader = ProjectLoader::new();
        let project = loader.open(TransactionalDirectory::new(Arc::clone(&fs), ""), &file_name)?;
        let mut open = OpenProject::new(project, fs, false, source);
        open.migration_log = loader.migration_log().cloned();
        open.upgraded = open.migration_log.is_some();
        Ok(OpenOutcome::Opened(Box::new(Self::new(open, lppz, false))))
    }

    fn new(project: OpenProject, lpp: &FilePath, writable: bool) -> Self {
        let state = project.editor.undo_stack().state_id();
        Self {
            shared: project.into_shared(),
            path: lpp.clone(),
            writable,
            checks: RefCell::default(),
            autosave_state: Cell::new(state),
            migration_notification: Cell::new(None),
        }
    }

    /// Wraps a project which is already open (e.g. created by the embedded
    /// MCP server); `None` if it has no project file on disk.
    pub fn from_shared(shared: SharedProject) -> Option<Self> {
        let (path, writable) = {
            let p = shared.lock();
            (p.project().file_path()?, p.file_system.is_writable())
        };
        Some(Self {
            shared,
            path,
            writable,
            checks: RefCell::default(),
            autosave_state: Cell::new(0),
            migration_notification: Cell::new(None),
        })
    }

    /// Saves the project (upstream `ProjectEditor::saveProject()`).
    pub fn save(&self) -> librepcb_editor::Result<()> {
        let mut p = self.shared.lock();
        p.save()?;
        self.autosave_state.set(p.editor.undo_stack().state_id());
        Ok(())
    }

    /// Writes an autosave backup (`.autosave/` of the project directory)
    /// if the project was modified since the last save or autosave
    /// (upstream `ProjectEditor::autosaveProject()`). Returns whether a
    /// backup was written; skipped while the project is busy (locked by
    /// another thread or an undo group is active) or read-only.
    pub fn autosave(&self) -> Result<bool, librepcb_editor::Error> {
        let Some(mut p) = self.shared.try_lock() else {
            return Ok(false);
        };
        let state = p.editor.undo_stack().state_id();
        // Note: the clean state of the undo stack does not matter, since
        // undoing to the clean state after an autosave makes the backup
        // outdated.
        if state == self.autosave_state.get() {
            return Ok(false);
        }
        if p.editor.undo_stack().is_group_active() {
            return Ok(false);
        }
        if !self.writable || !p.file_system.is_writable() {
            log::info!("Project directory is not writable, skipping autosave.");
            return Ok(false);
        }
        log::debug!("Autosave project...");
        p.editor.autosave()?;
        self.autosave_state.set(state);
        Ok(true)
    }

    /// Whether an autosave backup was restored when opening the project
    /// (and the project was not saved since).
    pub fn is_restored_from_autosave(&self) -> bool {
        self.shared.lock().file_system.is_restored_from_autosave()
    }

    /// The rule check results (ERC, DRC per board).
    pub fn checks(&self) -> &RefCell<ProjectChecks> {
        &self.checks
    }

    /// The shared project (editor, undo stack, directory lock).
    pub fn shared(&self) -> &SharedProject {
        &self.shared
    }

    /// The project file.
    pub fn path(&self) -> &FilePath {
        &self.path
    }

    /// Whether the project can be saved.
    pub fn is_writable(&self) -> bool {
        self.writable
    }

    /// The project's revision (changes with every modification).
    pub fn revision(&self) -> u64 {
        self.shared.lock().project().revision()
    }

    /// The schematic at a documents panel index.
    pub fn schematic_at(&self, index: usize) -> Option<(SchematicId, String)> {
        let p = self.shared.lock();
        p.project()
            .schematics()
            .get(index)
            .map(|s| (s.id(), s.properties().name.to_string()))
    }

    /// The board at a documents panel index.
    pub fn board_at(&self, index: usize) -> Option<(BoardId, String)> {
        let p = self.shared.lock();
        p.project()
            .boards()
            .get(index)
            .map(|b| (b.id(), b.properties().name.to_string()))
    }

    /// The data for `Data.projects`.
    pub fn ui_data(&self) -> ui::ProjectData {
        let p = self.shared.lock();
        let project = p.project();
        let checks = self.checks.borrow();
        let undo_state = p.editor.undo_stack().state_id();
        let read_only = !self.writable;
        let schematics = project
            .schematics()
            .iter()
            .map(|s| ui::SchematicData {
                name: s.properties().name.to_string().into(),
            })
            .collect();
        let boards = project
            .boards()
            .iter()
            .map(|b| ui::BoardData {
                name: b.properties().name.to_string().into(),
                drc: checks.drc_data(b.id(), undo_state, read_only),
                order_upload_progress: -1,
                ..Default::default()
            })
            .collect();
        // Upstream `ProjectEditor::getUiData()`: the buses sorted by name.
        let mut buses: Vec<(String, String)> = project
            .circuit()
            .buses()
            .iter()
            .map(|(id, b)| (b.name().to_string(), id.to_string()))
            .collect();
        buses.sort_by(|a, b| crate::dialogs::natural_cmp(&a.0, &b.0));
        let buses = buses
            .into_iter()
            .map(|(name, uuid)| ui::BusData {
                uuid: uuid.into(),
                name: name.into(),
            })
            .collect();
        ui::ProjectData {
            valid: true,
            path: self.path.to_native().into(),
            name: project.metadata().name.to_string().into(),
            schematics: vec_model(schematics),
            boards: vec_model(boards),
            writable: self.writable,
            ieee315_symbols: false,
            unsaved_changes: p.has_unsaved_changes(),
            buses: vec_model(buses),
            erc: checks.erc_data(read_only),
        }
    }
}
