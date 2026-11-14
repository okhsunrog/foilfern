//! Reviewed routing candidates (original implementation, no upstream counterpart).
//!
//! A native detached snapshot preserves unsaved model data and embedded assets.
//! Builtin routing and full DRC run on that snapshot, never on the live editor.
//! Candidates freeze exact model mutations, including generated UUIDs. Applying
//! one checks the owning service and source revision, then replays one undo group.
//! No general plugin abstraction is needed for this first builtin-only slice.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

use foilfern_core::fileio::FilePath;
use foilfern_core::project::{BoardId, Mutation, Project};
use foilfern_core::types::Uuid;
use serde::Serialize;

use crate::ProjectEditor;
use crate::commands::{
    Autoroute, AutorouteResult, ConnectLocalVia, EditRoutingGeometry, LocalViaConnection,
    PlaneStitch, StitchPlaneIsland,
};
use crate::routing_validation::{
    RoutingSnapshot, RoutingValidation, capture_routing_state, capture_routing_state_with_cancel,
};

mod comparison;
mod copper;
mod placement;
mod placement_repair;
pub(crate) mod region;
mod repair_search;
mod staged;
pub use comparison::{
    RoutingComparison, RoutingComparisonJob, RoutingTrial, RoutingTrials, TracePreference,
};
pub use copper::{CopperEndpoint, CopperPlan, PlannedPlacement, PlannedTrace};
pub use placement::{
    PlacementComparison, PlacementJob, PlacementRequest, PlacementResults, PlacementSearchStatus,
    PlacementSites, PlacementTarget, PlacementTrace, PlacementVia,
};
pub use placement_repair::{PlacementRepair, PlacementRepairTrial};
pub use region::{RegionalRepairProposal, RegionalRoutingRequest};
pub use repair_search::{
    RepairLaunch, RepairLaunchFailure, RepairLaunchSearch, RepairScreeningFailure,
    RepairSearchEvidence, RepairSearchJob, RepairSearchRequest, RepairSearchResults,
    RepairSearchStatus,
};
pub use staged::StagedRoutingRequest;

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
    #[error("Local repair search not found: {0}")]
    RepairSearchNotFound(Uuid),
    #[error("Placement search not found: {0}")]
    PlacementSearchNotFound(Uuid),
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
    /// A complete check of this unchanged source was retained by this session.
    /// Candidate and applied-model checks always run afresh.
    pub baseline_reused: bool,
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
    Copper {
        copper: CopperAddition,
        #[serde(skip_serializing_if = "Option::is_none")]
        route: Option<Box<RoutingEditTrial>>,
    },
    Regional {
        regional: Box<RegionalRoutingRequest>,
        routing: Box<AutorouteResult>,
    },
    Repair {
        repair: Box<repair_search::RepairCandidateDetails>,
    },
    Staged {
        staged: Box<staged::StagedRoutingResult>,
    },
    LocalVia {
        via_request: Box<ConnectLocalVia>,
        via_connection: LocalViaConnection,
    },
    PlaneStitch {
        stitch_request: Box<StitchPlaneIsland>,
        plane_stitch: PlaneStitch,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct CopperAddition {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_candidate: Option<Uuid>,
    pub removed_traces: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed_vias: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placements: Option<usize>,
    pub vias: usize,
    pub trace_segments: usize,
}

/// Result of routing selected nets on the edited snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct RoutingEditTrial {
    pub request: Autoroute,
    pub routing: AutorouteResult,
}

impl RoutingCandidateDetails {
    fn run_builtin(
        sandbox: &mut ProjectEditor,
        board: BoardId,
        mut request: Autoroute,
    ) -> Result<Self, RoutingError> {
        request.board = Some(board);
        let routing = sandbox.execute(request.clone())?;
        Ok(Self::Builtin {
            backend: "builtin",
            request,
            routing: Box::new(routing),
        })
    }

    fn validate(&self, before: &RoutingSnapshot, after: &RoutingSnapshot) -> RoutingValidation {
        let improves = match self {
            Self::LocalVia { via_connection, .. } => {
                let target = BTreeSet::from([via_connection.net]);
                after.unrouted_on(&target) < before.unrouted_on(&target)
            }
            Self::Regional { regional, .. } => regional
                .improve_net
                .is_none_or(|net| after.unrouted_for(net) < before.unrouted_for(net)),
            Self::Repair { repair } => repair.improves(before, after),
            _ => true,
        };
        let mut validation = RoutingValidation::compare_candidate(before, after);
        validation.acceptable &= improves;
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
    Staged(StagedRoutingRequest),
    Copper {
        plan: CopperPlan,
        base: Option<(Uuid, Vec<Mutation>)>,
        route: Option<Box<Autoroute>>,
    },
    LocalVia(ConnectLocalVia),
    PlaneStitch(StitchPlaneIsland),
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
    source: RoutingSource,
    work: RoutingWork,
}

#[derive(Debug)]
struct RoutingSource {
    owner: Uuid,
    project: Uuid,
    base_revision: u64,
    source_context: SourceContext,
    sandbox: ProjectEditor,
    baseline: Option<RoutingBaseline>,
    baseline_reused: bool,
}

/// One bounded, session-owned baseline, published only after freshness checks.
/// Native snapshots omit derived copper: reuse never skips rebuilding it.
#[derive(Debug, Clone)]
struct RoutingBaseline {
    project: Uuid,
    revision: u64,
    context: SourceContext,
    board: BoardId,
    snapshot: RoutingSnapshot,
}

impl RoutingJob {
    /// Runs builtin once and freezes its applied-model result. Live state is
    /// untouched even if search, validation or native loading fails.
    pub fn run(mut self) -> Result<RoutingCandidate, RoutingError> {
        let requested = match &self.work {
            RoutingWork::Builtin(request) => request.board,
            RoutingWork::Edit { edit, .. } => Some(edit.board),
            RoutingWork::Regional(regional) => regional.request.board,
            RoutingWork::Staged(staged) => staged.options.board,
            RoutingWork::Copper { plan, .. } => Some(plan.board),
            RoutingWork::LocalVia(request) => Some(request.board),
            RoutingWork::PlaneStitch(request) => Some(request.board),
        };
        let board = crate::commands::resolve_board(self.source.sandbox.project(), requested)?.id();
        let started = Instant::now();
        let before = self.source.capture_before(board)?;
        log::debug!(
            "routing_stage source_checks {} us reused={}",
            started.elapsed().as_micros(),
            self.source.baseline_reused
        );
        let started = Instant::now();
        let sandbox = &mut self.source.sandbox;
        let existing = |editor: &ProjectEditor| {
            let p = editor.project();
            region::copper(p, crate::commands::resolve_board(p, Some(board))?)
        };
        let details = match self.work {
            RoutingWork::Builtin(request) => {
                RoutingCandidateDetails::run_builtin(sandbox, board, request)?
            }
            RoutingWork::Edit { edit, route } => {
                sandbox.execute(edit.clone())?;
                let route = if let Some(mut request) = route {
                    request.board = Some(board);
                    sandbox
                        .update_derived_data(|project| project.rebuild_planes(board, None))?
                        .map_err(crate::Error::from_export)?;
                    let routing = sandbox.execute(request.clone())?;
                    Some(Box::new(RoutingEditTrial { request, routing }))
                } else {
                    None
                };
                RoutingCandidateDetails::Edit { edit, route }
            }
            RoutingWork::Regional(regional) => {
                let routing = regional.run(sandbox, board)?;
                RoutingCandidateDetails::Regional {
                    regional: Box::new(regional),
                    routing: Box::new(routing),
                }
            }
            RoutingWork::Staged(request) => RoutingCandidateDetails::Staged {
                staged: Box::new(request.run(sandbox, board, &before)?),
            },
            RoutingWork::Copper { plan, base, route } => {
                let base_candidate = match base {
                    Some((id, mutations)) => {
                        sandbox.apply_mutations("Replay routing candidate", mutations)?;
                        Some(id)
                    }
                    None => None,
                };
                let plan_counts = copper::PlanCounts::from(&plan);
                let (vias, trace_segments) = plan.run(sandbox)?;
                copper::finish_copper(
                    sandbox,
                    board,
                    base_candidate,
                    plan_counts,
                    (vias.len(), trace_segments),
                    route.map(|request| *request),
                    || false,
                )?
            }
            RoutingWork::LocalVia(request) => {
                let protected = existing(sandbox)?;
                let connection = sandbox.execute(request.clone())?;
                region::ensure_preserved_copper(&protected, &existing(sandbox)?, &BTreeSet::new())?;
                RoutingCandidateDetails::LocalVia {
                    via_request: Box::new(request),
                    via_connection: connection,
                }
            }
            RoutingWork::PlaneStitch(request) => {
                let protected = existing(sandbox)?;
                let plane_stitch = sandbox.execute(request.clone())?;
                region::ensure_preserved_copper(&protected, &existing(sandbox)?, &BTreeSet::new())?;
                RoutingCandidateDetails::PlaneStitch {
                    stitch_request: Box::new(request),
                    plane_stitch,
                }
            }
        };
        log::debug!(
            "routing_stage candidate_work {} us",
            started.elapsed().as_micros()
        );
        let started = Instant::now();
        let after = capture_routing_state(sandbox, board)?;
        log::debug!(
            "routing_stage candidate_checks {} us",
            started.elapsed().as_micros()
        );
        if let RoutingCandidateDetails::LocalVia { via_connection, .. } = &details {
            let target = BTreeSet::from([via_connection.net]);
            if after.unrouted_on(&target) >= before.unrouted_on(&target) {
                return Err(RoutingError::Refused(
                    "Local via did not reduce the number of open connections on its net.".into(),
                ));
            }
        }
        self.source.freeze(board, details, before, after)
    }
}

/// Collects the exact forward mutations from a disposable routing sandbox.
pub(super) fn freeze_mutations(mut sandbox: ProjectEditor) -> Result<Vec<Mutation>, RoutingError> {
    let mut groups = Vec::new();
    let mut count = 0;
    while sandbox.undo()? {
        let group = sandbox.undo_stack().redo_mutations()?;
        count += group.len();
        if count > MAX_CANDIDATE_MUTATIONS {
            return Err(RoutingError::Refused(
                "Candidate mutation limit exceeded.".into(),
            ));
        }
        groups.push(group);
    }
    Ok(groups.into_iter().rev().flatten().collect())
}

fn rebuild_connectivity(editor: &mut ProjectEditor, board: BoardId) -> Result<(), RoutingError> {
    editor
        .update_derived_data(|project| project.rebuild_planes(board, None))?
        .map_err(crate::Error::from_export)?;
    editor.update_derived_data(|project| {
        project
            .force_air_wires_rebuild(board)
            .map_err(crate::Error::from)
    })??;
    Ok(())
}

impl RoutingSource {
    fn fork(&self) -> Result<Self, RoutingError> {
        Ok(Self {
            owner: self.owner,
            project: self.project,
            base_revision: self.base_revision,
            source_context: self.source_context.clone(),
            baseline: None,
            baseline_reused: self.baseline_reused,
            sandbox: ProjectEditor::with_source(
                self.sandbox
                    .project()
                    .snapshot()
                    .map_err(crate::Error::from)?,
                Arc::clone(self.sandbox.source()),
            ),
        })
    }

    fn capture_before(&mut self, board: BoardId) -> Result<RoutingSnapshot, RoutingError> {
        self.capture_before_with_cancel(board, &|| false)
    }

    fn capture_before_with_cancel(
        &mut self,
        board: BoardId,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<RoutingSnapshot, RoutingError> {
        let cached = self.baseline.take().filter(|baseline| {
            baseline.project == self.project
                && baseline.revision == self.base_revision
                && baseline.context == self.source_context
                && baseline.board == board
        });
        if let Some(baseline) = cached {
            rebuild_connectivity(&mut self.sandbox, board)?;
            self.baseline_reused = true;
            Ok(baseline.snapshot)
        } else {
            Ok(capture_routing_state_with_cancel(
                &mut self.sandbox,
                board,
                cancelled,
            )?)
        }
    }

    fn freeze(
        self,
        board: BoardId,
        details: RoutingCandidateDetails,
        before: RoutingSnapshot,
        after: RoutingSnapshot,
    ) -> Result<RoutingCandidate, RoutingError> {
        let RoutingSource {
            owner,
            project,
            base_revision,
            source_context,
            sandbox,
            baseline_reused,
            ..
        } = self;
        let validation = details.validate(&before, &after);
        // Undo on the private sandbox exposes exact forward mutations.
        // A compound candidate can produce multiple entries.
        let mutations = freeze_mutations(sandbox)?;
        Ok(RoutingCandidate {
            owner,
            source_context,
            info: RoutingCandidateInfo {
                candidate: Uuid::new_random(),
                project,
                base_revision,
                board,
                baseline_reused,
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
    baseline: Option<RoutingBaseline>,
    repair_search: Option<Arc<repair_search::ActiveRepairSearch>>,
    placement_search: Option<Arc<placement::ActivePlacementSearch>>,
}

impl Default for RoutingService {
    fn default() -> Self {
        Self {
            owner: Uuid::new_random(),
            candidates: BTreeMap::new(),
            baseline: None,
            repair_search: None,
            placement_search: None,
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

    /// Captures a local via and short leads for preview and checked apply.
    pub fn prepare_local_via(
        &self,
        editor: &ProjectEditor,
        request: ConnectLocalVia,
    ) -> Result<RoutingJob, RoutingError> {
        self.prepare_work(editor, RoutingWork::LocalVia(request))
    }

    /// Captures a plane island stitch for the same preview and checked apply.
    pub fn prepare_plane_stitch(
        &self,
        editor: &ProjectEditor,
        request: StitchPlaneIsland,
    ) -> Result<RoutingJob, RoutingError> {
        self.prepare_work(editor, RoutingWork::PlaneStitch(request))
    }

    /// Captures a checked connected edit for the same preview/apply path.
    pub fn prepare_edit(
        &self,
        editor: &ProjectEditor,
        edit: EditRoutingGeometry,
    ) -> Result<RoutingJob, RoutingError> {
        self.prepare_work(editor, RoutingWork::Edit { edit, route: None })
    }

    /// Captures explicit vias and traces for checked preview and apply.
    pub fn prepare_copper(
        &self,
        editor: &ProjectEditor,
        plan: CopperPlan,
        base_candidate: Option<Uuid>,
    ) -> Result<RoutingJob, RoutingError> {
        self.prepare_copper_and_route(editor, plan, base_candidate, None)
    }

    /// Captures explicit copper and an optional builtin continuation on the
    /// same detached board. Existing and explicitly planned copper stay fixed
    /// during the continuation.
    pub fn prepare_copper_and_route(
        &self,
        editor: &ProjectEditor,
        plan: CopperPlan,
        base_candidate: Option<Uuid>,
        route: Option<Autoroute>,
    ) -> Result<RoutingJob, RoutingError> {
        if route
            .as_ref()
            .is_some_and(|request| request.nets.as_ref().is_none_or(Vec::is_empty))
        {
            return Err(crate::Error::InvalidArgument(
                "A copper-and-route trial requires at least one target net.".into(),
            )
            .into());
        }
        let base = if let Some(id) = base_candidate {
            let candidate = self.candidate(id)?;
            self.check_source(editor.project(), candidate)?;
            if candidate.info.board != plan.board {
                return Err(crate::Error::InvalidArgument(
                    "The base candidate belongs to a different board.".into(),
                )
                .into());
            }
            Some((id, candidate.mutations.clone()))
        } else {
            None
        };
        self.prepare_work(
            editor,
            RoutingWork::Copper {
                plan,
                base,
                route: route.map(Box::new),
            },
        )
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
        Ok(RoutingJob {
            source: self.capture_source(editor, 1)?,
            work,
        })
    }

    fn capture_source(
        &self,
        editor: &ProjectEditor,
        slots: usize,
    ) -> Result<RoutingSource, RoutingError> {
        if editor.undo_stack().is_group_active() {
            return Err(crate::Error::GroupActive.into());
        }
        self.check_capacity(slots)?;
        let project = editor.project();
        let started = Instant::now();
        let sandbox = ProjectEditor::with_source(
            project.snapshot().map_err(crate::Error::from)?,
            Arc::clone(editor.source()),
        );
        log::debug!(
            "routing_stage snapshot {} us",
            started.elapsed().as_micros()
        );
        Ok(RoutingSource {
            owner: self.owner,
            project: project.uuid(),
            base_revision: project.revision(),
            source_context: SourceContext::of(project),
            sandbox,
            baseline: self.baseline.clone(),
            baseline_reused: false,
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
        self.check_capacity(1)?;
        self.remember_baseline(&candidate);
        let info = candidate.info.clone();
        self.candidates.insert(info.candidate, candidate);
        Ok(info)
    }

    fn remember_baseline(&mut self, candidate: &RoutingCandidate) {
        if candidate.before.checks.ran && candidate.before.checks.complete {
            self.baseline = Some(RoutingBaseline {
                project: candidate.info.project,
                revision: candidate.info.base_revision,
                context: candidate.source_context.clone(),
                board: candidate.info.board,
                snapshot: candidate.before.clone(),
            });
        }
    }

    pub fn candidate(&self, id: Uuid) -> Result<&RoutingCandidate, RoutingError> {
        self.candidates.get(&id).ok_or(RoutingError::NotFound(id))
    }

    /// Freshness includes source attribute context as well as model revision.
    pub fn is_stale(&self, project: &Project, id: Uuid) -> Result<bool, RoutingError> {
        Ok(self.check_source(project, self.candidate(id)?).is_err())
    }

    /// Captures the original model and frozen patch for additional preview
    /// checks. A stale patch stays inspectable, but cannot be replayed here.
    pub fn preview_snapshot(
        &self,
        project: &Project,
        id: Uuid,
    ) -> Result<(ProjectEditor, Vec<Mutation>), RoutingError> {
        let candidate = self.candidate(id)?;
        self.check_source(project, candidate)?;
        Ok((
            ProjectEditor::new(project.snapshot().map_err(crate::Error::from)?),
            candidate.mutations.clone(),
        ))
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
            return Err(RoutingError::Refused("The candidate failed routing validation. Inspect its DRC, connectivity and board requirements.".into()));
        }
        let text = match candidate.info.details {
            RoutingCandidateDetails::Builtin { .. }
            | RoutingCandidateDetails::Regional { .. }
            | RoutingCandidateDetails::Repair { .. }
            | RoutingCandidateDetails::Staged { .. }
            | RoutingCandidateDetails::LocalVia { .. } => {
                foilfern_i18n::tr!("Autorouter", "Autoroute Board")
            }
            RoutingCandidateDetails::Edit { .. } => {
                foilfern_i18n::tr!("librepcb::editor::CmdBoardNetLineEdit", "Edit trace")
            }
            RoutingCandidateDetails::Copper { .. } => "Add routed copper".into(),
            RoutingCandidateDetails::PlaneStitch { .. } => "Stitch copper plane island".into(),
        };
        editor.execute_atomic(text, |tx| -> crate::Result<()> {
            let guard = crate::board_requirements::RoutingGuard::capture(
                tx.project(),
                candidate.info.board,
            )?;
            tx.apply_all(candidate.mutations.clone())?;
            guard.enforce(tx.project())
        })?;
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
        let started = Instant::now();
        let applied = capture_routing_state(editor, info.board);
        log::debug!(
            "routing_stage apply_checks {} us",
            started.elapsed().as_micros()
        );
        match applied {
            Ok(after) => info.validation = info.details.validate(&candidate.before, &after),
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

    fn check_capacity(&self, slots: usize) -> Result<(), RoutingError> {
        let reserved = self
            .repair_search
            .as_ref()
            .map_or(0, |search| search.reserved_slots())
            + self
                .placement_search
                .as_ref()
                .map_or(0, |search| search.reserved_slots());
        if slots > MAX_CANDIDATES - self.candidates.len() - reserved {
            Err(RoutingError::Refused(format!(
                "Candidate storage needs {slots} free slot(s); discard existing candidates and retry."
            )))
        } else {
            Ok(())
        }
    }

    fn check_source(
        &self,
        project: &Project,
        candidate: &RoutingCandidate,
    ) -> Result<(), RoutingError> {
        self.check_source_identity(
            project,
            candidate.owner,
            candidate.info.project,
            candidate.info.base_revision,
            &candidate.source_context,
        )
    }

    fn check_source_identity(
        &self,
        project: &Project,
        owner: Uuid,
        project_id: Uuid,
        revision: u64,
        context: &SourceContext,
    ) -> Result<(), RoutingError> {
        if self.owner != owner || project.uuid() != project_id {
            return Err(RoutingError::WrongSession);
        }
        if &SourceContext::of(project) != context {
            return Err(RoutingError::StaleContext);
        }
        if project.revision() != revision {
            return Err(RoutingError::StaleRevision {
                expected: revision,
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
    use foilfern_core::fileio::{FileSystem, TransactionalDirectory};
    use foilfern_core::types::ElementName;

    fn baseline_fixture() -> (ProjectEditor, RoutingService, BoardId) {
        let directory = TransactionalDirectory::new_temporary().unwrap();
        let project = Project::create(directory, "test.lpp", Uuid::new_random).unwrap();
        let mut editor = ProjectEditor::new(project);
        let board = editor
            .execute(AddBoard::new(ElementName::new("Board").unwrap()))
            .unwrap()
            .board;
        (editor, RoutingService::default(), board)
    }

    fn prepare_board(
        service: &RoutingService,
        editor: &ProjectEditor,
        board: BoardId,
    ) -> RoutingCandidate {
        service
            .prepare(
                editor,
                Autoroute {
                    board: Some(board),
                    ..Default::default()
                },
            )
            .unwrap()
            .run()
            .unwrap()
    }

    #[test]
    fn retained_baseline_reuses_only_the_source_check_without_live_changes() {
        let (editor, mut service, board) = baseline_fixture();
        let first = prepare_board(&service, &editor, board);
        assert!(!first.info.baseline_reused);
        let validation = first.info.validation.clone();
        let id = first.info.candidate;
        service.retain(editor.project(), first).unwrap();
        // Discarding a proposal does not discard a still-current source check.
        service.discard(id).unwrap();
        let revision = editor.project().revision();
        let files = editor.project().directory().file_system().save_state();
        let history = editor.undo_stack().history();
        let second = prepare_board(&service, &editor, board);
        assert!(second.info.baseline_reused);
        assert_eq!(second.info.validation, validation);
        assert_eq!(editor.project().revision(), revision);
        assert_eq!(editor.undo_stack().history(), history);
        assert_eq!(
            editor.project().directory().file_system().save_state(),
            files
        );
    }

    #[test]
    fn baseline_does_not_cross_boards_or_model_revisions() {
        let (mut editor, mut service, first) = baseline_fixture();
        let second = editor
            .execute(AddBoard::new(ElementName::new("Other").unwrap()))
            .unwrap()
            .board;
        let candidate = prepare_board(&service, &editor, first);
        service.retain(editor.project(), candidate).unwrap();
        assert!(
            !prepare_board(&service, &editor, second)
                .info
                .baseline_reused
        );
        assert!(prepare_board(&service, &editor, first).info.baseline_reused);
        let mut metadata = editor.project().metadata().clone();
        metadata.author.push_str(" changed");
        editor
            .apply_mutations("Edit author", vec![Mutation::SetProjectMetadata(metadata)])
            .unwrap();
        assert!(!prepare_board(&service, &editor, first).info.baseline_reused);
    }

    #[test]
    fn incomplete_and_stale_completions_cannot_seed_a_baseline() {
        let (mut editor, mut service, board) = baseline_fixture();
        let mut candidate = prepare_board(&service, &editor, board);
        candidate.before.checks.complete = false;
        service.retain(editor.project(), candidate).unwrap();
        assert!(!prepare_board(&service, &editor, board).info.baseline_reused);
        let candidate = prepare_board(&service, &editor, board);
        editor
            .execute(AddBoard::new(ElementName::new("Other").unwrap()))
            .unwrap();
        assert!(matches!(
            service.retain(editor.project(), candidate),
            Err(RoutingError::StaleRevision { .. })
        ));
        assert!(!prepare_board(&service, &editor, board).info.baseline_reused);
    }

    #[test]
    fn save_context_and_project_paths_invalidate_the_baseline() {
        let (mut editor, mut service, board) = baseline_fixture();
        let candidate = prepare_board(&service, &editor, board);
        service.retain(editor.project(), candidate).unwrap();
        let baseline = service.baseline.clone().unwrap();
        service.baseline.as_mut().unwrap().context.file =
            Some(FilePath::new(std::env::temp_dir().join("other-project.lpp")).unwrap());
        assert!(!prepare_board(&service, &editor, board).info.baseline_reused);
        service.baseline = Some(baseline.clone());
        service.baseline.as_mut().unwrap().context.directory =
            Some(FilePath::new(std::env::temp_dir().join("other-project")).unwrap());
        assert!(!prepare_board(&service, &editor, board).info.baseline_reused);
        service.baseline = Some(baseline);
        let job = service.prepare(&editor, Autoroute::default()).unwrap();
        let revision = editor.project().revision();
        // Save updates DATE/TIME text context even without a model mutation.
        editor
            .update_derived_data(|project| project.save().map_err(crate::Error::from))
            .unwrap()
            .unwrap();
        assert_eq!(editor.project().revision(), revision);
        let stale = job.run().unwrap();
        assert!(matches!(
            service.retain(editor.project(), stale),
            Err(RoutingError::StaleContext)
        ));
        assert!(!prepare_board(&service, &editor, board).info.baseline_reused);
    }

    #[test]
    fn baseline_is_owned_by_one_service_and_comparison_can_share_it() {
        use crate::commands::RoutingOrder;
        let (editor, mut service, board) = baseline_fixture();
        let candidate = prepare_board(&service, &editor, board);
        service.retain(editor.project(), candidate).unwrap();
        let other = RoutingService::default();
        assert!(!prepare_board(&other, &editor, board).info.baseline_reused);
        let orders = vec![RoutingOrder::ShortestFirst, RoutingOrder::LongestFirst];
        let trials = service
            .prepare_comparison(&editor, Autoroute::default(), orders)
            .unwrap()
            .run()
            .unwrap();
        let comparison = service.retain_comparison(editor.project(), trials).unwrap();
        assert!(comparison.candidates.iter().all(|entry| {
            service
                .candidate(entry.candidate)
                .unwrap()
                .info
                .baseline_reused
        }));
    }

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
