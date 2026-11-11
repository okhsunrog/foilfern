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
//! bounding box of the targets. It is weighted to reduce expansions.
//! States sufficiently above the best cost at the same node are pruned.
//!
//! The search is multi-source and multi-target: sources and targets carry an
//! initial/final cost (the stub from a terminal anchor to the grid node) and
//! the search finishes when no open state can beat the best target reached.
//!
//! Soft mode (rip-up): nodes occupied only by routes of other nets are
//! passable at a high penalty. Optional history discourages crossing locations
//! where previous paths already displaced other routes.
//!
//! The implementation is a hand-written A* over dense arrays (with a
//! generation stamp instead of clearing them) because generic crates such as
//! `pathfinding` keep the visited set in hash maps, which is several times
//! slower for the ~10⁵ states a search visits, and do not support multiple
//! sources with initial costs.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use crate::congestion::Congestion;
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
pub(crate) const TURN_45: u32 = 30;
const TURN_90: u32 = 90;
/// Penalty for using a node occupied by another route (soft mode).
const CONFLICT: u32 = 3000;
/// Upper bound of states expanded by one search.
const MAX_EXPANSIONS: usize = 3_000_000;
/// Weight of the heuristic in percent.
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
    /// Present only for a soft search after learning a conflict.
    pub congestion: Option<&'a Congestion>,
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

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SearchFailure {
    Exhausted,
    Budget,
}

/// A rejected grid step, indexed in the same coordinates as the query maps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockedStep {
    pub from: usize,
    pub to: usize,
    pub via: bool,
    pub generated: bool,
}

const FRONTIER_SAMPLES_PER_KIND: usize = 8;

#[derive(Debug, Default)]
struct Frontier {
    samples: [Vec<(u32, BlockedStep)>; 4],
}

impl Frontier {
    fn record(&mut self, score: u32, step: BlockedStep) {
        let kind = 2 * usize::from(step.via) + usize::from(step.generated);
        let samples = &mut self.samples[kind];
        if samples.len() == FRONTIER_SAMPLES_PER_KIND && score >= samples.last().unwrap().0 {
            return;
        }
        if samples.iter().any(|(_, seen)| seen.to == step.to) {
            return;
        }
        let index = samples.partition_point(|(value, _)| *value <= score);
        samples.insert(index, (score, step));
        samples.truncate(FRONTIER_SAMPLES_PER_KIND);
    }
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
    frontier: Frontier,
}

impl Search {
    pub fn run(&mut self, q: &Query<'_>, collect_evidence: bool) -> Result<Found, SearchFailure> {
        self.run_with_limit(q, MAX_EXPANSIONS, collect_evidence)
    }

    pub fn frontier(&self) -> impl Iterator<Item = BlockedStep> + '_ {
        self.frontier
            .samples
            .iter()
            .flatten()
            .map(|(_, step)| *step)
    }

    fn run_with_limit(
        &mut self,
        q: &Query<'_>,
        limit: usize,
        collect_evidence: bool,
    ) -> Result<Found, SearchFailure> {
        for samples in &mut self.frontier.samples {
            samples.clear();
        }
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
            return Err(SearchFailure::Exhausted);
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
            let f = cost.saturating_add(heuristic(node as usize % cells));
            heap.push(Reverse((f, u32::MAX - cost, s as u32)));
        }

        let mut best: Option<(u32, u32)> = None; // (total cost, state)
        let mut expansions = 0;
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
                if g > self.node_g[node].saturating_add(DOMINANCE) {
                    continue;
                }
                self.node_g[node] = self.node_g[node].min(g);
            } else {
                self.node_stamp[node] = generation;
                self.node_g[node] = g;
            }
            expansions += 1;
            if expansions > limit {
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
            let mut record_block = |to, target_cell, via, generated| {
                if collect_evidence {
                    self.frontier.record(
                        heuristic(target_cell),
                        BlockedStep {
                            from: node,
                            to,
                            via,
                            generated,
                        },
                    );
                }
            };

            let mut relax = |heap: &mut BinaryHeap<_>, ns: usize, ng: u32, nc: usize| {
                let nn = ns / STATES_PER_NODE;
                if self.node_stamp[nn] == generation
                    && ng > self.node_g[nn].saturating_add(DOMINANCE)
                {
                    return;
                }
                if self.stamp[ns] != generation || ng < self.g[ns] {
                    self.stamp[ns] = generation;
                    self.g[ns] = ng;
                    self.parent[ns] = s as u32;
                    heap.push(Reverse((
                        ng.saturating_add(heuristic(nc)),
                        u32::MAX - ng,
                        ns as u32,
                    )));
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
                    record_block(idx, nc, false, false);
                    continue;
                }
                let mut cost = if k % 2 == 0 { STEP } else { DIAG } * q.factors[layer][k] / 100;
                if !usable(q.trace_routes[idx], q.owner) {
                    if !q.soft {
                        record_block(idx, nc, false, true);
                        continue;
                    }
                    cost += CONFLICT;
                }
                if let Some(history) = q.congestion {
                    cost += history.cost(history.tile(layer, nc));
                }
                let ng = g.saturating_add(cost + turn);
                relax(&mut heap, idx * STATES_PER_NODE + k, ng, nc);
            }

            // Layer changes.
            if q.layers > 1 && !usable(q.via_fixed[c], q.owner) {
                record_block(node, c, true, false);
            } else if q.layers > 1 {
                let mut cost = q.via_cost;
                let via_free = usable(q.via_routes[c], q.owner);
                if !via_free && !q.soft {
                    record_block(node, c, true, true);
                } else {
                    if !via_free {
                        cost = cost.saturating_add(CONFLICT);
                    }
                    if let Some(history) = q.congestion {
                        cost = cost.saturating_add(history.cost(history.via_tile(c)));
                    }
                    for l in (0..q.layers).filter(|&l| l != layer) {
                        let idx = l * cells + c;
                        if !usable(q.trace_fixed[idx], q.owner) {
                            record_block(idx, c, true, false);
                            continue;
                        }
                        let mut extra = 0;
                        if !usable(q.trace_routes[idx], q.owner) {
                            if !q.soft {
                                record_block(idx, c, true, true);
                                continue;
                            }
                            extra = CONFLICT;
                        }
                        if let Some(history) = q.congestion {
                            extra += history.cost(history.tile(l, c));
                        }
                        relax(
                            &mut heap,
                            idx * STATES_PER_NODE + NO_DIR,
                            g.saturating_add(cost).saturating_add(extra),
                            c,
                        );
                    }
                }
            }
        }

        let Some((_, goal)) = best else {
            return Err(if expansions > limit {
                SearchFailure::Budget
            } else {
                SearchFailure::Exhausted
            });
        };
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
        Ok(Found { nodes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::P;
    use crate::grid::{BLOCKED, FREE};

    #[test]
    fn distinguishes_search_budget_from_exhausted_frontier() {
        let grid = Grid::new(P::new(0, 0), P::new(2, 0), 1);
        let mut query = Query {
            grid: &grid,
            layers: 1,
            trace_fixed: &[FREE; 3],
            trace_routes: &[FREE; 3],
            via_fixed: &[FREE; 3],
            via_routes: &[FREE; 3],
            owner: 1,
            via_cost: 0,
            factors: &[NEUTRAL],
            soft: false,
            congestion: None,
            sources: &[(0, 0)],
            targets: &HashMap::from([(2, 0)]),
        };
        let mut search = Search::default();
        assert_eq!(
            search.run_with_limit(&query, 1, false).unwrap_err(),
            SearchFailure::Budget
        );
        assert_eq!(search.run(&query, false).unwrap().nodes, vec![0, 1, 2]);
        query.trace_fixed = &[FREE, BLOCKED, FREE];
        assert_eq!(
            search.run_with_limit(&query, 1, true).unwrap_err(),
            SearchFailure::Exhausted
        );
        assert_eq!(
            search
                .frontier()
                .map(|sample| sample.to)
                .collect::<Vec<_>>(),
            [1]
        );
    }

    #[test]
    fn soft_search_switches_to_a_fresh_crossing_after_learning_contention() {
        let grid = Grid::new(P::new(0, 0), P::new(16, 8), 1);
        let cell = |x, y| y * grid.nx + x;
        let (hot, alternative) = (cell(8, 3), cell(8, 5));
        let mut fixed = vec![FREE; grid.cells()];
        for y in 0..grid.ny {
            let i = cell(8, y);
            if i != hot && i != alternative {
                fixed[i] = BLOCKED;
            }
        }
        let mut routes = vec![FREE; grid.cells()];
        routes[hot] = 2;
        routes[alternative] = 3;
        let vias = vec![FREE; grid.cells()];
        let mut query = Query {
            grid: &grid,
            layers: 1,
            trace_fixed: &fixed,
            trace_routes: &routes,
            via_fixed: &vias,
            via_routes: &vias,
            owner: 1,
            via_cost: 0,
            factors: &[NEUTRAL],
            soft: true,
            congestion: None,
            sources: &[(cell(1, 3) as u32, 0)],
            targets: &HashMap::from([(cell(15, 3) as u32, 0)]),
        };
        let mut search = Search::default();
        let path = search.run(&query, false).unwrap();
        assert!(path.nodes.contains(&(hot as u32)));
        let mut history = Congestion::new(grid, 1);
        for _ in 0..3 {
            history.record(vec![history.tile(0, hot)]);
        }
        query.congestion = Some(&history);
        let path = search.run(&query, false).unwrap();
        assert!(path.nodes.contains(&(alternative as u32)));
        assert!(!path.nodes.contains(&(hot as u32)));
        query.soft = false;
        query.congestion = None;
        assert_eq!(
            search.run(&query, false).unwrap_err(),
            SearchFailure::Exhausted
        );
    }
}
