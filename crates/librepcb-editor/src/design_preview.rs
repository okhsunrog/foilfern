//! Exact, revision-bound model changes for declarative schematic updates.

use librepcb_core::fileio::FilePath;
use librepcb_core::project::{Mutation, Project};
use librepcb_core::types::Uuid;

use crate::commands::{EditComponent, MoveSymbol};
use crate::{ProjectEditor, Result};

/// A resolved change to an existing design.
pub enum DesignEdit {
    Component(EditComponent),
    Symbol(MoveSymbol),
}

/// A proposal retained by the open project until it is applied or replaced.
#[derive(Debug)]
pub struct DesignPreview {
    pub id: Uuid,
    pub revision: u64,
    project_uuid: Uuid,
    path: Option<FilePath>,
    saved_at: Option<i64>,
    mutations: Vec<Mutation>,
}

impl DesignPreview {
    /// Executes the proposed edits on a detached copy and captures the exact patch.
    pub fn prepare(editor: &ProjectEditor, edits: Vec<DesignEdit>) -> Result<(Self, Project)> {
        if editor.undo_stack().is_group_active() {
            return Err(crate::Error::GroupActive);
        }
        let source = editor.project();
        let mut detached = ProjectEditor::new(source.snapshot()?);
        detached.execute_atomic("Preview schematic update", |tx| -> Result<()> {
            for edit in edits {
                match edit {
                    DesignEdit::Component(command) => {
                        tx.run(command)?;
                    }
                    DesignEdit::Symbol(command) => tx.run(command)?,
                }
            }
            Ok(())
        })?;
        let mutations = if detached.undo()? {
            let mutations = detached.undo_stack().redo_mutations()?;
            detached.redo()?;
            mutations
        } else {
            Vec::new()
        };
        Ok((
            Self {
                id: Uuid::new_random(),
                revision: source.revision(),
                project_uuid: source.uuid(),
                path: source.file_path(),
                saved_at: source.date_time().timestamp_nanos_opt(),
                mutations,
            },
            detached.into_project(),
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
        self.mutations.len()
    }

    /// Replays the reviewed patch as one undo step.
    pub fn apply(&self, editor: &mut ProjectEditor) -> Result<()> {
        if !self.matches(editor.project()) {
            return Err(crate::Error::InvalidArgument(
                "The schematic preview is stale.".into(),
            ));
        }
        if editor.undo_stack().is_group_active() {
            return Err(crate::Error::GroupActive);
        }
        if !self.mutations.is_empty() {
            editor.apply_mutations("Apply schematic design", self.mutations.clone())?;
        }
        Ok(())
    }
}
