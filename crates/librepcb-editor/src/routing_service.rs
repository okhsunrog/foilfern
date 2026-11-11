//! Reviewed routing candidates (original implementation, no upstream counterpart).
//!
//! A native detached snapshot preserves unsaved model data and embedded assets.
//! Builtin routing and full DRC run on that snapshot, never on the live editor.
//! Candidates freeze exact model mutations, including generated UUIDs. Applying
//! one checks the owning service and source revision, then replays one undo group.
//! No general plugin abstraction is needed for this first builtin-only slice.

use std::collections::BTreeMap;
use std::sync::Arc;

use librepcb_core::fileio::FilePath;
use librepcb_core::project::{BoardId, Mutation, Project};
use librepcb_core::types::Uuid;
use serde::Serialize;

use crate::ProjectEditor;
use crate::commands::{Autoroute, AutorouteResult, EditRoutingGeometry};
use crate::routing_validation::{RoutingSnapshot, RoutingValidation, capture_routing_state};

mod region;
pub use region::RegionalRoutingRequest;

/// Bounded session-local storage; callers explicitly discard unwanted candidates.
pub const MAX_CANDIDATES: usize = 8;
/// Maximum frozen patch size. Oversized proposals are refused, never truncated.
pub const MAX_CANDIDATE_MUTATIONS: usize = 100_000;

/// Candidate lifecycle errors, independent of MCP.
#[derive(Debug, thiserror::Error)]
pub enum RoutingError {
    #[error(transparent)]
    Editor(#[from] crate::Error),
    #[error("Routing candidate not found: {0}")]
    NotFound(Uuid),
    #[error("Routing candidate belongs to a different project session.")]
    WrongSession,
    #[error("Project save time or location changed; prepare a new routing candidate.")]
    StaleContext,
    #[error("Routing candidate is stale: expected revision {expected}, current {actual}.")]
    StaleRevision { expected: u64, actual: u64 },
    #[error("Routing candidate refused: {0}")]
    Refused(String),
}

/// Immutable metadata shared by prepare, preview and apply responses.
#[derive(Debug, Clone, Serialize)]
pub struct RoutingCandidateInfo {
    pub candidate: Uuid,
    pub project: Uuid,
    pub base_revision: u64,
    pub board: BoardId,
    #[serde(flatten)]
    pub details: RoutingCandidateDetails,
    pub mutation_count: usize,
    pub validation: RoutingValidation,
}

/// The operation that produced the patch. Builtin retains its existing wire
/// representation; edits report their explicit request and the shared checks.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum RoutingCandidateDetails {
    Builtin {
        backend: &'static str,
        request: Autoroute,
        routing: Box<AutorouteResult>,
    },
    Edit {
        edit: EditRoutingGeometry,
        #[serde(skip_serializing_if = "Option::is_none")]
        route: Option<Box<RoutingEditTrial>>,
    },
    Regional {
        regional: Box<RegionalRoutingRequest>,
        routing: Box<AutorouteResult>,
    },
}

/// Result of routing selected nets on the edited snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct RoutingEditTrial {
    pub request: Autoroute,
    pub routing: AutorouteResult,
}

impl RoutingCandidateDetails {
    fn validate(&self, before: RoutingSnapshot, after: RoutingSnapshot) -> RoutingValidation {
        let mut validation = RoutingValidation::compare(before, after);
        if !matches!(self, Self::Builtin { .. }) && !validation.unrouted_regressions.is_empty() {
            validation.acceptable = false;
        }
        validation
    }
}

#[derive(Debug)]
enum RoutingWork {
    Builtin(Autoroute),
    Edit {
        edit: EditRoutingGeometry,
        route: Option<Autoroute>,
    },
    Regional(RegionalRoutingRequest),
}

/// Date and path placeholders affect physical board text without necessarily
/// publishing a model mutation (for example on save or save-as).
#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceContext {
    time: String,
    file: Option<FilePath>,
    directory: Option<FilePath>,
}

impl SourceContext {
    fn of(project: &Project) -> Self {
        Self {
            time: project.date_time().to_rfc3339(),
            file: project.file_path(),
            directory: project.path(),
        }
    }
}

/// An exact checked patch. Only RoutingJob can construct it; clients cannot
/// replace its geometry or validation. It holds no live project/file handles.
#[derive(Debug)]
pub struct RoutingCandidate {
    owner: Uuid,
    source_context: SourceContext,
    info: RoutingCandidateInfo,
    mutations: Vec<Mutation>,
    before: RoutingSnapshot,
}

impl RoutingCandidate {
    pub fn info(&self) -> &RoutingCandidateInfo {
        &self.info
    }

    /// Exact forward patch for read-only preview, in application order.
    pub fn mutations(&self) -> &[Mutation] {
        &self.mutations
    }
}

/// Detached work, safe to move to a worker thread after releasing editor locks.
#[derive(Debug)]
pub struct RoutingJob {
    owner: Uuid,
    project: Uuid,
    base_revision: u64,
    source_context: SourceContext,
    sandbox: ProjectEditor,
    work: RoutingWork,
}

impl RoutingJob {
    /// Runs builtin once and freezes its applied-model result. Live state is
    /// untouched even if search, validation or native loading fails.
    pub fn run(mut self) -> Result<RoutingCandidate, RoutingError> {
        let requested = match &self.work {
            RoutingWork::Builtin(request) => request.board,
            RoutingWork::Edit { edit, .. } => Some(edit.board),
            RoutingWork::Regional(regional) => regional.request.board,
        };
        let board = crate::commands::resolve_board(self.sandbox.project(), requested)?.id();
        let before = capture_routing_state(&mut self.sandbox, board)?;
        let details = match self.work {
            RoutingWork::Builtin(mut request) => {
                request.board = Some(board);
                let routing = self.sandbox.execute(request.clone())?;
                RoutingCandidateDetails::Builtin {
                    backend: "builtin",
                    request,
                    routing: Box::new(routing),
                }
            }
            RoutingWork::Edit { edit, route } => {
                self.sandbox.execute(edit.clone())?;
                let route = if let Some(mut request) = route {
                    request.board = Some(board);
                    self.sandbox
                        .update_derived_data(|project| project.rebuild_planes(board, None))?
                        .map_err(crate::Error::from_export)?;
                    let routing = self.sandbox.execute(request.clone())?;
                    Some(Box::new(RoutingEditTrial { request, routing }))
                } else {
                    None
                };
                RoutingCandidateDetails::Edit { edit, route }
            }
            RoutingWork::Regional(regional) => {
                let routing = regional.run(&mut self.sandbox, board)?;
                RoutingCandidateDetails::Regional {
                    regional: Box::new(regional),
                    routing: Box::new(routing),
                }
            }
        };
        let after = capture_routing_state(&mut self.sandbox, board)?;
        let validation = details.validate(before.clone(), after);
        // Undo on the private sandbox exposes exact forward mutations.
        // An edit followed by routing can produce two entries.
        let mut groups = Vec::new();
        let mut count = 0;
        while self.sandbox.undo()? {
            let group = self.sandbox.undo_stack().redo_mutations()?;
            count += group.len();
            groups.push(group);
        }
        if count > MAX_CANDIDATE_MUTATIONS {
            return Err(RoutingError::Refused(
                "Candidate mutation limit exceeded.".into(),
            ));
        }
        let mutations: Vec<_> = groups.into_iter().rev().flatten().collect();
        Ok(RoutingCandidate {
            owner: self.owner,
            source_context: self.source_context,
            info: RoutingCandidateInfo {
                candidate: Uuid::new_random(),
                project: self.project,
                base_revision: self.base_revision,
                board,
                details,
                mutation_count: mutations.len(),
                validation,
            },
            mutations,
            before,
        })
    }
}

/// Candidate storage owned by one open project, shared by editor and MCP.
/// Closing/reopening a project allocates a new owner even for the same UUID.
#[derive(Debug)]
pub struct RoutingService {
    owner: Uuid,
    candidates: BTreeMap<Uuid, RoutingCandidate>,
}

impl Default for RoutingService {
    fn default() -> Self {
        Self {
            owner: Uuid::new_random(),
            candidates: BTreeMap::new(),
        }
    }
}

static_assertions::assert_impl_all!(RoutingService: Send, Sync);
static_assertions::assert_impl_all!(RoutingJob: Send, Sync);

impl RoutingService {
    /// Captures current unsaved state without changing revision, undo/redo,
    /// dirty state or the live transactional filesystem. Snapshot serialization
    /// happens under the caller's lock; search/checks happen later in run().
    pub fn prepare(
        &self,
        editor: &ProjectEditor,
        request: Autoroute,
    ) -> Result<RoutingJob, RoutingError> {
        self.prepare_work(editor, RoutingWork::Builtin(request))
    }

    /// Captures a checked connected edit for the same preview/apply path.
    pub fn prepare_edit(
        &self,
        editor: &ProjectEditor,
        edit: EditRoutingGeometry,
    ) -> Result<RoutingJob, RoutingError> {
        self.prepare_work(editor, RoutingWork::Edit { edit, route: None })
    }

    /// Captures a connected edit and a builtin trial on the edited board.
    pub fn prepare_edit_and_route(
        &self,
        editor: &ProjectEditor,
        edit: EditRoutingGeometry,
        route: Autoroute,
    ) -> Result<RoutingJob, RoutingError> {
        if route.nets.as_ref().is_none_or(Vec::is_empty) {
            return Err(crate::Error::InvalidArgument(
                "An edit-and-route trial requires at least one target net.".into(),
            )
            .into());
        }
        self.prepare_work(
            editor,
            RoutingWork::Edit {
                edit,
                route: Some(route),
            },
        )
    }

    /// Captures a builtin trial with explicit copper and region permissions.
    pub fn prepare_regional(
        &self,
        editor: &ProjectEditor,
        regional: RegionalRoutingRequest,
    ) -> Result<RoutingJob, RoutingError> {
        if regional.request.nets.as_ref().is_none_or(Vec::is_empty) {
            return Err(crate::Error::InvalidArgument(
                "Regional routing requires at least one target net.".into(),
            )
            .into());
        }
        self.prepare_work(editor, RoutingWork::Regional(regional))
    }

    fn prepare_work(
        &self,
        editor: &ProjectEditor,
        work: RoutingWork,
    ) -> Result<RoutingJob, RoutingError> {
        if editor.undo_stack().is_group_active() {
            return Err(crate::Error::GroupActive.into());
        }
        self.check_capacity()?;
        let project = editor.project();
        let sandbox = ProjectEditor::with_source(
            project.snapshot().map_err(crate::Error::from)?,
            Arc::clone(editor.source()),
        );
        Ok(RoutingJob {
            owner: self.owner,
            project: project.uuid(),
            base_revision: project.revision(),
            source_context: SourceContext::of(project),
            sandbox,
            work,
        })
    }

    /// Publishes only into the original, unchanged open project. Concurrent
    /// completions are capacity-checked again; existing candidates are retained.
    pub fn retain(
        &mut self,
        project: &Project,
        candidate: RoutingCandidate,
    ) -> Result<RoutingCandidateInfo, RoutingError> {
        self.check_source(project, &candidate)?;
        self.check_capacity()?;
        let info = candidate.info.clone();
        self.candidates.insert(info.candidate, candidate);
        Ok(info)
    }

    pub fn candidate(&self, id: Uuid) -> Result<&RoutingCandidate, RoutingError> {
        self.candidates.get(&id).ok_or(RoutingError::NotFound(id))
    }

    /// Freshness includes source attribute context as well as model revision.
    pub fn is_stale(&self, project: &Project, id: Uuid) -> Result<bool, RoutingError> {
        Ok(self.check_source(project, self.candidate(id)?).is_err())
    }

    /// Explicitly frees storage without modifying the board or its revision.
    pub fn discard(&mut self, id: Uuid) -> Result<(), RoutingError> {
        self.candidates
            .remove(&id)
            .ok_or(RoutingError::NotFound(id))?;
        Ok(())
    }

    /// Replays the reviewed patch as one undo step, without routing again.
    /// Unsafe/incomplete checks refuse before edits. Connectivity may remain
    /// partial; existing baseline errors remain visible. The candidate is
    /// consumed after commit, even if the following diagnostic check fails.
    pub fn apply(
        &mut self,
        editor: &mut ProjectEditor,
        id: Uuid,
    ) -> Result<RoutingCandidateInfo, RoutingError> {
        let candidate = self.candidate(id)?;
        self.check_source(editor.project(), candidate)?;
        if editor.undo_stack().is_group_active() {
            return Err(crate::Error::GroupActive.into());
        }
        if !candidate.info.validation.acceptable {
            return Err(RoutingError::Refused("Checks incomplete, netlist changed, new DRC errors or connectivity regression; inspect the candidate.".into()));
        }
        editor.apply_mutations(
            match candidate.info.details {
                RoutingCandidateDetails::Builtin { .. }
                | RoutingCandidateDetails::Regional { .. } => {
                    librepcb_i18n::tr!("Autorouter", "Autoroute Board")
                }
                RoutingCandidateDetails::Edit { .. } => {
                    librepcb_i18n::tr!("librepcb::editor::CmdBoardNetLineEdit", "Edit trace")
                }
            },
            candidate.mutations.clone(),
        )?;
        let candidate = self
            .candidates
            .remove(&id)
            .ok_or(RoutingError::NotFound(id))?;
        let mut info = candidate.info;
        if info.mutation_count == 0 {
            // Same source revision, no copper changed: reuse the detached full
            // check instead of publishing derived-only live revisions for a no-op.
            return Ok(info);
        }
        match capture_routing_state(editor, info.board) {
            Ok(after) => info.validation = info.details.validate(candidate.before, after),
            Err(error) => {
                // Copper has committed. Report the incomplete post-check honestly
                // instead of returning an error that suggests no write occurred.
                info.validation.acceptable = false;
                info.validation.drc_clean = false;
                info.validation.after.ran = false;
                info.validation.after.complete = false;
                info.validation.after.check_failures.push(error.to_string());
            }
        }
        Ok(info)
    }

    fn check_capacity(&self) -> Result<(), RoutingError> {
        if self.candidates.len() >= MAX_CANDIDATES {
            Err(RoutingError::Refused(
                "Candidate storage is full; discard a candidate before preparing another.".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn check_source(
        &self,
        project: &Project,
        candidate: &RoutingCandidate,
    ) -> Result<(), RoutingError> {
        if self.owner != candidate.owner || project.uuid() != candidate.info.project {
            return Err(RoutingError::WrongSession);
        }
        if SourceContext::of(project) != candidate.source_context {
            return Err(RoutingError::StaleContext);
        }
        if project.revision() != candidate.info.base_revision {
            return Err(RoutingError::StaleRevision {
                expected: candidate.info.base_revision,
                actual: project.revision(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::AddBoard;
    use librepcb_core::fileio::{FileSystem, TransactionalDirectory};
    use librepcb_core::types::ElementName;

    #[test]
    fn unsafe_and_incomplete_candidates_refuse_without_live_changes() {
        for incomplete in [false, true] {
            let directory = TransactionalDirectory::new_temporary().unwrap();
            let project = Project::create(directory, "test.lpp", Uuid::new_random).unwrap();
            let mut editor = ProjectEditor::new(project);
            editor
                .execute(AddBoard::new(ElementName::new("Board").unwrap()))
                .unwrap();
            let mut service = RoutingService::default();
            let mut candidate = service
                .prepare(&editor, Autoroute::default())
                .unwrap()
                .run()
                .unwrap();
            candidate.info.validation.acceptable = false;
            if incomplete {
                candidate.info.validation.after.complete = false;
            } else {
                candidate.info.validation.new_error_count = 1;
            }
            let id = candidate.info.candidate;
            service.retain(editor.project(), candidate).unwrap();
            let revision = editor.project().revision();
            let history = editor.undo_stack().history();
            let files = editor.project().directory().file_system().save_state();
            assert!(matches!(
                service.apply(&mut editor, id),
                Err(RoutingError::Refused(_))
            ));
            assert_eq!(editor.project().revision(), revision);
            assert_eq!(editor.undo_stack().history(), history);
            assert_eq!(
                editor.project().directory().file_system().save_state(),
                files
            );
            assert!(service.candidate(id).is_ok());
            assert!(
                !editor
                    .project()
                    .directory()
                    .abs_path("")
                    .unwrap()
                    .is_existing_dir()
            );
        }
    }
}
