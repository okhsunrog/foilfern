//! Reviewed replacement of a trace path using its existing end anchors.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::geometry::{Junction, Trace, TraceAnchor};
use librepcb_core::project::board::{Board, BoardSegmentElements, BoardSegmentItems};
use librepcb_core::project::{NetSegmentId, Project};
use librepcb_core::types::{Point, Uuid};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Replaces an ordered, unbranched chain of equal-width traces on one layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceReplacement {
    /// UUIDs of the existing traces in path order.
    pub traces: Vec<Uuid>,
    /// Absolute board positions of the new interior junctions, in path order.
    pub points: Vec<Point>,
}

#[derive(Default)]
pub(super) struct SegmentReplacement {
    pub added: BoardSegmentElements,
    pub removed: BoardSegmentItems,
}

fn opposite(trace: &Trace, anchor: TraceAnchor) -> Option<TraceAnchor> {
    if trace.p1() == anchor {
        Some(trace.p2())
    } else if trace.p2() == anchor {
        Some(trace.p1())
    } else {
        None
    }
}

pub(super) fn plan(
    project: &Project,
    board: &Board,
    replacements: Vec<TraceReplacement>,
    positions: &BTreeMap<TraceAnchor, Point>,
) -> Result<BTreeMap<NetSegmentId, SegmentReplacement>> {
    let mut edits = BTreeMap::<NetSegmentId, SegmentReplacement>::new();
    let mut selected = BTreeSet::new();
    for replacement in replacements {
        let Some(first) = replacement.traces.first() else {
            return Err(Error::InvalidArgument(
                "A trace replacement needs at least one trace.".into(),
            ));
        };
        let mut segments = board
            .net_segments()
            .iter()
            .filter(|(_, s)| s.traces().contains_key(first));
        let (segment_id, segment) = segments
            .next()
            .ok_or_else(|| Error::not_found("Trace", first))?;
        if segments.next().is_some() {
            return Err(Error::Ambiguous {
                kind: "Trace",
                id: first.to_string(),
            });
        }
        let traces = replacement
            .traces
            .iter()
            .map(|uuid| {
                if !selected.insert(*uuid) {
                    return Err(Error::InvalidArgument(format!(
                        "Trace {uuid} is selected more than once."
                    )));
                }
                segment.traces().get(uuid).ok_or_else(|| {
                    Error::InvalidArgument(format!(
                        "Trace {uuid} is not in the segment of {first}."
                    ))
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let first = traces[0];
        let start = match traces.get(1) {
            Some(second) if opposite(second, first.p1()).is_some() => first.p2(),
            _ => first.p1(),
        };
        let mut path = vec![start];
        let mut visited = BTreeSet::from([start]);
        let mut previous = start;
        for trace in traces {
            if trace.layer() != first.layer() || trace.width() != first.width() {
                return Err(Error::InvalidArgument(
                    "A replacement path must use one layer and width.".into(),
                ));
            }
            let next = opposite(trace, previous).ok_or_else(|| {
                Error::InvalidArgument(
                    "Replacement traces must form a path in the supplied order.".into(),
                )
            })?;
            if !visited.insert(next) {
                return Err(Error::InvalidArgument(
                    "A replacement path cannot revisit an anchor.".into(),
                ));
            }
            path.push(next);
            previous = next;
        }
        let end = previous;
        let interior = &path[1..path.len() - 1];
        let mut old_junctions = Vec::new();
        let mut old_points = Vec::new();
        for &anchor in interior {
            let TraceAnchor::Junction(uuid) = anchor else {
                return Err(Error::InvalidArgument(
                    "Only junctions can be inside a replacement path.".into(),
                ));
            };
            if segment.traces_at(anchor).count() != 2 {
                return Err(Error::InvalidArgument(format!(
                    "Junction {uuid} has a branch outside the replacement path."
                )));
            }
            if positions.contains_key(&anchor) {
                return Err(Error::InvalidArgument(format!(
                    "Junction {uuid} is both moved and replaced."
                )));
            }
            old_junctions.push(uuid);
            let junction = segment
                .junctions()
                .get(&uuid)
                .ok_or_else(|| Error::not_found("Junction", uuid))?;
            old_points.push(junction.position());
        }
        let position = |anchor| {
            positions
                .get(&anchor)
                .copied()
                .or_else(|| {
                    board.anchor_position(segment, anchor, project.library(), project.circuit())
                })
                .ok_or_else(|| Error::not_found("Routing anchor", format!("{anchor:?}")))
        };
        let start_position = position(start)?;
        let end_position = position(end)?;
        let mut previous_position = start_position;
        for next_position in replacement.points.iter().copied().chain([end_position]) {
            if previous_position == next_position {
                return Err(Error::InvalidArgument(
                    "A replacement path contains a zero-length trace.".into(),
                ));
            }
            previous_position = next_position;
        }
        if old_points == replacement.points {
            continue;
        }
        let edit = edits.entry(*segment_id).or_default();
        edit.removed.traces.extend(replacement.traces);
        edit.removed.junctions.extend(old_junctions);
        let mut previous = start;
        for point in replacement.points {
            let uuid = Uuid::new_random();
            let anchor = TraceAnchor::Junction(uuid);
            edit.added.junctions.push(Junction::new(uuid, point));
            edit.added.traces.push(Trace::new(
                Uuid::new_random(),
                first.layer(),
                first.width(),
                previous,
                anchor,
            ));
            previous = anchor;
        }
        edit.added.traces.push(Trace::new(
            Uuid::new_random(),
            first.layer(),
            first.width(),
            previous,
            end,
        ));
    }
    Ok(edits)
}
