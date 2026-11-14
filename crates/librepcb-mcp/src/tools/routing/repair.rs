//! Read-only regional repair suggestions from exact endpoint clearance probes.

use std::collections::BTreeSet;

use librepcb_core::project::board::Board;
use librepcb_core::project::{BoardId, NetSignalId, Project};
use librepcb_core::types::{Length, PositiveLength, Uuid};
use librepcb_editor::commands::autoroute::{
    AccessBlocker, Autoroute, ExtractedProblem, RoutingConnection, RoutingObject,
    RoutingSearchBlocker, UnroutedAirWire, diagnose_access,
};
use librepcb_editor::routing_service::{
    RegionalRepairProposal, RoutingCandidateDetails, RoutingService,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::RoutingRegionArgs;
use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::tools::layout;

const MAX_PROPOSALS: usize = 8;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepairArgs {
    /// Zero-based supported air wire index for this net (default 0).
    #[serde(default)]
    pub air_wire: usize,
    /// Region for new copper. Both original air-wire endpoints must fit.
    pub region: RoutingRegionArgs,
    /// Smaller region for existing copper to consider for removal.
    pub rip_up_region: RoutingRegionArgs,
    /// Only consider altering copper on these neighboring nets.
    pub editable_nets: Vec<String>,
    /// Optional builtin or staged candidate from this revision with search_evidence enabled.
    pub evidence_candidate: Option<String>,
    /// Max number of existing traces and vias in one suggestion, 1–16 (default 8).
    pub max_objects: Option<usize>,
}

fn candidate_evidence<'a>(
    service: &'a RoutingService,
    project: &Project,
    board: BoardId,
    candidate: &str,
) -> ToolResult<Vec<&'a UnroutedAirWire>> {
    let id = super::candidate_id(candidate)?;
    let info = service.candidate(id).map_err(super::routing_error)?.info();
    if info.board != board {
        return Err(ToolError::invalid(
            "The evidence candidate is for a different board.",
        ));
    }
    if service
        .is_stale(project, id)
        .map_err(super::routing_error)?
    {
        return Err(ToolError::new(
            ErrorKind::StaleRevision,
            "The evidence candidate is stale.",
        ));
    }
    match &info.details {
        RoutingCandidateDetails::Builtin { routing, .. } => {
            Ok(routing.failed_requests.iter().collect())
        }
        RoutingCandidateDetails::Staged { staged } => Ok(staged
            .trials
            .iter()
            .flat_map(|trial| &trial.failed_requests)
            .collect()),
        _ => Err(ToolError::invalid(
            "Use a builtin or staged routing candidate for search evidence.",
        )),
    }
}

pub(super) fn suggest(
    project: &Project,
    board: &Board,
    net: NetSignalId,
    options: &Autoroute,
    extracted: &ExtractedProblem,
    service: &RoutingService,
    (reach, args): (PositiveLength, RepairArgs),
) -> ToolResult<Value> {
    let region = args.region.bounds()?.region()?;
    let edits = args.rip_up_region.bounds()?;
    if !region.contains(edits.min, Length::ZERO) || !region.contains(edits.max, Length::ZERO) {
        return Err(ToolError::invalid("rip_up_region must lie inside region."));
    }
    let max_objects = args.max_objects.unwrap_or(8);
    if !(1..=16).contains(&max_objects) {
        return Err(ToolError::invalid("max_objects must be between 1 and 16."));
    }
    let allowed: BTreeSet<_> = args
        .editable_nets
        .iter()
        .map(|name| crate::resolve::net(project, name).map(|(id, _)| id))
        .collect::<ToolResult<_>>()?;
    let mut connections = extracted
        .problem
        .connections
        .iter()
        .filter_map(|c| match c {
            RoutingConnection::Terminals(a, b) => Some([*a, *b]),
            RoutingConnection::Net(_) => None,
        });
    let endpoints = connections.nth(args.air_wire).ok_or_else(|| {
        ToolError::invalid("air_wire must select a supported unrouted connection.")
    })?;
    let positions = endpoints.map(|id| extracted.problem.terminals[id.0].position);
    let native_anchors = endpoints.map(|id| extracted.terminal_anchors[id.0]);
    let anchors = std::array::from_fn::<_, 2, _>(|i| {
        layout::trace_anchor(project, board, Some(positions[i]), native_anchors[i])
    });
    if positions.iter().any(|&p| !region.contains(p, Length::ZERO)) {
        return Ok(json!({"air_wire":args.air_wire,"anchors":anchors,
            "outcome":"outside_region","proposals":[]}));
    }
    let reports = diagnose_access(&extracted.problem, &endpoints, reach)
        .map_err(librepcb_editor::Error::from)?;
    let evidence = args
        .evidence_candidate
        .as_deref()
        .map(|id| candidate_evidence(service, project, board.id(), id))
        .transpose()?
        .unwrap_or_default();
    let mut probe_blockers = Vec::new();
    for probe in reports.iter().flat_map(|report| &report.probes) {
        let object = match probe.blocker {
            Some(AccessBlocker::Obstacle(index)) => extracted.obstacle_objects[index],
            Some(AccessBlocker::Hole(index)) => extracted.hole_objects[index],
            Some(AccessBlocker::Terminal(id)) => {
                RoutingObject::from_anchor(extracted.terminal_anchors[id.0])
            }
            Some(AccessBlocker::BoardEdge) | None => continue,
        };
        if !probe_blockers.contains(&object) {
            probe_blockers.push(object);
        }
    }
    let mut search_blockers = Vec::new();
    for failed in evidence {
        if failed.net != net
            || ([failed.p1, failed.p2] != native_anchors
                && [failed.p2, failed.p1] != native_anchors)
        {
            continue;
        }
        for sample in &failed.search_evidence {
            if let Some(RoutingSearchBlocker::Existing { object }) = sample.blocker
                && !search_blockers.contains(&object)
            {
                search_blockers.push(object);
            }
        }
    }
    let blockers: Vec<_> = probe_blockers
        .iter()
        .chain(&search_blockers)
        .copied()
        .collect();
    let proposals = RegionalRepairProposal::suggest(
        project,
        board,
        net,
        edits,
        &blockers,
        &allowed,
        max_objects,
    )?;
    let proposal_count = proposals.len();
    let regional_request = |rip_up: &BTreeSet<Uuid>, nets: Vec<NetSignalId>| {
        json!({
            "board":board.id(),
            "expected_revision":project.revision(),
            "region":args.region,
            "rip_up_region":args.rip_up_region,
            "rip_up":rip_up,
            "improve_net":net,
            "route":{
                "nets":nets,
                "layers":options.layers.as_ref().map(|layers| layers.iter().map(|layer| layer.id()).collect::<Vec<_>>()).unwrap_or_default(),
                "trace_width":options.trace_width.map(|value| value.to_mm()),
                "grid_pitch":options.grid_pitch.map(|value| value.to_mm()),
                "via_drill":options.via_drill.map(|value| value.to_mm()),
                "via_size":options.via_size.map(|value| value.to_mm()),
                "via_cost":options.via_cost.map(|value| value.to_mm()),
                "pad_escape":options.pad_escape,
                "search_evidence":true,
                "detour_search": options.detour_search,
            },
        })
    };
    let combined = (proposal_count > 1)
        .then(|| {
            let rip_up: BTreeSet<_> = proposals
                .iter()
                .flat_map(|p| p.traces.iter().chain(&p.vias))
                .copied()
                .collect();
            (rip_up.len() <= max_objects).then(|| {
                let nets = BTreeSet::from_iter(
                    std::iter::once(net).chain(proposals.iter().map(|p| p.net)),
                );
                regional_request(&rip_up, nets.into_iter().collect())
            })
        })
        .flatten();
    let proposals: Vec<_> = proposals
        .into_iter()
        .take(MAX_PROPOSALS)
        .map(|proposal| {
            let rip_up = proposal.traces.union(&proposal.vias).copied().collect();
            let anchors: Vec<_> = proposal
                .anchors
                .iter()
                .map(|&(anchor, position)| {
                    layout::trace_anchor(project, board, Some(position), anchor)
                })
                .collect();
            json!({
                "strategy":"regional_rip_up",
                "neighbor_net":crate::resolve::net_name(project, proposal.net),
                "blockers":proposal.blockers,
                "remove_traces":proposal.traces,
                "remove_vias":proposal.vias,
                "retained_anchors":anchors,
                "region_request":regional_request(&rip_up, vec![net,proposal.net]),
            })
        })
        .collect();
    Ok(json!({
        "air_wire":args.air_wire,
        "anchors":anchors,
        "probe_blockers":probe_blockers,
        "search_blockers":search_blockers,
        "outcome":if proposal_count == 0 {"no_eligible_blockers"} else {"suggested"},
        "proposal_count":proposal_count,
        "proposals":proposals,
        "combined_region_request":combined,
    }))
}
