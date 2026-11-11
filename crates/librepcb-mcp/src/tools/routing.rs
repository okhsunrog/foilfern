//! Builtin reviewed routing: detached preparation, paginated exact preview,
//! single-transaction apply and explicit disposal. No upstream counterpart.

use librepcb_core::types::Uuid;
use librepcb_editor::routing_service::{
    RoutingCandidate, RoutingCandidateInfo, RoutingError, RoutingJob,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::{board_edit, write};
use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::session::Session;

/// Builtin-only options; external backends are deliberately not accepted.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingPrepareArgs {
    #[serde(default)]
    /// Board name, index or UUID; defaults to the primary board.
    pub board: Option<String>,
    #[serde(default)]
    /// Route these net names/UUIDs only; empty means all unrouted nets.
    pub nets: Vec<String>,
    #[serde(default)]
    /// Allowed copper layers, e.g. top/bottom; empty means all.
    pub layers: Vec<String>,
    #[serde(default)]
    /// Trace width in mm; defaults to effective board/net-class rules.
    pub trace_width: Option<f64>,
    #[serde(default)]
    /// Via drill diameter in mm; clamped to the board minimum.
    pub via_drill: Option<f64>,
    #[serde(default)]
    /// Via outer diameter in mm; respects the minimum annular ring.
    pub via_size: Option<f64>,
    #[serde(default)]
    /// Routing grid pitch in mm; omitted selects the builtin default.
    pub grid_pitch: Option<f64>,
    #[serde(default)]
    /// Reject if the current live revision differs.
    pub expected_revision: Option<u64>,
}

/// A candidate ID is opaque, session-local and consumed by successful apply.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingCandidateArgs {
    pub candidate: String,
    #[serde(default)]
    /// Reject if the current live revision differs.
    pub expected_revision: Option<u64>,
}

/// Geometry preview is the exact model patch, paginated without truncating
/// the stored candidate. An omitted limit returns up to 100 mutations.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingPreviewArgs {
    pub candidate: String,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub limit: Option<usize>,
}

fn routing_error(error: RoutingError) -> ToolError {
    match error {
        RoutingError::Editor(e) => e.into(),
        error @ RoutingError::NotFound(_) => ToolError::new(ErrorKind::NotFound, error.to_string()),
        error @ RoutingError::StaleRevision { .. } => {
            ToolError::new(ErrorKind::StaleRevision, error.to_string())
        }
        error @ RoutingError::StaleContext => {
            ToolError::new(ErrorKind::StaleRevision, error.to_string())
        }
        error @ RoutingError::WrongSession => {
            ToolError::new(ErrorKind::Conflict, error.to_string())
        }
        error @ RoutingError::Refused(_) => ToolError::new(ErrorKind::Refused, error.to_string()),
    }
}

fn candidate_id(id: &str) -> ToolResult<Uuid> {
    id.parse()
        .map_err(|_| ToolError::invalid("candidate must be a UUID returned by routing_prepare."))
}

/// Snapshot under the session lock; the returned job needs no live handles.
pub fn prepare_job(session: &Session, args: RoutingPrepareArgs) -> ToolResult<RoutingJob> {
    let open = session.project()?;
    write::check_revision(open.project(), args.expected_revision)?;
    let board = board_edit::board_id(open.project(), args.board.as_deref())?;
    let options = board_edit::AutorouteArgs {
        backend: board_edit::RouterBackend::Builtin,
        board: args.board,
        nets: args.nets,
        layers: args.layers,
        trace_width: args.trace_width,
        via_drill: args.via_drill,
        via_size: args.via_size,
        grid_pitch: args.grid_pitch,
        ..Default::default()
    };
    let command = board_edit::builtin_command(open.project(), board, &options)?;
    open.routing
        .prepare(&open.editor, command)
        .map_err(routing_error)
}

/// Run on a blocking worker outside the session lock.
pub fn run_job(job: RoutingJob) -> ToolResult<RoutingCandidate> {
    job.run().map_err(routing_error)
}

/// Reject stale/foreign completions rather than publishing them to a new project.
pub fn finish_prepare(
    session: &mut Session,
    candidate: RoutingCandidate,
) -> ToolResult<ToolOutput> {
    let open = session.project_mut()?;
    let info = open
        .routing
        .retain(open.editor.project(), candidate)
        .map_err(routing_error)?;
    let mut out = candidate_output("Prepared routing candidate; live board unchanged.", &info)?;
    out.result["applied"] = json!(false);
    Ok(out)
}

/// Synchronous entry point for non-server callers. The server separates its
/// three phases to keep the host responsive during search and DRC.
pub fn routing_prepare(session: &mut Session, args: RoutingPrepareArgs) -> ToolResult<ToolOutput> {
    let job = prepare_job(session, args)?;
    finish_prepare(session, run_job(job)?)
}

fn candidate_output(summary: &str, info: &RoutingCandidateInfo) -> ToolResult<ToolOutput> {
    let mut out = ToolOutput::new(summary, info)?;
    if !info.validation.acceptable || info.validation.after.unrouted > 0 {
        out = out.partial();
    }
    if !info.validation.acceptable {
        out = out.warn(
            "Validation is unacceptable: checks incomplete, changed netlist or new DRC errors.",
        );
    }
    if !info.validation.drc_clean {
        out = out.warn("Candidate is not DRC-clean; inspect validation and remaining connections.");
    }
    Ok(out)
}

/// Read-only exact patch preview. Stale candidates remain inspectable.
pub fn routing_preview(session: &Session, args: RoutingPreviewArgs) -> ToolResult<ToolOutput> {
    let limit = args.limit.unwrap_or(100);
    if !(1..=200).contains(&limit) {
        return Err(ToolError::invalid("limit must be between 1 and 200."));
    }
    let open = session.project()?;
    let candidate = open
        .routing
        .candidate(candidate_id(&args.candidate)?)
        .map_err(routing_error)?;
    let start = args.offset.min(candidate.mutations().len());
    let end = start.saturating_add(limit).min(candidate.mutations().len());
    let mut out = candidate_output(
        "Exact routing patch preview; live board unchanged.",
        candidate.info(),
    )?;
    out.result["applied"] = json!(false);
    out.result["mutations"] = json!(&candidate.mutations()[start..end]);
    out.result["offset"] = json!(start);
    out.result["next_offset"] = json!((end < candidate.mutations().len()).then_some(end));
    out.result["stale"] = json!(
        open.routing
            .is_stale(open.project(), candidate.info().candidate)
            .map_err(routing_error)?
    );
    Ok(out)
}

/// Apply one reviewed candidate with the editor's ordinary atomic undo path.
pub fn routing_apply(session: &mut Session, args: RoutingCandidateArgs) -> ToolResult<ToolOutput> {
    let open = session.project_mut()?;
    write::check_revision(open.project(), args.expected_revision)?;
    let revision_before = open.project().revision();
    let info = open
        .routing
        .apply(&mut open.editor, candidate_id(&args.candidate)?)
        .map_err(routing_error)?;
    let summary = if info.mutation_count == 0 {
        "Consumed a checked candidate with no model changes."
    } else {
        "Applied reviewed routing candidate as one undo step."
    };
    let mut out = candidate_output(summary, &info)?;
    let (changes, count) = write::changes_since(open.project(), revision_before);
    out.result["applied"] = json!(true);
    out.result["consumed"] = json!(true);
    out.result["revision_before"] = json!(revision_before);
    if changes
        .as_array()
        .is_some_and(|events| events.len() < count)
    {
        out.result["changes_truncated"] = json!(true);
    }
    out.result["changes"] = changes;
    out.result["change_count"] = json!(count);
    Ok(out)
}

/// Release a candidate without touching model, files or history.
pub fn routing_discard(
    session: &mut Session,
    args: RoutingCandidateArgs,
) -> ToolResult<ToolOutput> {
    let open = session.project_mut()?;
    write::check_revision(open.project(), args.expected_revision)?;
    let id = candidate_id(&args.candidate)?;
    open.routing.discard(id).map_err(routing_error)?;
    ToolOutput::new(
        "Discarded routing candidate; live board unchanged.",
        json!({"candidate":id}),
    )
}
