//! Port of libs/librepcb/editor/undostack.{h,cpp}, undocommand.{h,cpp} and
//! undocommandgroup.{h,cpp}.
//!
//! Differences to upstream:
//!
//! - There are no command objects: a group records the inverses of the
//!   [`Operation`]s applied while it was active (project [`Mutation`]s,
//!   whose inverses the model returns, and project library additions and
//!   removals, which are file backed and therefore not `Mutation`s). Undo
//!   applies the recorded inverses in reverse order and keeps *their*
//!   inverses for redo, and vice versa.
//! - Everything happens in groups (upstream `execCmd()` of a single command
//!   is a group with one operation). An empty group is dropped on commit,
//!   like upstream.
//! - The redo history is discarded when a group is committed (upstream:
//!   when it is started, so an aborted group also discards it).
//! - The Qt signal `stateModified()` is replaced by [`UndoStack::state_id()`]
//!   (changes whenever the state changes) and the project's change journal.
//! - If undoing or redoing fails (which violates the model invariant that
//!   inverses always succeed), the already applied steps are rolled back and
//!   the whole history is cleared, since it no longer matches the model.

use librepcb_core::fileio::TransactionalFileSystem;
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::library::cmp::Component;
use librepcb_core::library::dev::Device;
use librepcb_core::library::pkg::Package;
use librepcb_core::library::sym::Symbol;
use librepcb_core::project::{LibraryElementKind, Mutation, Project};
use librepcb_core::types::Uuid;

use crate::error::{Error, Result};

/// A project library element (file backed, owns its directory).
#[derive(Debug)]
pub enum LibraryElement {
    /// A symbol.
    Symbol(Symbol),
    /// A package.
    Package(Package),
    /// A component.
    Component(Component),
    /// A device.
    Device(Device),
}

impl LibraryElement {
    /// Returns the kind of the element.
    pub fn kind(&self) -> LibraryElementKind {
        match self {
            Self::Symbol(_) => LibraryElementKind::Symbol,
            Self::Package(_) => LibraryElementKind::Package,
            Self::Component(_) => LibraryElementKind::Component,
            Self::Device(_) => LibraryElementKind::Device,
        }
    }

    /// Returns the UUID of the element.
    pub fn uuid(&self) -> Uuid {
        match self {
            Self::Symbol(e) => e.metadata().uuid(),
            Self::Package(e) => e.metadata().uuid(),
            Self::Component(e) => e.metadata().uuid(),
            Self::Device(e) => e.metadata().uuid(),
        }
    }
}

/// A reversible change of a project: what an undo group records.
// Mutations dominate the recorded operations, boxing them would not help.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Operation {
    /// A mutation of the project model.
    Mutation(Mutation),
    /// Adds an element to the project library (upstream
    /// `CmdProjectLibraryAddElement`).
    AddLibraryElement(Box<LibraryElement>),
    /// Removes an element from the project library (upstream
    /// `CmdProjectLibraryRemoveElement`); its files are moved to a
    /// temporary file system and kept by the inverse.
    RemoveLibraryElement {
        /// Kind of the element.
        kind: LibraryElementKind,
        /// UUID of the element.
        uuid: Uuid,
    },
    /// Writes (`Some`) or removes (`None`) a file of the project directory
    /// (upstream: the file handling of `CmdSchematicImageAdd` and
    /// `CmdSchematicImageRemove`); the path is relative to the project
    /// directory.
    WriteFile {
        /// The path of the file.
        path: String,
        /// The new content, `None` to remove the file.
        content: Option<Vec<u8>>,
    },
}

impl Operation {
    /// Applies the operation to `project` and returns its inverse; on error
    /// nothing changed.
    pub fn apply(self, project: &mut Project) -> Result<Operation> {
        Ok(match self {
            Self::Mutation(m) => Self::Mutation(project.apply(m)?),
            Self::WriteFile { path, content } => {
                let dir = project.directory();
                let fs = dir.file_system();
                let full = TransactionalFileSystem::clean_path(&format!("{}/{path}", dir.path()));
                let old = fs.read_if_exists(&full)?;
                match &content {
                    Some(data) => fs.write(&full, data)?,
                    None if old.is_some() => fs.remove_file(&full)?,
                    None => {}
                }
                Self::WriteFile { path, content: old }
            }
            Self::AddLibraryElement(element) => {
                let (kind, uuid) = (element.kind(), element.uuid());
                match *element {
                    LibraryElement::Symbol(e) => project.add_library_symbol(e)?,
                    LibraryElement::Package(e) => project.add_library_package(e)?,
                    LibraryElement::Component(e) => project.add_library_component(e)?,
                    LibraryElement::Device(e) => project.add_library_device(e)?,
                }
                Self::RemoveLibraryElement { kind, uuid }
            }
            Self::RemoveLibraryElement { kind, uuid } => {
                let element = match kind {
                    LibraryElementKind::Symbol => {
                        LibraryElement::Symbol(project.remove_library_symbol(&uuid)?)
                    }
                    LibraryElementKind::Package => {
                        LibraryElement::Package(project.remove_library_package(&uuid)?)
                    }
                    LibraryElementKind::Component => {
                        LibraryElement::Component(project.remove_library_component(&uuid)?)
                    }
                    LibraryElementKind::Device => {
                        LibraryElement::Device(project.remove_library_device(&uuid)?)
                    }
                };
                Self::AddLibraryElement(Box::new(element))
            }
        })
    }
}

/// Applies `ops` in order and returns their inverses in the order to apply
/// them (reversed). On error, the already applied operations are rolled
/// back and the error is returned.
fn apply_all(project: &mut Project, ops: Vec<Operation>) -> Result<Vec<Operation>> {
    let mut inverses = Vec::with_capacity(ops.len());
    for op in ops {
        match op.apply(project) {
            Ok(inverse) => inverses.push(inverse),
            Err(e) => {
                rollback(project, inverses);
                return Err(e);
            }
        }
    }
    inverses.reverse();
    Ok(inverses)
}

/// Applies the inverses (in execution order) in reverse order.
fn rollback(project: &mut Project, inverses: Vec<Operation>) {
    for inverse in inverses.into_iter().rev() {
        // The inverse of a successful operation applied right afterwards
        // always succeeds (model invariant).
        if let Err(e) = inverse.apply(project) {
            log::error!("Rollback of an operation failed: {e}");
            debug_assert!(false, "rollback failed: {e}");
        }
    }
}

/// A committed group.
#[derive(Debug)]
struct Entry {
    /// Unique identifier (for [`UndoStack::state_id()`]).
    id: u64,
    text: String,
    /// The operations to apply to undo the group if it is done, or to redo
    /// it if it is undone.
    ops: Vec<Operation>,
}

/// The active (not yet committed) group.
#[derive(Debug)]
struct ActiveGroup {
    id: u64,
    text: String,
    /// Inverses of the applied operations, in execution order.
    inverses: Vec<Operation>,
}

/// An entry of the undo history (see [`UndoStack::history()`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HistoryEntry {
    /// The text of the group, e.g. "Add component".
    pub text: String,
    /// Whether the group is done (can be undone), otherwise it is undone
    /// (can be redone).
    pub done: bool,
}

/// The undo stack of a project editor: labeled groups of reversible
/// operations with undo/redo and a clean (saved) state.
///
/// Every method that changes the project takes it as argument; the stack
/// must always be used with the same project.
#[derive(Debug, Default)]
pub struct UndoStack {
    entries: Vec<Entry>,
    /// Number of done entries (upstream `mCurrentIndex`).
    index: usize,
    /// Index of the clean state, `None` if it no longer exists.
    clean_index: Option<usize>,
    active: Option<ActiveGroup>,
    next_id: u64,
}

static_assertions::assert_impl_all!(UndoStack: Send, Sync);

impl UndoStack {
    /// Creates an empty (clean) stack.
    pub fn new() -> Self {
        Self {
            clean_index: Some(0),
            ..Self::default()
        }
    }

    /// Returns the text of the group which would be undone, if any.
    pub fn undo_text(&self) -> Option<&str> {
        self.can_undo()
            .then(|| self.entries[self.index - 1].text.as_str())
    }

    /// Returns the text of the group which would be redone, if any.
    pub fn redo_text(&self) -> Option<&str> {
        self.can_redo()
            .then(|| self.entries[self.index].text.as_str())
    }

    /// Whether a group can be undone (not while a group is active).
    pub fn can_undo(&self) -> bool {
        self.active.is_none() && self.index > 0
    }

    /// Whether a group can be redone (not while a group is active).
    pub fn can_redo(&self) -> bool {
        self.active.is_none() && self.index < self.entries.len()
    }

    /// Returns an identifier of the current state: it changes whenever a
    /// group is committed, undone, redone or an operation is added to the
    /// active group, and returns to a previous value when the same state is
    /// reached again by undo/redo (upstream `getUniqueStateId()`).
    pub fn state_id(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for entry in &self.entries[..self.index] {
            entry.id.hash(&mut hasher);
        }
        if let Some(active) = &self.active {
            active.id.hash(&mut hasher);
            active.inverses.len().hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Whether the current state is the clean (saved) state.
    pub fn is_clean(&self) -> bool {
        self.clean_index == Some(self.index)
    }

    /// Marks the current state as clean (e.g. after saving).
    pub fn set_clean(&mut self) {
        self.clean_index = Some(self.index);
    }

    /// Whether a group is active.
    pub fn is_group_active(&self) -> bool {
        self.active.is_some()
    }

    /// Returns the text of the active group, if any.
    pub fn active_group_text(&self) -> Option<&str> {
        self.active.as_ref().map(|g| g.text.as_str())
    }

    /// Returns the number of operations recorded in the active group.
    pub fn active_group_len(&self) -> Option<usize> {
        self.active.as_ref().map(|g| g.inverses.len())
    }

    /// Returns all groups, oldest first (done ones, then undone ones).
    pub fn history(&self) -> Vec<HistoryEntry> {
        self.entries
            .iter()
            .enumerate()
            .map(|(i, e)| HistoryEntry {
                text: e.text.clone(),
                done: i < self.index,
            })
            .collect()
    }

    /// Returns the number of done groups.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Starts a new group (upstream `beginCmdGroup()`).
    pub fn begin_group(&mut self, text: impl Into<String>) -> Result<()> {
        if self.active.is_some() {
            return Err(Error::GroupActive);
        }
        self.next_id += 1;
        self.active = Some(ActiveGroup {
            id: self.next_id,
            text: text.into(),
            inverses: Vec::new(),
        });
        Ok(())
    }

    /// Applies `op` to `project` and records its inverse in the active
    /// group (upstream `appendToCmdGroup()`). On error nothing changed.
    pub fn execute(&mut self, project: &mut Project, op: Operation) -> Result<()> {
        let active = self.active.as_mut().ok_or(Error::NoGroupActive)?;
        let inverse = op.apply(project)?;
        active.inverses.push(inverse);
        Ok(())
    }

    /// Commits the active group (upstream `commitCmdGroup()`). Returns
    /// `false` if it was empty and therefore dropped.
    pub fn commit_group(&mut self) -> Result<bool> {
        let active = self.active.take().ok_or(Error::NoGroupActive)?;
        if active.inverses.is_empty() {
            return Ok(false);
        }
        // The clean state is no longer reachable if it was undone.
        if self.clean_index.is_some_and(|i| i > self.index) {
            self.clean_index = None;
        }
        self.entries.truncate(self.index);
        let mut ops = active.inverses;
        ops.reverse();
        self.entries.push(Entry {
            id: active.id,
            text: active.text,
            ops,
        });
        self.index += 1;
        Ok(true)
    }

    /// Aborts the active group and rolls back all its operations (upstream
    /// `abortCmdGroup()`).
    pub fn abort_group(&mut self, project: &mut Project) -> Result<()> {
        let active = self.active.take().ok_or(Error::NoGroupActive)?;
        rollback(project, active.inverses);
        Ok(())
    }

    /// Rolls back the operations of the active group after the first `len`
    /// ones (for nested commands which failed).
    pub(crate) fn rollback_active_to(&mut self, project: &mut Project, len: usize) {
        if let Some(active) = self.active.as_mut()
            && len < active.inverses.len()
        {
            let tail = active.inverses.split_off(len);
            rollback(project, tail);
        }
    }

    /// Undoes the last done group. Returns `false` if there is nothing to
    /// undo (or a group is active).
    pub fn undo(&mut self, project: &mut Project) -> Result<bool> {
        if !self.can_undo() {
            return Ok(false);
        }
        let ops = std::mem::take(&mut self.entries[self.index - 1].ops);
        match apply_all(project, ops) {
            Ok(redo_ops) => {
                self.entries[self.index - 1].ops = redo_ops;
                self.index -= 1;
                Ok(true)
            }
            Err(e) => {
                log::error!("Undo failed, clearing the undo stack: {e}");
                self.clear_history();
                Err(e)
            }
        }
    }

    /// Redoes the next undone group. Returns `false` if there is nothing to
    /// redo (or a group is active).
    pub fn redo(&mut self, project: &mut Project) -> Result<bool> {
        if !self.can_redo() {
            return Ok(false);
        }
        let ops = std::mem::take(&mut self.entries[self.index].ops);
        match apply_all(project, ops) {
            Ok(undo_ops) => {
                self.entries[self.index].ops = undo_ops;
                self.index += 1;
                Ok(true)
            }
            Err(e) => {
                log::error!("Redo failed, clearing the undo stack: {e}");
                self.clear_history();
                Err(e)
            }
        }
    }

    /// Aborts the active group (if any) and removes all groups (upstream
    /// `clear()`); the current state becomes the clean state.
    pub fn clear(&mut self, project: &mut Project) {
        if self.active.is_some()
            && let Err(e) = self.abort_group(project)
        {
            log::error!("Failed to abort the currently active command group: {e}");
        }
        self.clear_history();
        self.clean_index = Some(0);
    }

    fn clear_history(&mut self) {
        self.entries.clear();
        self.clean_index = if self.is_clean() { Some(0) } else { None };
        self.index = 0;
    }
}
