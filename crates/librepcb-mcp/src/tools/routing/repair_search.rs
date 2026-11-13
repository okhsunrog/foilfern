//! The MCP adapter for detached, pollable local repair searches.

use std::time::Duration;

use librepcb_core::types::Uuid;
use librepcb_editor::routing_service::{
    RepairSearchJob, RepairSearchRequest, RepairSearchResults, RoutingError,
};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{RoutingEditRouteArgs, RoutingRegionArgs, routing_error};
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
    /// Bounds for existing copper authorized for removal in mm.
    pub rip_up_region: RoutingRegionArgs,
    /// Bounds for new neighbor copper. Defaults to rip_up_region.
    pub neighbor_region: Option<RoutingRegionArgs>,
    /// One to eight groups of exact trace and via UUIDs, explicitly authorized for removal.
    pub groups: Vec<Vec<String>>,
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
    pub expected_revision: Option<u64>,
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
    route.region = Some(args.region.bounds()?);
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
    if !(1..=8).contains(&args.groups.len())
        || args
            .groups
            .iter()
            .any(|group| !(1..=16).contains(&group.len()))
    {
        return Err(ToolError::invalid(
            "Supply 1–8 groups, each with 1–16 copper UUIDs.",
        ));
    }
    let groups = args
        .groups
        .into_iter()
        .map(|group| {
            group
                .into_iter()
                .map(|id| crate::resolve::required_uuid(&id, "trace or via"))
                .collect()
        })
        .collect::<ToolResult<_>>()?;
    let (status, job) = open
        .routing
        .start_repair_search(
            &open.editor,
            RepairSearchRequest {
                target,
                route,
                neighbor_route,
                joint_route,
                rip_up_region: args.rip_up_region.bounds()?,
                groups,
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
