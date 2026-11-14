//! Read-only shortest anchored trace paths (original, no upstream counterpart).
//!
//! Uses the existing petgraph A* implementation with a zero heuristic. Junctions
//! are scoped to their segment and layer; footprint pads include the device ID.
//! Only via/pad copper joins layers. Plane paths, unanchored copper overlaps and
//! via-barrel lengths are excluded. This is a length measurement, not DRC.

use std::collections::BTreeMap;

use petgraph::algo::astar;
use petgraph::graph::{NodeIndex, UnGraph};

use super::{BoardExportError, BoardExportResult, pad_data::PadOnBoard};
use crate::geometry::TraceAnchor;
use crate::project::{
    BoardId, ComponentInstanceId, EntityKind, Error, NetSegmentId, NetSignalId, Project,
};
use crate::types::{Layer, Point, PositiveLength, Uuid};

/// One measured trace, in path order. Length is its centerline in millimeters.
#[derive(Debug, Clone)]
pub struct PathTrace {
    pub segment: NetSegmentId,
    pub trace: Uuid,
    pub layer: Layer,
    pub width: PositiveLength,
    pub length_mm: f64,
}

#[derive(Debug)]
pub struct TracePath {
    pub length_mm: f64,
    pub traces: Vec<PathTrace>,
    /// Unique via anchors touched by this path, with segment-scoped identities.
    pub vias: Vec<(NetSegmentId, Uuid)>,
    /// Changes of copper layer between consecutive trace segments.
    pub layer_changes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Node {
    segment: Option<NetSegmentId>,
    anchor: TraceAnchor,
    layer: Layer,
}

type Graph = UnGraph<Node, Option<PathTrace>>;

fn node(graph: &mut Graph, nodes: &mut BTreeMap<Node, NodeIndex>, key: Node) -> NodeIndex {
    *nodes.entry(key).or_insert_with(|| graph.add_node(key))
}

/// Floating-point centerline length in mm; no integer subtraction or model writes.
pub fn centerline_length_mm(from: Point, to: Point) -> f64 {
    libm::hypot(
        to.x.to_nm() as f64 - from.x.to_nm() as f64,
        to.y.to_nm() as f64 - from.y.to_nm() as f64,
    ) / 1e6
}

impl Project {
    /// Shortest path through explicit, anchored same-net traces. `None` does
    /// not mean electrically disconnected: a plane or copper overlap may join
    /// the endpoints. Callers resolve and validate the endpoint pads and net.
    pub fn trace_path(
        &self,
        board: BoardId,
        net: NetSignalId,
        from: (ComponentInstanceId, Uuid),
        to: (ComponentInstanceId, Uuid),
    ) -> BoardExportResult<Option<TracePath>> {
        let board = self.board(board).ok_or(Error::NotFound {
            kind: EntityKind::Board,
            uuid: board.0,
        })?;
        let from = TraceAnchor::FootprintPad {
            device: from.0.0,
            pad: from.1,
        };
        let to = TraceAnchor::FootprintPad {
            device: to.0.0,
            pad: to.1,
        };
        if from == to {
            return Ok(Some(TracePath {
                length_mm: 0.0,
                traces: Vec::new(),
                vias: Vec::new(),
                layer_changes: 0,
            }));
        }
        let mut graph = Graph::new_undirected();
        let mut nodes = BTreeMap::new();
        for (segment_id, segment) in board
            .net_segments()
            .iter()
            .filter(|(_, s)| s.net() == Some(net))
        {
            for trace in segment.traces().values() {
                let mut endpoints = Vec::with_capacity(2);
                for anchor in [trace.p1(), trace.p2()] {
                    let on_layer = match anchor {
                        TraceAnchor::FootprintPad { device, pad } => board
                            .device(ComponentInstanceId(device))
                            .map(|d| d.pad(&pad, self.library(), self.circuit()))
                            .transpose()?
                            .flatten()
                            .is_some_and(|pad| pad.is_on_layer(trace.layer())),
                        TraceAnchor::Via(id) => segment
                            .vias()
                            .get(&id)
                            .is_some_and(|v| v.is_on_layer(trace.layer())),
                        TraceAnchor::Pad(id) => segment.pads().get(&id).is_some_and(|p| {
                            PadOnBoard {
                                pad: p.pad(),
                                mirrored: false,
                            }
                            .is_on_layer(trace.layer())
                        }),
                        TraceAnchor::Junction(_) => true,
                    };
                    if !board.copper_layers().contains(&trace.layer()) || !on_layer {
                        return Err(BoardExportError::Logic(
                            "Trace endpoint is not on its copper layer.",
                        ));
                    }
                    let position = board
                        .anchor_position(segment, anchor, self.library(), self.circuit())
                        .ok_or(BoardExportError::Logic(
                            "Trace endpoint could not be resolved.",
                        ))?;
                    let key = Node {
                        segment: (!matches!(anchor, TraceAnchor::FootprintPad { .. }))
                            .then_some(*segment_id),
                        anchor,
                        layer: trace.layer(),
                    };
                    endpoints.push((node(&mut graph, &mut nodes, key), position));
                }
                let [(a, p), (b, q)] = endpoints[..] else {
                    unreachable!("two endpoints were inserted")
                };
                let length_mm = centerline_length_mm(p, q);
                graph.add_edge(
                    a,
                    b,
                    Some(PathTrace {
                        segment: *segment_id,
                        trace: trace.uuid(),
                        layer: trace.layer(),
                        width: trace.width(),
                        length_mm,
                    }),
                );
            }
        }
        // Same physical pad or via joins its valid copper layers. Junctions
        // never provide an implicit layer transition, even at the same XY.
        let mut joins: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for (key, index) in &nodes {
            if !matches!(key.anchor, TraceAnchor::Junction(_)) {
                joins
                    .entry((key.segment, key.anchor))
                    .or_default()
                    .push(*index);
            }
        }
        for indices in joins.values() {
            for pair in indices.windows(2) {
                graph.add_edge(pair[0], pair[1], None);
            }
        }
        let Some(start) = nodes
            .iter()
            .find(|(n, _)| n.anchor == from)
            .map(|(_, index)| *index)
        else {
            return Ok(None);
        };
        let Some((length_mm, path)) = astar(
            &graph,
            start,
            |index| graph[index].anchor == to,
            |edge| edge.weight().as_ref().map_or(0.0, |trace| trace.length_mm),
            |_| 0.0,
        ) else {
            return Ok(None);
        };
        let traces: Vec<_> = path
            .windows(2)
            .filter_map(|pair| {
                // A* returns adjacent nodes. Parallel traces on the same layer
                // share their endpoints and centerline length; either is shortest.
                graph
                    .find_edge(pair[0], pair[1])
                    .and_then(|edge| graph[edge].clone())
            })
            .collect();
        let layer_changes = traces
            .windows(2)
            .filter(|pair| pair[0].layer != pair[1].layer)
            .count();
        let vias = path
            .iter()
            .filter_map(|index| {
                let key = graph[*index];
                if let TraceAnchor::Via(id) = key.anchor {
                    Some((key.segment?, id))
                } else {
                    None
                }
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        Ok(Some(TracePath {
            length_mm,
            traces,
            vias,
            layer_changes,
        }))
    }
}
