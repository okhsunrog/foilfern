//! Checked staged builtin routing on a detached editor. No upstream counterpart.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

use librepcb_core::project::{BoardId, NetSignalId};
use librepcb_core::types::PositiveLength;
use serde::Serialize;

use super::{RoutingError, RoutingJob, RoutingService, RoutingWork, rebuild_connectivity, region};
use crate::commands::{Autoroute, NetRef, UnroutedAirWire, net_name, resolve_board, resolve_net};
use crate::routing_validation::{RoutingSnapshot, RoutingValidation, capture_routing_state};
use crate::{Error, ProjectEditor};

const MAX_FAILED_REQUEST_SAMPLES: usize = 8;

#[derive(Debug)]
pub struct StagedRoutingRequest {
    /// Includes an explicit allowlist of target nets.
    pub options: Autoroute,
    pub batch_size: usize,
    pub max_trials: usize,
    pub max_time: Duration,
    /// Defer nets whose longest initial air wire exceeds this length.
    pub max_gap: Option<PositiveLength>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StagedRoutingResult {
    pub request: Autoroute,
    pub batch_size: usize,
    pub max_trials: usize,
    pub max_time_ms: u128,
    pub max_gap_mm: Option<f64>,
    pub stop_reason: StopReason,
    pub deferred_nets: Vec<RemainingNet>,
    pub pending_nets: Vec<RemainingNet>,
    pub remaining_nets: Vec<RemainingNet>,
    pub trials: Vec<BatchTrial>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Completed,
    TrialLimit,
    TimeLimit,
    ChecksIncomplete,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemainingNet {
    pub net: NetSignalId,
    pub name: String,
    pub initial_max_gap_mm: f64,
    pub unrouted: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchTrial {
    pub nets: Vec<NetSignalId>,
    pub outcome: BatchOutcome,
    pub validation: RoutingValidation,
    pub failed_request_count: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failed_requests: Vec<UnroutedAirWire>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchOutcome {
    Retained,
    Rejected,
    NoProgress,
}

impl RoutingService {
    /// Each retained batch becomes fixed copper for subsequent trials.
    pub fn prepare_staged(
        &self,
        editor: &ProjectEditor,
        request: StagedRoutingRequest,
    ) -> Result<RoutingJob, RoutingError> {
        if request.options.nets.as_ref().is_none_or(Vec::is_empty) {
            return Err(Error::InvalidArgument("Select at least one target net.".into()).into());
        }
        if !(1..=32).contains(&request.batch_size)
            || !(1..=64).contains(&request.max_trials)
            || request.max_time.is_zero()
            || request.max_time > Duration::from_secs(300)
        {
            return Err(Error::InvalidArgument(
                "Expected batch_size 1–32, max_trials 1–64 and a time budget up to 300 s.".into(),
            )
            .into());
        }
        self.prepare_work(editor, RoutingWork::Staged(request))
    }
}

impl StagedRoutingRequest {
    pub(super) fn run(
        mut self,
        editor: &mut ProjectEditor,
        board: BoardId,
        before: &RoutingSnapshot,
    ) -> Result<StagedRoutingResult, RoutingError> {
        let started = Instant::now();
        self.options.board = Some(board);
        let project = editor.project();
        let allowed = self
            .options
            .nets
            .iter()
            .flatten()
            .map(|net| resolve_net(project, net))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let critical = self.options.critical_net_ids(project)?;
        let model = resolve_board(project, Some(board))?;
        let original_copper = region::copper(project, model)?;
        let mut initial_gaps = BTreeMap::new();
        let mut deferred = Vec::new();
        let mut ordered = Vec::new();
        for net in &allowed {
            let longest = model
                .derived()
                .air_wires()
                .get(&Some(*net))
                .into_iter()
                .flatten()
                .map(|wire| (wire.p2_position() - wire.p1_position()).length())
                .max();
            if let Some(longest) = longest {
                initial_gaps.insert(*net, longest);
                if !critical.contains(net) && self.max_gap.is_some_and(|limit| longest > *limit) {
                    deferred.push(*net);
                } else {
                    ordered.push((!critical.contains(net), longest, *net));
                }
            }
        }
        ordered.sort_unstable();
        let mut batches: VecDeque<Vec<_>> = ordered
            .chunks(self.batch_size)
            .map(|batch| batch.iter().map(|(_, _, net)| *net).collect())
            .collect();
        let mut current = before.clone();
        let mut trials = Vec::new();
        let mut checks_complete = current.checks.complete;
        let stop_reason = loop {
            if !checks_complete {
                break StopReason::ChecksIncomplete;
            }
            let Some(batch) = batches.front_mut() else {
                break StopReason::Completed;
            };
            batch.retain(|net| current.unrouted_for(*net) != 0);
            if batch.is_empty() {
                batches.pop_front();
                continue;
            }
            if trials.len() >= self.max_trials {
                break StopReason::TrialLimit;
            }
            if started.elapsed() >= self.max_time {
                break StopReason::TimeLimit;
            }
            let Some(batch) = batches.pop_front() else {
                break StopReason::Completed;
            };
            let targets: BTreeSet<_> = batch.iter().copied().collect();
            let prior_index = editor.undo_stack().index();
            let mut routing = editor.execute(Autoroute {
                nets: Some(batch.iter().copied().map(NetRef::Id).collect()),
                ..self.options.clone()
            })?;
            let after = capture_routing_state(editor, board)?;
            checks_complete = after.checks.complete;
            let validation = RoutingValidation::compare_candidate(&current, &after);
            let outcome = if !validation.acceptable {
                BatchOutcome::Rejected
            } else if after.unrouted_on(&targets) < current.unrouted_on(&targets) {
                BatchOutcome::Retained
            } else {
                BatchOutcome::NoProgress
            };
            let failed_request_count = routing.failed_requests.len();
            routing.failed_requests.truncate(MAX_FAILED_REQUEST_SAMPLES);
            trials.push(BatchTrial {
                nets: batch.clone(),
                outcome,
                validation,
                failed_request_count,
                failed_requests: routing.failed_requests,
            });
            if outcome == BatchOutcome::Retained {
                current = after;
                continue;
            }
            if editor.undo_stack().index() != prior_index {
                editor.undo()?;
                rebuild_connectivity(editor, board)?;
            }
            if batch.len() > 1 {
                let (first, second) = batch.split_at(batch.len() / 2);
                batches.push_front(second.to_vec());
                batches.push_front(first.to_vec());
            }
        };
        let project = editor.project();
        let model = resolve_board(project, Some(board))?;
        region::ensure_preserved_copper(
            &original_copper,
            &region::copper(project, model)?,
            &BTreeSet::new(),
        )?;
        let remaining_net = |net| {
            let unrouted = current.unrouted_for(net);
            let initial_max_gap_mm = initial_gaps.get(&net)?.to_mm();
            (unrouted > 0).then(|| RemainingNet {
                net,
                name: net_name(project, Some(net)),
                initial_max_gap_mm,
                unrouted,
            })
        };
        Ok(StagedRoutingResult {
            request: self.options,
            batch_size: self.batch_size,
            max_trials: self.max_trials,
            max_time_ms: self.max_time.as_millis(),
            max_gap_mm: self.max_gap.map(|limit| limit.to_mm()),
            stop_reason,
            deferred_nets: deferred.into_iter().filter_map(remaining_net).collect(),
            pending_nets: batches
                .into_iter()
                .flatten()
                .filter_map(remaining_net)
                .collect(),
            remaining_nets: allowed.into_iter().filter_map(remaining_net).collect(),
            trials,
        })
    }
}
