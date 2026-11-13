//! MCP arguments for a checked batch of explicit copper.

use std::collections::BTreeMap;

use librepcb_editor::commands::{AddVia, NetRef};
use librepcb_editor::routing_service::{
    CopperEndpoint, CopperPlan, PlannedPlacement, PlannedTrace, RoutingJob,
};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{RoutingEditRouteArgs, board_edit, candidate_id, routing_error, write};
use crate::error::{ToolError, ToolResult};
use crate::resolve;
use crate::session::Session;
use crate::units::{PointMm, angle};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingCopperArgs {
    /// Board name, index or UUID; defaults to the primary board.
    pub board: Option<String>,
    /// Refine an existing candidate on the same unchanged board, even if it
    /// failed validation. Review and apply the combined patch separately.
    pub base_candidate: Option<String>,
    /// Through vias. Labels may be used by traces in the same request.
    #[serde(default)]
    pub vias: Vec<PlannedViaArg>,
    /// Explicit same-layer paths, with optional corners in board mm.
    #[serde(default)]
    pub traces: Vec<PlannedTraceArg>,
    /// UUIDs of existing traces to replace in the same checked candidate.
    #[serde(default)]
    pub remove_traces: Vec<String>,
    /// UUIDs of existing vias to replace. List all their traces in remove_traces.
    #[serde(default)]
    pub remove_vias: Vec<String>,
    /// Moves and rotations of explicitly selected, unlocked board devices.
    #[serde(default)]
    pub placements: Vec<PlannedPlacementArg>,
    /// Continue builtin routing on the staged copper for these explicit target nets.
    pub route: Option<RoutingEditRouteArgs>,
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlannedPlacementArg {
    /// Component designator or UUID.
    pub component: String,
    /// Absolute board position in mm.
    pub position: Option<PointMm>,
    /// Absolute board rotation in degrees.
    pub rotation: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlannedViaArg {
    pub label: String,
    pub net: String,
    pub position: PointMm,
    pub drill: Option<f64>,
    pub size: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlannedViaRef {
    pub via: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PlannedEndpointArg {
    /// A via label declared in this request, e.g. {"via":"launch"}.
    NewVia(PlannedViaRef),
    /// Pad name, existing anchor UUID or coordinate in mm.
    Existing(board_edit::EndpointArg),
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlannedTraceArg {
    pub from: PlannedEndpointArg,
    pub to: PlannedEndpointArg,
    #[serde(default)]
    pub points: Vec<PointMm>,
    pub layer: Option<String>,
    pub width: Option<f64>,
    /// Net name or UUID; required when both ends are coordinates.
    pub net: Option<String>,
}

pub fn prepare_copper_job(session: &Session, args: RoutingCopperArgs) -> ToolResult<RoutingJob> {
    let total = args.vias.len()
        + args.traces.len()
        + args.remove_traces.len()
        + args.remove_vias.len()
        + args.placements.len()
        + args
            .traces
            .iter()
            .map(|trace| trace.points.len())
            .sum::<usize>();
    if total > 512 {
        return Err(ToolError::invalid(
            "Supply at most 512 placements, vias, traces, removals and intermediate points combined.",
        ));
    }
    if total == 0 && (args.base_candidate.is_none() || args.route.is_none()) {
        return Err(ToolError::invalid(
            "Supply a copper edit or a base_candidate with an explicit route.",
        ));
    }
    let open = session.project()?;
    let project = open.project();
    write::check_revision(project, args.expected_revision)?;
    let board = board_edit::board_id(project, args.board.as_deref())?;
    let route = args
        .route
        .map(|args| args.command(project, board))
        .transpose()?;
    let base_candidate = args
        .base_candidate
        .as_deref()
        .map(candidate_id)
        .transpose()?;
    let remove_traces = args
        .remove_traces
        .iter()
        .map(|id| resolve::required_uuid(id, "trace"))
        .collect::<ToolResult<_>>()?;
    let remove_vias = args
        .remove_vias
        .iter()
        .map(|id| resolve::required_uuid(id, "via"))
        .collect::<ToolResult<_>>()?;
    let placements = args
        .placements
        .into_iter()
        .map(|arg| {
            Ok(PlannedPlacement {
                component: resolve::component(project, &arg.component)?.0,
                position: arg.position.map(PointMm::to_point).transpose()?,
                rotation: arg.rotation.map(angle).transpose()?,
            })
        })
        .collect::<ToolResult<_>>()?;
    let net = |name: &str| resolve::net(project, name).map(|(id, _)| NetRef::Id(id));
    let mut labels = BTreeMap::new();
    let mut vias = Vec::with_capacity(args.vias.len());
    for via in args.vias {
        if via.label.trim().is_empty() || labels.insert(via.label.clone(), vias.len()).is_some() {
            return Err(ToolError::invalid(format!(
                "Invalid or duplicate via label: {:?}",
                via.label
            )));
        }
        vias.push(AddVia {
            board: Some(board),
            position: via.position.to_point()?,
            net: Some(net(&via.net)?),
            start_layer: None,
            end_layer: None,
            drill_diameter: via
                .drill
                .map(|d| board_edit::positive(d, "drill"))
                .transpose()?,
            size: via
                .size
                .map(|s| board_edit::positive(s, "size"))
                .transpose()?,
            exposure: None,
        });
    }
    let endpoint = |arg| match arg {
        PlannedEndpointArg::NewVia(arg) => labels
            .get(&arg.via)
            .copied()
            .map(CopperEndpoint::NewVia)
            .ok_or_else(|| ToolError::invalid(format!("Unknown via label: {}", arg.via))),
        PlannedEndpointArg::Existing(arg) => {
            board_edit::endpoint(project, board, &arg).map(CopperEndpoint::Existing)
        }
    };
    let traces = args
        .traces
        .into_iter()
        .map(|t| {
            Ok(PlannedTrace {
                start: endpoint(t.from)?,
                end: endpoint(t.to)?,
                points: t
                    .points
                    .into_iter()
                    .map(PointMm::to_point)
                    .collect::<ToolResult<_>>()?,
                layer: t
                    .layer
                    .as_deref()
                    .map(board_edit::copper_layer)
                    .transpose()?,
                width: t
                    .width
                    .map(|w| board_edit::positive(w, "width"))
                    .transpose()?,
                net: t.net.as_deref().map(net).transpose()?,
            })
        })
        .collect::<ToolResult<_>>()?;
    open.routing
        .prepare_copper_and_route(
            &open.editor,
            CopperPlan {
                board,
                remove_traces,
                remove_vias,
                placements,
                vias,
                traces,
            },
            base_candidate,
            route,
        )
        .map_err(routing_error)
}
