//! Session-local placement search lifecycle. All board work runs detached.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use librepcb_core::project::{BoardId, Project};
use librepcb_core::types::Uuid;
use parking_lot::Mutex;
use serde::Serialize;

use super::{
    PlacementComparison, PlacementJob, PlacementPose, PlacementRequest, PlacementResults, Pose,
    RoutingError, RoutingService, invalid,
};
use crate::ProjectEditor;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Phase {
    Queued,
    Baseline,
    Staging,
    Clearance,
    Routing,
    Validation,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum StopReason {
    Completed,
    TrialLimit,
    TimeLimit,
    Cancelled,
    Failed,
}

#[derive(Debug)]
struct Progress {
    phase: Phase,
    current: Vec<PlacementPose>,
    completed: usize,
    elapsed: Option<Duration>,
    stop: Option<StopReason>,
    comparison: Option<Arc<PlacementComparison>>,
    failure: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PlacementSearchStatus {
    pub search: Uuid,
    board: BoardId,
    base_revision: u64,
    phase: Phase,
    elapsed_ms: u128,
    budget_ms: u128,
    total_poses: usize,
    planned_trials: usize,
    completed_trials: usize,
    current_placements: Vec<PlacementPose>,
    cancel_requested: bool,
    stop_reason: Option<StopReason>,
    // Serialize the retained report through a lightweight status handle.
    comparison: Option<Comparison>,
    failure: Option<String>,
}

#[derive(Debug)]
struct Comparison(Arc<PlacementComparison>);

impl Serialize for Comparison {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.as_ref().serialize(serializer)
    }
}

#[derive(Debug)]
pub(in crate::routing_service) struct ActivePlacementSearch {
    id: Uuid,
    board: BoardId,
    revision: u64,
    started: Instant,
    budget: Duration,
    total: usize,
    planned: usize,
    reserved: usize,
    cancel: AtomicBool,
    progress: Mutex<Progress>,
}

impl ActivePlacementSearch {
    pub(in crate::routing_service) fn reserved_slots(&self) -> usize {
        if self.progress.lock().stop.is_none() {
            self.reserved
        } else {
            0
        }
    }

    pub(super) fn stop_requested(&self) -> Option<StopReason> {
        if self.cancel.load(Ordering::Relaxed) {
            Some(StopReason::Cancelled)
        } else {
            (self.started.elapsed() >= self.budget).then_some(StopReason::TimeLimit)
        }
    }

    pub(super) fn planned_trials(&self) -> usize {
        self.planned
    }

    pub(super) fn phase(&self, phase: Phase) {
        self.progress.lock().phase = phase;
    }

    pub(super) fn begin_trial(&self, poses: &[Pose]) {
        let mut progress = self.progress.lock();
        progress.phase = Phase::Staging;
        progress.current = poses.iter().copied().map(PlacementPose::from).collect();
    }

    pub(super) fn complete_trial(&self) {
        self.progress.lock().completed += 1;
    }

    fn status(&self) -> PlacementSearchStatus {
        let progress = self.progress.lock();
        PlacementSearchStatus {
            search: self.id,
            board: self.board,
            base_revision: self.revision,
            phase: progress.phase,
            elapsed_ms: progress
                .elapsed
                .unwrap_or_else(|| self.started.elapsed())
                .as_millis(),
            budget_ms: self.budget.as_millis(),
            total_poses: self.total,
            planned_trials: self.planned,
            completed_trials: progress.completed,
            current_placements: progress.current.clone(),
            cancel_requested: self.cancel.load(Ordering::Relaxed),
            stop_reason: progress.stop,
            comparison: progress.comparison.clone().map(Comparison),
            failure: progress.failure.clone(),
        }
    }

    fn cancel(&self) {
        if self.progress.lock().stop.is_none() {
            self.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl RoutingService {
    pub fn start_placement_search(
        &mut self,
        editor: &ProjectEditor,
        request: PlacementRequest,
        max_trials: usize,
    ) -> Result<(PlacementSearchStatus, PlacementJob), RoutingError> {
        if !(1..=64).contains(&max_trials) {
            return Err(invalid("Placement search requires max_trials between 1 and 64.").into());
        }
        if self
            .placement_search
            .as_ref()
            .is_some_and(|search| search.reserved_slots() > 0)
        {
            return Err(RoutingError::Refused(
                "A placement search is already running.".into(),
            ));
        }
        let mut job = self.prepare_placement(editor, request)?;
        let state = Arc::new(ActivePlacementSearch {
            id: Uuid::new_random(),
            board: job.request.copper.board,
            revision: job.source.base_revision,
            started: Instant::now(),
            budget: job.request.max_time,
            total: job.poses.len(),
            planned: max_trials.min(job.poses.len()),
            reserved: job.request.max_candidates,
            cancel: AtomicBool::new(false),
            progress: Mutex::new(Progress {
                phase: Phase::Queued,
                current: Vec::new(),
                completed: 0,
                elapsed: None,
                stop: None,
                comparison: None,
                failure: None,
            }),
        });
        self.placement_search = Some(Arc::clone(&state));
        job.worker = Some(Arc::clone(&state));
        Ok((state.status(), job))
    }

    pub fn placement_search_status(
        &self,
        id: Uuid,
        cancel: bool,
    ) -> Result<PlacementSearchStatus, RoutingError> {
        let state = self
            .placement_search
            .as_ref()
            .filter(|search| search.id == id)
            .ok_or(RoutingError::PlacementSearchNotFound(id))?;
        if cancel {
            state.cancel();
        }
        Ok(state.status())
    }

    pub fn cancel_placement_search(&self) {
        if let Some(state) = &self.placement_search {
            state.cancel();
        }
    }

    pub fn finish_placement_search(
        &mut self,
        project: &Project,
        id: Uuid,
        result: Result<PlacementResults, RoutingError>,
    ) {
        let Some(state) = self
            .placement_search
            .as_ref()
            .filter(|s| s.id == id)
            .cloned()
        else {
            return;
        };

        // Temporarily take the worker record to spend its own reservation.
        // This whole publication runs under the project/session write lock.
        self.placement_search = None;
        let result = result.and_then(|results| {
            self.check_source_identity(
                project,
                results.owner,
                results.project,
                state.revision,
                &results.context,
            )?;
            let stop = if state.cancel.load(Ordering::Relaxed) {
                StopReason::Cancelled
            } else {
                results.stop
            };
            let report = if results.report.is_some() {
                Some(self.retain_placement_comparison(project, results)?)
            } else {
                None
            };
            Ok((stop, report))
        });
        {
            let mut progress = state.progress.lock();
            progress.phase = Phase::Finished;
            progress.elapsed = Some(state.started.elapsed());
            progress.current.clear();
            match result {
                Ok((reason, report)) => {
                    progress.stop = Some(reason);
                    progress.comparison = report.map(Arc::new);
                }
                Err(error) => {
                    progress.stop = Some(StopReason::Failed);
                    progress.failure = Some(error.to_string());
                }
            }
        }
        self.placement_search = Some(state);
    }
}
