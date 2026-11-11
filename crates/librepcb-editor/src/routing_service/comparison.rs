//! Compare initial air-wire orders against one immutable builtin baseline.

use std::collections::BTreeSet;
use std::sync::Arc;

use librepcb_core::project::{BoardId, Project};
use librepcb_core::types::Uuid;
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
    orders: Vec<RoutingOrder>,
}

/// Detached results awaiting publication together to the original session.
#[derive(Debug)]
pub struct RoutingTrials {
    before: RoutingCheckSummary,
    candidates: Vec<(RoutingCandidate, ComparisonEntry)>,
}

#[derive(Debug, Serialize)]
pub struct RoutingComparison {
    pub project: Uuid,
    pub board: BoardId,
    pub base_revision: u64,
    pub baseline_reused: bool,
    pub before: RoutingCheckSummary,
    pub recommended: Option<Uuid>,
    pub candidates: Vec<ComparisonEntry>,
}

#[derive(Debug, Serialize)]
pub struct ComparisonEntry {
    pub candidate: Uuid,
    pub order: RoutingOrder,
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
        if !(2..=3).contains(&orders.len())
            || orders.iter().collect::<BTreeSet<_>>().len() != orders.len()
        {
            return Err(Error::InvalidArgument(
                "Choose two or three distinct routing orders.".into(),
            )
            .into());
        }
        Ok(RoutingComparisonJob {
            source: self.capture_source(editor, orders.len())?,
            request,
            orders,
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
    /// Fork the same detached source for each order. Only the private trial
    /// is changed; even derived state from a previous order cannot leak.
    pub fn run(mut self) -> Result<RoutingTrials, RoutingError> {
        let source = &mut self.source;
        let board = resolve_board(source.sandbox.project(), self.request.board)?.id();
        let critical = self.request.critical_net_ids(source.sandbox.project())?;
        let before = source.capture_before(board)?;
        let mut candidates = Vec::with_capacity(self.orders.len());
        for order in self.orders {
            let mut trial = RoutingSource {
                owner: source.owner,
                project: source.project,
                base_revision: source.base_revision,
                source_context: source.source_context.clone(),
                baseline: None,
                baseline_reused: source.baseline_reused,
                sandbox: ProjectEditor::with_source(
                    source.sandbox.project().snapshot().map_err(Error::from)?,
                    Arc::clone(source.sandbox.source()),
                ),
            };
            trial
                .sandbox
                .update_derived_data(|project| project.rebuild_planes(board, None))?
                .map_err(Error::from_export)?;
            let request = Autoroute {
                order: Some(order),
                ..self.request.clone()
            };
            let details = RoutingCandidateDetails::run_builtin(&mut trial.sandbox, board, request)?;
            let after = capture_routing_state(&mut trial.sandbox, board)?;
            let non_connectivity_errors = after.non_connectivity_errors();
            let critical_unrouted = after.unrouted_on(&critical);
            let (vias, trace_length_mm) = copper_cost(trial.sandbox.project(), board)?;
            let candidate = trial.freeze(board, details, before.clone(), after)?;
            let info = candidate.info();
            let entry = ComparisonEntry {
                candidate: info.candidate,
                order,
                acceptable: info.validation.acceptable,
                new_error_count: info.validation.new_error_count,
                non_connectivity_errors,
                critical_unrouted,
                regressed_nets: info.validation.unrouted_regressions.len(),
                after: info.validation.after.clone(),
                vias,
                trace_length_mm,
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
            candidates,
        })
    }
}

fn copper_cost(project: &Project, board: BoardId) -> Result<(usize, Option<f64>), Error> {
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
