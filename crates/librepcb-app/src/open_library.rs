//! An opened library (`*.lplib` directory) with its file system, shared by
//! the library tab and the element tabs of the library.
//!
//! Port of libs/librepcb/editor/library/libraryeditor.{h,cpp} and of
//! `GuiApplication::openLibrary()`: the file system is opened once (with its
//! directory lock; libraries installed from the server are read-only), the
//! library metadata is edited through a snapshot undo stack
//! ([`ElementUndoStack`], upstream `CmdLibraryEdit` on the library's
//! `UndoStack`), and saving writes the library files and commits the file
//! system.
//!
//! Differences to upstream: the element tabs have their own undo stacks
//! (the element editors of `librepcb-editor`) instead of sharing the
//! library's.

use std::cell::{Ref, RefCell};
use std::collections::BTreeSet;
use std::sync::Arc;

use librepcb_app_ui as ui;
use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::library::Library;
use librepcb_core::library::{BaseMetadata, LibraryBaseElement};
use librepcb_core::serialization::SExpression;
use librepcb_core::types::{SimpleString, Uuid};
use librepcb_editor::library_editor::ElementUndoStack;

/// The editable content of a library (the snapshot of its undo stack).
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryContent {
    /// Common metadata (names, version, author, approvals, ...).
    pub metadata: BaseMetadata,
    /// URL.
    pub url: String,
    /// Dependencies.
    pub dependencies: BTreeSet<Uuid>,
    /// Icon (PNG).
    pub icon: Vec<u8>,
    /// Manufacturer.
    pub manufacturer: SimpleString,
}

/// Errors of opening or saving a library.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    /// File system error.
    #[error(transparent)]
    FileIo(#[from] librepcb_core::fileio::Error),
    /// Library error.
    #[error(transparent)]
    Library(#[from] librepcb_core::library::Error),
    /// Undo stack error.
    #[error(transparent)]
    Editor(#[from] librepcb_editor::Error),
}

/// An opened library (upstream `LibraryEditor`).
pub struct OpenLibrary {
    path: FilePath,
    fs: Arc<TransactionalFileSystem>,
    library: RefCell<Library>,
    undo: RefCell<ElementUndoStack<LibraryContent>>,
    /// Modifications outside the undo stack (message approvals).
    manual_modifications: RefCell<bool>,
}

impl OpenLibrary {
    /// Opens the library in `dir`; read-only if `read_only` (libraries
    /// installed from the server).
    pub fn open(dir: &FilePath, read_only: bool) -> Result<Self, LibraryError> {
        let fs = Arc::new(TransactionalFileSystem::open(
            dir,
            !read_only,
            librepcb_core::fileio::RestoreMode::No,
            None,
        )?);
        let library = Library::open(TransactionalDirectory::new(Arc::clone(&fs), ""))?;
        Ok(Self {
            path: dir.clone(),
            fs,
            library: RefCell::new(library),
            undo: RefCell::new(ElementUndoStack::new()),
            manual_modifications: RefCell::new(false),
        })
    }

    /// The library directory.
    pub fn path(&self) -> &FilePath {
        &self.path
    }

    /// The file system of the library (for the element tabs).
    pub fn file_system(&self) -> &Arc<TransactionalFileSystem> {
        &self.fs
    }

    /// A directory of the library (e.g. `sym/<uuid>`).
    pub fn directory(&self, relative: &str) -> TransactionalDirectory {
        TransactionalDirectory::new(Arc::clone(&self.fs), relative)
    }

    /// The path of an element directory relative to the library, if it is
    /// in this library.
    pub fn relative_path(&self, element: &FilePath) -> Option<String> {
        element
            .is_located_in_dir(&self.path)
            .then(|| element.to_relative(&self.path))
    }

    /// Whether the library is writable.
    pub fn is_writable(&self) -> bool {
        self.fs.is_writable()
    }

    /// The library.
    pub fn library(&self) -> Ref<'_, Library> {
        self.library.borrow()
    }

    /// The `LibraryData` of `Data.libraries`.
    pub fn ui_data(&self) -> ui::LibraryData {
        ui::LibraryData {
            valid: true,
            path: self.path.to_native().into(),
            name: self.library().metadata().name().to_string().into(),
            writable: self.is_writable(),
        }
    }

    /// The current content.
    pub fn content(&self) -> LibraryContent {
        let lib = self.library();
        LibraryContent {
            metadata: lib.metadata().clone(),
            url: lib.url().clone(),
            dependencies: lib.dependencies().clone(),
            icon: lib.icon().clone(),
            manufacturer: lib.manufacturer().clone(),
        }
    }

    fn restore(&self, content: LibraryContent) {
        let mut lib = self.library.borrow_mut();
        *lib.metadata_mut() = content.metadata;
        lib.set_url(content.url);
        lib.set_dependencies(content.dependencies);
        lib.set_icon(content.icon);
        lib.set_manufacturer(content.manufacturer);
    }

    /// Applies a modified content as one undo step labeled `text`
    /// (upstream `CmdLibraryEdit`); returns whether something changed.
    pub fn edit(&self, text: &str, content: LibraryContent) -> Result<bool, LibraryError> {
        let before = self.content();
        if before == content {
            return Ok(false);
        }
        let mut undo = self.undo.borrow_mut();
        undo.begin_group(text, before)?;
        self.restore(content.clone());
        undo.commit_group(content)?;
        Ok(true)
    }

    /// Approves or unapproves a check message (upstream: a manual
    /// modification, not undoable).
    pub fn set_message_approved(&self, approval: &SExpression, approved: bool) {
        let changed = self
            .library
            .borrow_mut()
            .metadata_mut()
            .set_message_approved(approval, approved);
        if changed {
            *self.manual_modifications.borrow_mut() = true;
        }
    }

    /// Whether a step can be undone.
    pub fn can_undo(&self) -> bool {
        self.undo.borrow().can_undo()
    }

    /// Whether a step can be redone.
    pub fn can_redo(&self) -> bool {
        self.undo.borrow().can_redo()
    }

    /// The text of the step to undo.
    pub fn undo_text(&self) -> String {
        self.undo
            .borrow()
            .undo_text()
            .unwrap_or_default()
            .to_owned()
    }

    /// The text of the step to redo.
    pub fn redo_text(&self) -> String {
        self.undo
            .borrow()
            .redo_text()
            .unwrap_or_default()
            .to_owned()
    }

    /// Undoes the last step.
    pub fn undo(&self) {
        let content = self.undo.borrow_mut().undo();
        if let Some(content) = content {
            self.restore(content);
        }
    }

    /// Redoes the next step.
    pub fn redo(&self) {
        let content = self.undo.borrow_mut().redo();
        if let Some(content) = content {
            self.restore(content);
        }
    }

    /// Whether there are unsaved changes (upstream `hasUnsavedChanges()`).
    pub fn has_unsaved_changes(&self) -> bool {
        *self.manual_modifications.borrow() || !self.undo.borrow().is_clean()
    }

    /// A value identifying the undo state (to detect changes).
    pub fn state_id(&self) -> u64 {
        self.undo.borrow().state_id() ^ u64::from(*self.manual_modifications.borrow())
    }

    /// Runs the library checks.
    pub fn run_checks(
        &self,
    ) -> Result<Vec<librepcb_core::library::LibraryCheckMessage>, LibraryError> {
        Ok(self.library().run_checks()?)
    }

    /// Saves the library files and commits the file system (upstream
    /// `LibraryEditor::save()`); `obsolete_approvals` are removed first.
    pub fn save(&self, obsolete_approvals: &BTreeSet<SExpression>) -> Result<(), LibraryError> {
        {
            let mut lib = self.library.borrow_mut();
            let approvals = lib
                .metadata()
                .message_approvals()
                .difference(obsolete_approvals)
                .cloned()
                .collect();
            lib.metadata_mut().set_message_approvals(approvals);
            lib.save()?;
        }
        self.fs.save()?;
        self.undo.borrow_mut().set_clean();
        *self.manual_modifications.borrow_mut() = false;
        Ok(())
    }

    /// Discards unsaved changes (upstream `discardUnsavedChanges()`).
    pub fn discard_unsaved_changes(&self) {
        loop {
            let clean = self.undo.borrow().is_clean();
            if clean || !self.can_undo() {
                break;
            }
            self.undo();
        }
        self.undo.borrow_mut().clear();
        *self.manual_modifications.borrow_mut() = false;
    }
}
