//! The routing driver: problem setup, air wire ordering, rip-up and reroute,
//! path simplification, verification and result assembly.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::ops::ControlFlow;

use librepcb_core::algorithm::AirWiresBuilder;
use librepcb_core::types::{Layer, Length, Point, PositiveLength};

use crate::congestion::Congestion;
use crate::error::{Error, Result};
use crate::geometry::{Core, P, bounds_of};
use crate::grid::{Grid, Layered, owner_of, rasterize_trace_item, rasterize_via_item, usable};
use crate::items::{Item, Items, NO_NET, RouteId, Violation};
use crate::problem::{
    Connection, LayerScope, NetId, PreferredDirection, RoutingOrder, RoutingProblem, Shape,
    TerminalId,
};
use crate::result::{
    Endpoint, FrontierBlocker, FrontierSample, Progress, Route, RoutingResult, RoutingStats,
    Segment, Unrouted, UnroutedReason, Via,
};
use crate::search::{DirFactors, NEUTRAL, Query, STEP, Search, SearchFailure, TURN_45};

mod local_via;
pub use local_via::{LocalViaBridge, LocalViaSite, find_local_via};

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
    Terminal(usize, PadStub),
    /// On a route: on a trace of routing layer `Some(l)`, or on a via.
    Route {
        route: RouteId,
        layer: Option<usize>,
        p: P,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PadStub {
    elbow: Option<P>,
    neck_width: Option<PositiveLength>,
}

#[derive(Debug, Clone, Copy)]
struct Access {
    node: usize,
    cost: u32,
    stub: PadStub,
    escape: bool,
}

/// A sequence of points on one routing layer.
#[derive(Debug, Clone)]
struct Run {
    layer: usize,
    width: PositiveLength,
    points: Vec<P>,
}

impl Run {
    fn half_width(&self) -> i64 {
        self.width.to_nm() / 2
    }
}

/// A committed route.
#[derive(Debug, Clone)]
struct RouteState {
    wire: usize,
    net: u32,
    runs: Vec<Run>,
    vias: Vec<P>,
    start: Ep,
    end: Ep,
    items: Vec<usize>,
}

#[derive(Debug, Default, Clone)]
struct AttemptFailures {
    strict: Option<UnroutedReason>,
    soft: Option<UnroutedReason>,
    frontier: Vec<FrontierSample>,
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
    edge_count: usize,
    via_map: Layered,
    search: Search,
    congestion: Option<Congestion>,
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
        let (lo, hi) = if let Some(region) = problem.region {
            let (min, max) = region.bounds();
            let min = P::from(min);
            let max = P::from(max);
            let lo = P::new(lo.x.max(min.x), lo.y.max(min.y));
            let hi = P::new(hi.x.min(max.x), hi.y.min(max.y));
            if lo.x >= hi.x || lo.y >= hi.y {
                return Err(Error::RegionOutsideBoard);
            }
            (lo, hi)
        } else {
            (lo, hi)
        };
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
        let edge_count = items.len();
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
        if let Some(region) = problem.region {
            grid.block_outside_region(
                &mut via_map.fixed,
                region,
                Length::new(rules.via_radius.ceil() as i64),
            );
        }
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
            edge_count,
            via_map,
            search: Search::default(),
            congestion: None,
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
        for layer in map.fixed.chunks_exact_mut(cells) {
            self.grid.block_outside(layer, self.items.outline());
            if let Some(region) = self.problem.region {
                // Round the possibly half-nanometer trace radius outward.
                self.grid
                    .block_outside_region(layer, region, Length::new(h + 1));
            }
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
            for access in self.terminal_access(t, wire.net, h) {
                add(
                    &mut out,
                    access.node,
                    access.cost,
                    Ep::Terminal(t, access.stub),
                );
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
    fn terminal_access(&self, t: usize, net: u32, h: i64) -> Vec<Access> {
        let terminal = &self.problem.terminals[t];
        let pos = P::from(terminal.position);
        let is_pad = self.problem.pad_terminals.contains(&TerminalId(t));
        let within_region = |p: P, half_width| {
            self.problem
                .region
                .is_none_or(|region| region.contains(p.to_point(), Length::new(half_width + 1)))
        };
        let wide_inside = within_region(pos, h);
        let neck = self
            .problem
            .rules
            .neck_downs
            .get(&terminal.net)
            .filter(|neck| {
                let narrow = neck.width.to_nm() / 2;
                is_pad && narrow < h && within_region(pos, narrow)
            });
        if !wide_inside && neck.is_none() {
            return Vec::new();
        }
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
        let distance = |a: P, b: P| {
            let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
            (dx * dx + dy * dy).sqrt()
        };
        let cost_of =
            |a: P, b: P| -> u32 { (distance(a, b) / pitch * f64::from(STEP)).round() as u32 };
        let (lo, hi) = core.bounds();
        let r = radius.ceil() as i64;
        let collect =
            |stub: &dyn Fn(P, usize) -> Option<Option<P>>, lo: P, hi: P, escape| -> Vec<Access> {
                let g = &self.grid;
                let i0 = ((lo.x - g.x0) as f64 / pitch).ceil().max(0.0) as usize;
                let j0 = ((lo.y - g.y0) as f64 / pitch).ceil().max(0.0) as usize;
                let i1 =
                    (((hi.x - g.x0) as f64 / pitch).floor().max(-1.0) as i64).min(g.nx as i64 - 1);
                let j1 =
                    (((hi.y - g.y0) as f64 / pitch).floor().max(-1.0) as i64).min(g.ny as i64 - 1);
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
                            if let Some(elbow) = stub(p, cu) {
                                let cost = elbow.map_or_else(
                                    || cost_of(pos, p),
                                    |e| cost_of(pos, e) + TURN_45 + cost_of(e, p),
                                );
                                out.push(Access {
                                    node,
                                    cost,
                                    stub: PadStub {
                                        elbow,
                                        neck_width: None,
                                    },
                                    escape,
                                });
                            }
                        }
                    }
                }
                out
            };
        let clear_at = |a, b, cu, half_width: i64| {
            self.items
                .check_segment((a, b), cu, net, half_width as f64, None, &mut |_| false)
                .is_ok()
        };
        let clear = |a, b, cu| clear_at(a, b, cu, h);
        let inside_copper = |p: P| core.distance_to_segment(p, p) <= radius;
        let mut out = collect(
            &|p, cu| (wide_inside && inside_copper(p) && clear(pos, p, cu)).then_some(None),
            P::new(lo.x - r, lo.y - r),
            P::new(hi.x + r, hi.y + r),
            false,
        );
        let reach = 2 * self.grid.pitch;
        if wide_inside && out.is_empty() {
            // Small or off-grid terminal: nodes near the position.
            out = collect(
                &|p, cu| {
                    let (dx, dy) = ((p.x - pos.x) as f64, (p.y - pos.y) as f64);
                    ((dx * dx + dy * dy).sqrt() <= reach as f64 && clear(pos, p, cu))
                        .then_some(None)
                },
                P::new(pos.x - reach, pos.y - reach),
                P::new(pos.x + reach, pos.y + reach),
                false,
            );
        }
        if wide_inside && self.problem.rules.pad_escape && is_pad {
            let existing: BTreeSet<_> = out.iter().map(|a| a.node).collect();
            let escapes = collect(
                &|p, cu| {
                    if inside_copper(p) || core.distance_to_segment(p, p) > radius + reach as f64 {
                        return None;
                    }
                    if is_octilinear(pos, p) {
                        return clear(pos, p, cu).then_some(None);
                    }
                    escape_elbows(pos, p)
                        .into_iter()
                        .find(|&elbow| {
                            within_region(elbow, h) && clear(pos, elbow, cu) && clear(elbow, p, cu)
                        })
                        .map(Some)
                },
                P::new(lo.x - r - reach, lo.y - r - reach),
                P::new(hi.x + r + reach, hi.y + r + reach),
                true,
            );
            // Keep the nearest options in each direction on each layer.
            let mut by_direction: BTreeMap<_, Vec<Access>> = BTreeMap::new();
            for access in escapes.into_iter().filter(|a| !existing.contains(&a.node)) {
                let p = self.grid.point(access.node % cells);
                by_direction
                    .entry((access.node / cells, p.x.cmp(&pos.x), p.y.cmp(&pos.y)))
                    .or_default()
                    .push(access);
            }
            for mut exits in by_direction.into_values() {
                exits.sort_unstable_by_key(|a| (a.cost, a.node));
                out.extend(exits.into_iter().take(4));
            }
        }
        if let Some(neck) = neck {
            // Look for a short, exact-checked connection to an already
            // usable full-width node. The main search stays full width.
            let reach = neck.max_length.to_nm().min(24 * self.grid.pitch);
            let half = neck.width.to_nm() / 2;
            let stub = |p, cu| {
                if p == pos {
                    return None;
                }
                let fits = |elbow: Option<P>, hw| {
                    if let Some(e) = elbow {
                        within_region(e, hw) && clear_at(pos, e, cu, hw) && clear_at(e, p, cu, hw)
                    } else {
                        clear_at(pos, p, cu, hw)
                    }
                };
                let eligible = |elbow: Option<P>| {
                    let length = elbow
                        .map_or_else(|| distance(pos, p), |e| distance(pos, e) + distance(e, p));
                    length <= reach as f64 && fits(elbow, half) && !(wide_inside && fits(elbow, h))
                };
                if is_octilinear(pos, p) {
                    eligible(None).then_some(None)
                } else {
                    escape_elbows(pos, p)
                        .into_iter()
                        .map(Some)
                        .find(|&e| eligible(e))
                }
            };
            let mut narrow = collect(
                &stub,
                P::new(pos.x - reach, pos.y - reach),
                P::new(pos.x + reach, pos.y + reach),
                true,
            );
            for access in &mut narrow {
                access.stub.neck_width = Some(neck.width);
                access.cost = access.cost.saturating_mul(2).saturating_add(2 * STEP);
            }
            out.extend(narrow);
        }
        out
    }

    pub(crate) fn diagnose_access(
        &mut self,
        id: TerminalId,
        reach: PositiveLength,
    ) -> Result<crate::TerminalAccess> {
        let t = self
            .problem
            .terminals
            .get(id.0)
            .ok_or(Error::UnknownTerminal(id.0))?;
        let net = self
            .nets
            .binary_search(&t.net)
            .map_err(|_| Error::UnknownTerminal(id.0))? as u32;
        let width = self.width_of(net);
        let h = (*width).to_nm() / 2;
        self.ensure_trace_map(h);
        let nodes = self.terminal_access(id.0, net, h);
        let cells = self.grid.cells();
        let point = |p: P| Point::new(Length::new(p.x), Length::new(p.y));
        let access_points = nodes
            .iter()
            .take(16)
            .map(|access| {
                (
                    self.problem.copper_layers[self.routing[access.node / cells]],
                    point(self.grid.point(access.node % cells)),
                )
            })
            .collect();
        let mut probes = Vec::new();
        for &cu in &self.routing {
            let layer = self.problem.copper_layers[cu];
            if matches!(&t.layers, LayerScope::Only(layers) if !layers.contains(&layer)) {
                continue;
            }
            for (dx, dy) in [
                (1, 0),
                (1, 1),
                (0, 1),
                (-1, 1),
                (-1, 0),
                (-1, -1),
                (0, -1),
                (1, -1),
            ] {
                let origin = P::from(t.position);
                let step = if dx != 0 && dy != 0 {
                    (nm(*reach) * std::f64::consts::FRAC_1_SQRT_2).round() as i64
                } else {
                    (*reach).to_nm()
                };
                let end = P::new(
                    origin.x.saturating_add(dx * step),
                    origin.y.saturating_add(dy * step),
                );
                let blocker = match self.items.check_segment(
                    (origin, end),
                    cu,
                    net,
                    h as f64,
                    None,
                    &mut |_| false,
                ) {
                    Ok(()) => None,
                    Err(Violation::Static(Some(index))) => Some(self.blocker(index)),
                    Err(_) => Some(crate::AccessBlocker::BoardEdge),
                };
                probes.push(crate::AccessProbe {
                    layer,
                    end: point(end),
                    blocker,
                });
            }
        }
        Ok(crate::TerminalAccess {
            grid_pitch: Length::new(self.grid.pitch),
            width,
            access_count: nodes.len(),
            escape_count: nodes.iter().filter(|access| access.escape).count(),
            access_points,
            probes,
        })
    }

    fn blocker(&self, index: usize) -> crate::AccessBlocker {
        let Some(index) = index.checked_sub(self.edge_count) else {
            return crate::AccessBlocker::BoardEdge;
        };
        if index < self.problem.obstacles.len() {
            return crate::AccessBlocker::Obstacle(index);
        }
        let index = index - self.problem.obstacles.len();
        if index < self.problem.holes.len() {
            return crate::AccessBlocker::Hole(index);
        }
        crate::AccessBlocker::Terminal(TerminalId(index - self.problem.holes.len()))
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
    fn try_route(
        &mut self,
        w: usize,
        soft: bool,
        frontier: &mut Vec<FrontierSample>,
        cancelled: &dyn Fn() -> bool,
    ) -> (Result<(), UnroutedReason>, Vec<usize>) {
        if !soft {
            frontier.clear();
        }
        let wire = self.wires[w];
        let width = self.width_of(wire.net);
        let h = width.to_nm() / 2;
        self.ensure_trace_map(h);
        let comps = self.components();
        let sources = self.anchors(&comps, comps[wire.a], &wire, h);
        let targets = self.anchors(&comps, comps[wire.b], &wire, h);
        if sources.is_empty() || targets.is_empty() {
            return (Err(UnroutedReason::NoAccess), Vec::new());
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
            congestion: self
                .congestion
                .as_ref()
                .filter(|history| soft && history.used_tiles() > 0),
            sources: &source_list,
            targets: &target_costs,
        };
        self.stats.searches += 1;
        let collect = self.problem.rules.search_evidence && !soft;
        let result = self.search.run(&query, collect, cancelled);
        let retry = self.problem.rules.detour_search
            && !soft
            && matches!(result, Err(SearchFailure::Budget));
        let result = if retry {
            self.stats.detour_searches += 1;
            self.search
                .retry_around_obstacles(&query, collect, cancelled)
        } else {
            result
        };
        let found = match result {
            Ok(path) => path,
            Err(failure) => {
                if collect && failure != SearchFailure::Cancelled {
                    self.collect_frontier(wire.net, h, frontier);
                }
                let reason = match failure {
                    SearchFailure::Exhausted => UnroutedReason::NoPath,
                    SearchFailure::DisconnectedGrid => UnroutedReason::DisconnectedGrid,
                    SearchFailure::Budget => UnroutedReason::SearchBudget,
                    SearchFailure::Cancelled => UnroutedReason::Cancelled,
                };
                return (Err(reason), Vec::new());
            }
        };

        // Path to runs.
        let cells = self.grid.cells() as u32;
        let (Some(&first), Some(&last)) = (found.nodes.first(), found.nodes.last()) else {
            return (Err(UnroutedReason::GeometryRejected), Vec::new());
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
                        width,
                        points: vec![p],
                    });
                }
                None => runs.push(Run {
                    layer,
                    width,
                    points: vec![p],
                }),
            }
        }
        if let Ep::Terminal(t, stub) = start {
            attach_pad_stub(
                &mut runs,
                self.problem.terminals[t].position.into(),
                stub,
                true,
            );
        }
        if let Ep::Terminal(t, stub) = end {
            attach_pad_stub(
                &mut runs,
                self.problem.terminals[t].position.into(),
                stub,
                false,
            );
        }
        for run in &mut runs {
            merge_collinear(&mut run.points);
        }

        // Soft mode: rip up the routes in the way.
        let mut ripped = Vec::new();
        let mut contested = Vec::new();
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
                            run.half_width() as f64,
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
            if !ok {
                return (Err(UnroutedReason::GeometryRejected), ripped);
            }
            if !self.is_critical(wire.net)
                && conflicts.iter().any(|&id| {
                    self.routes[id]
                        .as_ref()
                        .is_some_and(|route| self.is_critical(route.net))
                })
            {
                return (Err(UnroutedReason::ProtectedRoute), ripped);
            }
            if locked {
                return (Err(UnroutedReason::RipUpLimit), ripped);
            }
            if conflicts.is_empty() {
                return (Err(UnroutedReason::NoRipUpConflict), ripped);
            }
            if self.problem.rules.congestion_history {
                // Learn only after the exact check found generated copper to
                // remove. The search maps locate contention along that path.
                let history = self
                    .congestion
                    .get_or_insert_with(|| Congestion::new(self.grid, self.routing.len()));
                let routes = &self.trace_maps[&h].routes;
                for &node in &found.nodes {
                    if !usable(routes[node as usize], owner_of(wire.net)) {
                        contested
                            .push(history.tile((node / cells) as usize, (node % cells) as usize));
                    }
                }
                for pair in found.nodes.windows(2) {
                    let cell = (pair[0] % cells) as usize;
                    if pair[0] / cells != pair[1] / cells
                        && !usable(self.via_map.routes[cell], owner_of(wire.net))
                    {
                        contested.push(history.via_tile(cell));
                    }
                }
            }
            for id in conflicts {
                self.rip_up(id, &mut ripped);
            }
        }

        // Shortcuts along 45° lines.
        for run in &mut runs {
            let h = run.half_width();
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
            run.points.windows(2).all(|pair| {
                self.check_segment_strict(pair[0], pair[1], run.layer, wire.net, run.half_width())
            })
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
            return (Err(UnroutedReason::GeometryRejected), ripped);
        }

        self.commit(w, runs, vias, start, end);
        if !contested.is_empty()
            && let Some(history) = &mut self.congestion
        {
            history.record(contested);
            self.stats.congestion_tiles = history.used_tiles();
        }
        (Ok(()), ripped)
    }

    fn collect_frontier(&self, net: u32, half_width: i64, out: &mut Vec<FrontierSample>) {
        let cells = self.grid.cells();
        for step in self.search.frontier() {
            let from = self.grid.point(step.from % cells);
            let to = self.grid.point(step.to % cells);
            let copper = self.routing[step.to / cells];
            let via_site = step.via && step.from == step.to;
            let radius = if via_site {
                self.rules.via_radius.ceil() as i64
            } else {
                half_width + 1
            };
            let blocker = if self
                .problem
                .region
                .is_some_and(|r| !r.contains(to.to_point(), Length::new(radius)))
            {
                Some(FrontierBlocker::RegionBoundary)
            } else {
                let mut allow_generated = |_| !step.generated;
                let hit = if via_site {
                    self.items.check_via(to, net, None, &mut allow_generated)
                } else {
                    self.items.check_segment(
                        (from, to),
                        copper,
                        net,
                        half_width as f64,
                        None,
                        &mut allow_generated,
                    )
                };
                match hit {
                    Err(Violation::Static(index)) if !step.generated => {
                        Some(FrontierBlocker::Existing(
                            index.map_or(crate::AccessBlocker::BoardEdge, |i| self.blocker(i)),
                        ))
                    }
                    Err(Violation::Route(id)) => {
                        self.routes[id]
                            .as_ref()
                            .map(|route| FrontierBlocker::Generated {
                                request: self.wires[route.wire].request,
                                net: self.nets[route.net as usize],
                            })
                    }
                    _ => None,
                }
            };
            out.push(FrontierSample {
                layer: self.problem.copper_layers[copper],
                from: from.to_point(),
                to: to.to_point(),
                via: step.via,
                generated: step.generated,
                blocker,
            });
        }
    }

    fn commit(&mut self, w: usize, runs: Vec<Run>, vias: Vec<P>, start: Ep, end: Ep) {
        let id = self.routes.len();
        let net = self.wires[w].net;
        let r = self.rules;
        let mut new_items = Vec::new();
        for run in &runs {
            let hw = run.half_width() as f64;
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
    fn verify_all(&mut self, failures: &mut [AttemptFailures]) {
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
                                run.half_width() as f64,
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
                    // Rip-up removes the rejected route first, then its attachments.
                    for (i, wire) in ripped.into_iter().enumerate() {
                        failures[wire].strict = Some(if i == 0 {
                            UnroutedReason::GeometryRejected
                        } else {
                            UnroutedReason::AttachmentRemoved
                        });
                    }
                }
                None => break,
            }
        }
    }

    fn is_critical(&self, net: u32) -> bool {
        self.problem
            .rules
            .critical_nets
            .contains(&self.nets[net as usize])
    }

    /// Rank marked pads by exclusive and total grid entries on the initial
    /// board. With pad escape enabled count only exterior entries.
    fn pad_access_pressure(&mut self) -> BTreeMap<usize, (usize, usize)> {
        let terminals: BTreeMap<_, _> = self
            .wires
            .iter()
            .flat_map(|w| [(w.a, w.net), (w.b, w.net)])
            .filter(|&(t, _)| self.problem.pad_terminals.contains(&TerminalId(t)))
            .collect();
        let mut owners = HashMap::new();
        let mut entries = Vec::with_capacity(terminals.len());
        for (t, net) in terminals {
            let h = self.width_of(net).to_nm() / 2;
            self.ensure_trace_map(h);
            let nodes: Vec<_> = self
                .terminal_access(t, net, h)
                .into_iter()
                .filter(|a| !self.problem.rules.pad_escape || a.escape)
                .map(|a| a.node)
                .collect();
            for &node in &nodes {
                let owner = owners.entry(node).or_insert(Some(net));
                if *owner != Some(net) {
                    *owner = None;
                }
            }
            entries.push((t, net, nodes));
        }
        entries
            .into_iter()
            .map(|(t, net, nodes)| {
                let exclusive = nodes.iter().filter(|n| owners[n] == Some(net)).count();
                (t, (exclusive, nodes.len()))
            })
            .collect()
    }

    pub fn run(
        mut self,
        progress: &mut dyn FnMut(Progress) -> ControlFlow<()>,
        stop: &dyn Fn() -> bool,
    ) -> RoutingResult {
        let problem = self.problem;
        let policy = match problem.rules.order {
            RoutingOrder::Auto if problem.rules.pad_escape => RoutingOrder::ConstrainedFirst,
            RoutingOrder::Auto => RoutingOrder::ShortestFirst,
            explicit => explicit,
        };
        let pressure = if policy == RoutingOrder::ConstrainedFirst {
            self.pad_access_pressure()
        } else {
            BTreeMap::new()
        };
        let mut order: Vec<usize> = (0..self.wires.len()).collect();
        let rank = |w: &Wire| {
            let (a, b) = (
                P::from(problem.terminals[w.a].position),
                P::from(problem.terminals[w.b].position),
            );
            let (dx, dy) = ((a.x - b.x) as f64, (a.y - b.y) as f64);
            let access = |t| {
                pressure
                    .get(&t)
                    .copied()
                    .unwrap_or((usize::MAX, usize::MAX))
            };
            (
                !self.is_critical(w.net),
                access(w.a).min(access(w.b)),
                (dx * dx + dy * dy).sqrt(),
            )
        };
        let ranks: Vec<_> = self.wires.iter().map(rank).collect();
        order.sort_by(|&x, &y| {
            let (critical_x, access_x, length_x) = ranks[x];
            let (critical_y, access_y, length_y) = ranks[y];
            critical_x
                .cmp(&critical_y)
                .then_with(|| access_x.cmp(&access_y))
                .then_with(|| match policy {
                    RoutingOrder::LongestFirst => length_y.total_cmp(&length_x),
                    _ => length_x.total_cmp(&length_y),
                })
                .then_with(|| x.cmp(&y))
        });

        let mut failures = vec![AttemptFailures::default(); self.wires.len()];
        let mut soft_tries = vec![0u32; self.wires.len()];
        let max = self.problem.rules.max_rip_ups;
        let mut queue: VecDeque<usize> = order.iter().copied().collect();
        let mut failed: Vec<usize> = Vec::new();
        let mut cancelled = false;
        while let Some(w) = queue.pop_front() {
            if stop() || self.report(progress).is_break() {
                cancelled = true;
                break;
            }
            if self.wire_route[w].is_some() || self.is_connected(w) {
                continue;
            }
            match self.try_route(w, false, &mut failures[w].frontier, stop).0 {
                Ok(()) => {
                    failures[w] = AttemptFailures::default();
                    continue;
                }
                Err(UnroutedReason::NoAccess) => {
                    failures[w].strict = Some(UnroutedReason::NoAccess);
                    failed.push(w);
                    continue;
                }
                Err(UnroutedReason::Cancelled) => {
                    cancelled = true;
                    break;
                }
                Err(reason) => failures[w].strict = Some(reason),
            }
            if soft_tries[w] < max {
                soft_tries[w] += 1;
                let (outcome, ripped) = self.try_route(w, true, &mut failures[w].frontier, stop);
                queue.extend(ripped);
                match outcome {
                    Ok(()) => {
                        failures[w] = AttemptFailures::default();
                        continue;
                    }
                    Err(UnroutedReason::Cancelled) => {
                        cancelled = true;
                        break;
                    }
                    Err(reason) => failures[w].soft = Some(reason),
                }
            }
            failed.push(w);
        }
        // Retry failed wires once, the situation may have changed.
        if !cancelled {
            for &w in &failed {
                if stop() || self.report(progress).is_break() {
                    cancelled = true;
                    break;
                }
                if self.wire_route[w].is_none() && !self.is_connected(w) {
                    match self.try_route(w, false, &mut failures[w].frontier, stop).0 {
                        Ok(()) => failures[w] = AttemptFailures::default(),
                        Err(UnroutedReason::Cancelled) => {
                            cancelled = true;
                            break;
                        }
                        Err(reason) => failures[w].strict = Some(reason),
                    }
                }
            }
        }
        self.verify_all(&mut failures);
        self.into_result(&failures, cancelled)
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

    fn into_result(self, failures: &[AttemptFailures], cancelled: bool) -> RoutingResult {
        let copper = &self.problem.copper_layers;
        let layer_of = |l: usize| copper[self.routing[l]];
        let rules = &self.problem.rules;
        let mut routes: Vec<Route> = Vec::new();
        for route in self.routes.iter().flatten() {
            let wire = self.wires[route.wire];
            let own_via = |p: P| route.vias.contains(&p);
            let endpoint = |ep: Ep| match ep {
                Ep::Terminal(t, _) => Endpoint::Terminal(TerminalId(t)),
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
                        width: run.width,
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
                reason: match failures[i].strict {
                    Some(r) => r,
                    None if cancelled => UnroutedReason::Cancelled,
                    None => UnroutedReason::NotRouted,
                },
                soft_reason: failures[i].soft,
                frontier: failures[i].frontier.clone(),
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

fn attach_pad_stub(runs: &mut Vec<Run>, pos: P, stub: PadStub, at_start: bool) {
    let index = if at_start { 0 } else { runs.len() - 1 };
    let run = &mut runs[index];
    let node = if at_start {
        run.points[0]
    } else {
        *run.points.last().unwrap()
    };
    if let Some(width) = stub.neck_width {
        let layer = run.layer;
        let mut points = vec![pos];
        points.extend(stub.elbow);
        points.push(node);
        if !at_start {
            points.reverse();
        }
        runs.insert(
            if at_start { 0 } else { index + 1 },
            Run {
                layer,
                width,
                points,
            },
        );
    } else if at_start {
        if let Some(elbow) = stub.elbow {
            run.points.insert(0, elbow);
        }
        if run.points[0] != pos {
            run.points.insert(0, pos);
        }
    } else {
        run.points.extend(stub.elbow);
        if run.points.last() != Some(&pos) {
            run.points.push(pos);
        }
    }
}

/// Both ways to join an off-grid anchor and a grid node with one 45° bend.
fn escape_elbows(a: P, b: P) -> [P; 2] {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let diagonal = dx.abs().min(dy.abs());
    let offset = P::new(dx.signum() * diagonal, dy.signum() * diagonal);
    [
        P::new(a.x + offset.x, a.y + offset.y),
        P::new(b.x - offset.x, b.y - offset.y),
    ]
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
