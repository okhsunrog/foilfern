//! Port of libs/librepcb/core/algorithm/netsegmentsimplifier.{h,cpp}.
//!
//! Differences to upstream:
//! - Layers are `Option<Layer>` (`None` for schematics) instead of
//!   nullable pointers.
//! - Junctions are merged in ascending ID order (upstream iterates a
//!   `QHash`, i.e. in an unspecified order).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::types::{Layer, Length, Point};
use crate::utils::toolbox::shortest_distance_between_point_and_line;

/// Type of a line anchor. The order matters: at the same position, vias are
/// preferred over pins/pads, which are preferred over junctions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AnchorType {
    /// A via (board only).
    Via,
    /// A symbol pin or footprint pad.
    Fixed,
    /// A junction (freely movable).
    Junction,
}

/// A net line (wire or trace) between two anchors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetSegmentLine {
    /// Line ID (see [`NetSegmentSimplifier::add_line()`]).
    pub id: usize,
    /// ID of the start anchor.
    pub p1: usize,
    /// ID of the end anchor.
    pub p2: usize,
    /// Layer (`None` for schematics).
    pub layer: Option<Layer>,
    /// Line width.
    pub width: Length,
    /// Whether the simplification modified the line (or created it).
    pub modified: bool,
}

/// Result of [`NetSegmentSimplifier::simplify()`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SimplifyResult {
    /// Remaining lines, sorted by ID. Lines created by splitting have new
    /// IDs, and may reference new junctions.
    pub lines: Vec<NetSegmentLine>,
    /// New junctions (anchor ID → position) created at line intersections.
    pub new_junctions: BTreeMap<usize, Point>,
    /// Pins/pads which were connected to a line before, but not anymore.
    pub disconnected_fixed_anchors: BTreeSet<usize>,
    /// Whether anything was modified.
    pub modified: bool,
}

#[derive(Debug, Clone)]
struct Anchor {
    id: usize,
    anchor_type: AnchorType,
    pos: Point,
    start_layer: Option<Layer>,
    end_layer: Option<Layer>,
    is_new: bool,
}

/// Algorithm to clean up/simplify the lines of a net segment.
///
/// Performed operations:
/// - Remove redundant junctions (same position, same layer).
/// - Remove redundant lines (same anchors, same layer), keeping the thickest.
/// - Remove zero-length lines.
/// - Remove useless junctions within straight line segments (join line
///   segments into the same direction to a single line).
/// - Split lines to connect with anchors on the path between their start
///   and end points.
/// - Split intersecting lines, placing a new junction to connect them (only
///   orthogonal intersections for now).
#[derive(Debug, Default)]
pub struct NetSegmentSimplifier {
    // Input.
    anchors: Vec<Anchor>,
    lines: BTreeMap<usize, NetSegmentLine>,
    next_free_line_id: usize,
    // State.
    /// Anchor IDs by position, fixed anchors (vias, pads) first.
    anchor_map: HashMap<Point, Vec<usize>>,
    pins_or_pads: BTreeSet<usize>,
    modified: bool,
}

impl NetSegmentSimplifier {
    /// Creates an empty simplifier.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a line anchor and returns its ID.
    ///
    /// `start` and `end` are the top-most and bottom-most copper layers of
    /// the anchor (`None` for schematics).
    pub fn add_anchor(
        &mut self,
        anchor_type: AnchorType,
        pos: Point,
        start: Option<Layer>,
        end: Option<Layer>,
    ) -> usize {
        let id = self.anchors.len();
        self.anchors.push(Anchor {
            id,
            anchor_type,
            pos,
            start_layer: start,
            end_layer: end,
            is_new: false,
        });
        id
    }

    /// Adds a line between two anchors and returns its ID.
    ///
    /// # Panics
    ///
    /// [`simplify()`](Self::simplify) panics if an anchor ID was not
    /// returned by [`add_anchor()`](Self::add_anchor).
    pub fn add_line(&mut self, p1: usize, p2: usize, layer: Option<Layer>, width: Length) -> usize {
        debug_assert!(p1 < self.anchors.len() && p2 < self.anchors.len());
        let id = self.lines.len();
        self.lines.insert(
            id,
            NetSegmentLine {
                id,
                p1,
                p2,
                layer,
                width,
                modified: false,
            },
        );
        id
    }

    /// Performs the simplification and resets the state, so the object can
    /// be reused for the next net segment.
    ///
    /// Attention: when lines are split, new anchor and line IDs are
    /// generated, so the returned lines may contain IDs not returned by
    /// [`add_anchor()`](Self::add_anchor) and [`add_line()`](Self::add_line).
    pub fn simplify(&mut self) -> SimplifyResult {
        // Clear state.
        self.anchor_map.clear();
        self.pins_or_pads.clear();
        self.next_free_line_id = self.lines.len();
        self.modified = false;

        // Group all anchors by position. Fixed anchors (pads & vias) must
        // appear first, and non-fixed anchors (junctions) last.
        for anchor in &self.anchors {
            self.anchor_map
                .entry(anchor.pos)
                .or_default()
                .push(anchor.id);
        }
        for ids in self.anchor_map.values_mut() {
            ids.sort_by_key(|&id| self.anchors[id].anchor_type);
        }

        // Get all IDs of pins or pads.
        self.pins_or_pads = self
            .anchors
            .iter()
            .filter(|a| a.anchor_type == AnchorType::Fixed)
            .map(|a| a.id)
            .collect();

        // Memorize which pins or pads are currently connected.
        let connected_fixed_anchors = self.connected_fixed_anchors();

        // Add junctions where lines are intersecting each other. Those lines
        // will then be split in the next step to connect with the new
        // anchors.
        self.add_junctions_at_line_intersections();

        // Split lines by anchors intersecting them.
        self.split_lines_at_anchors();

        // Replace unnecessary junctions by the first suitable anchor from
        // the anchor map. Pads and vias have priority, junctions are only
        // used if they are not redundant with any pad or via.
        self.remove_duplicate_junctions();

        // Remove redundant lines, keeping the thickest.
        self.remove_redundant_lines();

        // Remove unnecessary junctions in the middle of straight lines, one
        // by one until no more lines can be merged.
        while self.merge_next_lines() {
            self.modified = true;
        }

        let disconnected_fixed_anchors = connected_fixed_anchors
            .difference(&self.connected_fixed_anchors())
            .copied()
            .collect();
        let new_junctions = self
            .anchors
            .iter()
            .filter(|a| a.is_new)
            .map(|a| (a.id, a.pos))
            .collect();
        let result = SimplifyResult {
            lines: std::mem::take(&mut self.lines).into_values().collect(),
            new_junctions,
            disconnected_fixed_anchors,
            modified: self.modified,
        };
        self.anchors.clear();
        result
    }

    fn pos(&self, anchor: usize) -> Point {
        self.anchors[anchor].pos
    }

    fn connected_fixed_anchors(&self) -> BTreeSet<usize> {
        self.lines
            .values()
            .flat_map(|line| [line.p1, line.p2])
            .filter(|id| self.pins_or_pads.contains(id))
            .collect()
    }

    fn add_junctions_at_line_intersections(&mut self) {
        let lines: Vec<NetSegmentLine> = self.lines.values().cloned().collect();
        for (i, line0) in lines.iter().enumerate() {
            let (a1, a2) = (self.pos(line0.p1), self.pos(line0.p2));
            for line1 in &lines[i + 1..] {
                if line0.layer != line1.layer {
                    continue;
                }
                let (b1, b2) = (self.pos(line1.p1), self.pos(line1.p2));
                let Some(pos) = orthogonal_intersection(a1, a2, b1, b2) else {
                    continue;
                };
                if self.find_anchor(pos, line0.layer).is_none() {
                    let id = self.anchors.len();
                    self.anchors.push(Anchor {
                        id,
                        anchor_type: AnchorType::Junction,
                        pos,
                        start_layer: line0.layer,
                        end_layer: line0.layer,
                        is_new: true,
                    });
                    self.anchor_map.entry(pos).or_default().push(id);
                }
            }
        }
    }

    fn split_lines_at_anchors(&mut self) {
        let find_intersecting_anchor = |line: &NetSegmentLine| {
            let p1 = self.pos(line.p1);
            let p2 = self.pos(line.p2);
            if p1 == p2 {
                return None;
            }
            self.anchors
                .iter()
                .find(|a| {
                    a.pos != p1
                        && a.pos != p2
                        && is_anchor_on_layer(a, line.layer)
                        && is_straight_line(p1, a.pos, p2)
                })
                .map(|a| a.id)
        };

        // This is done iteratively because the same line may need to be
        // split multiple times. To recover from a (theoretical) endless
        // loop, the number of new lines is limited and the result is only
        // applied if the limit was not reached.
        let mut lines = self.lines.clone();
        let mut next_free_line_id = self.next_free_line_id;
        let mut finished_line_ids = BTreeSet::new();
        let max_lines_count = self.lines.len() * 2 + 10;
        let mut modified = false;
        loop {
            // Find the next line to split.
            let mut next_split = None;
            for line in lines.values() {
                if finished_line_ids.contains(&line.id) {
                    continue; // Already processed.
                }
                if let Some(anchor) = find_intersecting_anchor(line) {
                    next_split = Some((line.id, anchor));
                    break;
                }
                finished_line_ids.insert(line.id);
            }
            let Some((id, anchor)) = next_split else {
                break;
            };
            let Some(line) = lines.get_mut(&id) else {
                break; // Unreachable, the ID was just found.
            };
            let new_line = NetSegmentLine {
                id: next_free_line_id,
                p1: anchor,
                p2: line.p2,
                layer: line.layer,
                width: line.width,
                modified: true,
            };
            // Split the existing line.
            line.p2 = anchor;
            line.modified = true;
            lines.insert(new_line.id, new_line);
            next_free_line_id += 1;
            modified = true;
            if lines.len() >= max_lines_count {
                // Discard all changes (upstream logs a warning).
                return;
            }
        }

        // Apply result only on success.
        if modified {
            self.lines = lines;
            self.next_free_line_id = next_free_line_id;
            self.modified = true;
        }
    }

    fn remove_duplicate_junctions(&mut self) {
        let convert_line_anchor = |anchor: usize, layer: Option<Layer>| {
            let anchor = &self.anchors[anchor];
            if anchor.anchor_type == AnchorType::Junction
                && let Some(existing) = self.find_anchor(anchor.pos, layer)
            {
                return existing;
            }
            anchor.id
        };
        let ids: Vec<usize> = self.lines.keys().rev().copied().collect();
        let mut removed = Vec::new();
        let mut changed = Vec::new();
        for id in ids {
            let line = &self.lines[&id];
            let p1 = convert_line_anchor(line.p1, line.layer);
            let p2 = convert_line_anchor(line.p2, line.layer);
            if p1 == p2 {
                // Start and end anchor are now the same, which is invalid and
                // would be a zero-length line anyway, so remove it.
                removed.push(id);
            } else if unordered(line.p1, line.p2) != unordered(p1, p2) {
                changed.push((id, p1, p2));
            }
        }
        for id in removed {
            self.lines.remove(&id);
            self.modified = true;
        }
        for (id, p1, p2) in changed {
            if let Some(line) = self.lines.get_mut(&id) {
                line.p1 = p1;
                line.p2 = p2;
                line.modified = true;
                self.modified = true;
            }
        }
    }

    fn remove_redundant_lines(&mut self) {
        let ids: Vec<usize> = self.lines.keys().copied().collect();
        for id in ids {
            let line = &self.lines[&id];
            let is_duplicate = self.lines.values().any(|other| {
                other.id != line.id
                    && other.layer == line.layer
                    && other.width >= line.width
                    && unordered(other.p1, other.p2) == unordered(line.p1, line.p2)
            });
            if is_duplicate {
                self.lines.remove(&id);
                self.modified = true;
            }
        }
    }

    fn merge_next_lines(&mut self) -> bool {
        // Collect all junctions (no vias and no pads!) and their lines.
        let mut junction_lines: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for line in self.lines.values() {
            for anchor in [line.p1, line.p2] {
                if self.anchors[anchor].anchor_type == AnchorType::Junction {
                    junction_lines.entry(anchor).or_default().push(line.id);
                }
            }
        }

        // Find the next two lines which can be merged, i.e. a junction
        // located exactly between two line anchors.
        for (&junction, line_ids) in &junction_lines {
            let &[id0, id1] = line_ids.as_slice() else {
                continue;
            };
            let (line0, line1) = (&self.lines[&id0], &self.lines[&id1]);
            let anchor0 = if line0.p1 == junction {
                line0.p2
            } else {
                line0.p1
            };
            let anchor1 = if line1.p1 == junction {
                line1.p2
            } else {
                line1.p1
            };
            if line0.layer != line1.layer
                || line0.width != line1.width
                || !self.is_straight(anchor0, junction, anchor1)
            {
                continue;
            }
            // Merge these two lines! But check first if such a direct line
            // already exists. In that case, just remove the redundant lines
            // and keep the thicker width.
            let (layer, width) = (line0.layer, line0.width);
            let existing = self
                .lines
                .values()
                .find(|l| l.layer == layer && unordered(l.p1, l.p2) == unordered(anchor0, anchor1))
                .map(|l| l.id);
            if let Some(existing) = existing {
                if let Some(line) = self.lines.get_mut(&existing)
                    && line.width < width
                {
                    line.width = width;
                    line.modified = true;
                }
                self.lines.remove(&id0);
                self.lines.remove(&id1);
            } else {
                if let Some(line) = self.lines.get_mut(&id0) {
                    line.p1 = anchor0;
                    line.p2 = anchor1;
                    line.modified = true;
                }
                self.lines.remove(&id1);
            }
            return true;
        }
        false
    }

    /// Checks whether `junction` lies on the straight line between the two
    /// anchors.
    fn is_straight(&self, anchor0: usize, junction: usize, anchor1: usize) -> bool {
        let (p0, p1, p2) = (self.pos(anchor0), self.pos(junction), self.pos(anchor1));
        if p0 == p1 || p0 == p2 || p1 == p2 {
            // Redundant junctions should have been removed already
            // (upstream logs a warning).
            return false;
        }
        is_straight_line(p0, p1, p2)
    }

    /// Returns the first anchor at `pos` on `layer` (fixed anchors first).
    fn find_anchor(&self, pos: Point, layer: Option<Layer>) -> Option<usize> {
        self.anchor_map
            .get(&pos)?
            .iter()
            .copied()
            .find(|&id| is_anchor_on_layer(&self.anchors[id], layer))
    }
}

/// Returns the anchor pair as set (for comparisons like upstream's
/// `QSet<int>{p1, p2}`).
fn unordered(a: usize, b: usize) -> (usize, usize) {
    (a.min(b), a.max(b))
}

fn is_anchor_on_layer(anchor: &Anchor, layer: Option<Layer>) -> bool {
    match (layer, anchor.start_layer, anchor.end_layer) {
        (Some(layer), Some(start), Some(end)) => {
            (start.copper_number()..=end.copper_number()).contains(&layer.copper_number())
        }
        _ => true,
    }
}

/// Returns the intersection point of a horizontal and a vertical line
/// crossing each other (touching doesn't count).
fn orthogonal_intersection(a1: Point, a2: Point, b1: Point, b2: Point) -> Option<Point> {
    // Line 'a' must be horizontal and line 'b' vertical.
    let intersects = |a1: Point, a2: Point, b1: Point, b2: Point| {
        let ay = a1.y;
        let (ax0, ax1) = (a1.x.min(a2.x), a1.x.max(a2.x));
        let bx = b1.x;
        let (by0, by1) = (b1.y.min(b2.y), b1.y.max(b2.y));
        ax0 < bx && bx < ax1 && by0 < ay && ay < by1
    };
    if a1.y == a2.y && b1.x == b2.x && intersects(a1, a2, b1, b2) {
        Some(Point::new(b1.x, a1.y))
    } else if a1.x == a2.x && b1.y == b2.y && intersects(b1, b2, a1, a2) {
        Some(Point::new(a1.x, b1.y))
    } else {
        None
    }
}

/// Returns whether `p1` lies on the straight line from `p0` to `p2`.
fn is_straight_line(p0: Point, p1: Point, p2: Point) -> bool {
    if p0.x == p1.x {
        p2.x == p1.x && ((p0.y < p1.y) == (p1.y < p2.y))
    } else if p0.y == p1.y {
        p2.y == p1.y && ((p0.x < p1.x) == (p1.x < p2.x))
    } else {
        // Not sure what tolerance we should allow for non-90° lines...
        let length = *(p2 - p0).length();
        let tolerance = (length / 100).min(Length::new(50));
        let (distance, _) = shortest_distance_between_point_and_line(p1, p0, p2);
        *distance < tolerance
    }
}
