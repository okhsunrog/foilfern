//! Bounded trials of authorized local repairs using the builtin router.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use foilfern_core::geometry::TraceAnchor;
use foilfern_core::project::board::{AirWire, Board, FootprintPadView};
use foilfern_core::project::{BoardId, ComponentInstanceId, NetSignalId, Project};
use foilfern_core::types::{Layer, Length, Point, PositiveLength, Uuid};
use parking_lot::Mutex;
use serde::Serialize;

use super::{
    CopperPlan, RegionalRoutingRequest, RoutingCandidate, RoutingCandidateDetails, RoutingError,
    RoutingService, RoutingSource, SourceContext, TracePreference,
    comparison::{TracePreferenceResult, copper_cost},
    rebuild_connectivity,
};
use crate::commands::autoroute::{
    AccessBlocker, RoutingObject, RoutingSearchSample, diagnose_access, extract_problem,
};
use crate::commands::{
    AddVia, Autoroute, AutorouteResult, BoardSelection, NetRef, RoutingBounds, TraceEndpoint,
    UnroutedAirWire, net_name, resolve_board,
};
use crate::routing_validation::{
    RoutingSnapshot, RoutingValidation, capture_routing_state_with_cancel,
};
use crate::{Error, ProjectEditor};

#[derive(Debug, Clone)]
pub struct RepairSearchRequest {
    pub target: NetSignalId,
    pub route: Autoroute,
    pub neighbor_route: Autoroute,
    /// Optional settings for routing the target and neighbors in a single pass.
    pub joint_route: Option<Autoroute>,
    pub rip_up_region: RoutingBounds,
    pub groups: Vec<BTreeSet<Uuid>>,
    /// Optional exact endpoints by zero-based removal group index.
    pub neighbor_anchors: BTreeMap<usize, [TraceAnchor; 2]>,
    pub max_objects: usize,
    pub max_trials: usize,
    pub max_candidates: usize,
    pub time_budget: Duration,
    /// Hard pad-to-pad constraints checked on each complete trial.
    pub trace_requirements: Vec<TracePreference>,
    pub target_launch: Option<RepairLaunch>,
    pub target_launch_search: Option<RepairLaunchSearch>,
    /// Generate an additional launch on a neighbor net in joint trials.
    pub neighbor_launch_search: Option<RepairLaunchSearch>,
}

/// Find candidate via sites near a footprint pad before repairing.
#[derive(Debug, Clone)]
pub struct RepairLaunchSearch {
    pub from: (ComponentInstanceId, Uuid),
    pub layer: Layer,
    pub radius: PositiveLength,
    pub max_sites: usize,
}

/// An authorized additive, straight pad-to-via lead.
#[derive(Debug, Clone, Serialize)]
pub struct RepairLaunch {
    pub from: (ComponentInstanceId, Uuid),
    pub layer: Layer,
    pub position: Point,
    pub width: PositiveLength,
    pub drill: Option<PositiveLength>,
    pub size: Option<PositiveLength>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepairCandidateDetails {
    pub target: NetSignalId,
    pub groups: Vec<usize>,
    pub strategy: RepairStrategy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub neighbor: Option<RegionalRoutingRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub joint: Option<RegionalRoutingRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub neighbor_region: Option<RoutingBounds>,
    pub route: Autoroute,
    pub routing: AutorouteResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_launch: Option<RepairLaunch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub neighbor_launch: Option<RepairLaunch>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub pruned_vias: BTreeSet<Uuid>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub trace_requirements: Vec<RepairTraceCheck>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub neighbor_branches: Vec<AirWire>,
}

impl RepairCandidateDetails {
    pub(super) fn improves(&self, before: &RoutingSnapshot, after: &RoutingSnapshot) -> bool {
        after.unrouted_for(self.target) < before.unrouted_for(self.target)
            && self.trace_requirements.iter().all(|check| check.after.met)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RepairTraceCheck {
    pub requirement: TracePreference,
    pub before: TracePreferenceResult,
    pub after: TracePreferenceResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairStrategy {
    Sequential,
    Joint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairSearchPhase {
    Queued,
    Baseline,
    LaunchSites,
    Neighbors,
    Target,
    Joint,
    Validation,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum StopReason {
    Completed,
    TrialLimit,
    TimeLimit,
    Cancelled,
    Failed,
}

#[derive(Debug, Serialize)]
pub struct RepairSearchStatus {
    pub search: Uuid,
    pub board: BoardId,
    pub target: NetSignalId,
    pub target_name: String,
    pub base_revision: u64,
    pub phase: RepairSearchPhase,
    pub elapsed_ms: u128,
    pub budget_ms: u128,
    pub remaining_ms: u128,
    pub completed_trials: usize,
    pub planned_trials: usize,
    pub current_groups: Vec<usize>,
    pub current_strategy: Option<RepairStrategy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch_sites: Option<Vec<RepairLaunch>>,
    /// Bounded local probes when a pad has no clear escape, before routing trials exist.
    pub screening_failures: Vec<RepairScreeningFailure>,
    pub cancel_requested: bool,
    stop_reason: Option<StopReason>,
    pub trials: Vec<RepairTrial>,
    pub candidates: Vec<RepairChoice>,
    pub failure: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepairTrial {
    pub groups: Vec<usize>,
    pub strategy: RepairStrategy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_launch: Option<RepairLaunch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub neighbor_launch: Option<RepairLaunch>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub pruned_vias: BTreeSet<Uuid>,
    pub rip_up: BTreeSet<Uuid>,
    pub target_unrouted: Option<usize>,
    pub validation: Option<RoutingValidation>,
    pub failure: Option<String>,
    pub neighbor_failures: Vec<RepairFailure>,
    pub target_failures: Vec<RepairFailure>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub trace_requirements: Vec<RepairTraceCheck>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub neighbor_branches: Vec<AirWire>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepairFailure {
    pub net: NetSignalId,
    pub name: String,
    pub reason: String,
    pub anchors: [TraceAnchor; 2],
    pub positions: [Point; 2],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soft_reason: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub search_evidence: Vec<RoutingSearchSample>,
}

/// A failed local escape screening on a detached, partially ripped-up board.
/// Probes identify possible blockers; they do not prove that removing them suffices.
#[derive(Debug, Clone, Serialize)]
pub struct RepairScreeningFailure {
    pub groups: Vec<usize>,
    pub rip_up: BTreeSet<Uuid>,
    pub from: (ComponentInstanceId, Uuid),
    pub net: NetSignalId,
    pub layer: Layer,
    pub position: Point,
    pub target_launch: Option<RepairLaunch>,
    pub blockers: Vec<RoutingObject>,
}

/// Failed continuation from a checked additive escape trial.
#[derive(Debug, Clone)]
pub struct RepairLaunchFailure {
    pub launch: RepairLaunch,
    pub via: Uuid,
    pub wire: UnroutedAirWire,
}

/// Evidence and original limits for planning a subsequent, explicitly authorized search.
pub struct RepairSearchEvidence {
    pub launches: Vec<RepairLaunchFailure>,
    pub trials: Vec<RepairTrial>,
    pub screenings: Vec<RepairScreeningFailure>,
    pub request: RepairSearchRequest,
}

fn summarize_failures(
    result: &AutorouteResult,
    include: impl Fn(&UnroutedAirWire) -> bool,
) -> Vec<RepairFailure> {
    let summarize = |failure: &UnroutedAirWire| RepairFailure {
        net: failure.net,
        name: failure.net_name.clone(),
        reason: failure.reason.clone(),
        anchors: [failure.p1, failure.p2],
        positions: [failure.p1_position, failure.p2_position],
        soft_reason: failure.soft_reason.clone(),
        search_evidence: failure.search_evidence.clone(),
    };
    result
        .failed_requests
        .iter()
        .chain(&result.skipped_outside_region)
        .filter(|failure| include(failure))
        .take(8)
        .map(summarize)
        .collect()
}

fn require_branches_routed(
    result: &AutorouteResult,
    branches: &[AirWire],
) -> Result<(), RoutingError> {
    if let Some(failure) = result
        .failed_requests
        .iter()
        .find(|failure| branches.iter().any(|wire| wire.net() == failure.net))
    {
        return Err(RoutingError::Refused(format!(
            "Could not reconnect the selected branch of {} between its retained anchors: {}.",
            failure.net_name, failure.reason
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct RepairChoice {
    pub candidate: Uuid,
    pub groups: Vec<usize>,
    pub strategy: RepairStrategy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_launch: Option<RepairLaunch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub neighbor_launch: Option<RepairLaunch>,
    pub target_unrouted: usize,
    pub affected_objects: usize,
    pub new_warnings: usize,
    pub vias: usize,
    pub trace_length_mm: Option<f64>,
}

impl RepairChoice {
    fn rank(&self) -> (usize, usize, usize, usize) {
        (
            self.target_unrouted,
            self.new_warnings,
            self.affected_objects,
            self.vias,
        )
    }
}

#[derive(Debug)]
struct Progress {
    phase: RepairSearchPhase,
    groups: Vec<usize>,
    strategy: Option<RepairStrategy>,
    planned: usize,
    launch_sites: Option<Vec<RepairLaunch>>,
    launch_failures: Vec<RepairLaunchFailure>,
    screening_failures: Vec<RepairScreeningFailure>,
    trials: Vec<RepairTrial>,
    stop: Option<StopReason>,
    candidates: Vec<RepairChoice>,
    failure: Option<String>,
    elapsed: Option<Duration>,
}

#[derive(Debug, Clone, Default)]
struct TrialLaunches {
    target: Option<RepairLaunch>,
    neighbor: Option<RepairLaunch>,
}

#[derive(Debug)]
pub(super) struct ActiveRepairSearch {
    id: Uuid,
    project: Uuid,
    board: BoardId,
    target: NetSignalId,
    target_name: String,
    revision: u64,
    context: SourceContext,
    request: RepairSearchRequest,
    started: Instant,
    budget: Duration,
    reserved: usize,
    cancel: AtomicBool,
    progress: Mutex<Progress>,
}

impl ActiveRepairSearch {
    fn stop(&self) -> Option<StopReason> {
        if self.cancel.load(Ordering::Relaxed) {
            Some(StopReason::Cancelled)
        } else {
            (self.started.elapsed() >= self.budget).then_some(StopReason::TimeLimit)
        }
    }

    fn status(&self) -> RepairSearchStatus {
        let progress = self.progress.lock();
        let elapsed = progress.elapsed.unwrap_or_else(|| self.started.elapsed());
        RepairSearchStatus {
            search: self.id,
            board: self.board,
            target: self.target,
            target_name: self.target_name.clone(),
            base_revision: self.revision,
            phase: progress.phase,
            elapsed_ms: elapsed.as_millis(),
            budget_ms: self.budget.as_millis(),
            remaining_ms: self.budget.saturating_sub(elapsed).as_millis(),
            completed_trials: progress.trials.len(),
            planned_trials: progress.planned,
            current_groups: progress.groups.clone(),
            current_strategy: progress.strategy,
            launch_sites: progress.launch_sites.clone(),
            screening_failures: progress.screening_failures.clone(),
            cancel_requested: self.cancel.load(Ordering::Relaxed),
            stop_reason: progress.stop,
            trials: progress.trials.clone(),
            candidates: progress.candidates.clone(),
            failure: progress.failure.clone(),
        }
    }

    fn phase(&self, phase: RepairSearchPhase) {
        self.progress.lock().phase = phase;
    }

    pub(super) fn reserved_slots(&self) -> usize {
        if self.progress.lock().stop.is_none() {
            self.reserved
        } else {
            0
        }
    }
}

#[derive(Debug)]
struct Trial {
    groups: Vec<usize>,
    rip_up: BTreeSet<Uuid>,
    nets: BTreeSet<NetSignalId>,
    branches: Vec<AirWire>,
}

#[derive(Debug)]
pub struct RepairSearchJob {
    source: RoutingSource,
    state: Arc<ActiveRepairSearch>,
    request: RepairSearchRequest,
    trials: Vec<Trial>,
}

pub struct RepairSearchResults {
    search: Uuid,
    stop: StopReason,
    candidates: Vec<(RoutingCandidate, RepairChoice)>,
}

impl RoutingService {
    pub fn start_repair_search(
        &mut self,
        editor: &ProjectEditor,
        request: RepairSearchRequest,
    ) -> Result<(RepairSearchStatus, RepairSearchJob), RoutingError> {
        if self
            .repair_search
            .as_ref()
            .is_some_and(|search| search.progress.lock().stop.is_none())
        {
            return Err(RoutingError::Refused(
                "A local repair search is already running.".into(),
            ));
        }
        let project = editor.project();
        let board = resolve_board(project, request.route.board)?;
        let trials = request.plan(project, board)?;
        let strategy_count = request.strategies().len();
        let source = self.capture_source(editor, request.max_candidates)?;
        let state = Arc::new(ActiveRepairSearch {
            id: Uuid::new_random(),
            project: source.project,
            board: board.id(),
            target: request.target,
            target_name: net_name(project, Some(request.target)),
            revision: source.base_revision,
            context: source.source_context.clone(),
            request: request.clone(),
            started: Instant::now(),
            budget: request.time_budget,
            reserved: request.max_candidates,
            cancel: AtomicBool::new(false),
            progress: Mutex::new(Progress {
                phase: RepairSearchPhase::Queued,
                groups: Vec::new(),
                strategy: None,
                planned: (trials.len()
                    * strategy_count
                    * request
                        .target_launch_search
                        .as_ref()
                        .map_or(1, |search| search.max_sites)
                    * request
                        .neighbor_launch_search
                        .as_ref()
                        .map_or(1, |search| search.max_sites))
                .min(request.max_trials),
                launch_sites: None,
                launch_failures: Vec::new(),
                screening_failures: Vec::new(),
                trials: Vec::new(),
                stop: None,
                candidates: Vec::new(),
                failure: None,
                elapsed: None,
            }),
        });
        self.repair_search = Some(Arc::clone(&state));
        Ok((
            state.status(),
            RepairSearchJob {
                source,
                state,
                request,
                trials,
            },
        ))
    }

    pub fn repair_search_status(
        &self,
        id: Uuid,
        cancel: bool,
    ) -> Result<RepairSearchStatus, RoutingError> {
        let search = self
            .repair_search
            .as_ref()
            .filter(|s| s.id == id)
            .ok_or(RoutingError::RepairSearchNotFound(id))?;
        if cancel && search.progress.lock().stop.is_none() {
            search.cancel.store(true, Ordering::Relaxed);
        }
        Ok(search.status())
    }

    /// Returns evidence only for a finished search on the unchanged source.
    pub fn repair_search_evidence(
        &self,
        project: &Project,
        board: BoardId,
        target: NetSignalId,
        id: Uuid,
    ) -> Result<RepairSearchEvidence, RoutingError> {
        let search = self
            .repair_search
            .as_ref()
            .filter(|s| s.id == id)
            .ok_or(RoutingError::RepairSearchNotFound(id))?;
        if search.board != board || search.target != target {
            return Err(Error::InvalidArgument(
                "The repair search is for a different board or target net.".into(),
            )
            .into());
        }
        if search.project != project.uuid() || search.context != SourceContext::of(project) {
            return Err(RoutingError::StaleContext);
        }
        if search.revision != project.revision() {
            return Err(RoutingError::StaleRevision {
                expected: search.revision,
                actual: project.revision(),
            });
        }
        let progress = search.progress.lock();
        if progress.stop.is_none() || progress.stop == Some(StopReason::Failed) {
            return Err(RoutingError::Refused(
                "Use a finished repair search with checked trials.".into(),
            ));
        }
        Ok(RepairSearchEvidence {
            launches: progress.launch_failures.clone(),
            trials: progress.trials.clone(),
            screenings: progress.screening_failures.clone(),
            request: search.request.clone(),
        })
    }

    pub fn cancel_repair_search(&self) {
        if let Some(search) = &self.repair_search
            && search.progress.lock().stop.is_none()
        {
            search.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn finish_repair_search(
        &mut self,
        project: &Project,
        id: Uuid,
        result: Result<RepairSearchResults, RoutingError>,
    ) {
        let Some(state) = self.repair_search.as_ref().filter(|s| s.id == id).cloned() else {
            return;
        };
        let result = result.and_then(|result| {
            debug_assert_eq!(result.search, id);
            if state.project != project.uuid() || state.context != SourceContext::of(project) {
                return Err(RoutingError::StaleContext);
            }
            if state.revision != project.revision() {
                return Err(RoutingError::StaleRevision {
                    expected: state.revision,
                    actual: project.revision(),
                });
            }
            debug_assert!(result.candidates.len() <= state.reserved);
            for (candidate, _) in &result.candidates {
                self.check_source(project, candidate)?;
            }
            Ok(result)
        });
        let mut progress = state.progress.lock();
        progress.phase = RepairSearchPhase::Finished;
        progress.elapsed = Some(state.started.elapsed());
        progress.groups.clear();
        progress.strategy = None;
        match result {
            Ok(result) => {
                progress.stop = Some(if state.cancel.load(Ordering::Relaxed) {
                    StopReason::Cancelled
                } else {
                    result.stop
                });
                for (candidate, choice) in result.candidates {
                    self.remember_baseline(&candidate);
                    self.candidates.insert(choice.candidate, candidate);
                    progress.candidates.push(choice);
                }
            }
            Err(error) => {
                progress.stop = Some(StopReason::Failed);
                progress.failure = Some(error.to_string());
            }
        }
    }
}

impl Drop for RoutingService {
    fn drop(&mut self) {
        self.cancel_repair_search();
        self.cancel_placement_search();
    }
}

impl RepairSearchRequest {
    fn strategies(&self) -> &'static [RepairStrategy] {
        if self.neighbor_launch_search.is_some() {
            &[RepairStrategy::Joint]
        } else if self.groups.is_empty() || self.joint_route.is_none() {
            &[RepairStrategy::Sequential]
        } else {
            &[RepairStrategy::Sequential, RepairStrategy::Joint]
        }
    }

    fn launch_pad<'a>(
        project: &'a Project,
        board: &'a Board,
        (component, pad): (ComponentInstanceId, Uuid),
        layer: Layer,
        expected_net: Option<NetSignalId>,
    ) -> Result<FootprintPadView<'a>, RoutingError> {
        let view = board
            .device(component)
            .ok_or_else(|| Error::not_found("Device", component))?
            .pad(&pad, project.library(), project.circuit())
            .map_err(Error::from)?
            .ok_or_else(|| Error::not_found("Pad", pad))?;
        if view.net().is_none()
            || expected_net.is_some_and(|net| view.net() != Some(net))
            || !board.copper_layers().contains(&layer)
            || !view.is_on_layer(layer)
        {
            return Err(Error::InvalidArgument(
                "The launch pad must belong to the requested net and copper layer.".into(),
            )
            .into());
        }
        Ok(view)
    }

    fn plan(&self, project: &Project, board: &Board) -> Result<Vec<Trial>, RoutingError> {
        if self.groups.len() > 8
            || (self.groups.is_empty() && self.target_launch_search.is_none())
            || !(1..=16).contains(&self.max_objects)
            || !(1..=72).contains(&self.max_trials)
            || !(1..=3).contains(&self.max_candidates)
            || self.trace_requirements.len() > 16
            || self.time_budget.is_zero()
            || self.time_budget > Duration::from_secs(300)
        {
            return Err(Error::InvalidArgument("Invalid repair search limits.".into()).into());
        }
        let target_bounds = self
            .route
            .region
            .ok_or_else(|| Error::InvalidArgument("A target routing region is required.".into()))?
            .region()?;
        let edit_bounds = self.rip_up_region.region()?;
        let neighbor_bounds = self
            .neighbor_route
            .region
            .unwrap_or(self.rip_up_region)
            .region()?;
        if !self.groups.is_empty()
            && ![self.rip_up_region.min, self.rip_up_region.max]
                .into_iter()
                .all(|p| {
                    target_bounds.contains(p, Length::ZERO)
                        && neighbor_bounds.contains(p, Length::ZERO)
                })
        {
            return Err(Error::InvalidArgument(
                "rip_up_region must fit both routing regions.".into(),
            )
            .into());
        }
        if let Some(launch) = &self.target_launch {
            let view =
                Self::launch_pad(project, board, launch.from, launch.layer, Some(self.target))?;
            let lead_radius = Length::new(launch.width.to_nm() / 2 + launch.width.to_nm() % 2);
            if !target_bounds.contains(view.position(), lead_radius)
                || !target_bounds.contains(launch.position, lead_radius)
            {
                return Err(Error::InvalidArgument(
                    "The launch pad and lead must be on the target net and inside region.".into(),
                )
                .into());
            }
        }
        if let Some(search) = &self.target_launch_search {
            if self.target_launch.is_some()
                || !(1..=8).contains(&search.max_sites)
                || search.radius.to_nm() > 1_500_000
            {
                return Err(Error::InvalidArgument("Supply either target_launch or target_launch_search. The search radius is at most 1.5 mm with 1–8 sites.".into()).into());
            }
            let view =
                Self::launch_pad(project, board, search.from, search.layer, Some(self.target))?;
            if !target_bounds.contains(view.position(), Length::ZERO) {
                return Err(Error::InvalidArgument(
                    "The launch pad must lie inside region.".into(),
                )
                .into());
            }
        }
        let neighbor_net = if let Some(search) = &self.neighbor_launch_search {
            if self.joint_route.is_none()
                || self.target_launch_search.is_none()
                || !(1..=8).contains(&search.max_sites)
                || search.radius.to_nm() > 1_500_000
            {
                return Err(Error::InvalidArgument(
                    "A neighbor launch requires joint_route and target_launch_search, with radius at most 1.5 mm and 1–8 sites.".into(),
                ).into());
            }
            let view = Self::launch_pad(project, board, search.from, search.layer, None)?;
            let net = view.net().filter(|net| *net != self.target);
            if net.is_none()
                || !neighbor_bounds.contains(view.position(), Length::ZERO)
                || !target_bounds.contains(view.position(), Length::ZERO)
            {
                return Err(Error::InvalidArgument(
                    "The neighbor launch pad must be on a different net and inside both routing regions.".into(),
                ).into());
            }
            net
        } else {
            None
        };
        let copper = super::region::copper(project, board)?;
        if self
            .neighbor_anchors
            .keys()
            .any(|i| *i >= self.groups.len())
        {
            return Err(
                Error::InvalidArgument("neighbor_anchors must refer to a group.".into()).into(),
            );
        }
        let mut branches = BTreeMap::new();
        let mut seen = BTreeSet::new();
        let mut trials = Vec::new();
        for (i, group) in self.groups.iter().enumerate() {
            if group.is_empty() || group.len() > self.max_objects {
                return Err(Error::InvalidArgument(
                    "Each group needs 1 to max_objects copper UUIDs.".into(),
                )
                .into());
            }
            super::region::check_rip_up_permissions(project, board.id(), group)?;
            for id in group {
                let item = copper
                    .get(id)
                    .ok_or_else(|| Error::not_found("Trace or via", id))?;
                if item.net().is_none_or(|net| net == self.target) || !item.fits(edit_bounds) {
                    return Err(Error::InvalidArgument(format!(
                        "Copper {id} must belong to a neighboring net and fit rip_up_region."
                    ))
                    .into());
                }
                if item.is_via() {
                    item.select_via(*id, board, group, &mut Default::default())?;
                }
            }
            let mut branch = None;
            if let Some(&ends) = self.neighbor_anchors.get(&i) {
                let wire = super::region::retained_branch(&copper, board, group, ends).ok_or_else(|| {
                    Error::InvalidArgument(format!(
                        "Group {i} must be a single branch between the two retained neighbor anchors."
                    ))
                })?;
                branches.insert(i, wire.clone());
                branch = Some(wire);
            }
            let pair = branch.as_ref().map(|wire| [wire.p1(), wire.p2()]);
            if seen.insert((group.clone(), pair.into_iter().collect::<Vec<_>>())) {
                trials.push(Trial {
                    groups: vec![i],
                    rip_up: group.clone(),
                    nets: group.iter().filter_map(|id| copper[id].net()).collect(),
                    branches: branch.into_iter().collect(),
                });
            }
        }
        for (i, first) in self.groups.iter().enumerate() {
            for (j, second) in self.groups.iter().enumerate().skip(i + 1) {
                let rip_up: BTreeSet<_> = first.union(second).copied().collect();
                let constrained = [branches.get(&i), branches.get(&j)];
                // A constrained net has exactly one route request. An extra removal
                // on that net could delete an endpoint or require another connection.
                if constrained[0].is_some_and(|branch| {
                    second
                        .iter()
                        .any(|id| copper[id].net() == Some(branch.net()))
                }) || constrained[1].is_some_and(|branch| {
                    first
                        .iter()
                        .any(|id| copper[id].net() == Some(branch.net()))
                }) {
                    continue;
                }
                let branches: Vec<_> = constrained.into_iter().flatten().cloned().collect();
                let mut pairs: Vec<_> =
                    branches.iter().map(|wire| [wire.p1(), wire.p2()]).collect();
                pairs.sort_unstable();
                if rip_up.len() <= self.max_objects && seen.insert((rip_up.clone(), pairs)) {
                    let nets = rip_up.iter().filter_map(|id| copper[id].net()).collect();
                    trials.push(Trial {
                        groups: vec![i, j],
                        rip_up,
                        nets,
                        branches,
                    });
                }
            }
        }
        trials.sort_by_key(|trial| (trial.groups.len(), trial.rip_up.len()));
        if let Some(net) = neighbor_net {
            trials.retain(|trial| {
                trial.nets.contains(&net)
                    && !trial.branches.iter().any(|branch| branch.net() == net)
            });
            if trials.is_empty() {
                return Err(Error::InvalidArgument(
                    "A neighbor launch needs a removal group on that net without exact neighbor anchors.".into(),
                ).into());
            }
        }
        if trials.is_empty() {
            trials.push(Trial {
                groups: Vec::new(),
                rip_up: BTreeSet::new(),
                nets: BTreeSet::new(),
                branches: Vec::new(),
            });
        }
        Ok(trials)
    }
}

impl RepairLaunch {
    fn stage(
        &self,
        editor: &mut ProjectEditor,
        board: BoardId,
        net: NetSignalId,
    ) -> Result<Uuid, RoutingError> {
        let plan = CopperPlan {
            board,
            remove_traces: BTreeSet::new(),
            remove_vias: BTreeSet::new(),
            placements: Vec::new(),
            vias: vec![AddVia {
                board: Some(board),
                position: self.position,
                net: Some(NetRef::Id(net)),
                start_layer: None,
                end_layer: None,
                drill_diameter: self.drill,
                size: self.size,
                exposure: None,
            }],
            traces: vec![super::PlannedTrace {
                start: super::CopperEndpoint::Existing(TraceEndpoint::FootprintPad {
                    component: self.from.0,
                    pad: self.from.1,
                }),
                end: super::CopperEndpoint::NewVia(0),
                points: Vec::new(),
                layer: Some(self.layer),
                width: Some(self.width),
                net: Some(NetRef::Id(net)),
            }],
        };
        let (vias, _) = plan.run(editor)?;
        Ok(vias[0]) // The plan adds exactly one via.
    }
}

impl RepairSearchJob {
    /// Remove temporary vias that the native DRC found to have no layer change.
    /// Existing vias and existing attached traces require separate authorization.
    fn prune_generated_vias(
        &self,
        sandbox: &mut ProjectEditor,
        original: &BTreeMap<Uuid, super::region::Copper>,
        validation: &RoutingValidation,
    ) -> Result<Option<(BTreeSet<Uuid>, RoutingSnapshot)>, RoutingError> {
        let board_id = self.state.board;
        let board = resolve_board(sandbox.project(), Some(board_id))?;
        let useless: BTreeSet<_> = validation
            .new_warnings
            .iter()
            .filter(|warning| warning.kind == "UselessVia")
            .flat_map(|warning| warning.objects.iter().map(|object| object.uuid))
            .filter(|via| !original.contains_key(via))
            .collect();
        if useless.is_empty() {
            return Ok(None);
        }
        let mut selection = BoardSelection::default();
        for (id, segment) in board.net_segments() {
            for &via in segment.vias().keys().filter(|via| useless.contains(via)) {
                let traces: Vec<_> = segment.traces_at(TraceAnchor::Via(via)).collect();
                if traces
                    .iter()
                    .any(|trace| original.contains_key(&trace.uuid()))
                {
                    continue;
                }
                selection.vias.push((*id, via));
                // A single incident trace is a dead end. With multiple
                // traces, native removal substitutes a same-layer junction
                // at the via position and preserves the trace geometry.
                if let [trace] = traces.as_slice() {
                    selection.traces.push((*id, trace.uuid()));
                }
            }
        }
        if selection.vias.is_empty() {
            return Ok(None);
        }
        let removed = selection.vias.iter().map(|(_, via)| *via).collect();
        sandbox.execute(super::region::RipUp {
            board: board_id,
            selection,
        })?;
        let after =
            capture_routing_state_with_cancel(sandbox, board_id, &|| self.state.stop().is_some())?;
        Ok(Some((removed, after)))
    }

    fn check_trace_requirements(
        &self,
        project: &Project,
        baseline_paths: &[TracePreferenceResult],
    ) -> crate::Result<Vec<RepairTraceCheck>> {
        self.request
            .trace_requirements
            .iter()
            .zip(baseline_paths)
            .map(|(&requirement, before)| {
                Ok(RepairTraceCheck {
                    requirement,
                    before: before.clone(),
                    after: requirement.measure(project, self.state.board)?,
                })
            })
            .collect()
    }

    fn launch_snapshot(&self, trial: &Trial) -> Result<RoutingSource, RoutingError> {
        let mut source = self.source.fork()?;
        if trial.rip_up.is_empty() {
            return Ok(source);
        }
        let project = source.sandbox.project();
        let board = resolve_board(project, Some(self.state.board))?;
        let copper = super::region::copper(project, board)?;
        let selection = super::region::select_copper(board, &copper, &trial.rip_up)?;
        source.sandbox.execute(super::region::RipUp {
            board: self.state.board,
            selection,
        })?;
        Ok(source)
    }

    fn find_launch_sites(
        &self,
        project: &Project,
        search: &RepairLaunchSearch,
        route: &Autoroute,
    ) -> Result<Vec<RepairLaunch>, RoutingError> {
        let board = resolve_board(project, Some(self.state.board))?;
        let pad = RepairSearchRequest::launch_pad(project, board, search.from, search.layer, None)?;
        let net = pad
            .net()
            .ok_or_else(|| Error::InvalidArgument("Unconnected launch pad.".into()))?;
        let options = Autoroute {
            // The straight lead is legal on the pad layer even if the
            // continuation is restricted to other layers.
            layers: Some(vec![search.layer]),
            grid_pitch: None,
            ..route.clone()
        };
        let mut extracted = crate::commands::autoroute::extract_problem(
            project,
            board.id(),
            &options,
            Some(&BTreeSet::from([net])),
        )?;
        let problem = &mut extracted.problem;
        if problem.rules.no_vias.contains(&net.0)
            || problem
                .rules
                .net_layers
                .get(&net.0)
                .is_some_and(|layers| !layers.contains(&search.layer) || layers.len() < 2)
        {
            return Ok(Vec::new());
        }
        // Test exact lead and via geometry at the native rule thresholds.
        // The grid router keeps its extra margin for continuations.
        use crate::commands::autoroute::CLEARANCE_MARGIN;
        let rules = &mut problem.rules;
        rules.clearance -= CLEARANCE_MARGIN;
        for clearance in rules.net_clearances.values_mut() {
            *clearance -= CLEARANCE_MARGIN;
        }
        rules.board_clearance -= CLEARANCE_MARGIN;
        rules.npth_clearance -= CLEARANCE_MARGIN;
        rules.drill_clearance -= CLEARANCE_MARGIN;
        rules.drill_board_clearance -= CLEARANCE_MARGIN;
        for clearance in problem
            .terminals
            .iter_mut()
            .map(|t| &mut t.clearance)
            .chain(problem.obstacles.iter_mut().map(|o| &mut o.clearance))
            .flatten()
        {
            *clearance -= CLEARANCE_MARGIN;
        }
        let cancelled = || self.state.stop().is_some();
        let anchor = (pad.position(), search.layer);
        let sites = if self.request.neighbor_launch_search.is_some() {
            foilfern_autoroute::find_spaced_escape_vias(
                problem,
                net.0,
                anchor,
                search.radius,
                search.max_sites,
                cancelled,
            )
        } else {
            foilfern_autoroute::find_escape_vias(
                problem,
                net.0,
                anchor,
                search.radius,
                search.max_sites,
                cancelled,
            )
        }
        .map_err(Error::from)?;
        Ok(sites
            .into_iter()
            .map(|position| RepairLaunch {
                from: search.from,
                layer: search.layer,
                position,
                width: problem.rules.trace_width_of(&net.0),
                drill: Some(problem.rules.via_drill),
                size: Some(problem.rules.via_diameter),
            })
            .collect())
    }

    fn record_screening_failure(
        &self,
        project: &Project,
        search: &RepairLaunchSearch,
        trial: &Trial,
        target_launch: Option<RepairLaunch>,
        route: &Autoroute,
    ) -> Result<(), RoutingError> {
        // Sampling is diagnostic only and bounded independently of the trial budget.
        if self.state.progress.lock().screening_failures.len() >= 16 || self.state.stop().is_some()
        {
            return Ok(());
        }
        let board = resolve_board(project, Some(self.state.board))?;
        let pad = RepairSearchRequest::launch_pad(project, board, search.from, search.layer, None)?;
        let Some(net) = pad.net() else { return Ok(()) };
        let options = Autoroute {
            layers: Some(vec![search.layer]),
            ..route.clone()
        };
        let extracted =
            extract_problem(project, board.id(), &options, Some(&BTreeSet::from([net])))?;
        let anchor = TraceAnchor::FootprintPad {
            device: search.from.0.0,
            pad: search.from.1,
        };
        let terminals: Vec<_> = extracted
            .terminal_anchors
            .iter()
            .enumerate()
            .filter(|(_, a)| **a == anchor)
            .map(|(i, _)| foilfern_autoroute::TerminalId(i))
            .collect();
        let reports =
            diagnose_access(&extracted.problem, &terminals, search.radius).map_err(Error::from)?;
        let mut blockers = Vec::new();
        for probe in reports.iter().flat_map(|report| &report.probes) {
            let object = match probe.blocker {
                Some(AccessBlocker::Obstacle(i)) => extracted.obstacle_objects[i],
                Some(AccessBlocker::Hole(i)) => extracted.hole_objects[i],
                Some(AccessBlocker::Terminal(id)) => {
                    RoutingObject::from_anchor(extracted.terminal_anchors[id.0])
                }
                Some(AccessBlocker::BoardEdge) | None => continue,
            };
            if !blockers.contains(&object) {
                blockers.push(object);
            }
        }
        self.state
            .progress
            .lock()
            .screening_failures
            .push(RepairScreeningFailure {
                groups: trial.groups.clone(),
                rip_up: trial.rip_up.clone(),
                from: search.from,
                net,
                layer: search.layer,
                position: pad.position(),
                target_launch,
                blockers,
            });
        Ok(())
    }

    fn trial_launches(
        &self,
        search: &RepairLaunchSearch,
        trial: &Trial,
    ) -> Result<(Vec<RepairLaunch>, Vec<TrialLaunches>), RoutingError> {
        let source = self.launch_snapshot(trial)?;
        let Some(neighbor) = &self.request.neighbor_launch_search else {
            let sites =
                self.find_launch_sites(source.sandbox.project(), search, &self.request.route)?;
            if sites.is_empty() {
                self.record_screening_failure(
                    source.sandbox.project(),
                    search,
                    trial,
                    None,
                    &self.request.route,
                )?;
            }
            let launches = sites
                .iter()
                .cloned()
                .map(|target| TrialLaunches {
                    target: Some(target),
                    neighbor: None,
                })
                .collect();
            return Ok((sites, launches));
        };
        let mut route =
            self.request.joint_route.clone().ok_or_else(|| {
                Error::InvalidArgument("Joint routing settings are required.".into())
            })?;
        route.region = self.request.route.region;
        let sites = self.find_launch_sites(source.sandbox.project(), search, &route)?;
        if sites.is_empty() {
            self.record_screening_failure(source.sandbox.project(), search, trial, None, &route)?;
        }
        route.region = self
            .request
            .neighbor_route
            .region
            .or(Some(self.request.rip_up_region));
        let mut pairs = Vec::new();
        for (target_index, target) in sites.iter().enumerate() {
            if self.state.stop().is_some() {
                break;
            }
            let mut sandbox = source.fork()?.sandbox;
            target.stage(&mut sandbox, self.state.board, self.state.target)?;
            // The neighbor is screened against the staged lead and via, including drills.
            let neighbor_sites = self.find_launch_sites(sandbox.project(), neighbor, &route)?;
            if neighbor_sites.is_empty() {
                self.record_screening_failure(
                    sandbox.project(),
                    neighbor,
                    trial,
                    Some(target.clone()),
                    &route,
                )?;
            }
            for (neighbor_index, neighbor) in neighbor_sites.into_iter().enumerate() {
                pairs.push((
                    target_index + neighbor_index,
                    TrialLaunches {
                        target: Some(target.clone()),
                        neighbor: Some(neighbor),
                    },
                ));
            }
        }
        pairs.sort_by_key(|(rank, _)| *rank);
        Ok((sites, pairs.into_iter().map(|(_, pair)| pair).collect()))
    }

    fn run_trial(
        &self,
        sandbox: &mut ProjectEditor,
        trial: &Trial,
        report: &mut RepairTrial,
    ) -> Result<Option<(RepairCandidateDetails, Option<Uuid>)>, RoutingError> {
        let board = self.state.board;
        let target = self.state.target;
        let cancelled = || self.state.stop().is_some();
        let launch = report.target_launch.clone();
        let neighbor_launch = report.neighbor_launch.clone();
        let mut launch_via = None;
        let stage_launch = |sandbox: &mut ProjectEditor| {
            if let Some(launch) = &launch {
                launch_via = Some(launch.stage(sandbox, board, target)?);
            }
            if let Some(launch) = &neighbor_launch {
                let project = sandbox.project();
                let native = resolve_board(project, Some(board))?;
                let pad = RepairSearchRequest::launch_pad(
                    project,
                    native,
                    launch.from,
                    launch.layer,
                    None,
                )?;
                let net = pad.net().ok_or_else(|| {
                    Error::InvalidArgument("Unconnected neighbor launch pad.".into())
                })?;
                launch.stage(sandbox, board, net)?;
            }
            Ok(())
        };
        let neighbor_bounds = self
            .request
            .neighbor_route
            .region
            .unwrap_or(self.request.rip_up_region);
        let rip_up = trial.rip_up.iter().copied().collect();
        let (neighbor, joint, route, routing) = match report.strategy {
            RepairStrategy::Sequential => {
                let neighbor = RegionalRoutingRequest {
                    request: Autoroute {
                        board: Some(board),
                        nets: Some(trial.nets.iter().copied().map(NetRef::Id).collect()),
                        region: Some(neighbor_bounds),
                        ..self.request.neighbor_route.clone()
                    },
                    rip_up,
                    rip_up_region: Some(self.request.rip_up_region),
                    improve_net: None,
                };
                let neighbor = if trial.groups.is_empty() {
                    None
                } else {
                    let result = neighbor.run_with_staging(
                        sandbox,
                        board,
                        None,
                        &trial.branches,
                        cancelled,
                        |_| Ok(()),
                    )?;
                    report.neighbor_failures = summarize_failures(&result, |_| true);
                    if cancelled() {
                        return Ok(None);
                    }
                    require_branches_routed(&result, &trial.branches)?;
                    rebuild_connectivity(sandbox, board)?;
                    if cancelled() {
                        return Ok(None);
                    }
                    Some(neighbor)
                };
                self.state.phase(RepairSearchPhase::Target);
                let route = Autoroute {
                    board: Some(board),
                    nets: Some(vec![NetRef::Id(target)]),
                    ..self.request.route.clone()
                };
                let request = RegionalRoutingRequest {
                    request: route.clone(),
                    rip_up: Vec::new(),
                    rip_up_region: None,
                    improve_net: Some(target),
                };
                let routing =
                    request.run_with_staging(sandbox, board, None, &[], cancelled, stage_launch)?;
                (neighbor, None, route, routing)
            }
            RepairStrategy::Joint => {
                let Some(settings) = &self.request.joint_route else {
                    return Err(Error::InvalidArgument(
                        "Joint routing settings are required.".into(),
                    )
                    .into());
                };
                let route = Autoroute {
                    board: Some(board),
                    nets: Some(
                        std::iter::once(target)
                            .chain(trial.nets.iter().copied())
                            .map(NetRef::Id)
                            .collect(),
                    ),
                    region: self.request.route.region,
                    ..settings.clone()
                };
                let request = RegionalRoutingRequest {
                    request: route.clone(),
                    rip_up,
                    rip_up_region: Some(self.request.rip_up_region),
                    improve_net: Some(target),
                };
                let routing = request.run_with_staging(
                    sandbox,
                    board,
                    Some((target, neighbor_bounds.region()?)),
                    &trial.branches,
                    cancelled,
                    stage_launch,
                )?;
                report.neighbor_failures =
                    summarize_failures(&routing, |failure| failure.net != target);
                report.target_failures =
                    summarize_failures(&routing, |failure| failure.net == target);
                if cancelled() {
                    return Ok(None);
                }
                require_branches_routed(&routing, &trial.branches)?;
                (None, Some(request), route, routing)
            }
        };
        report.target_failures = summarize_failures(&routing, |failure| failure.net == target);
        Ok(Some((
            RepairCandidateDetails {
                target,
                groups: trial.groups.clone(),
                strategy: report.strategy,
                neighbor,
                joint,
                neighbor_region: (report.strategy == RepairStrategy::Joint)
                    .then_some(neighbor_bounds),
                route,
                routing,
                trace_requirements: Vec::new(),
                target_launch: launch,
                neighbor_launch,
                pruned_vias: BTreeSet::new(),
                neighbor_branches: trial.branches.clone(),
            },
            launch_via,
        )))
    }

    pub fn run(mut self) -> Result<RepairSearchResults, RoutingError> {
        let board = self.state.board;
        let target = self.state.target;
        let mut candidates = Vec::new();
        let mut stop = StopReason::Completed;
        if let Some(reason) = self.state.stop() {
            return Ok(RepairSearchResults {
                search: self.state.id,
                stop: reason,
                candidates,
            });
        }
        self.state.phase(RepairSearchPhase::Baseline);
        let before = self
            .source
            .capture_before_with_cancel(board, &|| self.state.stop().is_some())?;
        if let Some(reason) = self.state.stop() {
            return Ok(RepairSearchResults {
                search: self.state.id,
                stop: reason,
                candidates,
            });
        }
        if !before.checks.complete {
            return Err(RoutingError::Refused(
                "Source board checks did not complete.".into(),
            ));
        }
        if before.unrouted_for(target) == 0 {
            return Err(RoutingError::Refused(
                "The repair target has no open connections.".into(),
            ));
        }
        let baseline_paths = self
            .request
            .trace_requirements
            .iter()
            .map(|path| path.measure(self.source.sandbox.project(), board))
            .collect::<crate::Result<Vec<_>>>()?;
        let original_copper = self
            .request
            .neighbor_launch_search
            .as_ref()
            .map(|_| {
                let project = self.source.sandbox.project();
                super::region::copper(project, resolve_board(project, Some(board))?)
            })
            .transpose()?;
        let strategies = self.request.strategies();
        let launches = if let Some(search) = &self.request.target_launch_search {
            self.state.phase(RepairSearchPhase::LaunchSites);
            let mut launches = Vec::new();
            let mut sites = Vec::new();
            for trial in &self.trials {
                if self.state.stop().is_some() {
                    break;
                }
                let (trial_sites, trials) = self.trial_launches(search, trial)?;
                for site in &trial_sites {
                    if !sites
                        .iter()
                        .any(|other: &RepairLaunch| other.position == site.position)
                    {
                        sites.push(site.clone());
                    }
                }
                launches.push(trials);
            }
            let mut progress = self.state.progress.lock();
            let combinations = launches.iter().map(Vec::len).sum::<usize>() * strategies.len();
            progress.planned = combinations.min(self.request.max_trials);
            progress.launch_sites = Some(sites);
            launches
        } else {
            vec![
                vec![TrialLaunches {
                    target: self.request.target_launch.clone(),
                    neighbor: None,
                }];
                self.trials.len()
            ]
        };
        if let Some(reason) = self.state.stop() {
            return Ok(RepairSearchResults {
                search: self.state.id,
                stop: reason,
                candidates,
            });
        }
        // Give each scope a chance at its best site before trying its
        // second site. A tight budget can still reach a joint pair.
        let max_sites = launches.iter().map(Vec::len).max().unwrap_or(0);
        let trials = (0..max_sites).flat_map(|index| {
            self.trials
                .iter()
                .zip(&launches)
                .flat_map(move |(trial, sites)| {
                    sites.get(index).into_iter().flat_map(move |launch| {
                        strategies
                            .iter()
                            .map(move |&strategy| (trial, strategy, launch))
                    })
                })
        });
        for (i, (trial, strategy, launch)) in trials.enumerate() {
            if let Some(reason) = self.state.stop() {
                stop = reason;
                break;
            }
            if i == self.request.max_trials {
                stop = StopReason::TrialLimit;
                break;
            }
            {
                let mut progress = self.state.progress.lock();
                progress.groups.clone_from(&trial.groups);
                progress.strategy = Some(strategy);
                progress.phase = match strategy {
                    RepairStrategy::Sequential => RepairSearchPhase::Neighbors,
                    RepairStrategy::Joint => RepairSearchPhase::Joint,
                };
            }
            let mut source = self.source.fork()?;
            let mut report = RepairTrial {
                groups: trial.groups.clone(),
                strategy,
                target_launch: launch.target.clone(),
                neighbor_launch: launch.neighbor.clone(),
                pruned_vias: BTreeSet::new(),
                rip_up: trial.rip_up.clone(),
                target_unrouted: None,
                validation: None,
                failure: None,
                neighbor_failures: Vec::new(),
                target_failures: Vec::new(),
                trace_requirements: Vec::new(),
                neighbor_branches: trial.branches.clone(),
            };
            let routed = self.run_trial(&mut source.sandbox, trial, &mut report);
            if let Some(reason) = self.state.stop() {
                stop = reason;
                break;
            }
            let (mut repair, launch_via) = match routed {
                Ok(Some(result)) => result,
                Ok(None) => {
                    // run_trial returns None only after observing cancellation.
                    stop = self.state.stop().unwrap_or(StopReason::Cancelled);
                    break;
                }
                Err(error) => {
                    report.failure = Some(error.to_string());
                    self.state.progress.lock().trials.push(report);
                    continue;
                }
            };
            self.state.phase(RepairSearchPhase::Validation);
            let mut after = capture_routing_state_with_cancel(&mut source.sandbox, board, &|| {
                self.state.stop().is_some()
            })?;
            if let Some(reason) = self.state.stop()
                && !after.checks.complete
            {
                stop = reason;
                break;
            }
            let target_unrouted = after.unrouted_for(target);
            report.target_unrouted = Some(target_unrouted);
            let mut launch_failures = Vec::new();
            if self.request.groups.is_empty()
                && let (Some(via), Some(launch)) = (launch_via, &launch.target)
            {
                launch_failures.extend(
                    repair
                        .routing
                        .failed_requests
                        .iter()
                        .filter(|wire| {
                            wire.net == target
                                && !wire.search_evidence.is_empty()
                                && [wire.p1, wire.p2].contains(&TraceAnchor::Via(via))
                        })
                        .take(8)
                        .map(|wire| RepairLaunchFailure {
                            launch: launch.clone(),
                            via,
                            wire: wire.clone(),
                        }),
                );
            }
            if after.checks.complete {
                report.trace_requirements =
                    self.check_trace_requirements(source.sandbox.project(), &baseline_paths)?;
            }
            repair
                .trace_requirements
                .clone_from(&report.trace_requirements);
            let validate = |repair: &RepairCandidateDetails, after: &RoutingSnapshot| {
                let mut validation = RoutingValidation::compare_candidate(&before, after);
                validation.acceptable &= repair.improves(&before, after);
                validation
            };
            let mut validation = validate(&repair, &after);
            if validation.acceptable
                && let Some(original) = &original_copper
                && let Some((removed, pruned)) =
                    self.prune_generated_vias(&mut source.sandbox, original, &validation)?
            {
                let checked_paths = if pruned.checks.complete {
                    Some(self.check_trace_requirements(source.sandbox.project(), &baseline_paths)?)
                } else {
                    None
                };
                let pruned_validation = validate(&repair, &pruned);
                if pruned_validation.acceptable
                    && pruned_validation.after.unrouted <= validation.after.unrouted
                    && pruned_validation.new_warning_count < validation.new_warning_count
                    && let Some(paths) = checked_paths
                    && paths.iter().all(|check| check.after.met)
                {
                    repair.trace_requirements.clone_from(&paths);
                    report.trace_requirements = paths;
                    repair.pruned_vias.clone_from(&removed);
                    report.pruned_vias = removed;
                    after = pruned;
                    validation = pruned_validation;
                } else {
                    source.sandbox.undo()?;
                }
            }
            let checked_launch = validation.after.complete
                && validation.netlist_preserved
                && validation.new_error_count == 0
                && validation.unrouted_regressions.is_empty()
                && !validation
                    .board_requirements_after
                    .iter()
                    .any(crate::board_requirements::Finding::blocks_routing);
            if validation.acceptable {
                let (vias, trace_length_mm) = copper_cost(source.sandbox.project(), board)?;
                let details = RoutingCandidateDetails::Repair {
                    repair: Box::new(repair),
                };
                let candidate = source.freeze(board, details, before.clone(), after)?;
                let choice = RepairChoice {
                    candidate: candidate.info().candidate,
                    groups: trial.groups.clone(),
                    strategy,
                    target_launch: launch.target.clone(),
                    neighbor_launch: launch.neighbor.clone(),
                    target_unrouted,
                    affected_objects: trial.rip_up.len(),
                    new_warnings: validation.new_warning_count,
                    vias,
                    trace_length_mm,
                };
                candidates.push((candidate, choice));
                candidates.sort_by(|(_, a), (_, b)| {
                    a.rank().cmp(&b.rank()).then_with(|| {
                        a.trace_length_mm
                            .unwrap_or(f64::INFINITY)
                            .total_cmp(&b.trace_length_mm.unwrap_or(f64::INFINITY))
                    })
                });
                candidates.truncate(self.request.max_candidates);
            }
            report.validation = Some(validation);
            {
                let mut progress = self.state.progress.lock();
                progress.trials.push(report);
                if checked_launch {
                    progress.launch_failures.extend(launch_failures);
                }
            }
            if let Some(reason) = self.state.stop() {
                stop = reason;
                break;
            }
        }
        Ok(RepairSearchResults {
            search: self.state.id,
            stop,
            candidates,
        })
    }
}
