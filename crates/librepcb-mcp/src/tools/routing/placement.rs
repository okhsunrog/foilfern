//! MCP arguments for comparing a small, explicitly permitted placement set.

use std::time::Duration;

use librepcb_core::project::PlaneId;
use librepcb_core::types::Uuid;
use librepcb_editor::routing_service::{
    PlacementJob, PlacementRequest, PlacementResults, PlacementSites, PlacementTarget,
    PlacementTrace, PlacementVia, RoutingError,
};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{
    RoutingCopperArgs, RoutingRegionArgs, TracePreferenceArgs, board_edit, routing_error, write,
};
use crate::error::{ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::resolve;
use crate::session::Session;
use crate::units::{PointMm, angle};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingPlacementArgs {
    /// One or two explicitly movable devices.
    pub targets: Vec<PlacementTargetArgs>,
    /// Shared explicit copper template. Omit placements and base_candidate here.
    pub copper: RoutingCopperArgs,
    /// Footprint-local via coordinates or required via-center contact with a same-net plane.
    #[serde(default)]
    pub via_rules: Vec<PlacementViaArgs>,
    /// Required explicit pad-to-pad trace paths, in reporting order.
    #[serde(default)]
    pub trace_requirements: Vec<PlacementTraceArgs>,
    /// Retain the best 1–3 candidates for review. Default 2.
    pub max_candidates: Option<usize>,
    /// Cooperative time budget. Default 30000; maximum 120000 ms.
    pub max_time_ms: Option<u64>,
    /// Background search only. Try up to this many joint poses (1–64, default 16).
    pub max_trials: Option<usize>,
    /// Background only. Try bounded shifts prompted by exact staged copper conflicts.
    #[serde(default)]
    pub avoid_blockers: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlacementTargetArgs {
    /// Component designator or instance UUID.
    pub component: String,
    /// Allowed absolute footprint centers in board millimeters.
    pub positions: Option<Vec<PointMm>>,
    /// Alternatively generate centers in this board rectangle in a background search.
    pub region: Option<RoutingRegionArgs>,
    /// Pitch in mm for the grid starting at region.min. Required with region.
    pub pitch_mm: Option<f64>,
    /// Allowed absolute rotations in degrees. Omit to preserve the current rotation.
    pub rotations: Option<Vec<f64>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlacementViaArgs {
    /// Label of a through via in copper.vias.
    pub via: String,
    /// Interpret the template's via position in this moving footprint's local coordinates.
    pub component: Option<String>,
    /// Require the via center to fall inside the rebuilt connected copper of this plane UUID.
    pub plane: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlacementTraceArgs {
    pub requirement: TracePreferenceArgs,
    /// Include this path's length in the sum minimized among feasible poses.
    #[serde(default)]
    pub minimize: bool,
}

pub fn prepare_placement(
    session: &Session,
    args: RoutingPlacementArgs,
) -> ToolResult<PlacementJob> {
    if args.avoid_blockers
        || args.max_trials.is_some()
        || args.targets.iter().any(|target| target.region.is_some())
    {
        return Err(ToolError::invalid(
            "Use routing_placement_start for regions or trial limits.",
        ));
    }
    let request = placement_request(session, args)?;
    let open = session.project()?;
    open.routing
        .prepare_placement_comparison(&open.editor, request)
        .map_err(routing_error)
}

fn placement_request(
    session: &Session,
    args: RoutingPlacementArgs,
) -> ToolResult<PlacementRequest> {
    if !(1..=2).contains(&args.targets.len())
        || args.via_rules.len() > 16
        || args.trace_requirements.len() > 16
    {
        return Err(ToolError::invalid(
            "Choose 1–2 targets and at most 16 via rules and 16 trace requirements.",
        ));
    }

    if args.copper.base_candidate.is_some() || !args.copper.placements.is_empty() {
        return Err(ToolError::invalid(
            "Placement comparison supplies its own placements on the live board baseline.",
        ));
    }
    let open = session.project()?;
    let project = open.project();
    write::check_revision(project, args.copper.expected_revision)?;
    let board_id = board_edit::board_id(project, args.copper.board.as_deref())?;
    let board = project
        .board(board_id)
        .ok_or_else(|| ToolError::not_found("Candidate board is missing."))?;
    let targets = args
        .targets
        .into_iter()
        .map(|target| {
            let component = resolve::component(project, &target.component)?.0;
            let device = board.device(component).ok_or_else(|| {
                ToolError::not_found(format!("Device {} is not on the board.", target.component))
            })?;
            if target
                .positions
                .as_ref()
                .is_some_and(|points| points.len() > 64)
                || target
                    .rotations
                    .as_ref()
                    .is_some_and(|rotations| rotations.len() > 64)
            {
                return Err(ToolError::invalid(
                    "Supply at most 64 positions or rotations per target.",
                ));
            }
            let sites = match (target.positions, target.region, target.pitch_mm) {
                (Some(points), None, None) => PlacementSites::Explicit(
                    points
                        .into_iter()
                        .map(PointMm::to_point)
                        .collect::<ToolResult<_>>()?,
                ),
                (None, Some(region), Some(pitch)) => PlacementSites::Grid {
                    bounds: region.bounds()?,
                    pitch: board_edit::positive(pitch, "pitch_mm")?,
                },
                _ => {
                    return Err(ToolError::invalid(
                        "Choose positions or a region with pitch_mm for each target.",
                    ));
                }
            };
            Ok(PlacementTarget {
                component,
                sites,
                rotations: target
                    .rotations
                    .map(|values| values.into_iter().map(angle).collect::<ToolResult<_>>())
                    .transpose()?
                    .unwrap_or_else(|| vec![device.rotation()]),
            })
        })
        .collect::<ToolResult<_>>()?;
    let relative_vias = args
        .via_rules
        .into_iter()
        .map(|rule| {
            let via = args
                .copper
                .vias
                .iter()
                .position(|via| via.label == rule.via)
                .ok_or_else(|| ToolError::invalid(format!("Unknown via label: {}", rule.via)))?;
            let component = rule
                .component
                .as_deref()
                .map(|name| resolve::component(project, name).map(|(id, _)| id))
                .transpose()?;
            let plane = rule
                .plane
                .as_deref()
                .map(|id| resolve::required_uuid(id, "plane").map(PlaneId))
                .transpose()?;
            Ok(PlacementVia {
                via,
                component,
                plane,
            })
        })
        .collect::<ToolResult<_>>()?;
    let traces = args
        .trace_requirements
        .into_iter()
        .map(|args| {
            Ok(PlacementTrace {
                path: args.requirement.resolve(project, board, false)?,
                minimize: args.minimize,
            })
        })
        .collect::<ToolResult<_>>()?;
    let (copper, route) = args.copper.into_plan(project, board_id)?;
    Ok(PlacementRequest {
        targets,
        copper,
        route,
        relative_vias,
        traces,
        max_candidates: args.max_candidates.unwrap_or(2),
        max_time: Duration::from_millis(args.max_time_ms.unwrap_or(30_000)),
        avoid_blockers: args.avoid_blockers,
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlacementStatusArgs {
    /// Search UUID returned by routing_placement_start.
    pub search: String,
    /// Request cancellation and retain completed checked candidates.
    #[serde(default)]
    pub cancel: bool,
}

pub fn start(
    session: &mut Session,
    args: RoutingPlacementArgs,
) -> ToolResult<(Uuid, ToolOutput, PlacementJob)> {
    let max_trials = args.max_trials.unwrap_or(16);
    let request = placement_request(session, args)?;
    let open = session.project_mut()?;
    let (status, job) = open
        .routing
        .start_placement_search(&open.editor, request, max_trials)
        .map_err(routing_error)?;
    Ok((
        status.search,
        ToolOutput::new("Placement search started.", status)?,
        job,
    ))
}

pub fn status(session: &Session, args: PlacementStatusArgs) -> ToolResult<ToolOutput> {
    let search = resolve::required_uuid(&args.search, "placement search")?;
    let status = session
        .project()?
        .routing
        .placement_search_status(search, args.cancel)
        .map_err(routing_error)?;
    ToolOutput::new("Placement search status.", status)
}

pub fn finish(session: &mut Session, id: Uuid, result: Result<PlacementResults, RoutingError>) {
    if let Ok(open) = session.project_mut() {
        open.routing
            .finish_placement_search(open.editor.project(), id, result);
    }
}

pub fn run_placement(job: PlacementJob) -> ToolResult<PlacementResults> {
    job.run().map_err(routing_error)
}

pub fn finish_placement(
    session: &mut Session,
    results: PlacementResults,
) -> ToolResult<ToolOutput> {
    let open = session.project_mut()?;
    let report = open
        .routing
        .retain_placement_comparison(open.editor.project(), results)
        .map_err(routing_error)?;
    let mut output = ToolOutput::new(
        "Compared authorized placement and routing trials. Live board unchanged.",
        &report,
    )?;
    if report.recommended.is_none() {
        output = output.partial().warn("No placement passed all checks.");
    }
    if report.timed_out {
        output = output.partial().warn("Time budget reached between trials.");
    }
    Ok(output)
}
