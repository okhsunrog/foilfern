//! Exact, revision-bound model changes for declarative schematic updates.

use librepcb_core::fileio::FilePath;
use librepcb_core::geometry::Text;
use librepcb_core::project::{Mutation, Project, SchematicMutation, SymbolArtwork, SymbolRef};
use librepcb_core::types::Uuid;

use crate::commands::{EditComponent, MoveSymbol, WireGroup};
use crate::{Error, Operation, ProjectEditor, Result, Transaction};

/// A resolved change to an existing design.
pub enum DesignEdit {
    Component(EditComponent),
    Symbol(MoveSymbol),
    SymbolText { symbol: SymbolRef, text: Text },
    WireGroup(WireGroup),
    LibrarySymbolArtwork(SymbolArtwork),
}

impl DesignEdit {
    pub fn execute(self, tx: &mut Transaction<'_>) -> Result<()> {
        match self {
            Self::Component(command) => {
                tx.run(command)?;
            }
            Self::Symbol(command) => tx.run(command)?,
            Self::SymbolText { symbol, text } => {
                tx.apply(Mutation::Schematic(SchematicMutation::UpdateSymbolText {
                    symbol,
                    text,
                }))?;
            }
            Self::WireGroup(command) => tx.run(command)?,
            Self::LibrarySymbolArtwork(artwork) => {
                tx.apply(Mutation::SetLibrarySymbolArtwork(artwork))?
            }
        }
        Ok(())
    }
}

/// A proposal retained by the open project until it is applied or replaced.
#[derive(Debug)]
pub struct DesignPreview {
    pub id: Uuid,
    pub revision: u64,
    project_uuid: Uuid,
    path: Option<FilePath>,
    saved_at: Option<i64>,
    operations: Vec<Operation>,
}

impl DesignPreview {
    /// Executes the proposed edits on a detached copy and captures the exact patch.
    pub fn prepare<T, E: From<Error> + From<librepcb_core::project::Error>>(
        editor: &ProjectEditor,
        plan: impl FnOnce(&mut Transaction<'_>) -> Result<T, E>,
    ) -> Result<(Self, Project, T), E> {
        if editor.undo_stack().is_group_active() {
            return Err(Error::GroupActive.into());
        }
        let source = editor.project();
        let mut detached = ProjectEditor::with_source(source.snapshot()?, editor.source().clone());
        let result = detached.execute_atomic("Preview schematic update", plan)?;
        let preview_project = detached.project().snapshot()?;
        detached.undo()?;
        let operations = detached.take_redo_operations();
        Ok((
            Self {
                id: Uuid::new_random(),
                revision: source.revision(),
                project_uuid: source.uuid(),
                path: source.file_path(),
                saved_at: source.date_time().timestamp_nanos_opt(),
                operations,
            },
            preview_project,
            result,
        ))
    }

    /// Checks the source revision and save context within the owning session.
    pub fn matches(&self, project: &Project) -> bool {
        self.revision == project.revision()
            && self.project_uuid == project.uuid()
            && self.path == project.file_path()
            && self.saved_at == project.date_time().timestamp_nanos_opt()
    }

    /// Number of model mutations in the reviewed patch.
    pub fn mutation_count(&self) -> usize {
        self.operations
            .iter()
            .filter(|op| matches!(op, Operation::Mutation(_)))
            .count()
    }

    /// Number of model and file operations in the reviewed patch.
    pub fn operation_count(&self) -> usize {
        self.operations.len()
    }

    /// Replays the reviewed patch as one undo step.
    pub fn apply(self, editor: &mut ProjectEditor) -> Result<()> {
        if !self.matches(editor.project()) {
            return Err(crate::Error::InvalidArgument(
                "The schematic preview is stale.".into(),
            ));
        }
        if editor.undo_stack().is_group_active() {
            return Err(crate::Error::GroupActive);
        }
        if !self.operations.is_empty() {
            editor.execute_atomic("Apply schematic design", |tx| -> Result<()> {
                for operation in self.operations {
                    tx.apply_operation(operation)?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }
}
