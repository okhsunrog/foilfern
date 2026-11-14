//! Exact, revision-bound model changes for declarative schematic updates.

use librepcb_core::fileio::FilePath;
use librepcb_core::geometry::Text;
use librepcb_core::project::board_requirements::Requirements as BoardRequirements;
use librepcb_core::project::schematic_requirements::{Finding, Requirements};
use librepcb_core::project::{BoardId, Mutation, Project, SchematicMutation, SymbolRef};
use librepcb_core::types::Uuid;

use crate::board_requirements::RoutingGuard;
use crate::commands::{EditComponent, MoveSymbol, WireGroup};
use crate::{Error, Operation, ProjectEditor, Result, Transaction};

/// A resolved change to an existing design.
pub enum DesignEdit {
    Component(EditComponent),
    Symbol(MoveSymbol),
    SymbolText { symbol: SymbolRef, text: Text },
    WireGroup(WireGroup),
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
    requirements: Requirements,
    requirement_findings: Vec<Finding>,
    board_review: Option<BoardReview>,
}

#[derive(Debug)]
struct BoardReview {
    board: BoardId,
    before: BoardRequirements,
    after: BoardRequirements,
    blockers: Vec<String>,
}

/// The source context and independent model used to prepare one preview.
pub struct DesignPreviewJob {
    detached: ProjectEditor,
    revision: u64,
    project_uuid: Uuid,
    path: Option<FilePath>,
    saved_at: Option<i64>,
}

impl DesignPreviewJob {
    /// Captures a snapshot and revision while the live project is locked.
    pub fn capture(editor: &ProjectEditor) -> Result<Self> {
        if editor.undo_stack().is_group_active() {
            return Err(Error::GroupActive);
        }
        let source = editor.project();
        Ok(Self {
            detached: ProjectEditor::with_source(source.snapshot()?, editor.source().clone()),
            revision: source.revision(),
            project_uuid: source.uuid(),
            path: source.file_path(),
            saved_at: source.date_time().timestamp_nanos_opt(),
        })
    }

    pub fn project(&self) -> &Project {
        self.detached.project()
    }

    /// Executes the plan and returns the exact patch, baseline and proposed model.
    pub fn prepare<T, E: From<Error> + From<librepcb_core::project::Error>>(
        self,
        plan: impl FnOnce(&mut Transaction<'_>) -> Result<T, E>,
    ) -> Result<(DesignPreview, Project, Project, T), E> {
        let mut detached = self.detached;
        let requirements = Requirements::load(detached.project()).map_err(Error::from)?;
        let result = detached.execute_atomic("Preview schematic update", plan)?;
        let preview_project = detached.project().snapshot()?;
        let requirement_findings = requirements.evaluate(&preview_project);
        detached.undo()?;
        let operations = detached.take_redo_operations();
        Ok((
            DesignPreview {
                id: Uuid::new_random(),
                revision: self.revision,
                project_uuid: self.project_uuid,
                path: self.path,
                saved_at: self.saved_at,
                operations,
                requirements,
                requirement_findings,
                board_review: None,
            },
            detached.into_project(),
            preview_project,
            result,
        ))
    }
}

impl DesignPreview {
    /// Executes the proposed edits on a detached copy and captures the exact patch.
    pub fn prepare<T, E: From<Error> + From<librepcb_core::project::Error>>(
        editor: &ProjectEditor,
        plan: impl FnOnce(&mut Transaction<'_>) -> Result<T, E>,
    ) -> Result<(Self, Project, T), E> {
        let (preview, _, project, result) = DesignPreviewJob::capture(editor)?.prepare(plan)?;
        Ok((preview, project, result))
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

    pub fn requirement_findings(&self) -> &[Finding] {
        &self.requirement_findings
    }

    /// Includes the exact mutations from a detached routing trial in the
    /// reviewed design patch. The trial starts from the proposed board.
    pub fn append_board_routing(&mut self, mutations: Vec<Mutation>) {
        self.operations
            .extend(mutations.into_iter().map(Operation::Mutation));
    }

    /// Binds a board placement review to the exact patch and requirements.
    pub fn review_board(
        &mut self,
        board: BoardId,
        before: &Project,
        after: &Project,
        blockers: Vec<String>,
    ) -> Result<()> {
        self.board_review = Some(BoardReview {
            board,
            before: BoardRequirements::load(before)?,
            after: BoardRequirements::load(after)?,
            blockers,
        });
        Ok(())
    }

    /// Replays the reviewed patch as one undo step.
    pub fn apply(self, editor: &mut ProjectEditor) -> Result<()> {
        if !self.matches(editor.project()) {
            return Err(crate::Error::InvalidArgument(
                "The design preview is stale.".into(),
            ));
        }
        if editor.undo_stack().is_group_active() {
            return Err(crate::Error::GroupActive);
        }
        if Requirements::load(editor.project())? != self.requirements {
            return Err(Error::DesignRequirementsChanged);
        }
        check_requirements(&self.requirement_findings)?;
        let board_guard = self
            .board_review
            .map(|review| -> Result<_> {
                let project = editor.project();
                if BoardRequirements::load(project)? != review.before {
                    return Err(Error::BoardDesignRequirementsChanged);
                }
                if !review.blockers.is_empty() {
                    return Err(Error::BoardDesignBlocked(review.blockers.join(", ")));
                }
                let guard =
                    RoutingGuard::capture_with(project, review.board, review.after.clone())?;
                Ok((guard, review.after))
            })
            .transpose()?;
        if !self.operations.is_empty() {
            editor.execute_atomic("Apply design", |tx| -> Result<()> {
                for operation in self.operations {
                    tx.apply_operation(operation)?;
                }
                check_requirements(&self.requirements.evaluate(tx.project()))?;
                if let Some((guard, expected)) = board_guard {
                    if BoardRequirements::load(tx.project())? != expected {
                        return Err(Error::BoardDesignRequirementsChanged);
                    }
                    guard.enforce(tx.project())?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }
}

fn check_requirements(findings: &[Finding]) -> Result<()> {
    let blockers = findings
        .iter()
        .filter(|finding| finding.blocks_apply())
        .map(|finding| finding.requirement.id.as_str())
        .collect::<Vec<_>>();
    if blockers.is_empty() {
        Ok(())
    } else {
        Err(Error::DesignRequirementsBlocked(blockers.join(", ")))
    }
}
