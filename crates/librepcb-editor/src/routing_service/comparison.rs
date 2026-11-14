//! Compare builtin search settings against one immutable board baseline.

use std::collections::BTreeSet;

use librepcb_core::project::board::TracePath;
use librepcb_core::project::{BoardId, ComponentInstanceId, NetSignalId, Project};
use librepcb_core::types::{Length, Uuid};
use serde::Serialize;

use super::{
    RoutingCandidate, RoutingCandidateDetails, RoutingError, RoutingService, RoutingSource,
};
use crate::commands::{Autoroute, RoutingOrder, resolve_board};
use crate::routing_validation::{RoutingCheckSummary, capture_routing_state};
use crate::{Error, ProjectEditor};

#[derive(Debug)]
pub struct RoutingComparisonJob {
    source: RoutingSource,
    request: Autoroute,
    trials: Vec<RoutingTrial>,
    trace_preferences: Vec<TracePreference>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RoutingTrial {
    pub order: RoutingOrder,
    pub via_cost: Option<Length>,
}

/// A soft target on the shortest anchored trace path between two pads.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct TracePreference {
    pub net: NetSignalId,
    pub from: (ComponentInstanceId, Uuid),
    pub to: (ComponentInstanceId, Uuid),
    #[serde(rename = "max_length_mm", serialize_with = "serialize_length_mm")]
    pub max_length: Option<Length>,
    pub max_layer_changes: Option<usize>,
}

fn serialize_length_mm<S: serde::Serializer>(
    length: &Option<Length>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    length.map(Length::to_mm).serialize(serializer)
}

#[derive(Debug, Clone, Serialize)]
pub struct TracePreferenceResult {
    pub length_mm: Option<f64>,
    pub layer_changes: Option<usize>,
    pub met: bool,
}

impl TracePreference {
    pub(super) fn measure(
        self,
        project: &Project,
        board: BoardId,
    ) -> Result<TracePreferenceResult, Error> {
        let path = project
            .trace_path(board, self.net, self.from, self.to)
            .map_err(Error::from_export)?;
        let met = path.as_ref().is_some_and(|path| self.met_by(path));
        Ok(TracePreferenceResult {
            length_mm: path.as_ref().map(|path| path.length_mm),
            layer_changes: path.map(|path| path.layer_changes),
            met,
        })
    }

    fn met_by(self, path: &TracePath) -> bool {
        self.max_length
            .is_none_or(|limit| path.length_mm <= limit.to_mm())
            && self
                .max_layer_changes
                .is_none_or(|limit| path.layer_changes <= limit)
    }
}

/// Detached results awaiting publication together to the original session.
#[derive(Debug)]
pub struct RoutingTrials {
    before: RoutingCheckSummary,
    trace_preferences: Vec<TracePreference>,
    candidates: Vec<(RoutingCandidate, ComparisonEntry)>,
}

#[derive(Debug, Serialize)]
pub struct RoutingComparison {
    pub project: Uuid,
    pub board: BoardId,
    pub base_revision: u64,
    pub baseline_reused: bool,
    pub before: RoutingCheckSummary,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub trace_preferences: Vec<TracePreference>,
    pub recommended: Option<Uuid>,
    pub candidates: Vec<ComparisonEntry>,
}

#[derive(Debug, Serialize)]
pub struct ComparisonEntry {
    pub candidate: Uuid,
    pub order: RoutingOrder,
    pub via_cost_mm: f64,
    pub acceptable: bool,
    pub new_error_count: usize,
    pub non_connectivity_errors: usize,
    pub critical_unrouted: usize,
    pub regressed_nets: usize,
    pub after: RoutingCheckSummary,
    pub vias: usize,
    /// Total length of actual trace centerlines, including existing traces.
    /// Missing if an anchor cannot be resolved.
    pub trace_length_mm: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub trace_preferences: Vec<TracePreferenceResult>,
}

impl ComparisonEntry {
    fn rank(&self) -> impl Ord + '_ {
        (
            !self.acceptable,
            self.new_error_count,
            self.non_connectivity_errors,
            self.regressed_nets,
            self.critical_unrouted,
            self.after.unrouted,
            self.trace_preferences
                .iter()
                .filter(|preference| !preference.met)
                .count(),
            self.vias,
        )
    }
}

impl RoutingService {
    pub fn prepare_comparison(
        &self,
        editor: &ProjectEditor,
        request: Autoroute,
        orders: Vec<RoutingOrder>,
    ) -> Result<RoutingComparisonJob, RoutingError> {
        self.prepare_comparison_trials(
            editor,
            request,
            orders
                .into_iter()
                .map(|order| RoutingTrial {
                    order,
                    via_cost: None,
                })
                .collect(),
        )
    }

    pub fn prepare_comparison_trials(
        &self,
        editor: &ProjectEditor,
        request: Autoroute,
        trials: Vec<RoutingTrial>,
    ) -> Result<RoutingComparisonJob, RoutingError> {
        let default = librepcb_autoroute::RoutingRules::default().via_cost;
        let settings = trials.iter().map(|trial| {
            (
                trial.order,
                trial.via_cost.or(request.via_cost).unwrap_or(default),
            )
        });
        if !(2..=3).contains(&trials.len())
            || settings.collect::<BTreeSet<_>>().len() != trials.len()
        {
            return Err(Error::InvalidArgument(
                "Choose two or three distinct combinations of routing order and via cost.".into(),
            )
            .into());
        }
        Ok(RoutingComparisonJob {
            source: self.capture_source(editor, trials.len())?,
            request,
            trials,
            trace_preferences: Vec::new(),
        })
    }

    /// Check all candidates before publishing any, including after races for
    /// capacity or edits while the worker was running.
    pub fn retain_comparison(
        &mut self,
        project: &Project,
        trials: RoutingTrials,
    ) -> Result<RoutingComparison, RoutingError> {
        for (candidate, _) in &trials.candidates {
            self.check_source(project, candidate)?;
        }
        self.check_capacity(trials.candidates.len())?;
        self.remember_baseline(&trials.candidates[0].0);
        let info = trials.candidates[0].0.info();
        let recommended = trials
            .candidates
            .iter()
            .map(|(_, entry)| entry)
            .find(|entry| entry.acceptable && entry.regressed_nets == 0)
            .map(|entry| entry.candidate);
        Ok(RoutingComparison {
            project: info.project,
            board: info.board,
            base_revision: info.base_revision,
            baseline_reused: info.baseline_reused,
            before: trials.before,
            trace_preferences: trials.trace_preferences,
            recommended,
            candidates: trials
                .candidates
                .into_iter()
                .map(|(candidate, entry)| {
                    self.candidates.insert(entry.candidate, candidate);
                    entry
                })
                .collect(),
        })
    }
}

impl RoutingComparisonJob {
    pub fn with_trace_preferences(mut self, preferences: Vec<TracePreference>) -> Self {
        self.trace_preferences = preferences;
        self
    }

    /// Fork the same detached source for each trial, including derived state.
    pub fn run(mut self) -> Result<RoutingTrials, RoutingError> {
        let source = &mut self.source;
        let board = resolve_board(source.sandbox.project(), self.request.board)?.id();
        let critical = self
            .request
            .critical_net_ids(source.sandbox.project(), board)?;
        let before = source.capture_before(board)?;
        let mut candidates = Vec::with_capacity(self.trials.len());
        for settings in self.trials {
            let mut trial = source.fork()?;
            trial
                .sandbox
                .update_derived_data(|project| project.rebuild_planes(board, None))?
                .map_err(Error::from_export)?;
            let request = Autoroute {
                order: Some(settings.order),
                via_cost: settings.via_cost.or(self.request.via_cost),
                ..self.request.clone()
            };
            let via_cost_mm = request
                .via_cost
                .unwrap_or_else(|| librepcb_autoroute::RoutingRules::default().via_cost)
                .to_mm();
            let details = RoutingCandidateDetails::run_builtin(&mut trial.sandbox, board, request)?;
            let after = capture_routing_state(&mut trial.sandbox, board)?;
            let non_connectivity_errors = after.non_connectivity_errors();
            let critical_unrouted = after.unrouted_on(&critical);
            let (vias, trace_length_mm) = copper_cost(trial.sandbox.project(), board)?;
            let trace_preferences = self
                .trace_preferences
                .iter()
                .map(|preference| preference.measure(trial.sandbox.project(), board))
                .collect::<Result<_, _>>()?;
            let candidate = trial.freeze(board, details, before.clone(), after)?;
            let info = candidate.info();
            let entry = ComparisonEntry {
                candidate: info.candidate,
                order: settings.order,
                via_cost_mm,
                acceptable: info.validation.acceptable,
                new_error_count: info.validation.new_error_count,
                non_connectivity_errors,
                critical_unrouted,
                regressed_nets: info.validation.unrouted_regressions.len(),
                after: info.validation.after.clone(),
                vias,
                trace_length_mm,
                trace_preferences,
            };
            candidates.push((candidate, entry));
        }
        // Stable sort: preserve caller order when measured scores tie.
        candidates.sort_by(|(_, a), (_, b)| {
            a.rank().cmp(&b.rank()).then_with(|| {
                a.trace_length_mm
                    .unwrap_or(f64::INFINITY)
                    .total_cmp(&b.trace_length_mm.unwrap_or(f64::INFINITY))
            })
        });
        Ok(RoutingTrials {
            before: before.checks,
            trace_preferences: self.trace_preferences,
            candidates,
        })
    }
}

pub(super) fn copper_cost(
    project: &Project,
    board: BoardId,
) -> Result<(usize, Option<f64>), Error> {
    let board = resolve_board(project, Some(board))?;
    let mut length_nm = Some(0_i128);
    let mut vias = 0;
    for segment in board.net_segments().values() {
        vias += segment.vias().len();
        for trace in segment.traces().values() {
            length_nm = length_nm.and_then(|total| {
                let position = |anchor| {
                    board.anchor_position(segment, anchor, project.library(), project.circuit())
                };
                let delta = position(trace.p1())? - position(trace.p2())?;
                Some(total + i128::from(delta.length().to_nm()))
            });
        }
    }
    Ok((vias, length_nm.map(|nm| nm as f64 / 1_000_000.0)))
}
