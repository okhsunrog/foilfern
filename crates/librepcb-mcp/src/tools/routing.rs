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

mod diagnose_image;

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
            "Validation is unacceptable: inspect checks, netlist, new DRC errors and unrouted regressions.",
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
    /// Copper layers allowed for routing. Empty means all.
    #[serde(default)]
    pub layers: Vec<String>,
    /// Length of each straight clearance probe in mm (default 0.6, max 5).
    pub probe_length: Option<f64>,
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
    let options = board_edit::builtin_command(
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

/// Optional builtin trial on the edited board.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingEditRouteArgs {
    /// At least one target net name or UUID.
    pub nets: Vec<String>,
    /// Allowed copper layers. Empty means all.
    #[serde(default)]
    pub layers: Vec<String>,
    /// Trace width override in mm.
    pub trace_width: Option<f64>,
    /// Via drill override in mm.
    pub via_drill: Option<f64>,
    /// Via outer diameter override in mm.
    pub via_size: Option<f64>,
    /// Grid pitch override in mm.
    pub grid_pitch: Option<f64>,
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
        let command = board_edit::builtin_command(
            open.project(),
            board,
            &board_edit::AutorouteArgs {
                nets: route.nets,
                layers: route.layers,
                trace_width: route.trace_width,
                via_drill: route.via_drill,
                via_size: route.via_size,
                grid_pitch: route.grid_pitch,
                ..Default::default()
            },
        )?;
        open.routing
            .prepare_edit_and_route(&open.editor, edit, command)
    } else {
        open.routing.prepare_edit(&open.editor, edit)
    };
    job.map_err(routing_error)
}
