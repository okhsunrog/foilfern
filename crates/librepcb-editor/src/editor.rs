//! The project editor: a project with its undo stack and library element
//! source, and the transaction through which commands modify the project
//! (no direct upstream counterpart: upstream `ProjectEditor` owns the
//! project and the `UndoStack`, and each `UndoCommand` executes its child
//! commands itself).

use std::sync::Arc;

use librepcb_core::fileio::{FilePath, FileSystem, TransactionalDirectory};
use librepcb_core::project::{LibraryElementKind, Mutation, Project};
use librepcb_core::types::Uuid;

use crate::error::{Error, Result};
use crate::library_source::{LibraryElementSource, NoLibrarySource};
use crate::undo_stack::{LibraryElement, Operation, UndoStack};

/// A high-level editor command: plain parameters, executed as one undo
/// group (or as part of the active group).
pub trait Command {
    /// What the command returns (typically the UUIDs of the affected
    /// entities).
    type Output;

    /// The (translated) text of the undo group, e.g. "Add component".
    fn text(&self) -> String;

    /// Validates the parameters and performs the command through `tx`.
    /// On error, the editor rolls back everything the command applied.
    fn execute(self, tx: &mut Transaction<'_>) -> Result<Self::Output>;
}

/// The access of a running command to the project: every change goes
/// through [`apply()`](Self::apply) (or the library methods) and is
/// recorded in the active undo group.
pub struct Transaction<'a> {
    project: &'a mut Project,
    stack: &'a mut UndoStack,
    source: &'a dyn LibraryElementSource,
}

impl Transaction<'_> {
    /// Returns the project (current state, including the changes of this
    /// transaction).
    pub fn project(&self) -> &Project {
        self.project
    }

    /// Returns the library element source.
    pub fn source(&self) -> &dyn LibraryElementSource {
        self.source
    }

    /// Applies a mutation and records its inverse.
    pub fn apply(&mut self, mutation: Mutation) -> Result<()> {
        self.stack
            .execute(self.project, Operation::Mutation(mutation))
    }

    /// Applies several mutations (each recorded separately).
    pub fn apply_all(&mut self, mutations: impl IntoIterator<Item = Mutation>) -> Result<()> {
        for m in mutations {
            self.apply(m)?;
        }
        Ok(())
    }

    /// Adds an element to the project library (undoable).
    pub fn add_library_element(&mut self, element: LibraryElement) -> Result<()> {
        self.stack.execute(
            self.project,
            Operation::AddLibraryElement(Box::new(element)),
        )
    }

    /// Removes an element from the project library (undoable).
    pub fn remove_library_element(&mut self, kind: LibraryElementKind, uuid: Uuid) -> Result<()> {
        self.stack
            .execute(self.project, Operation::RemoveLibraryElement { kind, uuid })
    }

    /// Writes (`Some`) or removes (`None`) a file of the project directory
    /// (undoable; `path` is relative to the project directory).
    pub fn write_file(&mut self, path: impl Into<String>, content: Option<Vec<u8>>) -> Result<()> {
        self.stack.execute(
            self.project,
            Operation::WriteFile {
                path: path.into(),
                content,
            },
        )
    }

    /// Runs a nested command inside this transaction; if it fails, only its
    /// own changes are rolled back.
    pub fn run<C: Command>(&mut self, command: C) -> Result<C::Output> {
        let len = self.stack.active_group_len().unwrap_or(0);
        let result = command.execute(self);
        if result.is_err() {
            self.stack.rollback_active_to(self.project, len);
        }
        result
    }
}

/// A project opened for editing: the project, its undo stack and the
/// library element source for adding parts.
///
/// Concurrency: the application and the MCP server share the editor as
/// part of a [`SharedProject`](crate::SharedProject)
/// (`Arc<parking_lot::Mutex<OpenProject>>`); commands run under the lock,
/// readers take it only to extract data (see
/// `docs/project-model-design.md` §6). Locks are never held across
/// `.await`.
pub struct ProjectEditor {
    project: Project,
    undo_stack: UndoStack,
    source: Arc<dyn LibraryElementSource>,
    /// Modifications made outside the undo stack since the last save
    /// (upstream `ProjectEditor::setManualModificationsMade()`).
    manual_modifications: bool,
    /// Approvals of all ERC messages which occurred during this session
    /// (upstream `ProjectEditor::mSupportedErcApprovals`).
    supported_erc_approvals: std::collections::BTreeSet<librepcb_core::serialization::SExpression>,
}

static_assertions::assert_impl_all!(ProjectEditor: Send, Sync);

impl std::fmt::Debug for ProjectEditor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectEditor")
            .field("project", &self.project.file_name())
            .field("undo_stack", &self.undo_stack)
            .finish_non_exhaustive()
    }
}

/// Creates a new project in `directory` like upstream `Project::create()`:
/// the stroke fonts (`*.bene`) of `fonts_dir` (the application's
/// `share/librepcb/fontobene` resources directory) are copied into
/// `resources/fontobene/` before the project is created (the core's
/// `Project::create()` does not know the application resources yet).
/// Without fonts, upstream cannot open boards with texts.
pub fn create_project(
    mut directory: TransactionalDirectory,
    file_name: &str,
    fonts_dir: Option<&FilePath>,
) -> Result<Project> {
    if directory.file_exists(".librepcb-project") || directory.file_exists(file_name) {
        return Err(librepcb_core::project::Error::ProjectDirectoryNotEmpty(
            directory.abs_path(""),
        )
        .into());
    }
    if let Some(fonts_dir) = fonts_dir {
        let mut entries: Vec<_> = std::fs::read_dir(fonts_dir.as_path())
            .map_err(|e| {
                Error::InvalidArgument(format!("Could not read {}: {e}", fonts_dir.to_native()))
            })?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "bene") && p.is_file())
            .collect();
        entries.sort();
        for path in entries {
            let content = std::fs::read(&path).map_err(|e| {
                Error::InvalidArgument(format!("Could not read {}: {e}", path.display()))
            })?;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            directory.write(&format!("resources/fontobene/{name}"), &content)?;
        }
    }
    Ok(Project::create(directory, file_name, Uuid::new_random)?)
}

impl ProjectEditor {
    /// Creates an editor for `project` without library element source.
    pub fn new(project: Project) -> Self {
        Self::with_source(project, Arc::new(NoLibrarySource))
    }

    /// Creates an editor for `project` with the given library element
    /// source.
    pub fn with_source(project: Project, source: Arc<dyn LibraryElementSource>) -> Self {
        Self {
            project,
            undo_stack: UndoStack::new(),
            source,
            manual_modifications: false,
            supported_erc_approvals: Default::default(),
        }
    }

    /// Returns the project.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Consumes the editor and returns the project.
    pub fn into_project(self) -> Project {
        self.project
    }

    /// Returns the undo stack.
    pub fn undo_stack(&self) -> &UndoStack {
        &self.undo_stack
    }

    /// Returns the library element source.
    pub fn source(&self) -> &Arc<dyn LibraryElementSource> {
        &self.source
    }

    /// Replaces the library element source.
    pub fn set_source(&mut self, source: Arc<dyn LibraryElementSource>) {
        self.source = source;
    }

    /// Executes a command as one undo group labeled with its text, or as
    /// part of the active group if one was started with
    /// [`begin_group()`](Self::begin_group). On error, all changes of the
    /// command are rolled back.
    pub fn execute<C: Command>(&mut self, command: C) -> Result<C::Output> {
        if self.undo_stack.is_group_active() {
            let mut tx = self.transaction();
            return tx.run(command);
        }
        self.undo_stack.begin_group(command.text())?;
        let result = {
            let mut tx = self.transaction();
            command.execute(&mut tx)
        };
        match result {
            Ok(output) => {
                self.undo_stack.commit_group()?;
                Ok(output)
            }
            Err(e) => {
                self.undo_stack.abort_group(&mut self.project)?;
                Err(e)
            }
        }
    }

    /// Applies mutations as one undo group (the raw escape hatch for
    /// clients speaking the model's mutation format).
    pub fn apply_mutations(
        &mut self,
        text: impl Into<String>,
        mutations: Vec<Mutation>,
    ) -> Result<()> {
        self.execute(crate::commands::ApplyMutations {
            text: Some(text.into()),
            mutations,
        })
    }

    /// Starts a group spanning several commands (e.g. one agent action
    /// consisting of several tool calls).
    pub fn begin_group(&mut self, text: impl Into<String>) -> Result<()> {
        self.undo_stack.begin_group(text)
    }

    /// Commits the active group; returns `false` if it was empty (and
    /// therefore dropped).
    pub fn commit_group(&mut self) -> Result<bool> {
        self.undo_stack.commit_group()
    }

    /// Aborts the active group and rolls back its changes.
    pub fn abort_group(&mut self) -> Result<()> {
        self.undo_stack.abort_group(&mut self.project)
    }

    /// Returns the number of operations recorded in the active group
    /// (`None` if no group is active), e.g. to roll back to this point
    /// later with [`rollback_group_to()`](Self::rollback_group_to).
    pub fn active_group_len(&self) -> Option<usize> {
        self.undo_stack.active_group_len()
    }

    /// Rolls back the operations of the active group recorded after the
    /// first `len` ones (interactive tools replacing a live preview inside
    /// an open group, like upstream's temporary `setPosition()` calls).
    /// Does nothing if no group is active or it is not longer than `len`.
    pub fn rollback_group_to(&mut self, len: usize) {
        self.undo_stack.rollback_active_to(&mut self.project, len);
    }

    /// Rebuilds the air wires of the nets scheduled for rebuild on a board
    /// (upstream `Board::triggerAirWiresRebuild()`), also while a group is
    /// active (air wires are derived data: not recorded, rebuilt again
    /// after an abort or undo since the mutations schedule their nets).
    pub fn rebuild_air_wires(&mut self, board: librepcb_core::project::BoardId) -> Result<()> {
        Ok(self.project.rebuild_air_wires(board)?)
    }

    /// Undoes the last group; returns `false` if there was nothing to undo.
    pub fn undo(&mut self) -> Result<bool> {
        self.undo_stack.undo(&mut self.project)
    }

    /// Redoes the next group; returns `false` if there was nothing to redo.
    pub fn redo(&mut self) -> Result<bool> {
        self.undo_stack.redo(&mut self.project)
    }

    /// Runs `f` with mutable access to the project to update derived data
    /// which is not part of the undo history and not saved: plane
    /// fragments and air wires (e.g. [`Project::rebuild_planes()`],
    /// [`Project::run_drc()`], exports and output jobs, which rebuild
    /// planes first). Upstream does this outside the undo stack as well
    /// (`BoardPlaneFragmentsBuilder`, `Board::forceAirWiresRebuild()`).
    ///
    /// `f` must not apply model mutations: they would bypass the undo
    /// stack (checked in debug builds, logged as error otherwise). Fails
    /// while a group is active.
    pub fn update_derived_data<R>(&mut self, f: impl FnOnce(&mut Project) -> R) -> Result<R> {
        use librepcb_core::project::{BoardChange, Change, ChangesSince};
        if self.undo_stack.is_group_active() {
            return Err(Error::GroupActive);
        }
        let before = self.project.revision();
        let result = f(&mut self.project);
        let only_derived = match self.project.changes_since(before) {
            ChangesSince::Changes(changes) => changes.iter().all(|c| {
                matches!(
                    c,
                    Change::Board {
                        change: BoardChange::AirWires(..) | BoardChange::PlaneFragments(..),
                        ..
                    }
                )
            }),
            ChangesSince::Resync => false,
        };
        if !only_derived {
            log::error!("update_derived_data() modified more than derived data");
            debug_assert!(only_derived, "update_derived_data() modified model data");
        }
        Ok(result)
    }

    /// Shows or hides a plane (upstream `BI_Plane::setVisible()`, the
    /// "Visible" entry of the plane context menu): a view setting which is
    /// neither saved nor undoable, applied directly to the model (the
    /// change journal reports it, so scenes update). Returns whether it
    /// changed.
    pub fn set_plane_visible(
        &mut self,
        board: librepcb_core::project::BoardId,
        plane: librepcb_core::project::PlaneId,
        visible: bool,
    ) -> Result<bool> {
        use librepcb_core::project::{BoardMutation, Mutation};
        let Some(mut data) = self
            .project
            .board(board)
            .and_then(|b| b.plane(plane))
            .cloned()
        else {
            return Ok(false);
        };
        if data.visible() == visible {
            return Ok(false);
        }
        data.set_visible(visible);
        self.project
            .apply(Mutation::Board(BoardMutation::UpdatePlane {
                board,
                plane: data,
            }))?;
        Ok(true)
    }

    /// Removes the approvals of DRC messages which disappeared, after a
    /// check of `board` produced `result` (upstream
    /// `BoardEditor::setDrcResult()` calling
    /// `Board::updateDrcMessageApprovals()`, see
    /// [`Project::drc_approvals_update()`]). Like upstream, the change is
    /// not undoable: it is applied directly and marks the project as
    /// modified ([`has_manual_modifications()`](Self::has_manual_modifications)).
    /// Returns whether the approvals changed.
    pub fn update_drc_approvals(
        &mut self,
        board: librepcb_core::project::BoardId,
        result: &librepcb_core::project::board::drc::DrcResult,
    ) -> Result<bool> {
        let Some(mutation) = self.project.drc_approvals_update(board, result)? else {
            return Ok(false);
        };
        self.project.apply(mutation)?;
        self.manual_modifications = true;
        Ok(true)
    }

    /// Removes the approvals of ERC messages which disappeared during this
    /// session, after an ERC produced `messages` (upstream
    /// `ProjectEditor::runErc()`). Approvals of messages which never
    /// occurred during this session are kept (e.g. added by newer
    /// versions). Like upstream, the change is not undoable: it is applied
    /// directly and marks the project as modified. Returns whether the
    /// approvals changed.
    pub fn update_erc_approvals(
        &mut self,
        messages: &[librepcb_core::project::erc::ErcMessage],
    ) -> Result<bool> {
        let approvals: std::collections::BTreeSet<_> =
            messages.iter().map(|m| m.approval().clone()).collect();
        self.supported_erc_approvals
            .extend(approvals.iter().cloned());
        let supported = &self.supported_erc_approvals;
        let current = self.project.erc_approvals();
        let new: std::collections::BTreeSet<_> = current
            .iter()
            .filter(|a| !supported.contains(*a) || approvals.contains(*a))
            .cloned()
            .collect();
        if new == *current {
            return Ok(false);
        }
        self.project.apply(Mutation::SetErcApprovals(new))?;
        self.manual_modifications = true;
        Ok(true)
    }

    /// Whether the project was modified outside the undo stack since the
    /// last save (upstream `ProjectEditor::setManualModificationsMade()`).
    pub fn has_manual_modifications(&self) -> bool {
        self.manual_modifications
    }

    /// Marks the project as modified outside the undo stack (upstream
    /// `setManualModificationsMade()`); cleared by [`save()`](Self::save).
    pub fn set_manual_modifications_made(&mut self) {
        self.manual_modifications = true;
    }

    /// Whether the project is unmodified since the last save (neither
    /// through the undo stack nor manually).
    pub fn is_clean(&self) -> bool {
        self.undo_stack.is_clean() && !self.manual_modifications
    }

    /// Writes the project files and commits them to disk (upstream
    /// `ProjectEditor::saveProject()`), then marks the state clean. Fails
    /// while a group is active.
    pub fn save(&mut self) -> Result<()> {
        if self.undo_stack.is_group_active() {
            return Err(Error::GroupActive);
        }
        self.project.save()?;
        self.project.directory().file_system().save()?;
        self.undo_stack.set_clean();
        self.manual_modifications = false;
        Ok(())
    }

    /// Writes an autosave backup: all project files into the file system,
    /// which writes its modifications to `.autosave/` (upstream
    /// `ProjectEditor::autosaveProject()`). The undo stack stays unclean.
    pub fn autosave(&mut self) -> Result<()> {
        if self.undo_stack.is_group_active() {
            return Err(Error::GroupActive);
        }
        self.project.save()?;
        self.project.directory().file_system().autosave()?;
        Ok(())
    }

    fn transaction(&mut self) -> Transaction<'_> {
        Transaction {
            project: &mut self.project,
            stack: &mut self.undo_stack,
            source: self.source.as_ref(),
        }
    }
}
