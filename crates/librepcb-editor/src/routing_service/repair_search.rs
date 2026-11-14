//! Bounded trials of authorized local repairs using the builtin router.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use librepcb_core::geometry::TraceAnchor;
use librepcb_core::project::board::AirWire;
use librepcb_core::project::{BoardId, NetSignalId, Project};
use librepcb_core::types::{Length, Uuid};
use parking_lot::Mutex;
use serde::Serialize;

use super::{
    CopperPlan, RegionalRoutingRequest, RoutingCandidate, RoutingCandidateDetails, RoutingError,
    RoutingService, RoutingSource, SourceContext, TracePreference,
    comparison::{TracePreferenceResult, copper_cost},
    rebuild_connectivity,
};
use crate::commands::{
    AddVia, Autoroute, AutorouteResult, NetRef, RoutingBounds, TraceEndpoint, UnroutedAirWire,
    net_name, resolve_board,
};
use crate::routing_validation::{RoutingValidation, capture_routing_state_with_cancel};
use crate::{Error, ProjectEditor};

#[derive(Debug)]
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
}

/// An authorized additive, straight pad-to-via lead on the target net.
#[derive(Debug, Clone, Serialize)]
pub struct RepairLaunch {
    pub from: (librepcb_core::project::ComponentInstanceId, Uuid),
    pub layer: librepcb_core::types::Layer,
    pub position: librepcb_core::types::Point,
    pub width: librepcb_core::types::PositiveLength,
    pub drill: Option<librepcb_core::types::PositiveLength>,
    pub size: Option<librepcb_core::types::PositiveLength>,
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub trace_requirements: Vec<RepairTraceCheck>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub neighbor_branches: Vec<AirWire>,
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
}

fn summarize_failures(
    result: &AutorouteResult,
    include: impl Fn(&UnroutedAirWire) -> bool,
) -> Vec<RepairFailure> {
    let summarize = |failure: &UnroutedAirWire| RepairFailure {
        net: failure.net,
        name: failure.net_name.clone(),
        reason: failure.reason.clone(),
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
    trials: Vec<RepairTrial>,
    stop: Option<StopReason>,
    candidates: Vec<RepairChoice>,
    failure: Option<String>,
    elapsed: Option<Duration>,
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
    started: Instant,
    budget: Duration,
    planned: usize,
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
            planned_trials: self.planned,
            current_groups: progress.groups.clone(),
            current_strategy: progress.strategy,
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
        let strategy_count = 1 + usize::from(request.joint_route.is_some());
        let source = self.capture_source(editor, request.max_candidates)?;
        let state = Arc::new(ActiveRepairSearch {
            id: Uuid::new_random(),
            project: source.project,
            board: board.id(),
            target: request.target,
            target_name: net_name(project, Some(request.target)),
            revision: source.base_revision,
            context: source.source_context.clone(),
            started: Instant::now(),
            budget: request.time_budget,
            planned: (trials.len() * strategy_count).min(request.max_trials),
            reserved: request.max_candidates,
            cancel: AtomicBool::new(false),
            progress: Mutex::new(Progress {
                phase: RepairSearchPhase::Queued,
                groups: Vec::new(),
                strategy: None,
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
    fn plan(
        &self,
        project: &Project,
        board: &librepcb_core::project::board::Board,
    ) -> Result<Vec<Trial>, RoutingError> {
        if !(1..=8).contains(&self.groups.len())
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
        if ![self.rip_up_region.min, self.rip_up_region.max]
            .into_iter()
            .all(|p| {
                target_bounds.contains(p, Length::ZERO) && neighbor_bounds.contains(p, Length::ZERO)
            })
        {
            return Err(Error::InvalidArgument(
                "rip_up_region must fit both routing regions.".into(),
            )
            .into());
        }
        if let Some(launch) = &self.target_launch {
            let (component, pad) = launch.from;
            let view = board
                .device(component)
                .ok_or_else(|| Error::not_found("Device", component))?
                .pad(&pad, project.library(), project.circuit())
                .map_err(Error::from)?
                .ok_or_else(|| Error::not_found("Pad", pad))?;
            let lead_radius = Length::new(launch.width.to_nm() / 2 + launch.width.to_nm() % 2);
            if view.net() != Some(self.target)
                || !board.copper_layers().contains(&launch.layer)
                || !view.is_on_layer(launch.layer)
                || !target_bounds.contains(view.position(), lead_radius)
                || !target_bounds.contains(launch.position, lead_radius)
            {
                return Err(Error::InvalidArgument(
                    "The launch pad and lead must be on the target net and inside region.".into(),
                )
                .into());
            }
        }
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
        Ok(trials)
    }
}

impl RepairLaunch {
    fn stage(
        &self,
        editor: &mut ProjectEditor,
        board: BoardId,
        net: NetSignalId,
    ) -> Result<(), RoutingError> {
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
        plan.run(editor)?;
        Ok(())
    }
}

impl RepairSearchJob {
    fn stage_launch(
        &self,
        sandbox: &mut ProjectEditor,
        board: BoardId,
        target: NetSignalId,
    ) -> Result<(), RoutingError> {
        if let Some(launch) = &self.request.target_launch {
            launch.stage(sandbox, board, target)?;
        }
        Ok(())
    }

    fn run_trial(
        &self,
        sandbox: &mut ProjectEditor,
        trial: &Trial,
        report: &mut RepairTrial,
    ) -> Result<Option<RepairCandidateDetails>, RoutingError> {
        let board = self.state.board;
        let target = self.state.target;
        let cancelled = || self.state.stop().is_some();
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
                    request.run_with_staging(sandbox, board, None, &[], cancelled, |sandbox| {
                        self.stage_launch(sandbox, board, target)
                    })?;
                (Some(neighbor), None, route, routing)
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
                    |sandbox| self.stage_launch(sandbox, board, target),
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
        Ok(Some(RepairCandidateDetails {
            target,
            groups: trial.groups.clone(),
            strategy: report.strategy,
            neighbor,
            joint,
            neighbor_region: (report.strategy == RepairStrategy::Joint).then_some(neighbor_bounds),
            route,
            routing,
            trace_requirements: Vec::new(),
            target_launch: self.request.target_launch.clone(),
            neighbor_branches: trial.branches.clone(),
        }))
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
        let strategies = [RepairStrategy::Sequential, RepairStrategy::Joint];
        let strategies = &strategies[..1 + usize::from(self.request.joint_route.is_some())];
        let trials = self
            .trials
            .iter()
            .flat_map(|trial| strategies.iter().map(move |&strategy| (trial, strategy)));
        for (i, (trial, strategy)) in trials.enumerate() {
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
            let mut repair = match routed {
                Ok(Some(details)) => details,
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
            let after = capture_routing_state_with_cancel(&mut source.sandbox, board, &|| {
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
            if after.checks.complete {
                report.trace_requirements = self
                    .request
                    .trace_requirements
                    .iter()
                    .zip(&baseline_paths)
                    .map(|(&requirement, before)| {
                        Ok(RepairTraceCheck {
                            requirement,
                            before: before.clone(),
                            after: requirement.measure(source.sandbox.project(), board)?,
                        })
                    })
                    .collect::<crate::Result<Vec<_>>>()?;
            }
            repair
                .trace_requirements
                .clone_from(&report.trace_requirements);
            let details = RoutingCandidateDetails::Repair {
                repair: Box::new(repair),
            };
            let validation = details.validate(&before, &after);
            if validation.acceptable {
                let (vias, trace_length_mm) = copper_cost(source.sandbox.project(), board)?;
                let candidate = source.freeze(board, details, before.clone(), after)?;
                let choice = RepairChoice {
                    candidate: candidate.info().candidate,
                    groups: trial.groups.clone(),
                    strategy,
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
            self.state.progress.lock().trials.push(report);
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
