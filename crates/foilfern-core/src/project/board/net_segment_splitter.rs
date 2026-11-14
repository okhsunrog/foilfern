//! Port of libs/librepcb/core/project/board/boardnetsegmentsplitter.{h,cpp}.
//!
//! Splits the elements of a (modified) board net segment into cohesive
//! segments, e.g. after removing traces. Pads and vias can be replaced by
//! junctions (one per layer), e.g. when they are removed while traces stay.
//!
//! Differences to upstream: the new junction UUIDs come from a caller
//! provided generator (upstream `Uuid::createRandom()`), and the elements
//! are processed in insertion order with an iterative search.

use std::collections::{BTreeMap, BTreeSet};

use super::BoardPadData;
use crate::geometry::{Junction, Trace, TraceAnchor, Via};
use crate::types::{Layer, Point, Uuid};

/// A cohesive group of net segment elements, see
/// [`BoardNetSegmentSplitter::split()`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoardNetSegmentParts {
    /// Junctions.
    pub junctions: Vec<Junction>,
    /// Standalone pads.
    pub pads: Vec<BoardPadData>,
    /// Vias.
    pub vias: Vec<Via>,
    /// Traces.
    pub traces: Vec<Trace>,
}

/// Splits net segment elements into cohesive segments (upstream
/// `BoardNetSegmentSplitter`).
#[derive(Debug, Default)]
pub struct BoardNetSegmentSplitter {
    junctions: Vec<Junction>,
    pads: Vec<BoardPadData>,
    vias: Vec<Via>,
    traces: Vec<Trace>,
    anchors_to_replace: BTreeMap<TraceAnchor, Point>,
    replaced_anchors: BTreeMap<(TraceAnchor, Layer), TraceAnchor>,
}

impl BoardNetSegmentSplitter {
    /// Creates an empty splitter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the footprint pad `anchor` at `pos` by junctions in the
    /// traces added afterwards (upstream `replaceFootprintPadByJunctions()`).
    pub fn replace_footprint_pad_by_junctions(&mut self, anchor: TraceAnchor, pos: Point) {
        self.anchors_to_replace.insert(anchor, pos);
    }

    /// Adds a junction.
    pub fn add_junction(&mut self, junction: Junction) {
        self.junctions.push(junction);
    }

    /// Adds a standalone pad, or replaces it by junctions in the traces
    /// added afterwards.
    pub fn add_pad(&mut self, pad: BoardPadData, replace_by_junctions: bool) {
        if replace_by_junctions {
            self.anchors_to_replace
                .insert(TraceAnchor::Pad(pad.uuid()), pad.pad().position());
        } else {
            self.pads.push(pad);
        }
    }

    /// Adds a via, or replaces it by junctions in the traces added
    /// afterwards.
    pub fn add_via(&mut self, via: Via, replace_by_junctions: bool) {
        if replace_by_junctions {
            self.anchors_to_replace
                .insert(TraceAnchor::Via(via.uuid()), via.position());
        } else {
            self.vias.push(via);
        }
    }

    /// Adds a trace; anchors scheduled for replacement are replaced by
    /// junctions (one per anchor and layer) created with `new_uuid`.
    pub fn add_trace(&mut self, trace: &Trace, mut new_uuid: impl FnMut() -> Uuid) {
        let mut trace = trace.clone();
        let p1 = self.replace_anchor(trace.p1(), trace.layer(), &mut new_uuid);
        let p2 = self.replace_anchor(trace.p2(), trace.layer(), &mut new_uuid);
        trace.set_anchors(p1, p2);
        self.traces.push(trace);
    }

    /// Returns the cohesive segments: first those with traces (in the order
    /// of their first trace), then each remaining pad and via on its own.
    /// Junctions without traces are dropped.
    pub fn split(self) -> Vec<BoardNetSegmentParts> {
        let mut segments = Vec::new();
        let mut available_traces: BTreeSet<usize> = (0..self.traces.len()).collect();
        let mut available_pads: BTreeSet<usize> = (0..self.pads.len()).collect();
        let mut available_vias: BTreeSet<usize> = (0..self.vias.len()).collect();
        while let Some(&first) = available_traces.first() {
            let mut segment = BoardNetSegmentParts::default();
            let mut stack = vec![self.traces[first].p1()];
            let mut visited: BTreeSet<TraceAnchor> = BTreeSet::new();
            while let Some(anchor) = stack.pop() {
                if !visited.insert(anchor) {
                    continue;
                }
                self.take_anchor(
                    anchor,
                    &mut available_pads,
                    &mut available_vias,
                    &mut segment,
                );
                for (i, trace) in self.traces.iter().enumerate() {
                    if ((trace.p1() == anchor) || (trace.p2() == anchor))
                        && available_traces.remove(&i)
                    {
                        segment.traces.push(trace.clone());
                        stack.push(trace.p2());
                        stack.push(trace.p1());
                    }
                }
            }
            segments.push(segment);
        }
        for i in available_pads {
            segments.push(BoardNetSegmentParts {
                pads: vec![self.pads[i].clone()],
                ..Default::default()
            });
        }
        for i in available_vias {
            segments.push(BoardNetSegmentParts {
                vias: vec![self.vias[i].clone()],
                ..Default::default()
            });
        }
        segments
    }

    fn take_anchor(
        &self,
        anchor: TraceAnchor,
        available_pads: &mut BTreeSet<usize>,
        available_vias: &mut BTreeSet<usize>,
        segment: &mut BoardNetSegmentParts,
    ) {
        match anchor {
            TraceAnchor::Junction(uuid) => {
                if let Some(junction) = self.junctions.iter().find(|j| j.uuid() == uuid)
                    && !segment.junctions.iter().any(|j| j.uuid() == uuid)
                {
                    segment.junctions.push(junction.clone());
                }
            }
            TraceAnchor::Pad(uuid) => {
                if let Some(i) = self.pads.iter().position(|p| p.uuid() == uuid)
                    && available_pads.remove(&i)
                {
                    segment.pads.push(self.pads[i].clone());
                }
            }
            TraceAnchor::Via(uuid) => {
                if let Some(i) = self.vias.iter().position(|v| v.uuid() == uuid)
                    && available_vias.remove(&i)
                {
                    segment.vias.push(self.vias[i].clone());
                }
            }
            TraceAnchor::FootprintPad { .. } => {}
        }
    }

    fn replace_anchor(
        &mut self,
        anchor: TraceAnchor,
        layer: Layer,
        new_uuid: &mut impl FnMut() -> Uuid,
    ) -> TraceAnchor {
        let Some(&position) = self.anchors_to_replace.get(&anchor) else {
            return anchor;
        };
        if let Some(replaced) = self.replaced_anchors.get(&(anchor, layer)) {
            return *replaced;
        }
        let junction = Junction::new(new_uuid(), position);
        let replaced = TraceAnchor::Junction(junction.uuid());
        self.junctions.push(junction);
        self.replaced_anchors.insert((anchor, layer), replaced);
        replaced
    }
}
