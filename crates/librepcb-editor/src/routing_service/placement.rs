//! Compare authorized local device poses on independent native snapshots.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::{Duration, Instant};

use librepcb_core::project::board::PlaneFragmentStatus;
use librepcb_core::project::board::drc::DrcMessageKind;
use librepcb_core::project::{BoardId, ComponentInstanceId, PlaneId, Project};
use librepcb_core::rule_check::Severity;
use librepcb_core::serialization::SExpression;
use librepcb_core::types::{Angle, Point, Uuid};
use librepcb_core::utils::transform::Transform;
use serde::Serialize;

use super::comparison::TracePreferenceResult;
use super::copper::{PlanCounts, finish_copper};
use super::{CopperPlan, PlannedPlacement, RoutingCandidate, RoutingError, RoutingService};
use super::{RoutingSource, SourceContext, TracePreference};
use crate::commands::{Autoroute, NetRef, resolve_board};
use crate::routing_validation::{RoutingCheckSummary, RoutingValidation, capture_routing_state};
use crate::{Error, ProjectEditor, Result};

const MAX_POSES: usize = 16;

#[derive(Debug)]
pub struct PlacementRequest {
    pub targets: Vec<PlacementTarget>,
    pub copper: CopperPlan,
    pub route: Option<Autoroute>,
    pub relative_vias: Vec<PlacementVia>,
    pub traces: Vec<PlacementTrace>,
    pub max_candidates: usize,
    pub max_time: Duration,
}

#[derive(Debug)]
pub struct PlacementTarget {
    pub component: ComponentInstanceId,
    pub positions: Vec<Point>,
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

#[derive(Debug, Serialize)]
pub struct PlacementViaContact {
    pub via: usize,
    pub plane: PlaneId,
    pub position_mm: (f64, f64),
    pub connected: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct PlacementTrial {
    pub placements: Vec<PlacementPose>,
    pub candidate: Option<Uuid>,
    pub rejection: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub clearance_errors: Vec<String>,
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
}

#[derive(Debug)]
pub struct PlacementResults {
    owner: Uuid,
    context: SourceContext,
    report: PlacementComparison,
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
        let mut count = 1usize;
        for target in &request.targets {
            let positions = target.positions.iter().copied().collect::<HashSet<_>>();
            let rotations = target
                .rotations
                .iter()
                .copied()
                .map(Angle::mapped_to_180deg)
                .collect::<HashSet<_>>();
            count = count
                .saturating_mul(target.positions.len())
                .saturating_mul(target.rotations.len());
            if count == 0
                || count > MAX_POSES
                || positions.len() != target.positions.len()
                || rotations.len() != target.rotations.len()
            {
                return Err(invalid(
                    "Supply distinct positions and rotations forming 1–16 joint poses.",
                )
                .into());
            }

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

        Ok(PlacementJob {
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
        } = results;
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

#[derive(Clone, Copy)]
struct Pose {
    component: ComponentInstanceId,
    position: Point,
    rotation: Angle,
}

fn poses(targets: &[PlacementTarget]) -> Vec<Vec<Pose>> {
    let mut joint = vec![Vec::new()];
    for target in targets {
        let mut expanded = Vec::new();
        for prefix in joint {
            for &position in &target.positions {
                for &rotation in &target.rotations {
                    let mut combined = prefix.clone();
                    combined.push(Pose {
                        component: target.component,
                        position,
                        rotation,
                    });
                    expanded.push(combined);
                }
            }
        }
        joint = expanded;
    }
    joint
}

/// Quick native DRC on fixed copper, before builtin can spend time searching
/// around an impossible placement. Only explicit copper clearances are used:
/// this check omits planes, and disconnected copper is normal during staging.
fn clearance_errors(
    editor: &mut ProjectEditor,
    board: BoardId,
) -> Result<Option<BTreeMap<SExpression, String>>> {
    let check = editor
        .update_derived_data(|p| p.run_drc(board, None, true, &|_| {}))?
        .map_err(|error| Error::InvalidArgument(error.to_string()))?;
    let approvals = resolve_board(editor.project(), Some(board))?.drc_approvals();
    Ok(check.errors.is_empty().then(|| {
        check
            .messages
            .iter()
            .filter(|message| {
                message.message().severity() == Severity::Error
                    && matches!(
                        message.kind(),
                        DrcMessageKind::CopperCopperClearanceViolation
                            | DrcMessageKind::CopperBoardClearanceViolation
                            | DrcMessageKind::CopperHoleClearanceViolation
                    )
                    && !approvals.contains(message.approval())
            })
            .map(|message| {
                (
                    message.approval().clone(),
                    message.message().message().to_owned(),
                )
            })
            .collect()
    }))
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
        let before = self.source.capture_before(board)?;
        let (before_traces, before_objective_length_mm) =
            measure_traces(self.source.sandbox.project(), board, &self.request.traces)?;
        let clearance_baseline = clearance_errors(&mut self.source.sandbox, board)
            .ok()
            .flatten();
        let poses = poses(&self.request.targets);
        let total_poses = poses.len();
        let mut winners = Vec::new();
        let mut trials = Vec::new();
        let mut clearance_rejections = 0;
        let mut full_checks = 0;

        for placements in poses {
            if started.elapsed() >= self.request.max_time {
                break;
            }
            let mut source = self.source.fork()?;
            let mut trial = PlacementTrial::default();
            let mut plan = self.request.copper.clone();
            let mut transforms = BTreeMap::new();
            for pose in placements {
                trial.placements.push(PlacementPose {
                    component: pose.component,
                    position_mm: pose.position.to_mm(),
                    rotation_deg: pose.rotation.to_deg(),
                });
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
            let added = match plan.run(&mut source.sandbox) {
                Ok(added) => added,
                Err(error) => {
                    trial.rejection = Some(error.to_string());
                    trials.push(trial);
                    continue;
                }
            };

            if let Some(baseline) = &clearance_baseline
                && let Ok(Some(errors)) = clearance_errors(&mut source.sandbox, board)
            {
                trial.clearance_errors = errors
                    .into_iter()
                    .filter_map(|(key, message)| (!baseline.contains_key(&key)).then_some(message))
                    .take(8)
                    .collect();
                if !trial.clearance_errors.is_empty() {
                    trial.rejection = Some("staged_copper_clearance".into());
                    clearance_rejections += 1;
                    trials.push(trial);
                    continue;
                }
            }

            let details = match finish_copper(
                &mut source.sandbox,
                board,
                None,
                counts,
                added,
                self.request.route.clone(),
            ) {
                Ok(details) => details,
                Err(error) => {
                    trial.rejection = Some(error.to_string());
                    trials.push(trial);
                    continue;
                }
            };
            full_checks += 1;
            let after = capture_routing_state(&mut source.sandbox, board)?;
            let validation = details.validate(&before, &after);
            let (traces, length) =
                measure_traces(source.sandbox.project(), board, &self.request.traces)?;
            trial.traces = traces;
            trial.objective_length_mm = length;
            if after.checks.complete {
                trial.plane_contacts =
                    measure_contacts(source.sandbox.project(), board, &required)?;
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
            trials.push(trial);
        }

        let candidates: Vec<_> = winners
            .into_iter()
            .map(|(_, index, candidate)| {
                trials[index].candidate = Some(candidate.info.candidate);
                candidate
            })
            .collect();
        let recommended = candidates.first().map(|candidate| candidate.info.candidate);
        Ok(PlacementResults {
            owner: self.source.owner,
            context: self.source.source_context,
            candidates,
            report: PlacementComparison {
                project: self.source.project,
                board,
                base_revision: self.source.base_revision,
                before: before.checks,
                trace_requirements: self.request.traces,
                before_traces,
                before_objective_length_mm,
                recommended,
                total_poses,
                timed_out: trials.len() < total_poses,
                clearance_rejections,
                full_checks,
                trials,
            },
        })
    }
}
