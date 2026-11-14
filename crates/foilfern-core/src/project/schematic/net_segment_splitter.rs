//! Port of libs/librepcb/core/project/schematic/schematicnetsegmentsplitter.{h,cpp}.
//!
//! Splits the elements of a net segment (after removing some of them, or
//! when copying a selection) into cohesive segments.
//!
//! Differences to upstream: the depth-first search uses an explicit stack
//! instead of recursion (same visiting order, no stack overflow on long
//! wires); elements are returned as `Vec`s in the order they were found.

use std::collections::HashMap;

use crate::geometry::{Junction, NetLabel, NetLine, NetLineAnchor};
use crate::types::{Length, Point, Uuid};
use crate::utils::toolbox;

/// One of the segments returned by [`SchematicNetSegmentSplitter::split()`]
/// (upstream `Segment`); elements in the order they were found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SplitSegment {
    /// The junctions (only those connected to lines).
    pub junctions: Vec<Junction>,
    /// The net lines.
    pub lines: Vec<NetLine>,
    /// The net labels (each assigned to the nearest segment).
    pub labels: Vec<NetLabel>,
}

/// Splits net segment elements into cohesive segments, see the module
/// documentation.
#[derive(Debug, Clone, Default)]
pub struct SchematicNetSegmentSplitter {
    junctions: Vec<Junction>,
    lines: Vec<NetLine>,
    labels: Vec<NetLabel>,
    fixed_anchors_to_replace: HashMap<NetLineAnchor, NetLineAnchor>,
    fixed_anchor_positions: HashMap<NetLineAnchor, Point>,
}

impl SchematicNetSegmentSplitter {
    /// Creates an empty splitter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Declares the position of a fixed anchor (pin or bus junction). If
    /// `replace_by_junction` is set, lines added afterwards are attached to
    /// a new junction at `pos` instead (e.g. when the pin is not copied).
    pub fn add_fixed_anchor(
        &mut self,
        anchor: NetLineAnchor,
        pos: Point,
        replace_by_junction: bool,
    ) {
        if replace_by_junction {
            let junction = Junction::new(Uuid::new_random(), pos);
            self.fixed_anchors_to_replace
                .insert(anchor, NetLineAnchor::Junction(junction.uuid()));
            self.junctions.push(junction);
        } else {
            self.fixed_anchor_positions.insert(anchor, pos);
        }
    }

    /// Adds a junction.
    pub fn add_junction(&mut self, junction: Junction) {
        self.junctions.push(junction);
    }

    /// Adds a net line (anchors replaced as declared by
    /// [`add_fixed_anchor()`](Self::add_fixed_anchor)).
    pub fn add_net_line(&mut self, line: NetLine) {
        let mut line = line;
        let (p1, p2) = (self.replace(line.p1()), self.replace(line.p2()));
        line.set_anchors(p1, p2);
        self.lines.push(line);
    }

    /// Adds a net label.
    pub fn add_net_label(&mut self, label: NetLabel) {
        self.labels.push(label);
    }

    /// Splits the added elements into cohesive segments; junctions without
    /// lines are dropped, labels go to the nearest segment.
    pub fn split(&self) -> Vec<SplitSegment> {
        let mut available: Vec<usize> = (0..self.lines.len()).collect();
        let mut segments = Vec::new();
        while let Some(&first) = available.first() {
            let mut segment = SplitSegment::default();
            self.find_connected(self.lines[first].p1(), &mut available, &mut segment);
            segments.push(segment);
        }
        for label in &self.labels {
            self.add_label_to_nearest_segment(label, &mut segments);
        }
        segments
    }

    fn replace(&self, anchor: NetLineAnchor) -> NetLineAnchor {
        self.fixed_anchors_to_replace
            .get(&anchor)
            .copied()
            .unwrap_or(anchor)
    }

    /// Upstream `findConnectedLinesAndPoints()`: depth first, visiting the
    /// lines at an anchor in insertion order and for each line first its
    /// `p1`, then its `p2`.
    fn find_connected(
        &self,
        anchor: NetLineAnchor,
        available: &mut Vec<usize>,
        segment: &mut SplitSegment,
    ) {
        struct Frame {
            anchor: NetLineAnchor,
            next_line: usize,
            pending: Option<NetLineAnchor>,
        }
        fn frame(anchor: NetLineAnchor) -> Frame {
            Frame {
                anchor,
                next_line: 0,
                pending: None,
            }
        }
        self.enter(anchor, segment);
        let mut stack = vec![frame(anchor)];
        while let Some(top) = stack.last_mut() {
            if let Some(p2) = top.pending.take() {
                self.enter(p2, segment);
                stack.push(frame(p2));
                continue;
            }
            let anchor = top.anchor;
            let mut found = None;
            while top.next_line < self.lines.len() {
                let i = top.next_line;
                top.next_line += 1;
                let line = &self.lines[i];
                if (line.p1() == anchor || line.p2() == anchor)
                    && available.contains(&i)
                    && !segment.lines.iter().any(|l| l.uuid() == line.uuid())
                {
                    found = Some(i);
                    break;
                }
            }
            match found {
                Some(i) => {
                    let line = &self.lines[i];
                    segment.lines.push(line.clone());
                    if let Some(pos) = available.iter().position(|&a| a == i) {
                        available.remove(pos);
                    }
                    top.pending = Some(line.p2());
                    let p1 = line.p1();
                    self.enter(p1, segment);
                    stack.push(frame(p1));
                }
                None => {
                    stack.pop();
                }
            }
        }
    }

    /// Adds the junction of `anchor` (if it is a known junction) to the
    /// segment when the search reaches it.
    fn enter(&self, anchor: NetLineAnchor, segment: &mut SplitSegment) {
        if let NetLineAnchor::Junction(uuid) = anchor
            && let Some(junction) = self.junctions.iter().find(|j| j.uuid() == uuid)
            && !segment.junctions.iter().any(|j| j.uuid() == uuid)
        {
            segment.junctions.push(junction.clone());
        }
    }

    fn add_label_to_nearest_segment(&self, label: &NetLabel, segments: &mut [SplitSegment]) {
        let mut nearest: Option<(usize, Length)> = None;
        for (i, segment) in segments.iter().enumerate() {
            let distance = self.distance(label, segment);
            if nearest.is_none_or(|(_, d)| distance < d) {
                nearest = Some((i, distance));
            }
        }
        if let Some((i, _)) = nearest {
            segments[i].labels.push(label.clone());
        }
    }

    /// Upstream `getDistanceBetweenNetLabelAndNetSegment()` (0 for a
    /// segment without lines, which cannot occur).
    fn distance(&self, label: &NetLabel, segment: &SplitSegment) -> Length {
        segment
            .lines
            .iter()
            .map(|line| {
                *toolbox::shortest_distance_between_point_and_line(
                    label.position(),
                    self.anchor_position(line.p1()),
                    self.anchor_position(line.p2()),
                )
                .0
            })
            .min()
            .unwrap_or(Length::ZERO)
    }

    fn anchor_position(&self, anchor: NetLineAnchor) -> Point {
        if let Some(pos) = self.fixed_anchor_positions.get(&anchor) {
            return *pos;
        }
        if let NetLineAnchor::Junction(uuid) = anchor
            && let Some(junction) = self.junctions.iter().find(|j| j.uuid() == uuid)
        {
            return junction.position();
        }
        log::warn!("Failed to determine position of net label anchor while splitting segments!");
        Point::ORIGIN
    }
}
