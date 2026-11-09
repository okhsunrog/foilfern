//! A* path search on the routing grid.
//!
//! A search state is `(node, layer, direction)`: the direction of the move
//! that entered the node (8 compass directions, or "none" at the start and
//! after a via). Keeping the direction in the state allows
//!
//! - bend costs (45° and 90° turns cost extra, so paths have few corners
//!   instead of the staircases that ties on an 8-connected grid produce),
//! - forbidding acute angles (turns of more than 90°).
//!
//! Costs are integers: 100 per straight step, 141 per diagonal step, scaled
//! by the preferred direction factor of the layer. Layer changes (vias) cost
//! the configured via cost. The heuristic is the octile distance to the
//! bounding box of the targets, weighted by 1.5 (weighted A*: much fewer
//! expansions, paths at most 1.5 times the optimal cost). States that cost
//! more than a 90° turn above the best state of the same node are pruned.
//!
//! The search is multi-source and multi-target: sources and targets carry an
//! initial/final cost (the stub from a terminal anchor to the grid node) and
//! the search finishes when no open state can beat the best target reached.
//!
//! Soft mode (rip-up): nodes occupied only by routes of other nets are
//! passable at a high penalty, so the search finds the path that conflicts
//! with the fewest routes; those are then ripped up.
//!
//! The implementation is a hand-written A* over dense arrays (with a
//! generation stamp instead of clearing them) because generic crates such as
//! `pathfinding` keep the visited set in hash maps, which is several times
//! slower for the ~10⁵ states a search visits, and do not support multiple
//! sources with initial costs.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use crate::grid::{Grid, usable};

/// Direction deltas `(dx, dy)`, counter-clockwise starting east.
const DIRS: [(i64, i64); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];
/// Direction value for "no previous move".
const NO_DIR: usize = 8;
const STATES_PER_NODE: usize = 9;
const NONE: u32 = u32::MAX;

/// Cost of a straight step.
pub(crate) const STEP: u32 = 100;
const DIAG: u32 = 141;
const TURN_45: u32 = 30;
const TURN_90: u32 = 90;
/// Penalty for using a node occupied by another route (soft mode).
const CONFLICT: u32 = 3000;
/// Upper bound of states expanded by one search.
const MAX_EXPANSIONS: usize = 3_000_000;
/// Weight of the heuristic in percent (weighted A*: faster, paths at most
/// this much longer than optimal).
const HEURISTIC_WEIGHT: u32 = 200;
/// States costing more than this above the best state of their node are
/// pruned.
const DOMINANCE: u32 = TURN_45;

/// Preferred direction cost factors (percent) per direction index.
pub(crate) type DirFactors = [u32; 8];

/// Neutral factors.
pub(crate) const NEUTRAL: DirFactors = [100; 8];

/// Everything a search needs besides the reusable buffers.
pub(crate) struct Query<'a> {
    pub grid: &'a Grid,
    pub layers: usize,
    /// Trace map of the net's width (fixed and routes), `layers * cells`.
    pub trace_fixed: &'a [u32],
    pub trace_routes: &'a [u32],
    /// Via map (fixed and routes), `cells`.
    pub via_fixed: &'a [u32],
    pub via_routes: &'a [u32],
    /// Map value of the net (`net + 1`).
    pub owner: u32,
    pub via_cost: u32,
    pub factors: &'a [DirFactors],
    pub soft: bool,
    /// Source nodes (`layer * cells + cell`) with initial cost.
    pub sources: &'a [(u32, u32)],
    /// Target nodes with final cost.
    pub targets: &'a HashMap<u32, u32>,
}

/// A found path.
#[derive(Debug, Clone)]
pub(crate) struct Found {
    /// Nodes (`layer * cells + cell`) from source to target.
    pub nodes: Vec<u32>,
}

/// Reusable search buffers.
#[derive(Debug, Default)]
pub(crate) struct Search {
    g: Vec<u32>,
    parent: Vec<u32>,
    stamp: Vec<u32>,
    /// Best cost per node (any direction), for dominance pruning.
    node_g: Vec<u32>,
    node_stamp: Vec<u32>,
    generation: u32,
    pub expansions: usize,
}

impl Search {
    pub fn run(&mut self, q: &Query<'_>) -> Option<Found> {
        let cells = q.grid.cells();
        let nodes = cells * q.layers;
        let states = nodes * STATES_PER_NODE;
        if self.g.len() != states {
            self.g = vec![0; states];
            self.parent = vec![NONE; states];
            self.stamp = vec![0; states];
            self.node_g = vec![0; nodes];
            self.node_stamp = vec![0; nodes];
            self.generation = 0;
        }
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.stamp.iter_mut().for_each(|s| *s = 0);
            self.node_stamp.iter_mut().for_each(|s| *s = 0);
            self.generation = 1;
        }
        if q.sources.is_empty() || q.targets.is_empty() {
            return None;
        }

        // Target bounding box in grid coordinates for the heuristic.
        let nx = q.grid.nx;
        let (mut tx0, mut ty0, mut tx1, mut ty1) = (usize::MAX, usize::MAX, 0, 0);
        for &node in q.targets.keys() {
            let c = node as usize % cells;
            let (x, y) = (c % nx, c / nx);
            tx0 = tx0.min(x);
            ty0 = ty0.min(y);
            tx1 = tx1.max(x);
            ty1 = ty1.max(y);
        }
        let heuristic = |c: usize| -> u32 {
            let (x, y) = (c % nx, c / nx);
            let dx = if x < tx0 {
                tx0 - x
            } else {
                x.saturating_sub(tx1)
            } as u32;
            let dy = if y < ty0 {
                ty0 - y
            } else {
                y.saturating_sub(ty1)
            } as u32;
            let (lo, hi) = (dx.min(dy), dx.max(dy));
            (STEP * (hi - lo) + DIAG * lo) * HEURISTIC_WEIGHT / 100
        };

        let generation = self.generation;
        let mut heap: BinaryHeap<Reverse<(u32, u32, u32)>> = BinaryHeap::new();
        for &(node, cost) in q.sources {
            let s = node as usize * STATES_PER_NODE + NO_DIR;
            if self.stamp[s] == generation && self.g[s] <= cost {
                continue;
            }
            self.stamp[s] = generation;
            self.g[s] = cost;
            self.parent[s] = NONE;
            let f = cost + heuristic(node as usize % cells);
            heap.push(Reverse((f, u32::MAX - cost, s as u32)));
        }

        let mut best: Option<(u32, u32)> = None; // (total cost, state)
        self.expansions = 0;
        while let Some(Reverse((f, inv_g, s))) = heap.pop() {
            if best.is_some_and(|(cost, _)| f >= cost) {
                break;
            }
            let s = s as usize;
            let g = u32::MAX - inv_g;
            if self.g[s] != g {
                continue; // stale
            }
            let node = s / STATES_PER_NODE;
            let dir = s % STATES_PER_NODE;
            // Dominance pruning: a state much worse than the best state of
            // the same node (in another direction) is not worth expanding.
            if self.node_stamp[node] == generation {
                if g > self.node_g[node] + DOMINANCE {
                    continue;
                }
                self.node_g[node] = self.node_g[node].min(g);
            } else {
                self.node_stamp[node] = generation;
                self.node_g[node] = g;
            }
            self.expansions += 1;
            if self.expansions > MAX_EXPANSIONS {
                break;
            }
            if let Some(&extra) = q.targets.get(&(node as u32)) {
                let total = g.saturating_add(extra);
                if best.is_none_or(|(cost, _)| total < cost) {
                    best = Some((total, s as u32));
                }
            }
            let layer = node / cells;
            let c = node % cells;
            let (x, y) = ((c % nx) as i64, (c / nx) as i64);

            let mut relax = |heap: &mut BinaryHeap<_>, ns: usize, ng: u32, nc: usize| {
                let nn = ns / STATES_PER_NODE;
                if self.node_stamp[nn] == generation && ng > self.node_g[nn] + DOMINANCE {
                    return;
                }
                if self.stamp[ns] != generation || ng < self.g[ns] {
                    self.stamp[ns] = generation;
                    self.g[ns] = ng;
                    self.parent[ns] = s as u32;
                    heap.push(Reverse((ng + heuristic(nc), u32::MAX - ng, ns as u32)));
                }
            };

            // Moves within the layer.
            for (k, (dx, dy)) in DIRS.iter().enumerate() {
                let turn = if dir == NO_DIR {
                    0
                } else {
                    match (k + 8 - dir) % 8 {
                        0 => 0,
                        1 | 7 => TURN_45,
                        2 | 6 => TURN_90,
                        _ => continue,
                    }
                };
                let (nx_, ny_) = (x + dx, y + dy);
                if nx_ < 0 || ny_ < 0 || nx_ >= nx as i64 || ny_ >= q.grid.ny as i64 {
                    continue;
                }
                let nc = ny_ as usize * nx + nx_ as usize;
                let idx = layer * cells + nc;
                if !usable(q.trace_fixed[idx], q.owner) {
                    continue;
                }
                let mut cost = if k % 2 == 0 { STEP } else { DIAG } * q.factors[layer][k] / 100;
                if !usable(q.trace_routes[idx], q.owner) {
                    if !q.soft {
                        continue;
                    }
                    cost += CONFLICT;
                }
                let ng = g + cost + turn;
                relax(&mut heap, idx * STATES_PER_NODE + k, ng, nc);
            }

            // Layer changes.
            if q.layers > 1 && usable(q.via_fixed[c], q.owner) {
                let mut cost = q.via_cost;
                let via_free = usable(q.via_routes[c], q.owner);
                if via_free || q.soft {
                    if !via_free {
                        cost += CONFLICT;
                    }
                    for l in (0..q.layers).filter(|&l| l != layer) {
                        let idx = l * cells + c;
                        if !usable(q.trace_fixed[idx], q.owner) {
                            continue;
                        }
                        let mut extra = 0;
                        if !usable(q.trace_routes[idx], q.owner) {
                            if !q.soft {
                                continue;
                            }
                            extra = CONFLICT;
                        }
                        relax(
                            &mut heap,
                            idx * STATES_PER_NODE + NO_DIR,
                            g + cost + extra,
                            c,
                        );
                    }
                }
            }
        }

        let (_, goal) = best?;
        let mut nodes = Vec::new();
        let mut s = goal;
        while s != NONE {
            let node = s / STATES_PER_NODE as u32;
            if nodes.last() != Some(&node) {
                nodes.push(node);
            }
            s = self.parent[s as usize];
        }
        nodes.reverse();
        Some(Found { nodes })
    }
}
