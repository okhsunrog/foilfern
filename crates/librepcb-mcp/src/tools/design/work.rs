//! Admission, progress and cooperative cancellation for schematic previews.

use std::sync::Arc;
use std::time::{Duration, Instant};

use librepcb_core::types::Uuid;
use parking_lot::Mutex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::resolve;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stage {
    Capture,
    Plan,
    Check,
    Render,
    Publish,
}

#[derive(Default)]
pub(crate) struct WorkQueue {
    state: Mutex<QueueState>,
}

#[derive(Default)]
struct QueueState {
    active: Option<Arc<Work>>,
    last: Option<serde_json::Value>,
}

pub(crate) struct Work {
    id: Uuid,
    started: Instant,
    budget: Option<Duration>,
    state: Mutex<WorkState>,
}

struct WorkState {
    stage: Stage,
    stop: Option<Stop>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Stop {
    Cancelled,
    BudgetExhausted,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Complete,
    Stale,
    Failed,
    Cancelled,
    BudgetExhausted,
    RequestEnded,
}

impl From<Stop> for Outcome {
    fn from(stop: Stop) -> Self {
        match stop {
            Stop::Cancelled => Self::Cancelled,
            Stop::BudgetExhausted => Self::BudgetExhausted,
        }
    }
}

pub(crate) struct WorkGuard {
    queue: Arc<WorkQueue>,
    pub(crate) work: Arc<Work>,
    outcome: Outcome,
}

impl WorkQueue {
    pub(crate) fn try_start(self: &Arc<Self>, budget_ms: Option<u64>) -> ToolResult<WorkGuard> {
        self.start_at(budget_ms, Instant::now())
    }

    fn start_at(
        self: &Arc<Self>,
        budget_ms: Option<u64>,
        started: Instant,
    ) -> ToolResult<WorkGuard> {
        if budget_ms == Some(0) {
            return Err(ToolError::invalid("budget_ms must be greater than zero."));
        }
        let mut state = self.state.lock();
        if state.active.is_some() {
            return Err(ToolError::new(
                ErrorKind::Conflict,
                "A schematic preview is already being prepared.",
            ));
        }
        let work = Arc::new(Work {
            id: Uuid::new_random(),
            started,
            budget: budget_ms.map(Duration::from_millis),
            state: Mutex::new(WorkState {
                stage: Stage::Capture,
                stop: None,
            }),
        });
        state.active = Some(work.clone());
        Ok(WorkGuard {
            queue: self.clone(),
            work,
            outcome: Outcome::RequestEnded,
        })
    }

    pub(crate) fn status(&self) -> ToolResult<ToolOutput> {
        let state = self.state.lock();
        let active = state.active.as_ref().map(|work| work.report(None));
        ToolOutput::new(
            "Schematic preview status.",
            json!({"active":active,"last":state.last}),
        )
    }

    pub(crate) fn cancel(&self, args: CancelArgs) -> ToolResult<ToolOutput> {
        let id = resolve::required_uuid(&args.job, "preview job")?;
        let state = self.state.lock();
        let work = state
            .active
            .as_ref()
            .filter(|work| work.id == id)
            .ok_or_else(|| {
                ToolError::not_found("There is no active schematic preview with that job ID.")
            })?;
        let mut status = work.state.lock();
        if matches!(status.stage, Stage::Publish) {
            return Err(ToolError::new(
                ErrorKind::Conflict,
                "The schematic preview is already publishing.",
            ));
        }
        work.request_stop(&mut status, Stop::Cancelled);
        ToolOutput::new(
            "Schematic preview stop requested.",
            json!({"job":id,"stop_requested":status.stop}),
        )
    }
}

impl Work {
    fn request_stop(&self, state: &mut WorkState, requested: Stop) {
        let stop = if self
            .budget
            .is_some_and(|budget| self.started.elapsed() >= budget)
        {
            Stop::BudgetExhausted
        } else {
            requested
        };
        state.stop.get_or_insert(stop);
    }

    pub(crate) fn checkpoint(&self, stage: Stage) -> ToolResult<()> {
        let mut state = self.state.lock();
        if state.stop.is_none()
            && self
                .budget
                .is_some_and(|budget| self.started.elapsed() >= budget)
        {
            state.stop = Some(Stop::BudgetExhausted);
        }
        match state.stop {
            Some(Stop::Cancelled) => Err(ToolError::new(
                ErrorKind::Cancelled,
                "The schematic preview was cancelled.",
            )),
            Some(Stop::BudgetExhausted) => Err(ToolError::new(
                ErrorKind::BudgetExhausted,
                "The schematic preview exceeded its time budget.",
            )),
            None => {
                state.stage = stage;
                Ok(())
            }
        }
    }

    fn report(&self, outcome: Option<Outcome>) -> serde_json::Value {
        let state = self.state.lock();
        let elapsed = self.started.elapsed();
        let remaining = self.budget.map(|budget| budget.saturating_sub(elapsed));
        let outcome = match outcome {
            Some(Outcome::RequestEnded) => {
                Some(state.stop.map_or(Outcome::RequestEnded, Into::into))
            }
            other => other,
        };
        json!({"job":self.id,"stage":state.stage,"stop_requested":state.stop,
            "elapsed_ms":u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            "remaining_ms":remaining.map(|time| u64::try_from(time.as_millis()).unwrap_or(u64::MAX)),
            "outcome":outcome})
    }
}

impl WorkGuard {
    pub(crate) fn record<T>(&mut self, result: &ToolResult<T>) {
        self.outcome = match result.as_ref().err().map(|error| error.kind) {
            None => Outcome::Complete,
            Some(ErrorKind::Cancelled) => Outcome::Cancelled,
            Some(ErrorKind::BudgetExhausted) => Outcome::BudgetExhausted,
            Some(ErrorKind::StaleRevision) => Outcome::Stale,
            Some(_) => Outcome::Failed,
        };
    }

    pub(crate) fn record_failure<T>(&mut self, result: &ToolResult<T>) {
        if result.is_err() {
            self.record(result);
        }
    }
}

impl Drop for WorkGuard {
    fn drop(&mut self) {
        let mut state = self.queue.state.lock();
        state.last = Some(self.work.report(Some(self.outcome)));
        state.active = None;
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelArgs {
    /// Active job UUID from design_preview_status.
    job: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_budget_stops_publication_and_releases_capacity() {
        let queue = Arc::new(WorkQueue::default());
        let mut guard = queue
            .start_at(Some(1), Instant::now() - Duration::from_secs(1))
            .unwrap();
        let result = guard.work.checkpoint(Stage::Publish);
        assert_eq!(
            result.as_ref().unwrap_err().kind,
            ErrorKind::BudgetExhausted
        );
        guard.record(&result);
        drop(guard);
        let status = queue.status().unwrap().result;
        assert!(status["active"].is_null());
        assert_eq!(status["last"]["outcome"], "budget_exhausted");
        assert!(queue.try_start(None).is_ok());
    }
}
