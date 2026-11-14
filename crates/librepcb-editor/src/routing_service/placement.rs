//! Compare authorized local device poses on independent native snapshots.

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::time::{Duration, Instant};

use librepcb_core::project::board::PlaneFragmentStatus;
use librepcb_core::project::board::drc::DrcMessageKind;
use librepcb_core::project::{BoardId, ComponentInstanceId, PlaneId, Project};
use librepcb_core::types::{Angle, Point, Uuid};
use librepcb_core::utils::transform::Transform;
use serde::Serialize;

use super::comparison::TracePreferenceResult;
use super::copper::{PlanCounts, finish_copper};
use super::{CopperPlan, PlannedPlacement, RoutingCandidate, RoutingError, RoutingService};
use super::{RoutingCandidateDetails, RoutingSource, SourceContext, TracePreference};
use crate::commands::{Autoroute, NetRef, UnroutedAirWire, resolve_board};
use crate::routing_validation::{
    RoutingCheckSummary, RoutingValidation, capture_routing_state_with_cancel,
};
use crate::{Error, ProjectEditor, Result};

mod clearance;
mod poses;
use clearance::{PlacementClearanceConflict, clearance_errors};
mod worker;
pub use poses::PlacementSites;
use poses::{Pose, poses, rank_generated};
pub(super) use worker::ActivePlacementSearch;
pub use worker::PlacementSearchStatus;
use worker::{Phase, StopReason};

const MAX_COMPARE_POSES: usize = 16;

#[derive(Debug)]
pub struct PlacementRequest {
    pub targets: Vec<PlacementTarget>,
    pub copper: CopperPlan,
    pub route: Option<Autoroute>,
    pub relative_vias: Vec<PlacementVia>,
    pub traces: Vec<PlacementTrace>,
    pub max_candidates: usize,
    pub max_time: Duration,
    pub avoid_blockers: bool,
}

#[derive(Debug)]
pub struct PlacementTarget {
    pub component: ComponentInstanceId,
    pub sites: PlacementSites,
    pub rotations: Vec<Angle>,
}

/// The via's plan position is a footprint-local offset when attached to a
/// moving device. A plane requirement is also available for fixed vias.
#[derive(Debug)]
pub struct PlacementVia {
    pub via: usize,
    pub component: Option<ComponentInstanceId>,
    pub plane: Option<PlaneId>,
}

#[derive(Debug, Serialize)]
pub struct PlacementTrace {
    pub path: TracePreference,
    pub minimize: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct PlacementPose {
    pub component: ComponentInstanceId,
    pub position_mm: (f64, f64),
    pub rotation_deg: f64,
}

impl From<Pose> for PlacementPose {
    fn from(pose: Pose) -> Self {
        Self {
            component: pose.component,
            position_mm: pose.position.to_mm(),
            rotation_deg: pose.rotation.to_deg(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PlacementViaContact {
    pub via: usize,
    pub plane: PlaneId,
    pub position_mm: (f64, f64),
    pub connected: bool,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacementTrialStatus {
    #[default]
    Rejected,
    ValidUnretained,
    Retained,
}

#[derive(Debug, Default, Serialize)]
pub struct PlacementTrial {
    pub placements: Vec<PlacementPose>,
    pub candidate: Option<Uuid>,
    pub status: PlacementTrialStatus,
    /// Zero-based index of the rejected grid trial and conflict that suggested this pose.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_by: Option<[usize; 2]>,
    pub rejection: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub clearance_errors: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub clearance_conflicts: Vec<PlacementClearanceConflict>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub route_failures: Vec<UnroutedAirWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation: Option<RoutingValidation>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub traces: Vec<TracePreferenceResult>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub plane_contacts: Vec<PlacementViaContact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objective_length_mm: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct PlacementComparison {
    pub project: Uuid,
    pub board: BoardId,
    pub base_revision: u64,
    pub before: RoutingCheckSummary,
    pub trace_requirements: Vec<PlacementTrace>,
    pub before_traces: Vec<TracePreferenceResult>,
    pub before_objective_length_mm: Option<f64>,
    pub recommended: Option<Uuid>,
    pub total_poses: usize,
    pub timed_out: bool,
    pub clearance_rejections: usize,
    pub full_checks: usize,
    pub trials: Vec<PlacementTrial>,
}

#[derive(Debug)]
pub struct PlacementJob {
    source: RoutingSource,
    request: PlacementRequest,
    mirrored: BTreeMap<ComponentInstanceId, bool>,
    poses: Vec<Vec<Pose>>,
    worker: Option<std::sync::Arc<ActivePlacementSearch>>,
}

#[derive(Debug)]
pub struct PlacementResults {
    owner: Uuid,
    project: Uuid,
    context: SourceContext,
    report: Option<PlacementComparison>,
    stop: StopReason,
    candidates: Vec<RoutingCandidate>,
}

fn invalid(message: &str) -> Error {
    Error::InvalidArgument(message.into())
}

impl RoutingService {
    pub fn prepare_placement_comparison(
        &self,
        editor: &ProjectEditor,
        request: PlacementRequest,
    ) -> std::result::Result<PlacementJob, RoutingError> {
        let job = self.prepare_placement(editor, request)?;
        if job.request.avoid_blockers {
            return Err(
                invalid("Obstacle-guided poses require a background placement search.").into(),
            );
        }
        if job.poses.len() > MAX_COMPARE_POSES
            || job
                .request
                .targets
                .iter()
                .any(|target| matches!(target.sites, PlacementSites::Grid { .. }))
        {
            return Err(invalid("Compare up to 16 explicit poses here. Use placement search for a region or larger set.").into());
        }
        Ok(job)
    }

    fn prepare_placement(
        &self,
        editor: &ProjectEditor,
        request: PlacementRequest,
    ) -> std::result::Result<PlacementJob, RoutingError> {
        if !(1..=2).contains(&request.targets.len())
            || !request.copper.placements.is_empty()
            || !(1..=3).contains(&request.max_candidates)
            || request.traces.len() > 16
            || request.relative_vias.len() > 16
            || request.max_time.is_zero()
            || request.max_time > Duration::from_secs(120)
            || request
                .route
                .as_ref()
                .is_some_and(|route| route.nets.as_ref().is_none_or(Vec::is_empty))
        {
            return Err(invalid("Choose 1–2 devices, up to 16 trace and via rules each, 1–3 retained candidates, up to 120 seconds, and explicit nets for continuation.").into());
        }
        let board = resolve_board(editor.project(), Some(request.copper.board))?;
        let mut mirrored = BTreeMap::new();
        for target in &request.targets {
            let device = board
                .device(target.component)
                .ok_or_else(|| Error::not_found("Device", target.component))?;
            if device.locked()
                || mirrored
                    .insert(target.component, device.mirrored())
                    .is_some()
            {
                return Err(invalid("Select distinct unlocked devices.").into());
            }
        }

        let mut seen = BTreeSet::new();
        for via in &request.relative_vias {
            let spec = request
                .copper
                .vias
                .get(via.via)
                .ok_or_else(|| invalid("Via rule refers to a missing via."))?;
            if !seen.insert(via.via)
                || via.component.is_some_and(|id| !mirrored.contains_key(&id))
                || (via.component.is_none() && via.plane.is_none())
            {
                return Err(invalid(
                    "Each via rule must uniquely name a via and select a moving device or a plane.",
                )
                .into());
            }
            if let Some(plane) = via.plane {
                let plane = board
                    .plane(plane)
                    .ok_or_else(|| Error::not_found("Plane", plane))?;
                let net = plane.net().map(NetRef::Id);
                if net.is_none()
                    || net != spec.net
                    || spec.start_layer.is_some()
                    || spec.end_layer.is_some()
                {
                    return Err(invalid(
                        "A required plane must be on the planned through via's net.",
                    )
                    .into());
                }
            }
        }

        let mut poses = poses(&request.targets)?;
        rank_generated(
            &mut poses,
            &request.targets,
            &request.traces,
            board,
            editor.project(),
        )?;
        Ok(PlacementJob {
            poses,
            worker: None,
            source: self.capture_source(editor, request.max_candidates)?,
            request,
            mirrored,
        })
    }

    pub fn retain_placement_comparison(
        &mut self,
        project: &Project,
        results: PlacementResults,
    ) -> std::result::Result<PlacementComparison, RoutingError> {
        let PlacementResults {
            owner,
            context,
            report,
            candidates,
            ..
        } = results;
        let report = report.ok_or_else(|| {
            RoutingError::Refused("Placement search stopped during baseline checks.".into())
        })?;
        self.check_source_identity(
            project,
            owner,
            report.project,
            report.base_revision,
            &context,
        )?;
        self.check_capacity(candidates.len())?;
        if let Some(candidate) = candidates.first() {
            self.remember_baseline(candidate);
        }
        for candidate in candidates {
            self.candidates.insert(candidate.info.candidate, candidate);
        }
        Ok(report)
    }
}

fn placement_route_failure(failure: &UnroutedAirWire) -> UnroutedAirWire {
    let evidence = &failure.search_evidence;
    let via_reserve = evidence.iter().filter(|sample| sample.via).take(4).count();
    let mut samples = evidence
        .iter()
        .filter(|sample| !sample.via)
        .take(8 - via_reserve)
        .cloned()
        .collect::<Vec<_>>();
    samples.extend(
        evidence
            .iter()
            .filter(|sample| sample.via)
            .take(8 - samples.len())
            .cloned(),
    );
    UnroutedAirWire {
        search_evidence: samples,
        ..failure.clone()
    }
}

fn measure_traces(
    project: &Project,
    board: BoardId,
    requirements: &[PlacementTrace],
) -> Result<(Vec<TracePreferenceResult>, Option<f64>)> {
    let traces = requirements
        .iter()
        .map(|trace| trace.path.measure(project, board))
        .collect::<Result<Vec<_>>>()?;
    let mut objective = requirements
        .iter()
        .zip(&traces)
        .filter(|(request, _)| request.minimize)
        .map(|(_, result)| result.length_mm)
        .peekable();
    let length = if objective.peek().is_some() {
        objective.sum()
    } else {
        None
    };
    Ok((traces, length))
}

fn measure_contacts(
    project: &Project,
    board: BoardId,
    required: &[(usize, PlaneId, Point)],
) -> Result<Vec<PlacementViaContact>> {
    let mut fragments = BTreeMap::new();
    let mut contacts = Vec::with_capacity(required.len());
    for &(via, plane, position) in required {
        let fragments = match fragments.entry(plane) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                let job = project
                    .plane_job(board, None)
                    .map_err(Error::from_export)?
                    .ok_or_else(|| invalid("The candidate board has no planes to fill."))?;
                entry.insert(job.diagnose(plane).map_err(Error::from_export)?)
            }
        };
        contacts.push(PlacementViaContact {
            via,
            plane,
            position_mm: position.to_mm(),
            connected: fragments.iter().any(|fragment| {
                fragment.status == PlaneFragmentStatus::Connected && fragment.contains(position)
            }),
        });
    }
    Ok(contacts)
}

impl PlacementJob {
    pub fn run(mut self) -> std::result::Result<PlacementResults, RoutingError> {
        let started = Instant::now();
        let board = self.request.copper.board;
        let worker = self.worker.take();
        let stop = || match &worker {
            Some(state) => state.stop_requested(),
            None => (started.elapsed() >= self.request.max_time).then_some(StopReason::TimeLimit),
        };
        // Keep the synchronous comparison's between-trial budget. Only the
        // background worker can interrupt a trial that is already in flight.
        let interrupted = || worker.is_some() && stop().is_some();
        let phase = |phase| {
            if let Some(state) = &worker {
                state.phase(phase);
            }
        };
        let stopped_before_trials = |source: RoutingSource, reason| PlacementResults {
            owner: source.owner,
            project: source.project,
            context: source.source_context,
            report: None,
            stop: reason,
            candidates: Vec::new(),
        };
        phase(Phase::Baseline);
        if let Some(reason) = stop().filter(|_| worker.is_some()) {
            return Ok(stopped_before_trials(self.source, reason));
        }
        let before = self
            .source
            .capture_before_with_cancel(board, &interrupted)?;
        if let Some(reason) = stop().filter(|_| worker.is_some()) {
            return Ok(stopped_before_trials(self.source, reason));
        }
        let (before_traces, before_objective_length_mm) =
            measure_traces(self.source.sandbox.project(), board, &self.request.traces)?;
        let clearance_baseline = clearance_errors(&mut self.source.sandbox, board, &interrupted)
            .ok()
            .flatten();
        let total_poses = self.poses.len();
        let max_trials = worker
            .as_ref()
            .map_or(total_poses, |state| state.planned_trials());
        let mut winners = Vec::new();
        let mut trials = Vec::new();
        let record = |trials: &mut Vec<PlacementTrial>, trial| {
            trials.push(trial);
            if let Some(state) = &worker {
                state.complete_trial();
            }
        };
        let mut clearance_rejections = 0;
        let mut full_checks = 0;

        let mut grid = self.poses.into_iter();
        let mut next = VecDeque::new();
        let mut seen = HashSet::new();
        let mut try_suggestion = false;
        while trials.len() < max_trials && stop().is_none() {
            let from_grid = |poses| (poses, None);
            let (placements, suggested_by) = match if try_suggestion {
                next.pop_front().or_else(|| grid.next().map(from_grid))
            } else {
                grid.next().map(from_grid).or_else(|| next.pop_front())
            } {
                Some(trial) => trial,
                None => break,
            };
            try_suggestion = suggested_by.is_none();
            if !seen.insert(placements.clone()) {
                continue;
            }
            if let Some(state) = &worker {
                state.begin_trial(&placements);
            }
            let mut source = self.source.fork()?;
            let mut trial = PlacementTrial {
                suggested_by,
                ..PlacementTrial::default()
            };
            let mut plan = self.request.copper.clone();
            let mut transforms = BTreeMap::new();
            for &pose in &placements {
                trial.placements.push(pose.into());
                transforms.insert(
                    pose.component,
                    Transform::new(pose.position, pose.rotation, self.mirrored[&pose.component]),
                );
                plan.placements.push(PlannedPlacement {
                    component: pose.component,
                    position: Some(pose.position),
                    rotation: Some(pose.rotation),
                });
            }
            let mut required = Vec::new();
            for rule in &self.request.relative_vias {
                let via = &mut plan.vias[rule.via];
                if let Some(component) = rule.component {
                    via.position = transforms[&component].map(&via.position);
                }
                if let Some(plane) = rule.plane {
                    required.push((rule.via, plane, via.position));
                }
            }
            let counts = PlanCounts::from(&plan);
            let (vias, trace_segments) = match plan.run(&mut source.sandbox) {
                Ok(added) => added,
                Err(error) => {
                    trial.rejection = Some(error.to_string());
                    record(&mut trials, trial);
                    continue;
                }
            };

            if interrupted() {
                break;
            }
            phase(Phase::Clearance);
            if let Some(baseline) = &clearance_baseline
                && let Ok(Some(errors)) = clearance_errors(&mut source.sandbox, board, &interrupted)
            {
                if interrupted() {
                    break;
                }
                trial.clearance_conflicts =
                    PlacementClearanceConflict::introduced(baseline, errors, &vias);
                trial.clearance_errors = trial
                    .clearance_conflicts
                    .iter()
                    .map(|conflict| conflict.message().to_owned())
                    .collect();
                if !trial.clearance_conflicts.is_empty() {
                    trial.rejection = Some("staged_copper_clearance".into());
                    clearance_rejections += 1;
                    if self.request.avoid_blockers && suggested_by.is_none() {
                        let project = source.sandbox.project();
                        for (conflict_index, conflict) in
                            trial.clearance_conflicts.iter().enumerate()
                        {
                            let neighbors = conflict.neighbors(
                                project,
                                self.source.sandbox.project(),
                                board,
                                &placements,
                                &self.request.targets,
                                &self.request.relative_vias,
                            );
                            next.extend(
                                neighbors
                                    .into_iter()
                                    .filter(|poses| !seen.contains(poses))
                                    .take(2)
                                    .map(|poses| (poses, Some([trials.len(), conflict_index]))),
                            );
                            if !next.is_empty() {
                                break;
                            }
                        }
                    }
                    record(&mut trials, trial);
                    continue;
                }
            }

            if interrupted() {
                break;
            }
            phase(Phase::Routing);
            let details = match finish_copper(
                &mut source.sandbox,
                board,
                None,
                counts,
                (vias.len(), trace_segments),
                self.request.route.clone(),
                interrupted,
            ) {
                Ok(details) => details,
                Err(error) => {
                    trial.rejection = Some(error.to_string());
                    record(&mut trials, trial);
                    continue;
                }
            };
            if interrupted() {
                break;
            }
            phase(Phase::Validation);
            full_checks += 1;
            let after =
                capture_routing_state_with_cancel(&mut source.sandbox, board, &interrupted)?;
            if interrupted() {
                break;
            }
            if let RoutingCandidateDetails::Copper {
                route: Some(route), ..
            } = &details
                && route.request.search_evidence
            {
                trial.route_failures = route
                    .routing
                    .failed_requests
                    .iter()
                    .chain(&route.routing.skipped_outside_region)
                    .take(8)
                    .map(placement_route_failure)
                    .collect();
            }
            let validation = details.validate(&before, &after);
            let (traces, length) =
                measure_traces(source.sandbox.project(), board, &self.request.traces)?;
            trial.traces = traces;
            trial.objective_length_mm = length;
            if after.checks.complete {
                trial.plane_contacts =
                    measure_contacts(source.sandbox.project(), board, &required)?;
            }
            if interrupted() {
                break;
            }
            let rejection = if !validation.acceptable {
                Some("validation")
            } else if !validation.unrouted_regressions.is_empty() {
                Some("connectivity_regressed")
            } else if after.has_new_warning(&before, DrcMessageKind::DeviceInCourtyard) {
                Some("placement_warnings")
            } else if trial.traces.iter().any(|trace| !trace.met) {
                Some("trace_requirement")
            } else if trial
                .plane_contacts
                .iter()
                .any(|contact| !contact.connected)
            {
                Some("plane_contact")
            } else {
                None
            };
            if let Some(rejection) = rejection {
                trial.rejection = Some(rejection.into());
            } else {
                let rank = (
                    after.checks.unrouted,
                    length.unwrap_or_default(),
                    validation.new_warning_count,
                );
                let candidate = source.freeze(board, details, before.clone(), after)?;
                if candidate.mutations().is_empty() {
                    trial.rejection = Some("no_changes".into());
                } else {
                    trial.status = PlacementTrialStatus::ValidUnretained;
                    winners.push((rank, trials.len(), candidate));
                    winners.sort_by(|(a, _, _), (b, _, _)| {
                        a.0.cmp(&b.0)
                            .then_with(|| a.1.total_cmp(&b.1))
                            .then_with(|| a.2.cmp(&b.2))
                    });
                    winners.truncate(self.request.max_candidates);
                }
            }
            trial.validation = Some(validation);
            record(&mut trials, trial);
        }

        let candidates: Vec<_> = winners
            .into_iter()
            .map(|(_, index, candidate)| {
                trials[index].candidate = Some(candidate.info.candidate);
                trials[index].status = PlacementTrialStatus::Retained;
                candidate
            })
            .collect();
        let recommended = candidates.first().map(|candidate| candidate.info.candidate);
        let stop = if grid.len() == 0
            && next.is_empty()
            && (stop().is_none() || trials.len() == max_trials)
        {
            StopReason::Completed
        } else if trials.len() == max_trials {
            StopReason::TrialLimit
        } else {
            stop().unwrap_or(StopReason::TrialLimit)
        };
        Ok(PlacementResults {
            owner: self.source.owner,
            project: self.source.project,
            context: self.source.source_context,
            candidates,
            stop,
            report: Some(PlacementComparison {
                project: self.source.project,
                board,
                base_revision: self.source.base_revision,
                before: before.checks,
                trace_requirements: self.request.traces,
                before_traces,
                before_objective_length_mm,
                recommended,
                total_poses,
                timed_out: stop == StopReason::TimeLimit,
                clearance_rejections,
                full_checks,
                trials,
            }),
        })
    }
}
