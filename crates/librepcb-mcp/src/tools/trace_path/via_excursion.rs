//! Identify a branch-free pair of transitions on an explicitly measured path.

use std::collections::BTreeMap;

use librepcb_core::geometry::{Trace, TraceAnchor};
use librepcb_core::project::board::{Board, PathTrace, ViaView};
use librepcb_core::project::{NetSignalId, Project};
use librepcb_core::types::{Point, Uuid};
use serde_json::{Value, json};

use crate::{tools::layout, units::PointMm};

pub(super) fn suggestions(
    project: &Project,
    board: &Board,
    net: NetSignalId,
    path: &[PathTrace],
) -> Vec<Value> {
    let changes: Vec<_> = path
        .windows(2)
        .enumerate()
        .filter_map(|(i, pair)| (pair[0].layer != pair[1].layer).then_some(i))
        .collect();
    let mut proposals = Vec::new();
    for pair in changes.windows(2) {
        let [start, end] = [pair[0], pair[1] + 1];
        let run = &path[start..=end];
        let lead = &run[0];
        if run[run.len() - 1].layer != lead.layer
            || run
                .iter()
                .any(|trace| trace.segment != lead.segment || trace.width != lead.width)
        {
            continue;
        }
        let Some(segment) = board.net_segment(lead.segment) else {
            continue;
        };
        let Some(traces) = run
            .iter()
            .map(|trace| segment.traces().get(&trace.trace))
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };

        // Index only anchors on this selected route. Other incident traces
        // still count so that a branch cannot be discarded by a suggestion.
        let mut positions = BTreeMap::new();
        let mut degree = BTreeMap::new();
        for trace in &traces {
            for anchor in ends(trace) {
                positions.entry(anchor).or_insert_with(|| {
                    board.anchor_position(segment, anchor, project.library(), project.circuit())
                });
                degree.entry(anchor).or_insert(0);
            }
        }
        for trace in segment.traces().values() {
            for anchor in ends(trace) {
                if let Some(degree) = degree.get_mut(&anchor) {
                    *degree += 1;
                }
            }
        }
        let Some(shared) = traces
            .windows(2)
            .map(|p| {
                let anchor = shared_anchor(p[0], p[1])?;
                (degree[&anchor] == 2).then_some(anchor)
            })
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let [
            Some(&TraceAnchor::Via(first)),
            Some(&TraceAnchor::Via(last)),
        ] = [shared.first(), shared.last()]
        else {
            continue;
        };
        if first == last
            || shared[1..shared.len() - 1]
                .iter()
                .any(|a| !matches!(a, TraceAnchor::Junction(_)))
        {
            continue;
        }
        let entry = other_end(traces[0], TraceAnchor::Via(first));
        let exit = other_end(traces[traces.len() - 1], TraceAnchor::Via(last));
        if entry == exit
            || matches!(entry, TraceAnchor::Via(_))
            || matches!(exit, TraceAnchor::Via(_))
        {
            continue;
        }
        let Some(points) = positions.values().copied().collect::<Option<Vec<_>>>() else {
            continue;
        };
        let [Some(first_via), Some(last_via)] =
            [segment.vias().get(&first), segment.vias().get(&last)]
        else {
            continue;
        };
        let Some(outer_diameter) = [first_via, last_via]
            .into_iter()
            .map(|via| ViaView::new(via, board, Some(net), project.circuit()).ok())
            .collect::<Option<Vec<_>>>()
            .and_then(|views| views.into_iter().map(|view| view.size()).max())
        else {
            continue;
        };
        let margin = lead.width.to_nm().max(outer_diameter.to_nm()) / 2 + 750_000;
        let Some(bounds) = bounding_region(&points, margin) else {
            continue;
        };
        let rip_up: Vec<Uuid> = [first, last]
            .into_iter()
            .chain(traces.iter().map(|trace| trace.uuid()))
            .collect();
        let vias = [first_via, last_via]
            .map(|via| json!({"uuid":via.uuid(),"position":PointMm::from(via.position())}));
        proposals.push(json!({
            "layer": lead.layer.id(),
            "from": layout::trace_anchor(project, board, positions[&entry], entry),
            "to": layout::trace_anchor(project, board, positions[&exit], exit),
            "vias": vias,
            "existing_length_mm": run.iter().map(|trace| trace.length_mm).sum::<f64>(),
            "region_request": {
                "board": board.id(),
                "expected_revision": project.revision(),
                "region": bounds,
                "rip_up": rip_up,
                "route": {
                    "nets": [net],
                    "layers": [lead.layer.id()],
                    "trace_width": lead.width.to_mm(),
                    "pad_escape": true,
                    "search_evidence": true,
                },
            },
        }));
    }
    proposals
}

fn ends(trace: &Trace) -> [TraceAnchor; 2] {
    [trace.p1(), trace.p2()]
}

fn shared_anchor(a: &Trace, b: &Trace) -> Option<TraceAnchor> {
    ends(a).into_iter().find(|anchor| ends(b).contains(anchor))
}

fn other_end(trace: &Trace, anchor: TraceAnchor) -> TraceAnchor {
    if trace.p1() == anchor {
        trace.p2()
    } else {
        trace.p1()
    }
}

fn bounding_region(points: &[Point], margin: i64) -> Option<Value> {
    let (min_x, max_x) = (
        points.iter().map(|p| p.x).min()?,
        points.iter().map(|p| p.x).max()?,
    );
    let (min_y, max_y) = (
        points.iter().map(|p| p.y).min()?,
        points.iter().map(|p| p.y).max()?,
    );
    let min = Point::from_nm(
        min_x.to_nm().checked_sub(margin)?,
        min_y.to_nm().checked_sub(margin)?,
    );
    let max = Point::from_nm(
        max_x.to_nm().checked_add(margin)?,
        max_y.to_nm().checked_add(margin)?,
    );
    Some(json!({"min":PointMm::from(min),"max":PointMm::from(max)}))
}
