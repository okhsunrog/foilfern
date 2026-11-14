//! Read-only regional repair suggestions from exact endpoint clearance probes.

use std::collections::{BTreeMap, BTreeSet};

use foilfern_core::geometry::TraceAnchor;
use foilfern_core::project::board::Board;
use foilfern_core::project::{BoardId, NetSegmentId, NetSignalId, Project};
use foilfern_core::types::{Length, PositiveLength, Uuid};
use foilfern_editor::commands::autoroute::{
    AccessBlocker, Autoroute, ExtractedProblem, RoutingConnection, RoutingObject,
    RoutingObjectKind, RoutingSearchBlocker, UnroutedAirWire, diagnose_access,
};
use foilfern_editor::routing_service::{
    RegionalRepairProposal, RepairLaunch, RoutingCandidateDetails, RoutingService,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::RoutingRegionArgs;
use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::tools::layout;

const MAX_PROPOSALS: usize = 8;

mod guidance;

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
    /// Optional builtin, staged or additive copper continuation candidate with search_evidence.
    pub evidence_candidate: Option<String>,
    /// Finished repair search from this board, net and revision.
    pub evidence_search: Option<String>,
    /// Max number of existing traces and vias in one suggestion, 1–16 (default 8).
    pub max_objects: Option<usize>,
}

struct CandidateEvidence<'a> {
    failed: Vec<&'a UnroutedAirWire>,
    /// For additive copper continuations, match by the staged copper connections.
    staged: Option<foilfern_editor::ProjectEditor>,
}

fn candidate_evidence<'a>(
    service: &'a RoutingService,
    project: &Project,
    board: BoardId,
    candidate: &str,
) -> ToolResult<CandidateEvidence<'a>> {
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
    let mut staged = None;
    let failed = match &info.details {
        RoutingCandidateDetails::Builtin { routing, .. } => {
            routing.failed_requests.iter().collect()
        }
        RoutingCandidateDetails::Staged { staged } => staged
            .trials
            .iter()
            .flat_map(|trial| &trial.failed_requests)
            .collect(),
        RoutingCandidateDetails::Copper {
            copper,
            route: Some(route),
        } if copper.base_candidate.is_none()
            && copper.removed_traces == 0
            && copper.removed_vias.is_none()
            && copper.placements.is_none() =>
        {
            let (mut sandbox, mutations) = service
                .preview_snapshot(project, id)
                .map_err(super::routing_error)?;
            sandbox.apply_mutations("Inspect routing continuation", mutations)?;
            staged = Some(sandbox);
            route.routing.failed_requests.iter().collect()
        }
        _ => {
            return Err(ToolError::invalid(
                "Use a builtin, staged, or additive copper continuation candidate for search evidence.",
            ));
        }
    };
    Ok(CandidateEvidence { failed, staged })
}

fn copper_segments(board: &Board, net: NetSignalId) -> BTreeMap<TraceAnchor, NetSegmentId> {
    let mut segments = BTreeMap::new();
    for segment in board
        .net_segments()
        .values()
        .filter(|s| s.net() == Some(net))
    {
        for trace in segment.traces().values() {
            segments.insert(trace.p1(), segment.id());
            segments.insert(trace.p2(), segment.id());
        }
    }
    segments
}

fn launch_request(launch: &RepairLaunch) -> Value {
    let (component, pad) = launch.from;
    json!({
        "from": {"component":component,"pad":pad},
        "layer": launch.layer.id(),
        "position": crate::views::point(launch.position),
        "width": launch.width.to_mm(),
        "drill": launch.drill.map(|drill| drill.to_mm()),
        "size": launch.size.map(|size| size.to_mm()),
    })
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
    if args.evidence_search.is_some() && args.evidence_candidate.is_some() {
        return Err(ToolError::invalid(
            "Supply either evidence_search or evidence_candidate.",
        ));
    }
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
    let connections: Vec<_> = extracted
        .problem
        .connections
        .iter()
        .filter_map(|c| match c {
            RoutingConnection::Terminals(a, b) => Some([*a, *b]),
            RoutingConnection::Net(_) => None,
        })
        .collect();
    let endpoints = *connections.get(args.air_wire).ok_or_else(|| {
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
        .map_err(foilfern_editor::Error::from)?;
    let evidence = args
        .evidence_candidate
        .as_deref()
        .map(|id| candidate_evidence(service, project, board.id(), id))
        .transpose()?;
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
    let mut continuation_failures = Vec::new();
    let mut escape_sites = Vec::new();
    let mut escape_blockers = Vec::new();
    let mut validation_failures = Vec::new();
    let mut validation_blockers = Vec::new();
    let mut screening_failures = Vec::new();
    let mut routing_failures = Vec::new();
    let mut repair_request = None;
    if let Some(id) = &args.evidence_search {
        let id = crate::resolve::required_uuid(id, "repair search")?;
        let evidence = service
            .repair_search_evidence(project, board.id(), net, id)
            .map_err(super::routing_error)?;
        if !evidence
            .trials
            .iter()
            .any(|trial| trial.validation.as_ref().is_some_and(|v| v.acceptable))
        {
            repair_request = Some(evidence.request);
        }
        let failures = evidence.launches;
        let trials = evidence.trials;
        let screenings = evidence.screenings;
        let segments = copper_segments(board, net);
        let same_branch = |a, b| {
            a == b
                || segments
                    .get(&a)
                    .is_some_and(|id| segments.get(&b) == Some(id))
        };
        for failure in failures {
            let wire = &failure.wire;
            let launch = &failure.launch;
            let via = TraceAnchor::Via(failure.via);
            let other = if wire.p1 == via { wire.p2 } else { wire.p1 };
            let (component, pad) = launch.from;
            let origin = TraceAnchor::FootprintPad {
                device: component.0,
                pad,
            };
            let matches = |[a, b]: [TraceAnchor; 2]| {
                !same_branch(a, b)
                    && ((same_branch(a, origin) && same_branch(b, other))
                        || (same_branch(b, origin) && same_branch(a, other)))
            };
            if !matches(native_anchors)
                || connections
                    .iter()
                    .filter(|ids| matches(ids.map(|id| extracted.terminal_anchors[id.0])))
                    .take(2)
                    .count()
                    != 1
            {
                continue;
            }
            let mut blockers = Vec::new();
            for sample in &wire.search_evidence {
                if let Some(RoutingSearchBlocker::Existing { object }) = sample.blocker {
                    if !search_blockers.contains(&object) {
                        search_blockers.push(object);
                    }
                    if !blockers.contains(&object.uuid) {
                        blockers.push(object.uuid);
                    }
                }
            }
            escape_blockers.push(blockers.clone());
            escape_sites.push(json!({
                "target_launch": launch_request(launch),
                "reason":wire.reason,
                "soft_reason":wire.soft_reason,
                "blockers":blockers,
            }));
        }
        // A report for an explicit/generated launch is relevant to this
        // connection only when the launch lies on one of its source branches.
        let source_copper: BTreeSet<_> = board
            .net_segments()
            .values()
            .flat_map(|segment| segment.traces().keys().chain(segment.vias().keys()))
            .copied()
            .collect();
        for failure in screenings {
            let blockers: Vec<_> = failure
                .blockers
                .iter()
                .filter(|object| {
                    source_copper.contains(&object.uuid) && !failure.rip_up.contains(&object.uuid)
                })
                .copied()
                .collect();
            for object in &blockers {
                if !search_blockers.contains(object) {
                    search_blockers.push(*object);
                }
            }
            screening_failures.push(json!({
                "groups":failure.groups, "rip_up":failure.rip_up,
                "net":crate::resolve::net_name(project, failure.net),
                "anchor":layout::trace_anchor(project, board, Some(failure.position), TraceAnchor::FootprintPad {
                    device:failure.from.0.0, pad:failure.from.1,
                }),
                "layer":failure.layer, "reason":"no_clear_escape_sites",
                "target_launch":failure.target_launch.as_ref().map(launch_request),
                "source_blockers":blockers,
            }));
        }
        for (index, trial) in trials.iter().enumerate() {
            for failure in trial.target_failures.iter().chain(&trial.neighbor_failures) {
                let blockers: Vec<_> = failure
                    .search_evidence
                    .iter()
                    .filter_map(|sample| match sample.blocker {
                        Some(RoutingSearchBlocker::Existing { object })
                            if source_copper.contains(&object.uuid)
                                && !trial.rip_up.contains(&object.uuid) =>
                        {
                            Some(object)
                        }
                        _ => None,
                    })
                    .collect();
                for object in &blockers {
                    if !search_blockers.contains(object) {
                        search_blockers.push(*object);
                    }
                }
                if routing_failures.len() < MAX_PROPOSALS {
                    routing_failures
                        .push(json!({"trial":index,"failure":failure,"source_blockers":blockers}));
                }
            }
            if validation_failures.len() == MAX_PROPOSALS {
                break;
            }
            let Some(validation) = &trial.validation else {
                continue;
            };
            if !validation.after.complete || validation.acceptable {
                continue;
            }
            if connections.len() != 1 {
                let Some(launch) = &trial.target_launch else {
                    continue;
                };
                let origin = TraceAnchor::FootprintPad {
                    device: launch.from.0.0,
                    pad: launch.from.1,
                };
                let touches =
                    |ids: [TraceAnchor; 2]| ids.into_iter().any(|a| same_branch(a, origin));
                if !touches(native_anchors)
                    || connections
                        .iter()
                        .filter(|ids| touches(ids.map(|id| extracted.terminal_anchors[id.0])))
                        .take(2)
                        .count()
                        != 1
                {
                    continue;
                }
            }
            let found: Vec<_> = validation
                .new_errors
                .iter()
                .filter(|finding| !finding.objects.is_empty())
                .take(MAX_PROPOSALS)
                .collect();
            let mut blockers = Vec::new();
            for object in found.iter().flat_map(|finding| &finding.objects) {
                if matches!(
                    object.kind,
                    RoutingObjectKind::Trace | RoutingObjectKind::Via
                ) && source_copper.contains(&object.uuid)
                    && !trial.rip_up.contains(&object.uuid)
                {
                    if !validation_blockers.contains(object) {
                        validation_blockers.push(*object);
                    }
                    if !blockers.contains(&object.uuid) {
                        blockers.push(object.uuid);
                    }
                }
            }
            if !found.is_empty() || !validation.unrouted_regression_details.is_empty() {
                validation_failures.push((
                    blockers,
                    json!({
                        "trial": index,
                        "strategy": trial.strategy,
                        "rip_up": trial.rip_up,
                        "target_launch": trial.target_launch.as_ref().map(launch_request),
                        "clearance_conflicts": found,
                        "unrouted_regressions": validation.unrouted_regression_details,
                    }),
                ));
            }
        }
    }
    let staged_project = evidence
        .as_ref()
        .and_then(|evidence| evidence.staged.as_ref())
        .map(foilfern_editor::ProjectEditor::project);
    let evidence_board = staged_project.and_then(|project| project.board(board.id()));
    let staged_segments = evidence_board.map(|board| copper_segments(board, net));
    for failed in evidence.iter().flat_map(|evidence| &evidence.failed) {
        if failed.net != net {
            continue;
        }
        let exact =
            [failed.p1, failed.p2] == native_anchors || [failed.p2, failed.p1] == native_anchors;
        if !exact {
            let Some(segments) = &staged_segments else {
                continue;
            };
            let same_branch = |a, b| {
                a == b
                    || segments
                        .get(&a)
                        .is_some_and(|id| segments.get(&b) == Some(id))
            };
            let matches = |[a, b]: [TraceAnchor; 2]| {
                let pair = |a, b| {
                    !same_branch(a, b) && same_branch(a, failed.p1) && same_branch(b, failed.p2)
                };
                pair(a, b) || pair(b, a)
            };
            // Do not attribute evidence to an arbitrary original air wire if
            // the additions connect both ends of multiple original requests.
            if !matches(native_anchors)
                || connections
                    .iter()
                    .filter(|ids| matches(ids.map(|id| extracted.terminal_anchors[id.0])))
                    .take(2)
                    .count()
                    != 1
            {
                continue;
            }
        }
        if let Some((staged_project, staged)) = staged_project.zip(evidence_board) {
            continuation_failures.push(json!({
                "anchors": [
                    layout::trace_anchor(staged_project, staged, Some(failed.p1_position), failed.p1),
                    layout::trace_anchor(staged_project, staged, Some(failed.p2_position), failed.p2),
                ],
                "reason": failed.reason,
                "soft_reason": failed.soft_reason,
                "search_evidence": failed.search_evidence,
            }));
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
        .chain(&validation_blockers)
        .copied()
        .collect();
    let mut proposals = RegionalRepairProposal::suggest(
        project,
        board,
        net,
        edits,
        &blockers,
        &allowed,
        max_objects,
    )?;
    if evidence_board.is_some() || args.evidence_search.is_some() {
        proposals.sort_by_key(|proposal| {
            let has = |objects: &[RoutingObject]| {
                proposal
                    .blockers
                    .iter()
                    .any(|id| objects.iter().any(|object| object.uuid == *id))
            };
            (!has(&validation_blockers), !has(&search_blockers))
        });
    }
    let proposal_count = proposals.len();
    let next_actions = if let Some(request) = &repair_request {
        guidance::next_searches(
            project,
            board,
            request,
            &proposals,
            max_objects,
            args.region.bounds()?,
            edits,
        )?
    } else {
        Vec::new()
    };
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
            let mut result = json!({
                "strategy":"regional_rip_up",
                "neighbor_net":crate::resolve::net_name(project, proposal.net),
                "blockers":proposal.blockers,
                "remove_traces":proposal.traces,
                "remove_vias":proposal.vias,
                "retained_anchors":anchors,
                "region_request":regional_request(&rip_up, vec![net,proposal.net]),
            });
            if args.evidence_search.is_some() {
                result["escape_sites"] = json!(
                    escape_blockers
                        .iter()
                        .enumerate()
                        .filter(|(_, blockers)| proposal
                            .blockers
                            .iter()
                            .any(|id| blockers.contains(id)))
                        .map(|(index, _)| index)
                        .collect::<Vec<_>>()
                );
            }
            let next_trials: Vec<_> = validation_failures
                .iter()
                .filter(|(blockers, _)| proposal.blockers.iter().any(|id| blockers.contains(id)))
                .map(|(_, failure)| &failure["trial"])
                .collect();
            if !next_trials.is_empty() {
                result["addresses_clearance_in_trials"] = json!(next_trials);
            }
            if let Some(pair) = proposal.branch {
                let anchor = |anchor| match anchor {
                    TraceAnchor::Junction(id) => json!({"junction":id}),
                    TraceAnchor::Via(id) => json!({"via":id}),
                    TraceAnchor::Pad(id) => json!({"board_pad":id}),
                    TraceAnchor::FootprintPad { device, pad } => {
                        json!({"pad":{"component":device,"pad":pad}})
                    }
                };
                result["neighbor_anchors"] = json!([{
                    "group":0, "from":anchor(pair[0]), "to":anchor(pair[1]),
                }]);
            }
            result
        })
        .collect();
    let mut out = json!({
        "air_wire":args.air_wire,
        "anchors":anchors,
        "probe_blockers":probe_blockers,
        "search_blockers":search_blockers,
        "outcome":if proposal_count == 0 {"no_eligible_blockers"} else {"suggested"},
        "proposal_count":proposal_count,
        "proposals":proposals,
        "combined_region_request":combined,
        "next_actions":next_actions,
    });
    if evidence_board.is_some() {
        out["continuation_failures"] = json!(continuation_failures);
    }
    if args.evidence_search.is_some() {
        out["escape_sites"] = json!(escape_sites);
        out["screening_failures"] = json!(screening_failures);
        out["routing_failures"] = json!(routing_failures);
        out["validation_failures"] = json!(
            validation_failures
                .into_iter()
                .map(|(blockers, mut failure)| {
                    failure["source_copper"] = json!(blockers);
                    failure
                })
                .collect::<Vec<_>>()
        );
    }
    Ok(out)
}
