//! A project opened for editing from disk: the [`ProjectEditor`] plus the
//! locked file system of the project directory and the save state
//! (upstream: the part of `ProjectEditor`/`ControlPanel::openProject()`
//! which opens and locks a project; no direct counterpart).
//!
//! The application and the MCP server share open projects as
//! [`SharedProject`] (`Arc<parking_lot::Mutex<OpenProject>>`, see
//! `docs/ui-design.md`, decision 3 and 4): whoever edits takes the lock,
//! runs one command (one undo group) and releases it; locks are never
//! held across `.await` or while waiting for another thread.

use std::sync::Arc;

use librepcb_core::fileio::LockHandler;
use librepcb_core::fileio::{
    FilePath, LockStatus, RestoreMode, TransactionalDirectory, TransactionalFileSystem,
};
use librepcb_core::project::loader::MigrationLog;
use librepcb_core::project::{Project, ProjectLoader};

use crate::{LibraryElementSource, ProjectEditor, Result};

/// An open project shared between threads (UI, MCP server, workers).
pub type SharedProject = Arc<parking_lot::Mutex<OpenProject>>;

/// Lock handler which overrides stale locks (left behind by a crashed
/// process) and refuses locks of running applications, so upstream
/// LibrePCB and this process never edit the same directory concurrently.
pub fn override_stale_locks(
    _dir: &FilePath,
    status: LockStatus,
    _owner: &str,
) -> librepcb_core::fileio::Result<bool> {
    Ok(status == LockStatus::StaleLock)
}

/// A project opened from disk, edited through its [`ProjectEditor`].
#[derive(Debug)]
pub struct OpenProject {
    /// The project editor (project, undo stack, library element source).
    pub editor: ProjectEditor,
    /// The file system of the project directory; while it exists, the
    /// directory is locked (upstream compatible `.lock` file).
    pub file_system: Arc<TransactionalFileSystem>,
    /// Revision at the last save (or open).
    pub saved_revision: u64,
    /// Whether the project was modified by the loader (file format
    /// upgrade) and not saved yet.
    pub upgraded: bool,
    /// The log of the file format upgrade (if [`upgraded`](Self::upgraded)
    /// by [`open_with()`](Self::open_with)).
    pub migration_log: Option<MigrationLog>,
}

static_assertions::assert_impl_all!(OpenProject: Send, Sync);

impl OpenProject {
    /// Wraps a loaded or newly created project.
    pub fn new(
        project: Project,
        file_system: Arc<TransactionalFileSystem>,
        upgraded: bool,
        source: Arc<dyn LibraryElementSource>,
    ) -> Self {
        Self {
            saved_revision: project.revision(),
            editor: ProjectEditor::with_source(project, source),
            file_system,
            upgraded,
            migration_log: None,
        }
    }

    /// Opens the project file `file_name` in the directory `dir`: locks
    /// the directory (stale locks are overridden, locks of running
    /// applications refused) and loads the project, upgrading older file
    /// formats in memory ([`upgraded`](Self::upgraded)).
    pub fn open(
        dir: &FilePath,
        file_name: &str,
        source: Arc<dyn LibraryElementSource>,
    ) -> Result<Self> {
        let mut handler = override_stale_locks;
        Self::open_with(dir, file_name, source, RestoreMode::No, Some(&mut handler))
    }

    /// Like [`open()`](Self::open), with the decisions about an autosave
    /// backup (`restore`) and an existing directory lock (`lock_handler`)
    /// left to the caller (upstream `GuiApplication::openProject()` asks
    /// the user). The file format migration log is kept in
    /// [`migration_log`](Self::migration_log).
    pub fn open_with(
        dir: &FilePath,
        file_name: &str,
        source: Arc<dyn LibraryElementSource>,
        restore: RestoreMode<'_>,
        lock_handler: Option<LockHandler<'_>>,
    ) -> Result<Self> {
        let fs = Arc::new(TransactionalFileSystem::open(
            dir,
            true,
            restore,
            lock_handler,
        )?);
        let mut loader = ProjectLoader::new();
        let project = loader.open(TransactionalDirectory::new(Arc::clone(&fs), ""), file_name)?;
        let migration_log = loader.migration_log().cloned();
        let mut open = Self::new(project, fs, migration_log.is_some(), source);
        open.migration_log = migration_log;
        Ok(open)
    }

    /// Moves the project behind a shared mutex.
    pub fn into_shared(self) -> SharedProject {
        Arc::new(parking_lot::Mutex::new(self))
    }

    /// Returns the project.
    pub fn project(&self) -> &Project {
        self.editor.project()
    }

    /// Whether there are changes which are not saved to disk yet: the undo
    /// stack is not in its clean (saved) state, or the file format was
    /// upgraded. Derived data (air wires, plane fragments) is not saved
    /// and does not count.
    pub fn has_unsaved_changes(&self) -> bool {
        self.upgraded || !self.editor.is_clean()
    }

    /// Returns the path of the project file (`*.lpp`).
    pub fn file_path(&self) -> String {
        self.project()
            .file_path()
            .map(|p| p.to_native())
            .unwrap_or_default()
    }

    /// Writes all project files and commits them to disk.
    pub fn save(&mut self) -> Result<()> {
        self.editor.save()?;
        self.saved_revision = self.project().revision();
        self.upgraded = false;
        self.migration_log = None;
        Ok(())
    }
}
