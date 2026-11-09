//! The editor of one library element: port of the non-graphical parts of
//! libs/librepcb/editor/library/libraryeditortab.{h,cpp} and the element
//! tabs (`SymbolTab`, `PackageTab`, `ComponentTab`, `DeviceTab`): undo
//! stack, dirty state ("manual modifications"), interface check, message
//! approvals and saving.

use std::collections::BTreeSet;

use librepcb_core::fileio::{FileSystem, TransactionalDirectory};
use librepcb_core::library::LibraryCheckMessage;
use librepcb_core::rule_check::all_approvals;
use librepcb_core::serialization::SExpression;
use librepcb_core::types::PositiveLength;

use super::element::EditableElement;
use super::undo::ElementUndoStack;
use crate::error::{Error, Result};
use crate::undo_stack::HistoryEntry;

/// A command modifying a library element: plain parameters, executed as
/// one undo group (or as part of the active group). Port of the library
/// element `Cmd*` classes of upstream.
pub trait ElementCommand<E> {
    /// What the command returns (e.g. the UUIDs of added objects).
    type Output;

    /// The (translated) text of the undo group.
    fn text(&self) -> String;

    /// Validates the parameters and modifies `element`. On error, the
    /// editor restores the content from before the command.
    fn execute(self, element: &mut E) -> Result<Self::Output>;
}

/// A library element opened for editing, with its undo stack.
///
/// Every modification goes through [`execute()`](Self::execute) (one undo
/// group per command) or through an explicitly started group
/// ([`begin_group()`](Self::begin_group), [`modify()`](Self::modify),
/// [`commit_group()`](Self::commit_group)); the editor FSMs use the latter
/// for live previews. The grid interval and message approvals are changed
/// without undo (upstream "manual modifications").
#[derive(Debug)]
pub struct LibraryElementEditor<E: EditableElement> {
    element: E,
    stack: ElementUndoStack<E::Content>,
    /// Modifications outside the undo stack since the last save.
    manual_modifications: bool,
    /// Content at the last save (for the interface check).
    original: E::Content,
    /// Whether the element was newly created (never saved to its library).
    is_new: bool,
    /// All approvals the checks have ever produced (upstream
    /// `mSupportedApprovals`).
    supported_approvals: BTreeSet<SExpression>,
    /// Supported approvals which the last check did not produce anymore.
    disappeared_approvals: BTreeSet<SExpression>,
    /// Files in the element directory at the last save.
    saved_files: BTreeSet<String>,
}

impl<E: EditableElement> LibraryElementEditor<E> {
    /// Creates an editor for an existing (loaded) element.
    pub fn new(element: E) -> Self {
        let original = element.content();
        let saved_files = element.directory().files("").into_iter().collect();
        Self {
            saved_files,
            element,
            stack: ElementUndoStack::new(),
            manual_modifications: false,
            original,
            is_new: false,
            supported_approvals: BTreeSet::new(),
            disappeared_approvals: BTreeSet::new(),
        }
    }

    /// Creates an editor for a new element (no interface to break yet,
    /// dirty until saved).
    pub fn new_element(element: E) -> Self {
        let mut editor = Self::new(element);
        editor.is_new = true;
        editor.manual_modifications = true;
        editor
    }

    /// Opens the element stored in `directory` (upstream: the element tab
    /// constructors call `Symbol::open()` etc.).
    pub fn open(directory: TransactionalDirectory) -> Result<Self> {
        Ok(Self::new(E::open(directory)?))
    }

    /// The element.
    pub fn element(&self) -> &E {
        &self.element
    }

    /// Consumes the editor and returns the element.
    pub fn into_element(self) -> E {
        self.element
    }

    /// The content at the last save.
    pub fn original_content(&self) -> &E::Content {
        &self.original
    }

    /// Whether the element is newly created.
    pub fn is_new_element(&self) -> bool {
        self.is_new
    }

    /// Whether the element's directory is writable (upstream
    /// `isWritable()`: read-only libraries can only be viewed). New
    /// elements (in a temporary directory until saved into a library) are
    /// always writable.
    pub fn is_writable(&self) -> bool {
        self.is_new || self.element.directory().is_writable()
    }

    // --- Undo stack ---

    /// The undo stack.
    pub fn undo_stack(&self) -> &ElementUndoStack<E::Content> {
        &self.stack
    }

    /// Whether a group can be undone.
    pub fn can_undo(&self) -> bool {
        self.stack.can_undo()
    }

    /// Whether a group can be redone.
    pub fn can_redo(&self) -> bool {
        self.stack.can_redo()
    }

    /// The undo history.
    pub fn history(&self) -> Vec<HistoryEntry> {
        self.stack.history()
    }

    /// Identifier of the current state (changes with every modification).
    pub fn state_id(&self) -> u64 {
        self.stack.state_id()
    }

    /// Whether a group is active.
    pub fn is_group_active(&self) -> bool {
        self.stack.is_group_active()
    }

    /// Executes a command as one undo group labeled with its text, or as
    /// part of the active group. On error, the content is restored.
    pub fn execute<C: ElementCommand<E>>(&mut self, command: C) -> Result<C::Output> {
        if !self.is_writable() {
            return Err(read_only_error());
        }
        let nested = self.stack.is_group_active();
        let before = self.element.content();
        if !nested {
            self.stack.begin_group(command.text(), before.clone())?;
        }
        match command.execute(&mut self.element) {
            Ok(output) => {
                if nested {
                    self.stack.touch();
                } else {
                    let after = self.element.content();
                    self.stack.commit_group(after)?;
                }
                Ok(output)
            }
            Err(e) => {
                self.element.set_content(before);
                if !nested {
                    self.stack.abort_group()?;
                }
                Err(e)
            }
        }
    }

    /// Starts a group spanning several modifications (upstream
    /// `beginCmdGroup()`).
    pub fn begin_group(&mut self, text: impl Into<String>) -> Result<()> {
        if !self.is_writable() {
            return Err(read_only_error());
        }
        self.stack.begin_group(text, self.element.content())
    }

    /// Modifies the element inside the active group (upstream: edit
    /// commands with `immediate = true`, i.e. live previews).
    pub fn modify<R>(&mut self, f: impl FnOnce(&mut E) -> R) -> Result<R> {
        if !self.stack.is_group_active() {
            return Err(Error::NoGroupActive);
        }
        let result = f(&mut self.element);
        self.stack.touch();
        Ok(result)
    }

    /// Returns a snapshot of the current content (e.g. to go back to it
    /// inside the active group with [`restore()`](Self::restore)).
    pub fn snapshot(&self) -> E::Content {
        self.element.content()
    }

    /// Restores a snapshot inside the active group (upstream: discarding a
    /// temporary child command).
    pub fn restore(&mut self, content: E::Content) -> Result<()> {
        self.modify(|e| e.set_content(content))
    }

    /// The content at the start of the active group.
    pub fn group_start_content(&self) -> Option<&E::Content> {
        self.stack.active_before()
    }

    /// Commits the active group; returns `false` if nothing was modified
    /// (the group is dropped).
    pub fn commit_group(&mut self) -> Result<bool> {
        self.stack.commit_group(self.element.content())
    }

    /// Aborts the active group and restores the content (upstream
    /// `abortCmdGroup()`).
    pub fn abort_group(&mut self) -> Result<()> {
        let before = self.stack.abort_group()?;
        self.element.set_content(before);
        Ok(())
    }

    /// Undoes the last group; returns `false` if there was nothing to undo.
    pub fn undo(&mut self) -> Result<bool> {
        if self.stack.is_group_active() {
            return Err(Error::GroupActive);
        }
        Ok(match self.stack.undo() {
            Some(content) => {
                self.element.set_content(content);
                true
            }
            None => false,
        })
    }

    /// Redoes the next group; returns `false` if there was nothing to redo.
    pub fn redo(&mut self) -> Result<bool> {
        if self.stack.is_group_active() {
            return Err(Error::GroupActive);
        }
        Ok(match self.stack.redo() {
            Some(content) => {
                self.element.set_content(content);
                true
            }
            None => false,
        })
    }

    // --- Dirty state and manual modifications ---

    /// Whether the element has unsaved modifications (upstream
    /// `!mUndoStack->isClean() || mManualModificationsMade`).
    pub fn is_dirty(&self) -> bool {
        !self.stack.is_clean() || self.manual_modifications
    }

    /// Whether the interface of the element was broken since the last save
    /// (upstream `isInterfaceBroken()`; not updated while a group is active
    /// to avoid flickering during intermediate states, like upstream).
    pub fn is_interface_broken(&self) -> bool {
        if self.is_new {
            return false;
        }
        let current = match self.stack.active_before() {
            Some(before) => before.clone(),
            None => self.element.content(),
        };
        E::is_interface_broken(&self.original, &current)
    }

    /// Runs `f` on the element without undo, marking the element as
    /// modified (upstream: changes bypassing the undo stack, e.g. the grid
    /// interval). Must not modify the [`EditableElement::Content`].
    pub fn modify_manually<R>(&mut self, f: impl FnOnce(&mut E) -> R) -> R {
        self.manual_modifications = true;
        f(&mut self.element)
    }

    /// Approves or unapproves a check message (upstream
    /// `messageApprovalChanged()`, bypassing the undo stack).
    pub fn set_message_approved(&mut self, approval: &SExpression, approved: bool) -> bool {
        let modified = self
            .element
            .metadata_mut()
            .set_message_approved(approval, approved);
        if modified {
            self.manual_modifications = true;
        }
        modified
    }

    // --- Checks and saving ---

    /// Runs the library element checks (upstream
    /// `LibraryEditorTab::runChecks()`), memorizing the approvals of all
    /// messages seen so far to remove obsolete approvals on save.
    pub fn run_checks(&mut self) -> Result<Vec<LibraryCheckMessage>> {
        let messages = self.element.run_checks()?;
        let rule_msgs: Vec<_> = messages.iter().map(|m| m.to_message()).collect();
        let approvals = all_approvals(&rule_msgs);
        self.supported_approvals.extend(approvals.iter().cloned());
        self.disappeared_approvals = self
            .supported_approvals
            .difference(&approvals)
            .cloned()
            .collect();
        Ok(messages)
    }

    /// Saves the element to its directory and the file system to disk
    /// (upstream `SymbolTab::save()` etc.): runs the checks, removes the
    /// approvals of disappeared messages, writes the files and marks the
    /// state clean. Fails while a group is active.
    pub fn save(&mut self) -> Result<()> {
        if self.stack.is_group_active() {
            return Err(Error::GroupActive);
        }
        self.run_checks()?;
        let approvals: BTreeSet<SExpression> = self
            .element
            .metadata()
            .message_approvals()
            .difference(&self.disappeared_approvals)
            .cloned()
            .collect();
        self.element.metadata_mut().set_message_approvals(approvals);
        self.remove_obsolete_files()?;
        self.element.save()?;
        self.element.directory().file_system().save()?;
        self.stack.set_clean();
        self.manual_modifications = false;
        self.original = self.element.content();
        self.saved_files = self.element.directory().files("").into_iter().collect();
        self.is_new = false;
        Ok(())
    }

    /// Removes auxiliary files which are no longer referenced (see
    /// [`EditableElement::referenced_files()`]).
    fn remove_obsolete_files(&mut self) -> Result<()> {
        let referenced = E::referenced_files(&self.element.content());
        let mut candidates = E::referenced_files(&self.original);
        candidates.extend(
            self.element
                .directory()
                .files("")
                .into_iter()
                .filter(|f| !self.saved_files.contains(f)),
        );
        for file in candidates.difference(&referenced) {
            if self.element.directory().file_exists(file) {
                self.element.directory_mut().remove_file(file)?;
            }
        }
        Ok(())
    }

    /// Saves the element into `dest` ("save as" of a new element into a
    /// library, upstream: `saveTo()` if the path is outside the library)
    /// and the file system of `dest` to disk.
    pub fn save_to(&mut self, dest: &mut TransactionalDirectory) -> Result<()> {
        if self.stack.is_group_active() {
            return Err(Error::GroupActive);
        }
        self.run_checks()?;
        self.remove_obsolete_files()?;
        self.element.save_to(dest)?;
        self.element.directory().file_system().save()?;
        self.stack.set_clean();
        self.manual_modifications = false;
        self.original = self.element.content();
        self.saved_files = self.element.directory().files("").into_iter().collect();
        self.is_new = false;
        Ok(())
    }
}

impl<E: EditableElement + GridElement> LibraryElementEditor<E> {
    /// Sets the grid interval (upstream `setGridInterval()`: not through
    /// the undo stack, but marks the element as modified).
    pub fn set_grid_interval(&mut self, interval: PositiveLength) {
        if self.element.grid_interval() != interval {
            self.modify_manually(|e| e.set_grid_interval(interval));
        }
    }
}

/// Elements with a grid interval (symbols and packages).
pub trait GridElement {
    /// The grid interval.
    fn grid_interval(&self) -> PositiveLength;
    /// Sets the grid interval.
    fn set_grid_interval(&mut self, interval: PositiveLength) -> bool;
}

impl GridElement for librepcb_core::library::sym::Symbol {
    fn grid_interval(&self) -> PositiveLength {
        librepcb_core::library::sym::Symbol::grid_interval(self)
    }
    fn set_grid_interval(&mut self, interval: PositiveLength) -> bool {
        librepcb_core::library::sym::Symbol::set_grid_interval(self, interval)
    }
}

impl GridElement for librepcb_core::library::pkg::Package {
    fn grid_interval(&self) -> PositiveLength {
        librepcb_core::library::pkg::Package::grid_interval(self)
    }
    fn set_grid_interval(&mut self, interval: PositiveLength) -> bool {
        librepcb_core::library::pkg::Package::set_grid_interval(self, interval)
    }
}

fn read_only_error() -> Error {
    Error::InvalidArgument(librepcb_i18n::tr!(
        "librepcb::editor::LibraryEditorTab",
        "The library element is read-only."
    ))
}

static_assertions::assert_impl_all!(
    LibraryElementEditor<librepcb_core::library::sym::Symbol>: Send, Sync
);
static_assertions::assert_impl_all!(
    LibraryElementEditor<librepcb_core::library::pkg::Package>: Send, Sync
);
static_assertions::assert_impl_all!(
    LibraryElementEditor<librepcb_core::library::cmp::Component>: Send, Sync
);
static_assertions::assert_impl_all!(
    LibraryElementEditor<librepcb_core::library::dev::Device>: Send, Sync
);
