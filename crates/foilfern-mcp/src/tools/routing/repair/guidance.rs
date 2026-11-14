//! Revision-bound next searches; suggestions never edit or route the live board.

use std::cmp::Reverse;
use std::collections::BTreeSet;

use foilfern_core::project::board::Board;
use foilfern_core::project::{ComponentInstanceId, Project};
use foilfern_core::types::Uuid;
use foilfern_editor::commands::{Autoroute, RoutingBounds};
use foilfern_editor::routing_service::{
    RegionalRepairProposal, RepairLaunchSearch, RepairSearchRequest,
};
use serde_json::{Value, json};

use crate::error::ToolResult;

fn ranked(mut actions: Vec<Value>) -> Vec<Value> {
    // Preserve the newly implicated branch first; prefer trimming a tail of
    // the scope that already failed. These remain untested suggestions.
    actions.sort_by_key(|action| {
        (
            Reverse(action["new_scope_objects"].as_u64().unwrap_or(0)),
            action["affected_objects"].as_u64().unwrap_or(u64::MAX),
        )
    });
    actions
}

fn pad((component, pad): (ComponentInstanceId, Uuid)) -> Value {
    json!({"component":component,"pad":pad})
}

fn bounds(region: RoutingBounds) -> Value {
    json!({"min":{"x":region.min.x.to_mm(),"y":region.min.y.to_mm()},
        "max":{"x":region.max.x.to_mm(),"y":region.max.y.to_mm()}})
}

fn launch(search: &RepairLaunchSearch) -> Value {
    json!({"from":pad(search.from),"layer":search.layer.id(),
        "radius":search.radius.to_mm(),"max_sites":search.max_sites})
}

fn route(options: &Autoroute) -> Value {
    json!({
        "layers":options.layers.as_ref().map(|ls| ls.iter().map(|l| l.id()).collect::<Vec<_>>()).unwrap_or_default(),
        "grid_pitch":options.grid_pitch.map(|v| v.to_mm()),
        "trace_width":options.trace_width.map(|v| v.to_mm()),
        "via_size":options.via_size.map(|v| v.to_mm()),
        "via_drill":options.via_drill.map(|v| v.to_mm()),
        "via_cost":options.via_cost.map(|v| v.to_mm()),
        "neck_down":options.neck_down.map(|v| json!({"width":v.width.to_mm(),"max_length":v.max_length.to_mm()})),
        "pad_escape":options.pad_escape,"detour_search":options.detour_search,
        "search_evidence":options.search_evidence,"congestion_history":options.congestion_history,
        "critical_nets":options.critical_nets,
    })
}

/// Offer at most eight distinct paired searches, preserving the caller's path
/// constraints and geometry settings. Permissions come only from the current
/// diagnosis proposals, not from spatial proximity alone.
pub(super) fn next_searches(
    project: &Project,
    board: &Board,
    original: &RepairSearchRequest,
    proposals: &[RegionalRepairProposal],
    max_objects: usize,
    region: RoutingBounds,
    edits: RoutingBounds,
) -> ToolResult<Vec<Value>> {
    let (Some(target), Some(neighbor), Some(joint)) = (
        original.target_launch_search.as_ref(),
        original.neighbor_launch_search.as_ref(),
        original.joint_route.as_ref(),
    ) else {
        return Ok(Vec::new());
    };
    // Exact branch endpoints are a caller contract; do not silently replace it
    // with a new group numbering or different retained junctions.
    if !original.neighbor_anchors.is_empty() {
        return Ok(Vec::new());
    }
    let neighbor_region = original
        .neighbor_route
        .region
        .unwrap_or(original.rip_up_region);
    let target_bounds = region.region()?;
    let neighbor_bounds = neighbor_region.region()?;
    if ![neighbor_region.min, neighbor_region.max]
        .into_iter()
        .all(|p| target_bounds.contains(p, foilfern_core::types::Length::ZERO))
        || ![edits.min, edits.max]
            .into_iter()
            .all(|p| neighbor_bounds.contains(p, foilfern_core::types::Length::ZERO))
    {
        return Ok(Vec::new());
    }
    let Some(device) = board.device(neighbor.from.0) else {
        return Ok(Vec::new());
    };
    let Some(neighbor_pad) = device
        .pad(&neighbor.from.1, project.library(), project.circuit())
        .map_err(foilfern_editor::Error::from)?
    else {
        return Ok(Vec::new());
    };
    let Some(neighbor_net) = neighbor_pad.net() else {
        return Ok(Vec::new());
    };
    let limit = max_objects.min(original.max_objects);
    let mut seen = BTreeSet::new();
    let mut actions = Vec::new();
    let selected = |p: &RegionalRepairProposal| -> BTreeSet<Uuid> {
        p.traces.union(&p.vias).copied().collect()
    };
    let original_union: BTreeSet<_> = original.groups.iter().flatten().copied().collect();
    for (i, a) in proposals.iter().enumerate() {
        for b in proposals.iter().skip(i + 1) {
            if a.net == b.net || (a.net != neighbor_net && b.net != neighbor_net) {
                continue;
            }
            let mut variants = Vec::new();
            let total = selected(a).union(&selected(b)).count();
            if total <= limit {
                variants.push((a.clone(), b.clone()));
            } else {
                // Try both allocations: preserve one complete proposal and
                // trim only the unblocked tail of the other branch.
                if let Some(budget) = limit.checked_sub(selected(b).len())
                    && let Some(smaller) = a.fit_budget(project, board, budget)?
                {
                    variants.push((smaller, b.clone()));
                }
                if let Some(budget) = limit.checked_sub(selected(a).len())
                    && let Some(smaller) = b.fit_budget(project, board, budget)?
                {
                    variants.push((a.clone(), smaller));
                }
            }
            for (a, b) in variants {
                let groups = [selected(&a), selected(&b)];
                let union: BTreeSet<_> = groups.iter().flatten().copied().collect();
                if union == original_union || !seen.insert(union.clone()) {
                    continue;
                }
                let requirements: Vec<_> = original
                    .trace_requirements
                    .iter()
                    .map(|r| {
                        json!({
                            "path":{"from":pad(r.from),"to":pad(r.to)},
                            "max_length_mm":r.max_length.map(|v| v.to_mm()),
                            "max_layer_changes":r.max_layer_changes,
                        })
                    })
                    .collect();
                let arguments = json!({
                    "board":board.id(),"expected_revision":project.revision(),"target":original.target,
                    "region":bounds(region),"rip_up_region":bounds(edits),
                    "neighbor_region":bounds(neighbor_region),
                    "groups":groups,"route":route(&original.route),
                    "neighbor_route":route(&original.neighbor_route),"joint_route":route(joint),
                    "target_launch_search":launch(target),"neighbor_launch_search":launch(neighbor),
                    "max_objects":limit,"max_trials":original.max_trials,"max_candidates":original.max_candidates,
                    "max_time_ms":original.time_budget.as_millis(),"trace_requirements":requirements,
                });
                // Keep generated requests aligned with the strict public schema.
                serde_json::from_value::<super::super::repair_search::RepairSearchArgs>(
                    arguments.clone(),
                )
                .map_err(|e| {
                    crate::error::ToolError::new(
                        crate::error::ErrorKind::Internal,
                        format!("Invalid repair suggestion: {e}"),
                    )
                })?;
                actions.push(json!({
                    "kind":"retry_paired_escape","reason":"Inspect neighboring escape blockers with an adjusted branch scope",
                    "new_scope_objects":union.difference(&original_union).count(),
                    "affected_objects":union.len(),"tool":"routing_repair_start","arguments":arguments,
                }));
                if actions.len() == 8 {
                    return Ok(ranked(actions));
                }
            }
        }
    }
    Ok(ranked(actions))
}
