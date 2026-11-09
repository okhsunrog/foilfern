//! The routing driver: problem setup, air wire ordering, rip-up and reroute,
//! path simplification, verification and result assembly.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::ops::ControlFlow;

use librepcb_core::algorithm::AirWiresBuilder;
use librepcb_core::types::{Layer, Length, PositiveLength};

use crate::error::{Error, Result};
use crate::geometry::{Core, P, bounds_of};
use crate::grid::{Grid, Layered, owner_of, rasterize_trace_item, rasterize_via_item, usable};
use crate::items::{Item, Items, NO_NET, RouteId};
use crate::problem::{
    Connection, LayerScope, NetId, PreferredDirection, RoutingProblem, Shape, TerminalId,
};
use crate::result::{
    Endpoint, Progress, Route, RoutingResult, RoutingStats, Segment, Unrouted, UnroutedReason, Via,
};
use crate::search::{DirFactors, NEUTRAL, Query, STEP, Search};

/// Default grid pitch (0.1 mm).
const DEFAULT_PITCH: i64 = 100_000;
/// Search states (nodes × layers × 9 directions) the automatic pitch
/// selection aims to stay below (~12 bytes each).
const AUTO_MAX_STATES: u64 = 16_000_000;
/// Hard limit for an explicitly configured pitch.
const MAX_STATES: u64 = 80_000_000;

/// An air wire to route.
#[derive(Debug, Clone, Copy)]
struct Wire {
    request: usize,
    net: u32,
    a: usize,
    b: usize,
}

/// Where a path starts or ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ep {
    Terminal(usize),
    /// On a route: on a trace of routing layer `Some(l)`, or on a via.
    Route {
        route: RouteId,
        layer: Option<usize>,
        p: P,
    },
}

/// A sequence of points on one routing layer.
#[derive(Debug, Clone)]
struct Run {
    layer: usize,
    points: Vec<P>,
}

/// A committed route.
#[derive(Debug, Clone)]
struct RouteState {
    wire: usize,
    net: u32,
    half_width: i64,
    width: PositiveLength,
    runs: Vec<Run>,
    vias: Vec<P>,
    start: Ep,
    end: Ep,
    items: Vec<usize>,
}

/// Why an attempt failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fail {
    NoAccess,
    NoPath,
}

/// Numeric rules in nanometers.
#[derive(Debug, Clone, Copy)]
struct Rules {
    clearance: f64,
    via_radius: f64,
    via_drill_radius: f64,
    drill_clearance: f64,
}

pub(crate) struct Router<'p> {
    problem: &'p RoutingProblem,
    grid: Grid,
    /// Copper layer index of each routing layer.
    routing: Vec<usize>,
    layer_bits: Vec<u64>,
    all_bits: u64,
    nets: Vec<NetId>,
    rules: Rules,
    via_cost: u32,
    factors: Vec<DirFactors>,
    items: Items,
    trace_maps: BTreeMap<i64, Layered>,
    via_map: Layered,
    search: Search,
    wires: Vec<Wire>,
    routes: Vec<Option<RouteState>>,
    wire_route: Vec<Option<RouteId>>,
    rip_count: Vec<u32>,
    stats: RoutingStats,
}

fn nm(l: Length) -> f64 {
    l.to_nm() as f64
}

/// Core and radius of a shape.
fn core_of(shape: &Shape) -> (Core, f64) {
    match shape {
        Shape::Polygon(v) => (Core::Polygon(v.iter().map(|p| P::from(*p)).collect()), 0.0),
        Shape::Circle { center, diameter } => (Core::Point((*center).into()), nm(**diameter) / 2.0),
        Shape::Segment { start, end, width } => (
            Core::Segment((*start).into(), (*end).into()),
            nm(**width) / 2.0,
        ),
    }
}

impl<'p> Router<'p> {
    pub fn new(problem: &'p RoutingProblem) -> Result<Self> {
        let r = &problem.rules;
        if r.via_drill >= r.via_diameter {
            return Err(Error::InvalidVia);
        }
        if [
            r.clearance,
            r.board_clearance,
            r.npth_clearance,
            r.drill_clearance,
            r.drill_board_clearance,
            r.via_cost,
        ]
        .iter()
        .any(|l| *l < Length::ZERO)
        {
            return Err(Error::NegativeClearance);
        }

        // Layers.
        let copper: Vec<Layer> = problem.copper_layers.iter().copied().take(64).collect();
        let all_bits = if copper.len() >= 64 {
            u64::MAX
        } else {
            (1u64 << copper.len()) - 1
        };
        let routing: Vec<usize> = copper
            .iter()
            .enumerate()
            .filter(|(_, l)| r.allowed_layers.as_ref().is_none_or(|a| a.contains(l)))
            .map(|(i, _)| i)
            .collect();
        if routing.is_empty() {
            return Err(Error::NoRoutingLayers);
        }
        let layer_bits: Vec<u64> = routing.iter().map(|i| 1u64 << i).collect();
        let bits_of = |scope: &LayerScope| -> u64 {
            match scope {
                LayerScope::All => all_bits,
                LayerScope::Only(layers) => layers
                    .iter()
                    .filter_map(|l| copper.iter().position(|c| c == l))
                    .fold(0, |acc, i| acc | 1u64 << i),
            }
        };

        // Outline and grid.
        let outline: Vec<Vec<P>> = problem
            .outline
            .iter()
            .filter(|poly| poly.len() >= 3)
            .map(|poly| poly.iter().map(|p| P::from(*p)).collect())
            .collect();
        if outline.is_empty() {
            return Err(Error::NoOutline);
        }
        let all_points: Vec<P> = outline.iter().flatten().copied().collect();
        let (lo, hi) = bounds_of(&all_points);
        if lo.x >= hi.x || lo.y >= hi.y {
            return Err(Error::NoOutline);
        }
        let layers = routing.len() as u64;
        let states = |g: &Grid| g.cells() as u64 * layers * 9;
        let grid = match r.grid_pitch {
            Some(pitch) => {
                let grid = Grid::new(lo, hi, pitch.to_nm());
                if states(&grid) > MAX_STATES {
                    return Err(Error::GridTooLarge(grid.cells() as u64));
                }
                grid
            }
            None => {
                let mut pitch = DEFAULT_PITCH;
                loop {
                    let grid = Grid::new(lo, hi, pitch);
                    if states(&grid) <= AUTO_MAX_STATES {
                        break grid;
                    }
                    pitch += pitch / 4;
                    pitch -= pitch % 25_000;
                }
            }
        };

        // Nets.
        let net_set: BTreeSet<NetId> = problem
            .terminals
            .iter()
            .map(|t| t.net)
            .chain(problem.obstacles.iter().filter_map(|o| o.net))
            .collect();
        let nets: Vec<NetId> = net_set.into_iter().collect();
        let net_index =
            |n: &NetId| -> u32 { nets.binary_search(n).map(|i| i as u32).unwrap_or(NO_NET) };

        let rules = Rules {
            clearance: nm(r.clearance),
            via_radius: nm(*r.via_diameter) / 2.0,
            via_drill_radius: nm(*r.via_drill) / 2.0,
            drill_clearance: nm(r.drill_clearance),
        };

        // Static items.
        let mut items = Vec::new();
        let edge_via_keep = (nm(r.board_clearance) + rules.via_radius)
            .max(nm(r.drill_board_clearance) + rules.via_drill_radius);
        for poly in &outline {
            for (a, b) in crate::geometry::polygon_edges(poly) {
                items.push(Item {
                    core: Core::Segment(a, b),
                    net: NO_NET,
                    layers: all_bits,
                    trace_keep: Some(nm(r.board_clearance)),
                    via_keep_other: Some(edge_via_keep),
                    via_keep_own: None,
                    route: None,
                });
            }
        }
        for o in &problem.obstacles {
            let (core, radius) = core_of(&o.shape);
            let cl = rules.clearance.max(o.clearance.map_or(0.0, nm));
            items.push(Item {
                core,
                net: o.net.as_ref().map_or(NO_NET, net_index),
                layers: bits_of(&o.layers),
                trace_keep: Some(radius + cl),
                via_keep_other: Some(radius + cl + rules.via_radius),
                via_keep_own: None,
                route: None,
            });
        }
        for h in &problem.holes {
            let (core, radius) = core_of(&h.shape);
            let drill = radius + rules.drill_clearance + rules.via_drill_radius;
            let (trace_keep, via_keep) = if h.plated {
                (None, drill)
            } else {
                let npth = radius + nm(r.npth_clearance);
                (Some(npth), drill.max(npth + rules.via_radius))
            };
            items.push(Item {
                core,
                net: NO_NET,
                layers: all_bits,
                trace_keep,
                via_keep_other: Some(via_keep),
                via_keep_own: None,
                route: None,
            });
        }
        for t in &problem.terminals {
            let (core, radius) = core_of(&t.shape);
            let cl = rules.clearance.max(t.clearance.map_or(0.0, nm));
            let via_keep = radius + cl + rules.via_radius;
            items.push(Item {
                core,
                net: net_index(&t.net),
                layers: bits_of(&t.layers),
                trace_keep: Some(radius + cl),
                via_keep_other: Some(via_keep),
                via_keep_own: Some(via_keep),
                route: None,
            });
        }
        let items = Items::new(outline, items);

        // Air wires.
        let mut wires = Vec::new();
        let terminal = |i: usize| problem.terminals.get(i).ok_or(Error::UnknownTerminal(i));
        for (request, c) in problem.connections.iter().enumerate() {
            match c {
                Connection::Terminals(TerminalId(a), TerminalId(b)) => {
                    let (ta, tb) = (terminal(*a)?, terminal(*b)?);
                    if ta.net != tb.net {
                        return Err(Error::NetMismatch(*a, *b));
                    }
                    if a != b {
                        wires.push(Wire {
                            request,
                            net: net_index(&ta.net),
                            a: *a,
                            b: *b,
                        });
                    }
                }
                Connection::Net(net) => {
                    let ids: Vec<usize> = (0..problem.terminals.len())
                        .filter(|&i| problem.terminals[i].net == *net)
                        .collect();
                    let mut builder = AirWiresBuilder::new();
                    for &i in &ids {
                        builder.add_point(problem.terminals[i].position);
                    }
                    let mut pairs: Vec<(usize, usize)> = builder
                        .build_air_wires()
                        .into_iter()
                        .map(|(a, b)| (ids[a.min(b)], ids[a.max(b)]))
                        .collect();
                    pairs.sort_unstable();
                    wires.extend(pairs.into_iter().map(|(a, b)| Wire {
                        request,
                        net: net_index(net),
                        a,
                        b,
                    }));
                }
            }
        }

        let pitch = grid.pitch as f64;
        let via_cost = (nm(r.via_cost) / pitch * f64::from(STEP)).round() as u32;
        let factors = routing
            .iter()
            .map(|&i| match r.preferred_directions.get(&copper[i]) {
                None => NEUTRAL,
                Some(PreferredDirection::Horizontal) => [100, 150, 250, 150, 100, 150, 250, 150],
                Some(PreferredDirection::Vertical) => [250, 150, 100, 150, 250, 150, 100, 150],
            })
            .collect();

        let mut via_map = Layered::new(grid.cells());
        grid.block_outside(&mut via_map.fixed, items.outline());
        for item in items.iter() {
            rasterize_via_item(&grid, &mut via_map.fixed, item);
        }

        let n_wires = wires.len();
        Ok(Self {
            problem,
            grid,
            routing,
            layer_bits,
            all_bits,
            nets,
            rules,
            via_cost,
            factors,
            items,
            trace_maps: BTreeMap::new(),
            via_map,
            search: Search::default(),
            wires,
            routes: Vec::new(),
            wire_route: vec![None; n_wires],
            rip_count: vec![0; n_wires],
            stats: RoutingStats {
                grid_pitch: grid.pitch,
                air_wires: n_wires,
                ..RoutingStats::default()
            },
        })
    }

    fn width_of(&self, net: u32) -> PositiveLength {
        self.problem.rules.trace_width_of(&self.nets[net as usize])
    }

    /// Makes sure the trace map for half width `h` exists.
    fn ensure_trace_map(&mut self, h: i64) {
        if self.trace_maps.contains_key(&h) {
            return;
        }
        let cells = self.grid.cells();
        let mut map = Layered::new(cells * self.routing.len());
        for l in 0..self.routing.len() {
            self.grid.block_outside(
                &mut map.fixed[l * cells..(l + 1) * cells],
                self.items.outline(),
            );
        }
        for item in self.items.iter() {
            let target = if item.route.is_some() {
                &mut map.routes
            } else {
                &mut map.fixed
            };
            rasterize_trace_item(&self.grid, target, &self.layer_bits, item, h as f64);
        }
        self.trace_maps.insert(h, map);
    }

    /// Union-find representatives of the terminals, joined by the routes.
    fn components(&self) -> Vec<usize> {
        let mut parent: Vec<usize> = (0..self.problem.terminals.len()).collect();
        fn find(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        for route in self.routes.iter().flatten() {
            let w = self.wires[route.wire];
            let (ra, rb) = (find(&mut parent, w.a), find(&mut parent, w.b));
            parent[ra.max(rb)] = ra.min(rb);
        }
        (0..parent.len()).map(|i| find(&mut parent, i)).collect()
    }

    fn is_connected(&self, w: usize) -> bool {
        let comps = self.components();
        let wire = self.wires[w];
        comps[wire.a] == comps[wire.b]
    }

    /// Anchor nodes of a component: terminal access nodes, then route vias,
    /// then route trace nodes.
    fn anchors(
        &self,
        comps: &[usize],
        comp: usize,
        wire: &Wire,
        h: i64,
    ) -> HashMap<u32, (u32, Ep)> {
        let cells = self.grid.cells();
        let map = &self.trace_maps[&h];
        let owner = owner_of(wire.net);
        let usable_node =
            |node: usize| usable(map.fixed[node], owner) && usable(map.routes[node], owner);
        let mut out: HashMap<u32, (u32, Ep)> = HashMap::new();
        let add = |out: &mut HashMap<u32, (u32, Ep)>, node: usize, cost: u32, ep: Ep| {
            if !usable_node(node) {
                return;
            }
            let e = out.entry(node as u32).or_insert((cost, ep));
            if cost < e.0 {
                *e = (cost, ep);
            }
        };
        for t in (0..self.problem.terminals.len()).filter(|&t| comps[t] == comp) {
            for (node, cost) in self.terminal_access(t, wire.net, h) {
                add(&mut out, node, cost, Ep::Terminal(t));
            }
        }
        let route_ids: Vec<RouteId> = self
            .routes
            .iter()
            .enumerate()
            .filter_map(|(id, r)| {
                let r = r.as_ref()?;
                (comps[self.wires[r.wire].a] == comp).then_some(id)
            })
            .collect();
        for &id in &route_ids {
            let Some(route) = &self.routes[id] else {
                continue;
            };
            for &v in &route.vias {
                if let Some(c) = self.grid.cell_at(v) {
                    for l in 0..self.routing.len() {
                        add(
                            &mut out,
                            l * cells + c,
                            0,
                            Ep::Route {
                                route: id,
                                layer: None,
                                p: v,
                            },
                        );
                    }
                }
            }
        }
        for &id in &route_ids {
            let Some(route) = &self.routes[id] else {
                continue;
            };
            for run in &route.runs {
                for pair in run.points.windows(2) {
                    for p in self.grid_points_on(pair[0], pair[1]) {
                        if let Some(c) = self.grid.cell_at(p) {
                            add(
                                &mut out,
                                run.layer * cells + c,
                                0,
                                Ep::Route {
                                    route: id,
                                    layer: Some(run.layer),
                                    p,
                                },
                            );
                        }
                    }
                }
            }
        }
        out
    }

    /// Grid nodes on a 45° multiple segment with at least one endpoint on
    /// the grid.
    fn grid_points_on(&self, a: P, b: P) -> Vec<P> {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        if !(dx == 0 || dy == 0 || dx.abs() == dy.abs()) {
            return Vec::new();
        }
        let (from, to) = if self.grid.cell_at(a).is_some() {
            (a, b)
        } else if self.grid.cell_at(b).is_some() {
            (b, a)
        } else {
            return Vec::new();
        };
        let pitch = self.grid.pitch;
        let step = P::new(
            (to.x - from.x).signum() * pitch,
            (to.y - from.y).signum() * pitch,
        );
        let n = ((to.x - from.x).abs().max((to.y - from.y).abs())) / pitch;
        (0..=n)
            .map(|k| P::new(from.x + k * step.x, from.y + k * step.y))
            .collect()
    }

    /// Access nodes of a terminal with the cost of the stub from the
    /// terminal position.
    fn terminal_access(&self, t: usize, net: u32, h: i64) -> Vec<(usize, u32)> {
        let terminal = &self.problem.terminals[t];
        let pos = P::from(terminal.position);
        let (core, radius) = core_of(&terminal.shape);
        let cells = self.grid.cells();
        let map = &self.trace_maps[&h];
        let owner = owner_of(net);
        let pitch = self.grid.pitch as f64;
        let scope_bits = match &terminal.layers {
            LayerScope::All => self.all_bits,
            LayerScope::Only(layers) => layers
                .iter()
                .filter_map(|l| self.problem.copper_layers.iter().position(|c| c == l))
                .filter(|&i| i < 64)
                .fold(0, |acc, i| acc | 1u64 << i),
        };
        let cost_of = |p: P| -> u32 {
            let (dx, dy) = ((p.x - pos.x) as f64, (p.y - pos.y) as f64);
            ((dx * dx + dy * dy).sqrt() / pitch * f64::from(STEP)).round() as u32
        };
        let (lo, hi) = core.bounds();
        let r = radius.ceil() as i64;
        let collect = |inside: &dyn Fn(P) -> bool, lo: P, hi: P| -> Vec<(usize, u32)> {
            let g = &self.grid;
            let i0 = ((lo.x - g.x0) as f64 / pitch).ceil().max(0.0) as usize;
            let j0 = ((lo.y - g.y0) as f64 / pitch).ceil().max(0.0) as usize;
            let i1 = (((hi.x - g.x0) as f64 / pitch).floor().max(-1.0) as i64).min(g.nx as i64 - 1);
            let j1 = (((hi.y - g.y0) as f64 / pitch).floor().max(-1.0) as i64).min(g.ny as i64 - 1);
            let mut out = Vec::new();
            for (l, &cu) in self.routing.iter().enumerate() {
                if scope_bits & (1u64 << cu) == 0 {
                    continue;
                }
                for j in j0 as i64..=j1 {
                    for i in i0 as i64..=i1 {
                        let c = j as usize * g.nx + i as usize;
                        let node = l * cells + c;
                        if !usable(map.fixed[node], owner) || !usable(map.routes[node], owner) {
                            continue;
                        }
                        let p = g.point(c);
                        if !inside(p) {
                            continue;
                        }
                        let ok = self
                            .items
                            .check_segment((pos, p), cu, net, h as f64, None, &mut |_| false)
                            .is_ok();
                        if ok {
                            out.push((node, cost_of(p)));
                        }
                    }
                }
            }
            out
        };
        let inside_copper = |p: P| core.distance_to_segment(p, p) <= radius;
        let out = collect(
            &inside_copper,
            P::new(lo.x - r, lo.y - r),
            P::new(hi.x + r, hi.y + r),
        );
        if !out.is_empty() {
            return out;
        }
        // Small or off-grid terminal: nodes near the position.
        let reach = 2 * self.grid.pitch;
        let near = |p: P| {
            let (dx, dy) = ((p.x - pos.x) as f64, (p.y - pos.y) as f64);
            (dx * dx + dy * dy).sqrt() <= reach as f64
        };
        collect(
            &near,
            P::new(pos.x - reach, pos.y - reach),
            P::new(pos.x + reach, pos.y + reach),
        )
    }

    fn check_segment_strict(&self, a: P, b: P, layer: usize, net: u32, h: i64) -> bool {
        self.items
            .check_segment(
                (a, b),
                self.routing[layer],
                net,
                h as f64,
                None,
                &mut |_| false,
            )
            .is_ok()
    }

    /// Tries to route a wire. Returns the wires of ripped routes (soft mode)
    /// along with the outcome.
    fn try_route(&mut self, w: usize, soft: bool) -> (Result<(), Fail>, Vec<usize>) {
        let wire = self.wires[w];
        let width = self.width_of(wire.net);
        let h = width.to_nm() / 2;
        self.ensure_trace_map(h);
        let comps = self.components();
        let sources = self.anchors(&comps, comps[wire.a], &wire, h);
        let targets = self.anchors(&comps, comps[wire.b], &wire, h);
        if sources.is_empty() || targets.is_empty() {
            return (Err(Fail::NoAccess), Vec::new());
        }
        let mut source_list: Vec<(u32, u32)> = sources.iter().map(|(n, (c, _))| (*n, *c)).collect();
        source_list.sort_unstable();
        let target_costs: HashMap<u32, u32> = targets.iter().map(|(n, (c, _))| (*n, *c)).collect();
        let map = &self.trace_maps[&h];
        let query = Query {
            grid: &self.grid,
            layers: self.routing.len(),
            trace_fixed: &map.fixed,
            trace_routes: &map.routes,
            via_fixed: &self.via_map.fixed,
            via_routes: &self.via_map.routes,
            owner: owner_of(wire.net),
            via_cost: self.via_cost,
            factors: &self.factors,
            soft,
            sources: &source_list,
            targets: &target_costs,
        };
        self.stats.searches += 1;
        let found = self.search.run(&query);
        let Some(found) = found else {
            return (Err(Fail::NoPath), Vec::new());
        };

        // Path to runs.
        let cells = self.grid.cells() as u32;
        let (Some(&first), Some(&last)) = (found.nodes.first(), found.nodes.last()) else {
            return (Err(Fail::NoPath), Vec::new());
        };
        let start = sources[&first].1;
        let end = targets[&last].1;
        let mut runs: Vec<Run> = Vec::new();
        let mut vias = Vec::new();
        for &node in &found.nodes {
            let layer = (node / cells) as usize;
            let p = self.grid.point((node % cells) as usize);
            match runs.last_mut() {
                Some(run) if run.layer == layer => run.points.push(p),
                Some(_) => {
                    vias.push(p);
                    runs.push(Run {
                        layer,
                        points: vec![p],
                    });
                }
                None => runs.push(Run {
                    layer,
                    points: vec![p],
                }),
            }
        }
        if let Ep::Terminal(t) = start {
            let pos = P::from(self.problem.terminals[t].position);
            if runs[0].points[0] != pos {
                runs[0].points.insert(0, pos);
            }
        }
        if let Ep::Terminal(t) = end {
            let pos = P::from(self.problem.terminals[t].position);
            let run = runs.last_mut().expect("path has at least one node");
            if run.points.last() != Some(&pos) {
                run.points.push(pos);
            }
        }
        for run in &mut runs {
            merge_collinear(&mut run.points);
        }

        // Soft mode: rip up the routes in the way.
        let mut ripped = Vec::new();
        if soft {
            let mut conflicts = BTreeSet::new();
            let mut collect = |route: RouteId| {
                conflicts.insert(route);
                true
            };
            let mut ok = true;
            for run in &runs {
                for pair in run.points.windows(2) {
                    ok &= self
                        .items
                        .check_segment(
                            (pair[0], pair[1]),
                            self.routing[run.layer],
                            wire.net,
                            h as f64,
                            None,
                            &mut collect,
                        )
                        .is_ok();
                }
            }
            for &v in &vias {
                ok &= self
                    .items
                    .check_via(v, wire.net, None, &mut collect)
                    .is_ok();
            }
            let max = self.problem.rules.max_rip_ups;
            let locked = conflicts.iter().any(|&id| {
                self.routes[id]
                    .as_ref()
                    .is_some_and(|r| self.rip_count[r.wire] >= max)
            });
            if !ok || locked || conflicts.is_empty() {
                return (Err(Fail::NoPath), ripped);
            }
            for id in conflicts {
                self.rip_up(id, &mut ripped);
            }
        }

        // Shortcuts along 45° lines.
        for run in &mut runs {
            let pts = &run.points;
            if pts.len() <= 2 {
                continue;
            }
            let mut out = vec![pts[0]];
            let mut i = 0;
            while i < pts.len() - 1 {
                let mut next = i + 1;
                for j in (i + 2..pts.len()).rev() {
                    if is_octilinear(pts[i], pts[j])
                        && self.check_segment_strict(pts[i], pts[j], run.layer, wire.net, h)
                    {
                        next = j;
                        break;
                    }
                }
                out.push(pts[next]);
                i = next;
            }
            run.points = out;
            merge_collinear(&mut run.points);
        }

        // Exact verification.
        let ok_segments = runs.iter().all(|run| {
            run.points
                .windows(2)
                .all(|pair| self.check_segment_strict(pair[0], pair[1], run.layer, wire.net, h))
        });
        let ok_vias = vias.iter().all(|&v| {
            self.items
                .check_via(v, wire.net, None, &mut |_| false)
                .is_ok()
        });
        let min_via_distance = self.rules.drill_clearance + 2.0 * self.rules.via_drill_radius;
        let ok_own_vias = vias.iter().enumerate().all(|(i, a)| {
            vias[i + 1..].iter().all(|b| {
                let (dx, dy) = ((a.x - b.x) as f64, (a.y - b.y) as f64);
                (dx * dx + dy * dy).sqrt() >= min_via_distance
            })
        });
        if !(ok_segments && ok_vias && ok_own_vias) {
            return (Err(Fail::NoPath), ripped);
        }

        self.commit(w, width, h, runs, vias, start, end);
        (Ok(()), ripped)
    }

    #[allow(clippy::too_many_arguments)]
    fn commit(
        &mut self,
        w: usize,
        width: PositiveLength,
        h: i64,
        runs: Vec<Run>,
        vias: Vec<P>,
        start: Ep,
        end: Ep,
    ) {
        let id = self.routes.len();
        let net = self.wires[w].net;
        let hw = h as f64;
        let r = self.rules;
        let mut new_items = Vec::new();
        for run in &runs {
            for pair in run.points.windows(2) {
                new_items.push(Item {
                    core: Core::Segment(pair[0], pair[1]),
                    net,
                    layers: self.layer_bits[run.layer],
                    trace_keep: Some(hw + r.clearance),
                    via_keep_other: Some(hw + r.clearance + r.via_radius),
                    via_keep_own: None,
                    route: Some(id),
                });
            }
        }
        let drill_keep = r.drill_clearance + 2.0 * r.via_drill_radius;
        for &v in &vias {
            new_items.push(Item {
                core: Core::Point(v),
                net,
                layers: self.all_bits,
                trace_keep: Some(r.via_radius + r.clearance),
                via_keep_other: Some((2.0 * r.via_radius + r.clearance).max(drill_keep)),
                via_keep_own: Some(drill_keep),
                route: Some(id),
            });
        }
        let mut indices = Vec::new();
        for item in new_items {
            for (&mh, map) in &mut self.trace_maps {
                rasterize_trace_item(
                    &self.grid,
                    &mut map.routes,
                    &self.layer_bits,
                    &item,
                    mh as f64,
                );
            }
            rasterize_via_item(&self.grid, &mut self.via_map.routes, &item);
            indices.push(self.items.insert(item));
        }
        self.routes.push(Some(RouteState {
            wire: w,
            net,
            half_width: h,
            width,
            runs,
            vias,
            start,
            end,
            items: indices,
        }));
        self.wire_route[w] = Some(id);
    }

    /// Rips up a route and all routes attached to it; collects their wires.
    fn rip_up(&mut self, id: RouteId, ripped: &mut Vec<usize>) {
        let mut stack = vec![id];
        while let Some(id) = stack.pop() {
            let Some(route) = self.routes[id].take() else {
                continue;
            };
            for &index in &route.items {
                self.items.remove(index);
            }
            self.wire_route[route.wire] = None;
            self.rip_count[route.wire] += 1;
            self.stats.rip_ups += 1;
            ripped.push(route.wire);
            for (other, r) in self.routes.iter().enumerate() {
                if let Some(r) = r {
                    let attached = |ep: &Ep| matches!(ep, Ep::Route { route, .. } if *route == id);
                    if attached(&r.start) || attached(&r.end) {
                        stack.push(other);
                    }
                }
            }
        }
        self.rebuild_route_maps();
    }

    fn rebuild_route_maps(&mut self) {
        self.via_map.routes.iter_mut().for_each(|c| *c = 0);
        for map in self.trace_maps.values_mut() {
            map.routes.iter_mut().for_each(|c| *c = 0);
        }
        for item in self.items.iter().filter(|i| i.route.is_some()) {
            for (&mh, map) in &mut self.trace_maps {
                rasterize_trace_item(
                    &self.grid,
                    &mut map.routes,
                    &self.layer_bits,
                    item,
                    mh as f64,
                );
            }
            rasterize_via_item(&self.grid, &mut self.via_map.routes, item);
        }
    }

    /// Final safety net: re-verifies every route against everything else.
    fn verify_all(&mut self) {
        loop {
            let bad = self.routes.iter().enumerate().find_map(|(id, r)| {
                let r = r.as_ref()?;
                let ok_seg = r.runs.iter().all(|run| {
                    run.points.windows(2).all(|pair| {
                        self.items
                            .check_segment(
                                (pair[0], pair[1]),
                                self.routing[run.layer],
                                r.net,
                                r.half_width as f64,
                                Some(id),
                                &mut |_| false,
                            )
                            .is_ok()
                    })
                });
                let ok_via = r.vias.iter().all(|&v| {
                    self.items
                        .check_via(v, r.net, Some(id), &mut |_| false)
                        .is_ok()
                });
                (!(ok_seg && ok_via)).then_some(id)
            });
            match bad {
                Some(id) => {
                    let mut ripped = Vec::new();
                    self.rip_up(id, &mut ripped);
                }
                None => break,
            }
        }
    }

    pub fn run(mut self, progress: &mut dyn FnMut(Progress) -> ControlFlow<()>) -> RoutingResult {
        let problem = self.problem;
        let mut order: Vec<usize> = (0..self.wires.len()).collect();
        let length = |w: &Wire| {
            let (a, b) = (
                P::from(problem.terminals[w.a].position),
                P::from(problem.terminals[w.b].position),
            );
            let (dx, dy) = ((a.x - b.x) as f64, (a.y - b.y) as f64);
            (dx * dx + dy * dy).sqrt()
        };
        order.sort_by(|&x, &y| {
            length(&self.wires[x])
                .total_cmp(&length(&self.wires[y]))
                .then(x.cmp(&y))
        });

        let mut reasons: Vec<Option<UnroutedReason>> = vec![None; self.wires.len()];
        let mut soft_tries = vec![0u32; self.wires.len()];
        let max = self.problem.rules.max_rip_ups;
        let mut queue: VecDeque<usize> = order.iter().copied().collect();
        let mut failed: Vec<usize> = Vec::new();
        let mut cancelled = false;
        while let Some(w) = queue.pop_front() {
            if self.report(progress).is_break() {
                cancelled = true;
                break;
            }
            if self.wire_route[w].is_some() || self.is_connected(w) {
                continue;
            }
            match self.try_route(w, false).0 {
                Ok(()) => {
                    reasons[w] = None;
                    continue;
                }
                Err(Fail::NoAccess) => {
                    reasons[w] = Some(UnroutedReason::NoAccess);
                    failed.push(w);
                    continue;
                }
                Err(Fail::NoPath) => reasons[w] = Some(UnroutedReason::NoPath),
            }
            if soft_tries[w] < max {
                soft_tries[w] += 1;
                let (outcome, ripped) = self.try_route(w, true);
                queue.extend(ripped);
                if outcome.is_ok() {
                    reasons[w] = None;
                    continue;
                }
            }
            failed.push(w);
        }
        // Retry failed wires once, the situation may have changed.
        if !cancelled {
            for &w in &failed {
                if self.report(progress).is_break() {
                    cancelled = true;
                    break;
                }
                if self.wire_route[w].is_none()
                    && !self.is_connected(w)
                    && self.try_route(w, false).0.is_ok()
                {
                    reasons[w] = None;
                }
            }
        }
        self.verify_all();
        self.into_result(reasons, cancelled)
    }

    fn report(&self, progress: &mut dyn FnMut(Progress) -> ControlFlow<()>) -> ControlFlow<()> {
        let comps = self.components();
        let connected = self
            .wires
            .iter()
            .filter(|w| comps[w.a] == comps[w.b])
            .count();
        progress(Progress {
            connected,
            total: self.wires.len(),
            searches: self.stats.searches,
        })
    }

    fn into_result(self, reasons: Vec<Option<UnroutedReason>>, cancelled: bool) -> RoutingResult {
        let copper = &self.problem.copper_layers;
        let layer_of = |l: usize| copper[self.routing[l]];
        let rules = &self.problem.rules;
        let mut routes: Vec<Route> = Vec::new();
        for route in self.routes.iter().flatten() {
            let wire = self.wires[route.wire];
            let own_via = |p: P| route.vias.contains(&p);
            let endpoint = |ep: Ep| match ep {
                Ep::Terminal(t) => Endpoint::Terminal(TerminalId(t)),
                Ep::Route { layer: None, p, .. } => Endpoint::Via(p.to_point()),
                Ep::Route { p, .. } if own_via(p) => Endpoint::Via(p.to_point()),
                Ep::Route {
                    layer: Some(l), p, ..
                } => Endpoint::Junction {
                    layer: layer_of(l),
                    position: p.to_point(),
                },
            };
            let segments = route
                .runs
                .iter()
                .flat_map(|run| {
                    run.points.windows(2).map(move |pair| Segment {
                        layer: layer_of(run.layer),
                        start: pair[0].to_point(),
                        end: pair[1].to_point(),
                        width: route.width,
                    })
                })
                .collect();
            routes.push(Route {
                request: wire.request,
                net: self.nets[wire.net as usize],
                terminals: (TerminalId(wire.a), TerminalId(wire.b)),
                start: endpoint(route.start),
                end: endpoint(route.end),
                segments,
                vias: route
                    .vias
                    .iter()
                    .map(|v| Via {
                        position: v.to_point(),
                        drill: rules.via_drill,
                        diameter: rules.via_diameter,
                    })
                    .collect(),
            });
        }
        split_at_junctions(&mut routes);

        let comps = self.components();
        let unrouted = self
            .wires
            .iter()
            .enumerate()
            .filter(|(_, w)| comps[w.a] != comps[w.b])
            .map(|(i, w)| Unrouted {
                request: w.request,
                net: self.nets[w.net as usize],
                terminals: (TerminalId(w.a), TerminalId(w.b)),
                reason: match reasons[i] {
                    Some(r) => r,
                    None if cancelled => UnroutedReason::Cancelled,
                    None => UnroutedReason::NoPath,
                },
            })
            .collect();
        RoutingResult {
            routes,
            unrouted,
            stats: self.stats,
        }
    }
}

fn is_octilinear(a: P, b: P) -> bool {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    dx == 0 || dy == 0 || dx.abs() == dy.abs()
}

/// Removes duplicate points and interior points of straight lines.
fn merge_collinear(points: &mut Vec<P>) {
    points.dedup();
    if points.len() < 3 {
        return;
    }
    let mut out: Vec<P> = Vec::with_capacity(points.len());
    for &p in points.iter() {
        if out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            let cross = i128::from(b.x - a.x) * i128::from(p.y - b.y)
                - i128::from(b.y - a.y) * i128::from(p.x - b.x);
            let dot = i128::from(b.x - a.x) * i128::from(p.x - b.x)
                + i128::from(b.y - a.y) * i128::from(p.y - b.y);
            if cross == 0 && dot > 0 {
                out.pop();
            }
        }
        out.push(p);
    }
    *points = out;
}

/// Splits segments at endpoints and vias of the same net lying strictly
/// inside them, so all connections are at segment endpoints.
fn split_at_junctions(routes: &mut [Route]) {
    let mut points: BTreeMap<NetId, Vec<(Option<Layer>, P)>> = BTreeMap::new();
    for route in routes.iter() {
        let list = points.entry(route.net).or_default();
        for s in &route.segments {
            list.push((Some(s.layer), s.start.into()));
            list.push((Some(s.layer), s.end.into()));
        }
        list.extend(route.vias.iter().map(|v| (None, P::from(v.position))));
    }
    for route in routes.iter_mut() {
        let list = &points[&route.net];
        let mut out = Vec::with_capacity(route.segments.len());
        for s in &route.segments {
            let (a, b) = (P::from(s.start), P::from(s.end));
            let mut cuts: Vec<P> = list
                .iter()
                .filter(|(layer, p)| {
                    layer.is_none_or(|l| l == s.layer)
                        && *p != a
                        && *p != b
                        && crate::geometry::point_on_segment(*p, a, b)
                })
                .map(|(_, p)| *p)
                .collect();
            let d2 = |p: &P| {
                let (dx, dy) = (i128::from(p.x - a.x), i128::from(p.y - a.y));
                dx * dx + dy * dy
            };
            cuts.sort_by_key(d2);
            cuts.dedup();
            let mut prev = a;
            for c in cuts.into_iter().chain([b]) {
                out.push(Segment {
                    start: prev.to_point(),
                    end: c.to_point(),
                    ..*s
                });
                prev = c;
            }
        }
        route.segments = out;
    }
}
