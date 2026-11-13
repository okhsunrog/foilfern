//! Builtin reviewed routing: detached preparation, paginated exact preview,
//! single-transaction apply and explicit disposal. No upstream counterpart.

use librepcb_core::project::{BoardId, Mutation, PlaneId, Project};
use librepcb_core::types::{Point, Uuid};
use librepcb_editor::ProjectEditor;
use librepcb_editor::commands::{
    Autoroute, ConnectLocalVia, NetRef, PadNeckDown, RoutingOrder, StitchPlaneIsland,
};
use librepcb_editor::routing_service::{
    RegionalRoutingRequest, RoutingCandidate, RoutingCandidateInfo, RoutingComparisonJob,
    RoutingError, RoutingJob, RoutingTrial, RoutingTrials,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::{board_edit, ground_reference, trace_path, write};
use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::session::Session;
use crate::units::PointMm;

mod diagnose_image;
mod explicit_copper;
pub use explicit_copper::{RoutingCopperArgs, prepare_copper_job};

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
    /// Preferred trace width in mm. Omit to use net-class or board defaults.
    /// The board and each net class still set the minimum.
    pub trace_width: Option<f64>,
    /// Optional short reduced-width escape at pads.
    pub neck_down: Option<PadNeckDownArgs>,
    #[serde(default)]
    /// Via drill diameter in mm; clamped to the board minimum.
    pub via_drill: Option<f64>,
    #[serde(default)]
    /// Via outer diameter in mm; respects the minimum annular ring.
    pub via_size: Option<f64>,
    /// Cost of a layer change as equivalent trace length in mm (default 2).
    pub via_cost: Option<f64>,
    #[serde(default)]
    /// Routing grid pitch in mm; omitted selects the builtin default.
    pub grid_pitch: Option<f64>,
    /// Include samples of rejected grid steps for failed requests.
    #[serde(default)]
    pub search_evidence: bool,
    /// Try extra exact-checked exits from nearby pads.
    #[serde(default)]
    pub pad_escape: bool,
    /// Penalize tiles contested by earlier successful rip-ups in this trial.
    #[serde(default)]
    pub congestion_history: bool,
    /// Net names or UUIDs to route first and protect from lower priority nets.
    #[serde(default)]
    pub critical_nets: Vec<String>,
    #[serde(default)]
    /// Reject if the current live revision differs.
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PadNeckDownArgs {
    /// Requested reduced width in mm, raised to board and net-class DRC minima.
    pub width: f64,
    /// Maximum length in mm per pad escape.
    pub max_length: f64,
}

impl PadNeckDownArgs {
    fn command(self) -> ToolResult<PadNeckDown> {
        Ok(PadNeckDown {
            width: board_edit::positive(self.width, "neck_down.width")?,
            max_length: board_edit::positive(self.max_length, "neck_down.max_length")?,
        })
    }
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoutingOrderArg {
    ShortestFirst,
    LongestFirst,
    ConstrainedFirst,
}

impl From<RoutingOrderArg> for RoutingOrder {
    fn from(order: RoutingOrderArg) -> Self {
        match order {
            RoutingOrderArg::ShortestFirst => Self::ShortestFirst,
            RoutingOrderArg::LongestFirst => Self::LongestFirst,
            RoutingOrderArg::ConstrainedFirst => Self::ConstrainedFirst,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingCompareArgs {
    /// Common builtin settings, including optional expected_revision.
    #[serde(default)]
    pub options: RoutingPrepareArgs,
    /// Two or three distinct initial orders. Omit to compare all three.
    pub orders: Option<Vec<RoutingOrderArg>>,
    /// Two or three distinct order and via-cost combinations, instead of orders.
    pub trials: Option<Vec<RoutingTrialArgs>>,
    /// Prefer candidates meeting these trace path limits after DRC and connectivity.
    #[serde(default)]
    pub trace_preferences: Vec<TracePreferenceArgs>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TracePreferenceArgs {
    pub path: trace_path::TracePathPair,
    /// Maximum explicit trace centerline length in mm.
    pub max_length_mm: Option<f64>,
    /// Maximum layer changes on the measured trace path.
    pub max_layer_changes: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingTrialArgs {
    pub order: RoutingOrderArg,
    /// Cost of a layer change in equivalent mm. Omit to inherit options.via_cost.
    pub via_cost: Option<f64>,
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
    /// Compare coverage of these signals on the candidate's original snapshot.
    pub reference: Option<ground_reference::ReferenceArgs>,
    /// Measure up to 16 physical pad pairs before and after the frozen patch.
    #[serde(default)]
    pub trace_paths: Vec<trace_path::TracePathPair>,
    /// Compare fill at up to 16 fixed points of one selected plane.
    pub plane_probes: Option<PlaneProbeArgs>,
}

impl RoutingPreviewArgs {
    pub(crate) fn has_measurements(&self) -> bool {
        self.reference.is_some() || !self.trace_paths.is_empty() || self.plane_probes.is_some()
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlaneProbeArgs {
    /// Plane UUID from board_get.
    pub plane: String,
    /// Fixed coordinates in board millimeters.
    pub points: Vec<PointMm>,
}

struct PlaneProbes {
    plane: PlaneId,
    points: Vec<Point>,
}

impl PlaneProbes {
    fn new(args: PlaneProbeArgs) -> ToolResult<Self> {
        if !(1..=16).contains(&args.points.len()) {
            return Err(ToolError::invalid(
                "Select between 1 and 16 plane probe points.",
            ));
        }
        Ok(Self {
            plane: PlaneId(crate::resolve::required_uuid(&args.plane, "plane")?),
            points: args
                .points
                .into_iter()
                .map(PointMm::to_point)
                .collect::<ToolResult<_>>()?,
        })
    }

    fn measure(&self, project: &Project, board: BoardId) -> ToolResult<Vec<Option<&'static str>>> {
        let native_board = project
            .board(board)
            .ok_or_else(|| ToolError::not_found("Candidate board is missing."))?;
        if native_board.plane(self.plane).is_none() {
            return Err(ToolError::not_found("Plane was not found on this board."));
        }
        let job = project
            .plane_job(board, None)?
            .ok_or_else(|| ToolError::internal("The board has no planes to fill."))?;
        let fragments = job.diagnose(self.plane)?;
        Ok(self
            .points
            .iter()
            .map(|&point| {
                fragments
                    .iter()
                    .find(|fragment| fragment.contains(point))
                    .map(|fragment| board_edit::plane_fragment_status(fragment.status).1)
            })
            .collect())
    }

    fn comparison(
        &self,
        before: Vec<Option<&'static str>>,
        after: Vec<Option<&'static str>>,
    ) -> serde_json::Value {
        let points: Vec<_> = self
            .points
            .iter()
            .zip(before.into_iter().zip(after))
            .map(|(&point, (before, after))| {
                json!({"point":PointMm::from(point),"before":before,"after":after})
            })
            .collect();
        json!({"plane":self.plane,"points":points})
    }
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
    id.parse().map_err(|_| {
        ToolError::invalid("candidate must be a UUID returned by a routing preparation tool.")
    })
}

fn critical_nets(project: &Project, keys: &[String]) -> ToolResult<Vec<NetRef>> {
    keys.iter()
        .map(|key| crate::resolve::net(project, key).map(|(id, _)| NetRef::Id(id)))
        .collect()
}

fn via_cost(value: Option<f64>) -> ToolResult<Option<librepcb_core::types::Length>> {
    value
        .map(|value| board_edit::unsigned(value, "via_cost").map(|length| length.get()))
        .transpose()
}

fn builtin_request(project: &Project, args: RoutingPrepareArgs) -> ToolResult<Autoroute> {
    write::check_revision(project, args.expected_revision)?;
    let board = board_edit::board_id(project, args.board.as_deref())?;
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
    let mut command = board_edit::builtin_command(project, board, &options)?;
    command.via_cost = via_cost(args.via_cost)?;
    command.search_evidence = args.search_evidence;
    command.neck_down = args.neck_down.map(PadNeckDownArgs::command).transpose()?;
    command.pad_escape = args.pad_escape;
    command.congestion_history = args.congestion_history;
    command.critical_nets = critical_nets(project, &args.critical_nets)?;
    Ok(command)
}

/// Snapshot under the session lock; the returned job needs no live handles.
pub fn prepare_job(session: &Session, args: RoutingPrepareArgs) -> ToolResult<RoutingJob> {
    let open = session.project()?;
    let command = builtin_request(open.project(), args)?;
    open.routing
        .prepare(&open.editor, command)
        .map_err(routing_error)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingViaPrepareArgs {
    /// Existing footprint pad (e.g. U1.21), via UUID, or junction UUID.
    pub from: String,
    pub from_layer: String,
    /// Existing footprint pad, via UUID, or junction UUID on the same net.
    pub to: String,
    pub to_layer: String,
    /// Exact via center in board mm. Omit to search around the first anchor.
    pub position: Option<crate::units::PointMm>,
    /// Automatic search radius in mm (default 1, maximum 1.5).
    pub search_radius: Option<f64>,
    /// Trace width in mm. Defaults to the net class or board rules.
    pub trace_width: Option<f64>,
    pub via_drill: Option<f64>,
    pub via_size: Option<f64>,
    pub board: Option<String>,
    pub expected_revision: Option<u64>,
}

pub fn prepare_via_job(session: &Session, args: RoutingViaPrepareArgs) -> ToolResult<RoutingJob> {
    let open = session.project()?;
    let project = open.project();
    write::check_revision(project, args.expected_revision)?;
    let board = board_edit::board_id(project, args.board.as_deref())?;
    let endpoint = |key| board_edit::endpoint(project, board, &board_edit::EndpointArg::Ref(key));
    let radius = args.search_radius.unwrap_or(1.0);
    if radius > 1.5 || (args.position.is_some() && args.search_radius.is_some()) {
        return Err(ToolError::invalid(
            "Pass an exact position or a search radius up to 1.5 mm.",
        ));
    }
    let dim = |value: Option<f64>, name| value.map(|v| board_edit::positive(v, name)).transpose();
    let request = ConnectLocalVia {
        board,
        from: endpoint(args.from)?,
        from_layer: board_edit::copper_layer(&args.from_layer)?,
        to: endpoint(args.to)?,
        to_layer: board_edit::copper_layer(&args.to_layer)?,
        position: args
            .position
            .map(crate::units::PointMm::to_point)
            .transpose()?,
        search_radius: board_edit::positive(radius, "search_radius")?,
        trace_width: dim(args.trace_width, "trace_width")?,
        via_drill: dim(args.via_drill, "via_drill")?,
        via_size: dim(args.via_size, "via_size")?,
    };
    open.routing
        .prepare_local_via(&open.editor, request)
        .map_err(routing_error)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlaneStitchPrepareArgs {
    /// UUID of the plane with a discarded island, from plane_diagnose.
    pub plane: String,
    /// Existing same-net pad, via or junction.
    pub from: String,
    /// Copper layer of the lead, different from the plane layer.
    pub from_layer: String,
    /// Through-via center strictly inside the discarded copper, in board mm.
    pub position: PointMm,
    /// Optional intermediate points for the lead, in board mm.
    #[serde(default)]
    pub points: Vec<PointMm>,
    /// Lead width in mm. Defaults to the normal trace-add width.
    pub trace_width: Option<f64>,
    /// Via drill in mm. Omit for the board's automatic drill.
    pub via_drill: Option<f64>,
    /// Via outer diameter in mm. Omit for the automatic size.
    pub via_size: Option<f64>,
    pub board: Option<String>,
    pub expected_revision: Option<u64>,
}

pub fn prepare_plane_stitch_job(
    session: &Session,
    args: PlaneStitchPrepareArgs,
) -> ToolResult<RoutingJob> {
    let open = session.project()?;
    let project = open.project();
    write::check_revision(project, args.expected_revision)?;
    let board = board_edit::board_id(project, args.board.as_deref())?;
    let dim = |value: Option<f64>, name| value.map(|v| board_edit::positive(v, name)).transpose();
    let request = StitchPlaneIsland {
        board,
        plane: PlaneId(crate::resolve::required_uuid(&args.plane, "plane")?),
        from: board_edit::endpoint(project, board, &board_edit::EndpointArg::Ref(args.from))?,
        from_layer: board_edit::copper_layer(&args.from_layer)?,
        position: args.position.to_point()?,
        points: args
            .points
            .into_iter()
            .map(PointMm::to_point)
            .collect::<ToolResult<Vec<_>>>()?,
        trace_width: dim(args.trace_width, "trace_width")?,
        via_drill: dim(args.via_drill, "via_drill")?,
        via_size: dim(args.via_size, "via_size")?,
    };
    open.routing
        .prepare_plane_stitch(&open.editor, request)
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

pub fn prepare_comparison_job(
    session: &Session,
    args: RoutingCompareArgs,
) -> ToolResult<RoutingComparisonJob> {
    let open = session.project()?;
    let command = builtin_request(open.project(), args.options)?;
    if args.trace_preferences.len() > 16 {
        return Err(ToolError::invalid("Select at most 16 trace preferences."));
    }
    let project = open.project();
    let board = command
        .board
        .and_then(|board| project.board(board))
        .ok_or_else(|| ToolError::not_found("Candidate board is missing."))?;
    let trace_preferences = args
        .trace_preferences
        .iter()
        .map(|args| {
            if args.max_length_mm.is_none() && args.max_layer_changes.is_none() {
                return Err(ToolError::invalid(
                    "Each trace preference requires at least one limit.",
                ));
            }
            let max_length = args
                .max_length_mm
                .map(|v| board_edit::unsigned(v, "max_length_mm").map(|length| length.get()))
                .transpose()?;
            let path = trace_path::ResolvedTracePath::new(project, board, &args.path)?;
            Ok(path.preference(max_length, args.max_layer_changes))
        })
        .collect::<ToolResult<_>>()?;
    if let Some(trials) = args.trials {
        if args.orders.is_some() {
            return Err(ToolError::invalid("Select orders or trials, not both."));
        }
        let trials = trials
            .into_iter()
            .map(|trial| {
                Ok(RoutingTrial {
                    order: trial.order.into(),
                    via_cost: via_cost(trial.via_cost)?,
                })
            })
            .collect::<ToolResult<_>>()?;
        return open
            .routing
            .prepare_comparison_trials(&open.editor, command, trials)
            .map(|job| job.with_trace_preferences(trace_preferences))
            .map_err(routing_error);
    }
    let orders = args.orders.map_or_else(
        || {
            vec![
                RoutingOrder::ShortestFirst,
                RoutingOrder::LongestFirst,
                RoutingOrder::ConstrainedFirst,
            ]
        },
        |orders| orders.into_iter().map(Into::into).collect(),
    );
    open.routing
        .prepare_comparison(&open.editor, command, orders)
        .map(|job| job.with_trace_preferences(trace_preferences))
        .map_err(routing_error)
}

pub fn run_comparison_job(job: RoutingComparisonJob) -> ToolResult<RoutingTrials> {
    job.run().map_err(routing_error)
}

pub fn finish_comparison(session: &mut Session, trials: RoutingTrials) -> ToolResult<ToolOutput> {
    let open = session.project_mut()?;
    let comparison = open
        .routing
        .retain_comparison(open.editor.project(), trials)
        .map_err(routing_error)?;
    let recommended = comparison.recommended.and_then(|id| {
        comparison
            .candidates
            .iter()
            .find(|entry| entry.candidate == id)
    });
    let mut output = ToolOutput::new(
        "Compared builtin routing trials; live board unchanged.",
        &comparison,
    )?;
    if let Some(entry) = recommended {
        if entry.after.unrouted > 0 {
            output = output
                .partial()
                .warn("Recommended candidate leaves open connections.");
        }
    } else {
        output = output
            .partial()
            .warn("No acceptable candidate without connectivity regressions.");
    }
    Ok(output)
}

fn candidate_output(summary: &str, info: &RoutingCandidateInfo) -> ToolResult<ToolOutput> {
    let mut out = ToolOutput::new(summary, info)?;
    if !info.validation.acceptable || info.validation.after.unrouted > 0 {
        out = out.partial();
    }
    if !info.validation.acceptable {
        out = out.warn(
            "Validation is unacceptable: inspect checks, netlist, new DRC errors and unrouted regressions.",
        );
    }
    if !info.validation.drc_clean {
        out = out.warn("Candidate is not DRC-clean; inspect validation and remaining connections.");
    }
    if info.validation.new_warning_count > 0 {
        out = out.warn(format!(
            "Candidate introduces {} DRC warning(s); see validation.new_warnings.",
            info.validation.new_warning_count
        ));
    }
    Ok(out)
}

/// Read-only exact patch preview. Stale candidates remain inspectable.
pub fn routing_preview(session: &Session, args: RoutingPreviewArgs) -> ToolResult<ToolOutput> {
    if args.has_measurements() {
        let work = prepare_measured_preview(session, args)?;
        return finish_measured_preview(session, run_measured_preview(work)?);
    }
    patch_preview(session, args)
}

pub struct MeasuredPreviewJob {
    id: Uuid,
    preview: ToolOutput,
    board: BoardId,
    revision: u64,
    snapshot: ProjectEditor,
    reference: Option<ground_reference::ReferenceArgs>,
    paths: Vec<trace_path::ResolvedTracePath>,
    plane_probes: Option<PlaneProbes>,
    patch: Vec<Mutation>,
}

pub fn prepare_measured_preview(
    session: &Session,
    mut args: RoutingPreviewArgs,
) -> ToolResult<MeasuredPreviewJob> {
    let reference = args.reference.take();
    let requested_paths = std::mem::take(&mut args.trace_paths);
    let plane_probes = args.plane_probes.take().map(PlaneProbes::new).transpose()?;
    if requested_paths.len() > 16 {
        return Err(ToolError::invalid("Select at most 16 trace paths."));
    }
    let id = candidate_id(&args.candidate)?;
    let preview = patch_preview(session, args)?;
    let open = session.project()?;
    let board = open
        .routing
        .candidate(id)
        .map_err(routing_error)?
        .info()
        .board;
    let (snapshot, patch) = open
        .routing
        .preview_snapshot(open.project(), id)
        .map_err(routing_error)?;
    let source = open.project();
    let native_board = source
        .board(board)
        .ok_or_else(|| ToolError::not_found("Candidate board is missing."))?;
    let paths = requested_paths
        .iter()
        .map(|pair| trace_path::ResolvedTracePath::new(source, native_board, pair))
        .collect::<ToolResult<_>>()?;
    Ok(MeasuredPreviewJob {
        id,
        preview,
        board,
        revision: source.revision(),
        snapshot,
        reference,
        paths,
        plane_probes,
        patch,
    })
}

pub fn run_measured_preview(work: MeasuredPreviewJob) -> ToolResult<(Uuid, ToolOutput)> {
    let MeasuredPreviewJob {
        id,
        mut preview,
        board,
        revision,
        mut snapshot,
        reference,
        paths,
        plane_probes,
        patch,
    } = work;
    let before = paths
        .iter()
        .map(|path| path.measure(snapshot.project(), board))
        .collect::<ToolResult<Vec<_>>>()?;
    let mut after = Vec::new();
    let plane_before = plane_probes
        .as_ref()
        .map(|probes| probes.measure(snapshot.project(), board))
        .transpose()?;
    let mut plane_comparison = None;
    let compare = |project: &Project| -> ToolResult<()> {
        for (path, before) in paths.iter().zip(before) {
            after.push(path.comparison(before, path.measure(project, board)?));
        }
        if let Some((probes, before)) = plane_probes.as_ref().zip(plane_before) {
            plane_comparison = Some(probes.comparison(before, probes.measure(project, board)?));
        }
        Ok(())
    };
    if let Some(reference) = reference {
        let reference = ground_reference::ReferenceJob::new(snapshot, board, reference, revision)?;
        preview.result["reference_coverage"] = reference.compare(patch, compare)?;
    } else {
        snapshot.apply_mutations("Preview routing measurements", patch)?;
        compare(snapshot.project())?;
    }
    if !after.is_empty() {
        preview.result["trace_paths"] = json!(after);
    }
    if let Some(plane_comparison) = plane_comparison {
        preview.result["plane_probes"] = plane_comparison;
    }
    Ok((id, preview))
}

pub fn finish_measured_preview(
    session: &Session,
    (id, preview): (Uuid, ToolOutput),
) -> ToolResult<ToolOutput> {
    let open = session.project()?;
    if open
        .routing
        .is_stale(open.project(), id)
        .map_err(routing_error)?
    {
        return Err(ToolError::new(
            ErrorKind::StaleRevision,
            "The routing candidate became stale during preview measurements.",
        ));
    }
    Ok(preview)
}

fn patch_preview(session: &Session, args: RoutingPreviewArgs) -> ToolResult<ToolOutput> {
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

/// Read-only access diagnostics for one net's remaining air-wire endpoints.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingDiagnoseArgs {
    /// Net name or UUID.
    pub net: String,
    pub board: Option<String>,
    /// Trace width in mm. Omit to use builtin defaults.
    pub trace_width: Option<f64>,
    /// Routing grid in mm. Omit to use builtin defaults.
    pub grid_pitch: Option<f64>,
    /// Via drill in mm, with the same default and minimum as routing_prepare.
    pub via_drill: Option<f64>,
    /// Via outer diameter in mm, with the same default and minimum as routing_prepare.
    pub via_size: Option<f64>,
    /// Cost of a layer change as equivalent trace length in mm (default 2).
    pub via_cost: Option<f64>,
    /// Copper layers allowed for routing. Empty means all.
    #[serde(default)]
    pub layers: Vec<String>,
    /// Length of each straight clearance probe in mm (default 0.6, max 5).
    pub probe_length: Option<f64>,
    /// Include exact-checked local pad exits in the access report.
    #[serde(default)]
    pub pad_escape: bool,
    /// Attach one focused PNG for a selected endpoint and copper layer.
    pub image: Option<RoutingDiagnoseImageArgs>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingDiagnoseImageArgs {
    /// Zero-based index in the returned endpoints array (at most 16 entries).
    pub endpoint: usize,
    /// Copper layer (top, bottom, inner1, or a layer ID). Defaults to the first probed layer.
    pub layer: Option<String>,
}

pub fn routing_diagnose(session: &Session, args: RoutingDiagnoseArgs) -> ToolResult<ToolOutput> {
    use librepcb_editor::commands::autoroute::{
        AccessBlocker, RoutingConnection, RoutingObject, RoutingObjectKind, diagnose_access,
        extract_problem,
    };

    let p = session.project()?.project();
    let (_, board, b) = crate::resolve::board(p, args.board.as_deref())?;
    let (net, _) = crate::resolve::net(p, &args.net)?;
    let reach = args.probe_length.unwrap_or(0.6);
    if reach > 5.0 {
        return Err(ToolError::invalid("probe_length must be at most 5 mm."));
    }
    let reach = board_edit::positive(reach, "probe_length")?;
    let mut options = board_edit::builtin_command(
        p,
        board,
        &board_edit::AutorouteArgs {
            layers: args.layers,
            trace_width: args.trace_width,
            grid_pitch: args.grid_pitch,
            via_drill: args.via_drill,
            via_size: args.via_size,
            ..Default::default()
        },
    )?;
    options.via_cost = via_cost(args.via_cost)?;
    options.pad_escape = args.pad_escape;
    let extracted = extract_problem(p, board, &options, Some(&[net].into_iter().collect()))?;
    let problem = &extracted.problem;
    let mm = crate::units::mm;
    let anchor = |id: librepcb_editor::commands::autoroute::RoutingTerminalId| {
        super::layout::trace_anchor(
            p,
            b,
            Some(problem.terminals[id.0].position),
            extracted.terminal_anchors[id.0],
        )
    };
    let ids: std::collections::BTreeSet<_> = problem
        .connections
        .iter()
        .filter_map(|connection| match connection {
            RoutingConnection::Terminals(a, b) => Some([*a, *b]),
            RoutingConnection::Net(_) => None,
        })
        .flatten()
        .collect();
    let selected: Vec<_> = ids.iter().copied().take(16).collect();
    let access = if selected.is_empty() {
        Vec::new()
    } else {
        diagnose_access(problem, &selected, reach).map_err(librepcb_editor::Error::from)?
    };
    let obstacle_net = |id| crate::resolve::net_name(p, librepcb_core::project::NetSignalId(id));
    let clearance = |pad: Option<librepcb_core::types::Length>| match pad
        .filter(|value| *value > problem.rules.clearance)
    {
        Some(value) => (mm(value), "pad_copper_clearance"),
        None => (mm(problem.rules.clearance), "copper_clearance"),
    };
    let endpoints: Vec<_> = selected.iter().copied().zip(&access).map(|(id, report)| {
        let probes: Vec<_> = report.probes.iter().map(|probe| {
            let blocker = match probe.blocker {
                None => serde_json::Value::Null,
                Some(AccessBlocker::BoardEdge) => json!({
                    "kind":"board_edge","clearance_mm":mm(problem.rules.board_clearance),
                    "clearance_source":"copper_board_clearance",
                }),
                Some(AccessBlocker::Hole(index)) => json!({
                    "kind":"hole","index":index,"object":extracted.hole_objects[index],
                    "clearance_mm":mm(problem.rules.npth_clearance),
                    "clearance_source":"copper_npth_clearance",
                }),
                Some(AccessBlocker::Terminal(id)) => {
                    let terminal = &problem.terminals[id.0];
                    let (clearance_mm, source) = clearance(terminal.clearance);
                    json!({"kind":"terminal","anchor":anchor(id),"net":obstacle_net(terminal.net),
                        "object":RoutingObject::from_anchor(extracted.terminal_anchors[id.0]),
                        "clearance_mm":clearance_mm,"clearance_source":source})
                }
                Some(AccessBlocker::Obstacle(index)) => {
                    let item = &problem.obstacles[index];
                    let object = extracted.obstacle_objects[index];
                    let (clearance_mm, source) = clearance(item.clearance);
                    json!({"kind":"obstacle","index":index,"object":object,
                        "trace_uuid":(object.kind == RoutingObjectKind::Trace).then_some(object.uuid),
                        "net":item.net.map(obstacle_net),
                        "clearance_mm":clearance_mm,"clearance_source":source})
                }
            };
            json!({"layer":probe.layer.id(),"end":crate::views::point(probe.end),"blocker":blocker})
        }).collect();
        json!({
            "anchor":anchor(id), "grid_pitch_mm":mm(report.grid_pitch),"width_mm":mm(*report.width),
            "access_count":report.access_count,
            "escape_count":report.escape_count,
            "access_points":report.access_points.iter().map(|(layer, point)|
                json!({"layer":layer.id(),"position":crate::views::point(*point)})).collect::<Vec<_>>(),
            "straight_probes":probes,
        })
    }).collect();
    let rules = extracted.rule_report(
        p,
        b,
        &options,
        net,
        access.first().map(|report| report.grid_pitch),
    );
    let mut out = ToolOutput::new("Inspected builtin routing rules, terminal access and local clearance.", json!({
        "net":args.net,"board":board,"clearance_mm":mm(problem.rules.clearance),
        "clearance_source":"maximum board and net-class clearance plus builtin margin",
        "rules":rules,
        "probe_length_mm":mm(*reach),"unsupported_air_wires":extracted.unsupported.len(),
        "endpoint_count":ids.len(),"endpoints":endpoints,
    }))?.warn("Straight probes show local clearance only. A blocked probe does not rule out another path.");
    if let Some(image) = args.image {
        let (&id, report) = selected
            .get(image.endpoint)
            .zip(access.get(image.endpoint))
            .ok_or_else(|| {
                ToolError::invalid(
                    "image.endpoint must be an index in the returned endpoints array",
                )
            })?;
        let layer = match image.layer.as_deref() {
            Some(layer) => board_edit::copper_layer(layer)?,
            None => report
                .probes
                .first()
                .map(|probe| probe.layer)
                .or_else(|| {
                    options
                        .layers
                        .as_ref()
                        .and_then(|layers| layers.first().copied())
                })
                .unwrap_or(problem.copper_layers[0]),
        };
        if !problem.copper_layers.contains(&layer) {
            return Err(ToolError::invalid(
                "image.layer is not a copper layer of this board",
            ));
        }
        let (mut info, png) =
            diagnose_image::render(p, board, &extracted, id, report, layer, reach)?;
        info["endpoint"] = json!(image.endpoint);
        out.result["image"] = info;
        out = out.image(png);
    }
    Ok(out)
}

/// Explicit via dimensions in mm. The existing position and layers are kept
/// unless the same via is also listed in moves.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingViaSizeArgs {
    pub uuid: String,
    pub drill: f64,
    pub size: f64,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoutingAnchorKind {
    Via,
    Junction,
}

/// Move a connected via or junction to an absolute board position in mm.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingMoveArgs {
    pub kind: RoutingAnchorKind,
    pub uuid: String,
    pub position: crate::units::PointMm,
}

/// Replaces a connected path between existing anchors.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingTraceReplacementArgs {
    /// Trace UUIDs in path order, with the same layer and width.
    pub traces: Vec<String>,
    /// New interior points in board millimeters. For one trace, start at its
    /// `from` anchor in board_get. An empty list creates a straight trace.
    pub points: Vec<crate::units::PointMm>,
}

/// Builtin settings for a regional or post-edit trial.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingEditRouteArgs {
    /// At least one target net name or UUID.
    pub nets: Vec<String>,
    /// Allowed copper layers. Empty means all.
    #[serde(default)]
    pub layers: Vec<String>,
    /// Preferred trace width in mm, clamped to board and net-class minima.
    pub trace_width: Option<f64>,
    /// Optional short reduced-width escape at pads.
    pub neck_down: Option<PadNeckDownArgs>,
    /// Via drill override in mm.
    pub via_drill: Option<f64>,
    /// Via outer diameter override in mm.
    pub via_size: Option<f64>,
    /// Cost of a layer change as equivalent trace length in mm (default 2).
    pub via_cost: Option<f64>,
    /// Grid pitch override in mm.
    pub grid_pitch: Option<f64>,
    /// Include samples of rejected grid steps for failed requests.
    #[serde(default)]
    pub search_evidence: bool,
    /// Try extra exact-checked exits from nearby pads.
    #[serde(default)]
    pub pad_escape: bool,
    /// Penalize tiles contested by earlier successful rip-ups in this trial.
    #[serde(default)]
    pub congestion_history: bool,
    /// Net names or UUIDs to route first and protect from lower priority nets.
    #[serde(default)]
    pub critical_nets: Vec<String>,
}

impl RoutingEditRouteArgs {
    fn command(
        self,
        project: &Project,
        board: librepcb_core::project::BoardId,
    ) -> ToolResult<librepcb_editor::commands::Autoroute> {
        let mut command = board_edit::builtin_command(
            project,
            board,
            &board_edit::AutorouteArgs {
                nets: self.nets,
                layers: self.layers,
                trace_width: self.trace_width,
                via_drill: self.via_drill,
                via_size: self.via_size,
                grid_pitch: self.grid_pitch,
                ..Default::default()
            },
        )?;
        command.via_cost = via_cost(self.via_cost)?;
        command.search_evidence = self.search_evidence;
        command.neck_down = self.neck_down.map(PadNeckDownArgs::command).transpose()?;
        command.pad_escape = self.pad_escape;
        command.congestion_history = self.congestion_history;
        command.critical_nets = critical_nets(project, &self.critical_nets)?;
        Ok(command)
    }
}

/// Rectangular bounds for new copper, in board millimeters.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingRegionArgs {
    pub min: crate::units::PointMm,
    pub max: crate::units::PointMm,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingRegionPrepareArgs {
    pub board: Option<String>,
    pub region: RoutingRegionArgs,
    /// Optional smaller rectangle for removals. Defaults to the routing region.
    pub rip_up_region: Option<RoutingRegionArgs>,
    pub route: RoutingEditRouteArgs,
    /// UUIDs of existing traces and vias authorized for rip-up.
    #[serde(default)]
    pub rip_up: Vec<String>,
    pub expected_revision: Option<u64>,
}

pub fn prepare_regional_job(
    session: &Session,
    args: RoutingRegionPrepareArgs,
) -> ToolResult<RoutingJob> {
    use librepcb_editor::commands::RoutingBounds;

    let open = session.project()?;
    let project = open.project();
    write::check_revision(project, args.expected_revision)?;
    let board = board_edit::board_id(project, args.board.as_deref())?;
    let mut request = args.route.command(project, board)?;
    let bounds = |region: RoutingRegionArgs| -> ToolResult<RoutingBounds> {
        let min = region.min.to_point()?;
        let max = region.max.to_point()?;
        if min.x >= max.x || min.y >= max.y {
            return Err(ToolError::invalid(
                "A routing rectangle must have positive width and height.",
            ));
        }
        Ok(RoutingBounds { min, max })
    };
    request.region = Some(bounds(args.region)?);
    let rip_up_region = args.rip_up_region.map(bounds).transpose()?;
    let rip_up = args
        .rip_up
        .iter()
        .map(|uuid| crate::resolve::required_uuid(uuid, "trace or via"))
        .collect::<ToolResult<_>>()?;
    open.routing
        .prepare_regional(
            &open.editor,
            RegionalRoutingRequest {
                request,
                rip_up,
                rip_up_region,
            },
        )
        .map_err(routing_error)
}

/// Prepare one atomic connected edit for preview and validation.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingEditArgs {
    pub board: Option<String>,
    #[serde(default)]
    pub vias: Vec<RoutingViaSizeArgs>,
    #[serde(default)]
    pub moves: Vec<RoutingMoveArgs>,
    #[serde(default)]
    pub replacements: Vec<RoutingTraceReplacementArgs>,
    /// Try routing these nets after the edit, in the same reviewed candidate.
    pub route: Option<RoutingEditRouteArgs>,
    pub expected_revision: Option<u64>,
}

pub fn prepare_edit_job(session: &Session, args: RoutingEditArgs) -> ToolResult<RoutingJob> {
    use librepcb_core::geometry::TraceAnchor;
    use librepcb_editor::commands::{
        EditRoutingGeometry, RoutingAnchorMove, TraceReplacement, ViaDimensions,
    };

    let open = session.project()?;
    write::check_revision(open.project(), args.expected_revision)?;
    let board = board_edit::board_id(open.project(), args.board.as_deref())?;
    let vias = args
        .vias
        .into_iter()
        .map(|via| {
            Ok(ViaDimensions {
                uuid: crate::resolve::required_uuid(&via.uuid, "via")?,
                drill: board_edit::positive(via.drill, "drill")?,
                size: board_edit::positive(via.size, "size")?,
            })
        })
        .collect::<ToolResult<_>>()?;
    let moves = args
        .moves
        .into_iter()
        .map(|movement| {
            let uuid = crate::resolve::required_uuid(&movement.uuid, "anchor")?;
            let anchor = match movement.kind {
                RoutingAnchorKind::Via => TraceAnchor::Via(uuid),
                RoutingAnchorKind::Junction => TraceAnchor::Junction(uuid),
            };
            Ok(RoutingAnchorMove {
                anchor,
                position: movement.position.to_point()?,
            })
        })
        .collect::<ToolResult<_>>()?;
    let replacements = args
        .replacements
        .into_iter()
        .map(|replacement| {
            Ok(TraceReplacement {
                traces: replacement
                    .traces
                    .iter()
                    .map(|uuid| crate::resolve::required_uuid(uuid, "trace"))
                    .collect::<ToolResult<_>>()?,
                points: replacement
                    .points
                    .into_iter()
                    .map(crate::units::PointMm::to_point)
                    .collect::<ToolResult<_>>()?,
            })
        })
        .collect::<ToolResult<_>>()?;
    let edit = EditRoutingGeometry {
        board,
        vias,
        moves,
        replacements,
    };
    let job = if let Some(route) = args.route {
        let command = route.command(open.project(), board)?;
        open.routing
            .prepare_edit_and_route(&open.editor, edit, command)
    } else {
        open.routing.prepare_edit(&open.editor, edit)
    };
    job.map_err(routing_error)
}
