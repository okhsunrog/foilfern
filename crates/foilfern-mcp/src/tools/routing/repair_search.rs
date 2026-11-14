//! The MCP adapter for detached, pollable local repair searches.

use std::collections::BTreeMap;
use std::time::Duration;

use foilfern_core::geometry::TraceAnchor;
use foilfern_core::project::Project;
use foilfern_core::project::board::Board;
use foilfern_core::types::Uuid;
use foilfern_editor::routing_service::{
    RepairLaunch, RepairLaunchSearch, RepairSearchJob, RepairSearchRequest, RepairSearchResults,
    RoutingError,
};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{RoutingEditRouteArgs, RoutingRegionArgs, TracePreferenceArgs, routing_error};
use crate::error::{ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::session::Session;
use crate::tools::{board_edit, write};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepairSearchArgs {
    pub board: Option<String>,
    /// Name or UUID of the net to improve.
    pub target: String,
    /// Bounds for new target-net copper in mm.
    pub region: RoutingRegionArgs,
    /// Bounds for authorized removals in mm. Required when groups are supplied.
    pub rip_up_region: Option<RoutingRegionArgs>,
    /// Bounds for new neighbor copper. Defaults to rip_up_region.
    pub neighbor_region: Option<RoutingRegionArgs>,
    /// Up to eight groups of authorized trace/via UUIDs. May be empty for launch search.
    #[serde(default)]
    pub groups: Vec<Vec<String>>,
    /// Optional exact replacements with two retained endpoints for each named group index.
    #[serde(default)]
    pub neighbor_anchors: Vec<NeighborAnchorsArgs>,
    /// Target routing options. The net comes from target.
    #[serde(default)]
    pub route: RoutingEditRouteArgs,
    /// Neighbor routing options. The nets come from the selected copper.
    #[serde(default)]
    pub neighbor_route: RoutingEditRouteArgs,
    /// Also try routing the target and neighbors together, with these builtin settings.
    pub joint_route: Option<RoutingEditRouteArgs>,
    /// Limit on distinct removals per trial, 1–16. Default 12.
    pub max_objects: Option<usize>,
    /// Total trial limit across both strategies, 1–72. Default 24.
    pub max_trials: Option<usize>,
    /// Maximum number of checked candidates to retain, 1–3. Default 2.
    pub max_candidates: Option<usize>,
    /// Cooperative time budget in ms, 1–300000. Default 120000.
    pub max_time_ms: Option<u64>,
    /// Up to 16 hard pad-to-pad constraints for the completed repair.
    #[serde(default)]
    pub trace_requirements: Vec<TracePreferenceArgs>,
    /// Add one explicit straight target pad-to-through-via lead before routing the target.
    pub target_launch: Option<RepairLaunchArgs>,
    /// Generate and compare nearby pad-to-via leads. Mutually exclusive with target_launch.
    pub target_launch_search: Option<RepairLaunchSearchArgs>,
    /// Also search for a pad-to-via lead on an authorized neighbor in joint trials.
    pub neighbor_launch_search: Option<RepairLaunchSearchArgs>,
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NeighborAnchorsArgs {
    /// Zero-based index in groups.
    pub group: usize,
    pub from: NeighborAnchorArg,
    pub to: NeighborAnchorArg,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NeighborAnchorArg {
    /// A retained junction UUID.
    Junction(String),
    /// A retained via UUID.
    Via(String),
    /// A standalone board pad UUID.
    BoardPad(String),
    /// A footprint pad, e.g. R1.2.
    Pad(crate::tools::trace_path::PadAddress),
}

impl NeighborAnchorArg {
    fn resolve(&self, project: &Project, board: &Board) -> ToolResult<TraceAnchor> {
        use crate::resolve::required_uuid;
        Ok(match self {
            Self::Junction(id) => TraceAnchor::Junction(required_uuid(id, "junction")?),
            Self::Via(id) => TraceAnchor::Via(required_uuid(id, "via")?),
            Self::BoardPad(id) => TraceAnchor::Pad(required_uuid(id, "board pad")?),
            Self::Pad(pad) => {
                let (device, pad) =
                    crate::tools::trace_path::footprint_pad_id(project, board, pad)?;
                TraceAnchor::FootprintPad {
                    device: device.0,
                    pad,
                }
            }
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepairLaunchArgs {
    /// Name of a target-net footprint pad, e.g. U1.12.
    pub from: crate::tools::trace_path::PadAddress,
    pub layer: String,
    pub position: crate::units::PointMm,
    pub width: f64,
    pub drill: Option<f64>,
    pub size: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepairLaunchSearchArgs {
    /// Name of a footprint pad, e.g. U1.12.
    pub from: crate::tools::trace_path::PadAddress,
    pub layer: String,
    /// Search radius in mm. Positive, at most 1.5 (default 1.0).
    pub radius: Option<f64>,
    /// Number of clear sites to evaluate, 1–8 (default 6).
    pub max_sites: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepairStatusArgs {
    pub search: String,
    /// Request cancellation, keeping candidates whose validation has completed.
    #[serde(default)]
    pub cancel: bool,
}

pub fn start(
    session: &mut Session,
    args: RepairSearchArgs,
) -> ToolResult<(Uuid, ToolOutput, RepairSearchJob)> {
    let open = session.project_mut()?;
    let project = open.editor.project();
    write::check_revision(project, args.expected_revision)?;
    let board = board_edit::board_id(project, args.board.as_deref())?;
    let (target, _) = crate::resolve::net(project, &args.target)?;
    if !args.route.nets.is_empty()
        || !args.neighbor_route.nets.is_empty()
        || args
            .joint_route
            .as_ref()
            .is_some_and(|route| !route.nets.is_empty())
    {
        return Err(ToolError::invalid(
            "Use target and groups to select the nets for repair.",
        ));
    }
    let mut route = args.route.command(project, board)?;
    let region = args.region.bounds()?;
    route.region = Some(region);
    let rip_up_region = match args.rip_up_region {
        Some(bounds) => bounds.bounds()?,
        None if args.groups.is_empty() => region,
        None => {
            return Err(ToolError::invalid(
                "Supply rip_up_region when authorizing removals.",
            ));
        }
    };
    let mut neighbor_route = args.neighbor_route.command(project, board)?;
    let joint_route = args
        .joint_route
        .map(|route| route.command(project, board))
        .transpose()?;
    neighbor_route.region = args
        .neighbor_region
        .as_ref()
        .map(RoutingRegionArgs::bounds)
        .transpose()?;
    if args.groups.len() > 8
        || (args.groups.is_empty() && args.target_launch_search.is_none())
        || args
            .groups
            .iter()
            .any(|group| !(1..=16).contains(&group.len()))
    {
        return Err(ToolError::invalid(
            "Supply 1–8 groups with 1–16 copper UUIDs each, or use target_launch_search without removals.",
        ));
    }
    let groups: Vec<_> = args
        .groups
        .into_iter()
        .map(|group| {
            group
                .into_iter()
                .map(|id| crate::resolve::required_uuid(&id, "trace or via"))
                .collect()
        })
        .collect::<ToolResult<_>>()?;
    if args.trace_requirements.len() > 16 {
        return Err(ToolError::invalid("Supply at most 16 trace requirements."));
    }
    let native_board = project
        .board(board)
        .ok_or_else(|| ToolError::not_found("Candidate board is missing."))?;
    let mut neighbor_anchors = BTreeMap::new();
    if args.neighbor_anchors.len() > groups.len() {
        return Err(ToolError::invalid(
            "Supply at most one anchor pair per group.",
        ));
    }
    for pair in args.neighbor_anchors {
        let ends = [
            pair.from.resolve(project, native_board)?,
            pair.to.resolve(project, native_board)?,
        ];
        if neighbor_anchors.insert(pair.group, ends).is_some() {
            return Err(ToolError::invalid(
                "Supply at most one anchor pair per group.",
            ));
        }
    }
    let trace_requirements = args
        .trace_requirements
        .iter()
        .map(|path| path.resolve(project, native_board, false))
        .collect::<ToolResult<_>>()?;
    let target_launch = args
        .target_launch
        .map(|launch| -> ToolResult<_> {
            Ok(RepairLaunch {
                from: crate::tools::trace_path::footprint_pad_id(
                    project,
                    native_board,
                    &launch.from,
                )?,
                layer: board_edit::copper_layer(&launch.layer)?,
                position: launch.position.to_point()?,
                width: board_edit::positive(launch.width, "width")?,
                drill: launch
                    .drill
                    .map(|d| board_edit::positive(d, "drill"))
                    .transpose()?,
                size: launch
                    .size
                    .map(|s| board_edit::positive(s, "size"))
                    .transpose()?,
            })
        })
        .transpose()?;
    let resolve_launch_search = |search: RepairLaunchSearchArgs| -> ToolResult<_> {
        Ok(RepairLaunchSearch {
            from: crate::tools::trace_path::footprint_pad_id(project, native_board, &search.from)?,
            layer: board_edit::copper_layer(&search.layer)?,
            radius: board_edit::positive(search.radius.unwrap_or(1.0), "launch search radius")?,
            max_sites: search.max_sites.unwrap_or(6),
        })
    };
    let target_launch_search = args
        .target_launch_search
        .map(resolve_launch_search)
        .transpose()?;
    let neighbor_launch_search = args
        .neighbor_launch_search
        .map(resolve_launch_search)
        .transpose()?;
    let (status, job) = open
        .routing
        .start_repair_search(
            &open.editor,
            RepairSearchRequest {
                target,
                route,
                neighbor_route,
                joint_route,
                rip_up_region,
                groups,
                neighbor_anchors,
                trace_requirements,
                target_launch,
                target_launch_search,
                neighbor_launch_search,
                max_objects: args.max_objects.unwrap_or(12),
                max_trials: args.max_trials.unwrap_or(24),
                max_candidates: args.max_candidates.unwrap_or(2),
                time_budget: Duration::from_millis(args.max_time_ms.unwrap_or(120_000)),
            },
        )
        .map_err(routing_error)?;
    Ok((
        status.search,
        ToolOutput::new("Local repair search started.", status)?,
        job,
    ))
}

pub fn status(session: &Session, args: RepairStatusArgs) -> ToolResult<ToolOutput> {
    let search = crate::resolve::required_uuid(&args.search, "repair search")?;
    let status = session
        .project()?
        .routing
        .repair_search_status(search, args.cancel)
        .map_err(routing_error)?;
    ToolOutput::new("Local repair search status.", status)
}

pub fn finish(session: &mut Session, id: Uuid, result: Result<RepairSearchResults, RoutingError>) {
    if let Ok(open) = session.project_mut() {
        open.routing
            .finish_repair_search(open.editor.project(), id, result);
    }
}
