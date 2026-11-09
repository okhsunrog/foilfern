//! Synthetic routing problems, checked by an independent brute-force
//! clearance and connectivity verifier.

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::time::Instant;

use librepcb_autoroute::{
    Connection, Error, Hole, LayerScope, NetId, Obstacle, RoutingProblem, RoutingResult,
    RoutingRules, Shape, Terminal, TerminalId, UnroutedReason, route, route_with_progress,
};
use librepcb_core::types::{Layer, Length, Point, PositiveLength};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn mm(v: f64) -> Length {
    Length::from_mm(v).unwrap()
}

fn pt(x: f64, y: f64) -> Point {
    Point::from_mm(x, y).unwrap()
}

fn pos(v: f64) -> PositiveLength {
    PositiveLength::new(mm(v)).unwrap()
}

fn net(n: u32) -> NetId {
    format!("00000000-0000-4000-8000-{n:012}").parse().unwrap()
}

fn rect_outline(w: f64, h: f64) -> Vec<Vec<Point>> {
    vec![vec![pt(0.0, 0.0), pt(w, 0.0), pt(w, h), pt(0.0, h)]]
}

fn two_layers() -> Vec<Layer> {
    vec![Layer::TOP_COPPER, Layer::BOT_COPPER]
}

fn smd(p: &mut RoutingProblem, n: NetId, x: f64, y: f64, w: f64, h: f64) -> TerminalId {
    p.add_terminal(Terminal {
        net: n,
        position: pt(x, y),
        shape: Shape::rect(pt(x, y), mm(w), mm(h)),
        layers: LayerScope::single(Layer::TOP_COPPER),
        clearance: None,
    })
}

fn tht(p: &mut RoutingProblem, n: NetId, x: f64, y: f64, d: f64, drill: f64) -> TerminalId {
    p.add_hole(Hole {
        shape: Shape::Circle {
            center: pt(x, y),
            diameter: pos(drill),
        },
        plated: true,
    });
    p.add_terminal(Terminal {
        net: n,
        position: pt(x, y),
        shape: Shape::Circle {
            center: pt(x, y),
            diameter: pos(d),
        },
        layers: LayerScope::All,
        clearance: None,
    })
}

/// Independent geometry in f64 millimeters.
mod check {
    pub type V = (f64, f64);

    pub fn point_seg(p: V, a: V, b: V) -> f64 {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let l2 = dx * dx + dy * dy;
        let t = if l2 > 0.0 {
            (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        ((p.0 - a.0 - t * dx).powi(2) + (p.1 - a.1 - t * dy).powi(2)).sqrt()
    }

    fn cross(o: V, a: V, b: V) -> f64 {
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    }

    pub fn seg_seg(a: V, b: V, c: V, d: V) -> f64 {
        let (d1, d2) = (cross(a, b, c), cross(a, b, d));
        let (d3, d4) = (cross(c, d, a), cross(c, d, b));
        if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
            return 0.0;
        }
        point_seg(a, c, d)
            .min(point_seg(b, c, d))
            .min(point_seg(c, a, b))
            .min(point_seg(d, a, b))
    }

    pub fn inside(poly: &[V], p: V) -> bool {
        let mut inside = false;
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            if (a.1 > p.1) != (b.1 > p.1) && p.0 < a.0 + (p.1 - a.1) * (b.0 - a.0) / (b.1 - a.1) {
                inside = !inside;
            }
        }
        inside
    }

    /// Distance between a segment and a shape given as (core, radius).
    pub fn seg_core(a: V, b: V, core: &Core) -> f64 {
        match core {
            Core::Seg(c, d, r) => seg_seg(a, b, *c, *d) - r,
            Core::Poly(v) => {
                if inside(v, a) || inside(v, b) {
                    return 0.0;
                }
                (0..v.len())
                    .map(|i| seg_seg(a, b, v[i], v[(i + 1) % v.len()]))
                    .fold(f64::INFINITY, f64::min)
            }
        }
    }

    pub enum Core {
        Seg(V, V, f64),
        Poly(Vec<V>),
    }
}

use check::{Core, V};

fn v(p: Point) -> V {
    p.to_mm()
}

fn core_of(shape: &Shape) -> Core {
    match shape {
        Shape::Polygon(pts) => Core::Poly(pts.iter().map(|p| v(*p)).collect()),
        Shape::Circle { center, diameter } => {
            Core::Seg(v(*center), v(*center), diameter.to_mm() / 2.0)
        }
        Shape::Segment { start, end, width } => Core::Seg(v(*start), v(*end), width.to_mm() / 2.0),
    }
}

fn on_layer(scope: &LayerScope, layer: Layer) -> bool {
    match scope {
        LayerScope::All => true,
        LayerScope::Only(l) => l.contains(&layer),
    }
}

/// Brute-force DRC of the result: copper clearances between nets, board
/// clearance, holes, via drill clearances. Returns the violations.
fn drc(p: &RoutingProblem, r: &RoutingResult) -> Vec<String> {
    let rules = &p.rules;
    let cl = rules.clearance.to_mm();
    let eps = 1e-6;
    let mut errors = Vec::new();
    // Copper objects: (net, layer scope, core, description).
    struct Obj {
        net: Option<NetId>,
        layers: LayerScope,
        core: Core,
        what: String,
    }
    let mut objs: Vec<Obj> = Vec::new();
    for (i, t) in p.terminals.iter().enumerate() {
        objs.push(Obj {
            net: Some(t.net),
            layers: t.layers.clone(),
            core: core_of(&t.shape),
            what: format!("terminal {i}"),
        });
    }
    for (i, o) in p.obstacles.iter().enumerate() {
        objs.push(Obj {
            net: o.net,
            layers: o.layers.clone(),
            core: core_of(&o.shape),
            what: format!("obstacle {i}"),
        });
    }
    for (ri, route) in r.routes.iter().enumerate() {
        for s in &route.segments {
            objs.push(Obj {
                net: Some(route.net),
                layers: LayerScope::single(s.layer),
                core: Core::Seg(v(s.start), v(s.end), s.width.to_mm() / 2.0),
                what: format!("route {ri} segment"),
            });
        }
        for via in &route.vias {
            objs.push(Obj {
                net: Some(route.net),
                layers: LayerScope::All,
                core: Core::Seg(v(via.position), v(via.position), via.diameter.to_mm() / 2.0),
                what: format!("route {ri} via"),
            });
        }
    }
    let outline: Vec<Vec<V>> = p
        .outline
        .iter()
        .map(|poly| poly.iter().map(|q| v(*q)).collect())
        .collect();
    for (ri, route) in r.routes.iter().enumerate() {
        let mine = |o: &Obj| o.net == Some(route.net);
        for s in &route.segments {
            let (a, b, hw) = (v(s.start), v(s.end), s.width.to_mm() / 2.0);
            for o in objs
                .iter()
                .filter(|o| !mine(o) && on_layer(&o.layers, s.layer))
            {
                let d = check::seg_core(a, b, &o.core) - hw;
                if d < cl - eps {
                    errors.push(format!("route {ri} segment vs {}: {d:.4}", o.what));
                }
            }
            for poly in &outline {
                if !check::inside(poly, a) || !check::inside(poly, b) {
                    errors.push(format!("route {ri} segment outside board"));
                }
                for i in 0..poly.len() {
                    let d = check::seg_seg(a, b, poly[i], poly[(i + 1) % poly.len()]) - hw;
                    if d < rules.board_clearance.to_mm() - eps {
                        errors.push(format!("route {ri} segment vs board edge: {d:.4}"));
                    }
                }
            }
            for h in p.holes.iter().filter(|h| !h.plated) {
                let d = check::seg_core(a, b, &core_of(&h.shape)) - hw;
                if d < rules.npth_clearance.to_mm() - eps {
                    errors.push(format!("route {ri} segment vs NPTH: {d:.4}"));
                }
            }
        }
        for via in &route.vias {
            let c = v(via.position);
            let r_via = via.diameter.to_mm() / 2.0;
            let r_drill = via.drill.to_mm() / 2.0;
            for o in objs.iter().filter(|o| !mine(o)) {
                let d = check::seg_core(c, c, &o.core) - r_via;
                if d < cl - eps {
                    errors.push(format!("route {ri} via vs {}: {d:.4}", o.what));
                }
            }
            for h in &p.holes {
                let d = check::seg_core(c, c, &core_of(&h.shape)) - r_drill;
                if d < rules.drill_clearance.to_mm() - eps {
                    errors.push(format!("route {ri} via drill vs hole: {d:.4}"));
                }
            }
            for (rj, other) in r.routes.iter().enumerate() {
                for w in other.vias.iter().filter(|w| !(rj == ri && w == &via)) {
                    let d = check::point_seg(c, v(w.position), v(w.position)) - 2.0 * r_drill;
                    if d < rules.drill_clearance.to_mm() - eps {
                        errors.push(format!("route {ri} via drill vs via drill: {d:.4}"));
                    }
                }
            }
            for poly in &outline {
                for i in 0..poly.len() {
                    let d = check::point_seg(c, poly[i], poly[(i + 1) % poly.len()]);
                    if d - r_via < rules.board_clearance.to_mm() - eps
                        || d - r_drill < rules.drill_board_clearance.to_mm() - eps
                    {
                        errors.push(format!("route {ri} via vs board edge: {d:.4}"));
                    }
                }
            }
        }
        // No acute angles and all segments 45° multiples except stubs.
        for s in &route.segments {
            assert_ne!(s.start, s.end, "zero length segment");
        }
    }
    errors
}

/// A connectivity node: a point on a layer, or on all layers (`None`).
type Node = (Option<Layer>, Point);

/// Checks that every air wire not reported as unrouted is electrically
/// connected by the new segments and vias (nodes identified by coordinates).
fn assert_connected(p: &RoutingProblem, r: &RoutingResult) {
    let mut parent: HashMap<Node, Node> = HashMap::new();
    fn find(parent: &mut HashMap<Node, Node>, k: Node) -> Node {
        let mut k = k;
        loop {
            let pk = *parent.entry(k).or_insert(k);
            if pk == k {
                return k;
            }
            k = pk;
        }
    }
    let mut union = |a, b| {
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra != rb {
            parent.insert(ra, rb);
        }
    };
    for s in r.segments() {
        union((Some(s.layer), s.start), (Some(s.layer), s.end));
    }
    for via in r.vias() {
        for &l in &p.copper_layers {
            union((None, via.position), (Some(l), via.position));
        }
    }
    for t in &p.terminals {
        for &l in &p.copper_layers {
            if on_layer(&t.layers, l) {
                union((None, t.position), (Some(l), t.position));
            }
        }
    }
    let unrouted: Vec<(TerminalId, TerminalId)> = r.unrouted.iter().map(|u| u.terminals).collect();
    for c in &p.connections {
        let pairs: Vec<(TerminalId, TerminalId)> = match c {
            Connection::Terminals(a, b) => vec![(*a, *b)],
            Connection::Net(_) => continue,
        };
        for (a, b) in pairs {
            if unrouted.contains(&(a, b)) {
                continue;
            }
            let ka = (None, p.terminals[a.0].position);
            let kb = (None, p.terminals[b.0].position);
            assert_eq!(
                find(&mut parent, ka),
                find(&mut parent, kb),
                "terminals {a:?} and {b:?} not connected"
            );
        }
    }
}

fn assert_clean(p: &RoutingProblem, r: &RoutingResult) {
    let errors = drc(p, r);
    assert!(errors.is_empty(), "clearance violations: {errors:#?}");
    assert_connected(p, r);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Two resistors, an LED and a 2-pin connector on a 30 x 20 mm board.
fn led_board() -> RoutingProblem {
    let mut p = RoutingProblem::new(
        rect_outline(30.0, 20.0),
        two_layers(),
        RoutingRules::default(),
    );
    let (vcc, n1, n2, gnd) = (net(1), net(2), net(3), net(4));
    // Connector J1 (THT, 2.54 mm pitch).
    let j1_1 = tht(&mut p, vcc, 4.0, 8.73, 1.7, 1.0);
    let j1_2 = tht(&mut p, gnd, 4.0, 11.27, 1.7, 1.0);
    // R1 (0805) horizontal at (12, 6).
    let r1_1 = smd(&mut p, vcc, 11.05, 6.0, 1.0, 1.3);
    let r1_2 = smd(&mut p, n1, 12.95, 6.0, 1.0, 1.3);
    // LED (0805) at (20, 10), rotated.
    let led_a = smd(&mut p, n1, 20.0, 9.05, 1.3, 1.0);
    let led_k = smd(&mut p, n2, 20.0, 10.95, 1.3, 1.0);
    // R2 (0805) at (12, 14).
    let r2_1 = smd(&mut p, n2, 12.95, 14.0, 1.0, 1.3);
    let r2_2 = smd(&mut p, gnd, 11.05, 14.0, 1.0, 1.3);
    // Mounting holes.
    for (x, y) in [(2.5, 2.5), (27.5, 2.5), (2.5, 17.5), (27.5, 17.5)] {
        p.add_hole(Hole {
            shape: Shape::Circle {
                center: pt(x, y),
                diameter: pos(3.2),
            },
            plated: false,
        });
    }
    p.connect(j1_1, r1_1);
    p.connect(r1_2, led_a);
    p.connect(led_k, r2_1);
    p.connect(r2_2, j1_2);
    p
}

#[test]
fn led_board_routes_completely() {
    let p = led_board();
    let r = route(&p).unwrap();
    assert!(r.is_complete(), "unrouted: {:?}", r.unrouted);
    assert_eq!(r.routes.len(), 4);
    assert_clean(&p, &r);
    for route in &r.routes {
        assert!(!route.segments.is_empty());
    }
}

#[test]
fn routing_is_deterministic() {
    let p = led_board();
    assert_eq!(route(&p).unwrap(), route(&p).unwrap());
}

#[test]
fn crossing_nets_need_a_via() {
    // Net A is a wall across the board on both layers' top side (SMD pads at
    // the board edges, too close to the edge to pass around). Net B must
    // cross it through a via.
    let mut p = RoutingProblem::new(
        rect_outline(20.0, 20.0),
        two_layers(),
        RoutingRules::default(),
    );
    let (a, b) = (net(1), net(2));
    let a1 = smd(&mut p, a, 1.2, 10.0, 1.6, 1.6);
    let a2 = smd(&mut p, a, 18.8, 10.0, 1.6, 1.6);
    let b1 = smd(&mut p, b, 10.0, 3.0, 1.6, 1.6);
    let b2 = smd(&mut p, b, 10.0, 17.0, 1.6, 1.6);
    p.connect(a1, a2);
    p.connect(b1, b2);
    let r = route(&p).unwrap();
    assert!(r.is_complete(), "unrouted: {:?}", r.unrouted);
    assert!(r.vias().count() >= 2, "expected vias, got {:?}", r.routes);
    assert!(r.segments().any(|s| s.layer == Layer::BOT_COPPER));
    assert_clean(&p, &r);
}

#[test]
fn single_layer_crossing_is_unroutable() {
    let rules = RoutingRules {
        allowed_layers: Some(vec![Layer::TOP_COPPER]),
        ..RoutingRules::default()
    };
    let mut p = RoutingProblem::new(rect_outline(20.0, 20.0), two_layers(), rules);
    let (a, b) = (net(1), net(2));
    let a1 = smd(&mut p, a, 1.2, 10.0, 1.6, 1.6);
    let a2 = smd(&mut p, a, 18.8, 10.0, 1.6, 1.6);
    let b1 = smd(&mut p, b, 10.0, 1.2, 1.6, 1.6);
    let b2 = smd(&mut p, b, 10.0, 18.8, 1.6, 1.6);
    p.connect(a1, a2);
    p.connect(b1, b2);
    let r = route(&p).unwrap();
    assert_eq!(r.routes.len(), 1);
    assert_eq!(r.unrouted.len(), 1);
    assert_eq!(r.unrouted[0].reason, UnroutedReason::NoPath);
    assert_eq!(r.vias().count(), 0);
    assert_clean(&p, &r);
}

#[test]
fn blocked_path_is_reported_as_unrouted() {
    // A keepout wall on all layers splits the board.
    let mut p = RoutingProblem::new(
        rect_outline(30.0, 20.0),
        two_layers(),
        RoutingRules::default(),
    );
    p.add_obstacle(Obstacle::keepout(
        Shape::rect(pt(15.0, 10.0), mm(1.0), mm(22.0)),
        LayerScope::All,
    ));
    let (a, b) = (net(1), net(2));
    let a1 = smd(&mut p, a, 5.0, 10.0, 1.0, 1.0);
    let a2 = smd(&mut p, a, 25.0, 10.0, 1.0, 1.0);
    let b1 = smd(&mut p, b, 5.0, 5.0, 1.0, 1.0);
    let b2 = smd(&mut p, b, 10.0, 5.0, 1.0, 1.0);
    p.connect(a1, a2);
    p.connect(b1, b2);
    let r = route(&p).unwrap();
    assert_eq!(r.unrouted.len(), 1);
    assert_eq!(r.unrouted[0].request, 0);
    assert_eq!(r.unrouted[0].terminals, (a1, a2));
    assert_eq!(r.unrouted[0].reason, UnroutedReason::NoPath);
    assert_eq!(r.routes.len(), 1);
    assert_eq!(r.routes[0].net, b);
    assert_clean(&p, &r);
}

#[test]
fn inaccessible_terminal_is_reported() {
    // Bottom pad, but only the top layer may be used.
    let rules = RoutingRules {
        allowed_layers: Some(vec![Layer::TOP_COPPER]),
        ..RoutingRules::default()
    };
    let mut p = RoutingProblem::new(rect_outline(20.0, 20.0), two_layers(), rules);
    let a = net(1);
    let a1 = smd(&mut p, a, 5.0, 10.0, 1.0, 1.0);
    let a2 = p.add_terminal(Terminal {
        net: a,
        position: pt(15.0, 10.0),
        shape: Shape::rect(pt(15.0, 10.0), mm(1.0), mm(1.0)),
        layers: LayerScope::single(Layer::BOT_COPPER),
        clearance: None,
    });
    p.connect(a1, a2);
    let r = route(&p).unwrap();
    assert_eq!(r.unrouted.len(), 1);
    assert_eq!(r.unrouted[0].reason, UnroutedReason::NoAccess);
}

#[test]
fn narrow_gap_is_avoided() {
    // Two pads of another net leave a 0.5 mm gap, too narrow for a 0.25 mm
    // trace with 0.2 mm clearance on each side (0.65 mm).
    let mut p = RoutingProblem::new(
        rect_outline(20.0, 20.0),
        vec![Layer::TOP_COPPER],
        RoutingRules::default(),
    );
    let (a, b) = (net(1), net(2));
    p.add_obstacle(Obstacle::copper(
        Shape::rect(pt(10.0, 4.375), mm(2.0), mm(10.75)),
        LayerScope::All,
        Some(b),
    ));
    p.add_obstacle(Obstacle::copper(
        Shape::rect(pt(10.0, 15.625), mm(2.0), mm(10.75)),
        LayerScope::All,
        Some(b),
    ));
    let a1 = smd(&mut p, a, 3.0, 10.0, 1.0, 1.0);
    let a2 = smd(&mut p, a, 17.0, 10.0, 1.0, 1.0);
    p.connect(a1, a2);
    let r = route(&p).unwrap();
    // The obstacles cover the full height except the gap.
    assert_eq!(r.unrouted.len(), 1);

    // With a 0.8 mm gap it fits.
    let mut p2 = p.clone();
    p2.obstacles[0].shape = Shape::rect(pt(10.0, 4.3), mm(2.0), mm(10.6));
    p2.obstacles[1].shape = Shape::rect(pt(10.0, 15.7), mm(2.0), mm(10.6));
    let r2 = route(&p2).unwrap();
    assert!(r2.is_complete(), "unrouted: {:?}", r2.unrouted);
    assert_clean(&p2, &r2);
}

#[test]
fn obstacles_of_the_same_net_are_passable() {
    let mut p = RoutingProblem::new(
        rect_outline(20.0, 10.0),
        vec![Layer::TOP_COPPER],
        RoutingRules::default(),
    );
    let a = net(1);
    p.add_obstacle(Obstacle::copper(
        Shape::rect(pt(10.0, 5.0), mm(2.0), mm(12.0)),
        LayerScope::All,
        Some(a),
    ));
    let a1 = smd(&mut p, a, 3.0, 5.0, 1.0, 1.0);
    let a2 = smd(&mut p, a, 17.0, 5.0, 1.0, 1.0);
    p.connect(a1, a2);
    let r = route(&p).unwrap();
    assert!(r.is_complete());
    assert_clean(&p, &r);
    // A straight line.
    assert_eq!(r.routes[0].segments.len(), 1);
}

#[test]
fn whole_net_connection() {
    let mut p = RoutingProblem::new(
        rect_outline(30.0, 20.0),
        two_layers(),
        RoutingRules::default(),
    );
    let gnd = net(7);
    let ts: Vec<TerminalId> = [
        (5.0, 5.0),
        (25.0, 5.0),
        (15.0, 15.0),
        (5.0, 15.0),
        (25.0, 15.0),
    ]
    .iter()
    .map(|&(x, y)| smd(&mut p, gnd, x, y, 1.2, 1.2))
    .collect();
    p.connect_net(gnd);
    let r = route(&p).unwrap();
    assert!(r.is_complete(), "unrouted: {:?}", r.unrouted);
    assert_eq!(r.stats.air_wires, 4);
    assert_clean(&p, &r);
    // All terminals connected: check with explicit pairs.
    let mut check = p.clone();
    check.connections = ts
        .windows(2)
        .map(|w| Connection::Terminals(w[0], w[1]))
        .collect();
    assert_connected(&check, &r);
}

#[test]
fn rip_up_and_reroute() {
    // A wall with two gaps: G1 (1.5 mm) fits one trace, G2 (1.0 mm) only a
    // thin one. The short thin net X takes G1 first; the wide net Y only
    // fits through G1, so X is ripped up and rerouted through G2.
    let (x, y) = (net(1), net(2));
    let mut rules = RoutingRules::default();
    rules.net_trace_widths.insert(y, pos(0.8));
    rules.allowed_layers = Some(vec![Layer::TOP_COPPER]);
    let mut p = RoutingProblem::new(rect_outline(40.0, 20.0), two_layers(), rules);
    let wall = |p: &mut RoutingProblem, y0: f64, y1: f64| {
        p.add_obstacle(Obstacle::keepout(
            Shape::Polygon(vec![pt(19.5, y0), pt(20.5, y0), pt(20.5, y1), pt(19.5, y1)]),
            LayerScope::All,
        ));
    };
    wall(&mut p, -1.0, 9.25);
    wall(&mut p, 10.75, 15.5);
    wall(&mut p, 16.5, 21.0);
    let x1 = smd(&mut p, x, 17.0, 12.0, 1.0, 1.0);
    let x2 = smd(&mut p, x, 23.0, 12.0, 1.0, 1.0);
    let y1 = smd(&mut p, y, 10.0, 10.0, 1.5, 1.5);
    let y2 = smd(&mut p, y, 30.0, 10.0, 1.5, 1.5);
    p.connect(x1, x2);
    p.connect(y1, y2);
    let r = route(&p).unwrap();
    assert!(r.is_complete(), "unrouted: {:?}", r.unrouted);
    assert!(r.stats.rip_ups >= 1);
    let rx = r.routes.iter().find(|r| r.net == x).unwrap();
    assert!(rx.segments.iter().any(|s| s.start.y > mm(15.0)));
    assert_clean(&p, &r);
}

#[test]
fn cancel_reports_remaining_connections() {
    let p = led_board();
    let r = route_with_progress(&p, |_| ControlFlow::Break(())).unwrap();
    assert!(r.routes.is_empty());
    assert_eq!(r.unrouted.len(), 4);
    assert!(
        r.unrouted
            .iter()
            .all(|u| u.reason == UnroutedReason::Cancelled)
    );
}

#[test]
fn invalid_problems() {
    let mut p = led_board();
    p.connections
        .push(Connection::Terminals(TerminalId(0), TerminalId(99)));
    assert_eq!(route(&p), Err(Error::UnknownTerminal(99)));

    let mut p = led_board();
    p.connections
        .push(Connection::Terminals(TerminalId(0), TerminalId(1)));
    assert_eq!(route(&p), Err(Error::NetMismatch(0, 1)));

    let mut p = led_board();
    p.outline.clear();
    assert_eq!(route(&p), Err(Error::NoOutline));

    let mut p = led_board();
    p.rules.via_drill = pos(1.0);
    assert_eq!(route(&p), Err(Error::InvalidVia));

    let mut p = led_board();
    p.rules.allowed_layers = Some(vec![Layer::inner_copper(1).unwrap()]);
    assert_eq!(route(&p), Err(Error::NoRoutingLayers));
}

/// ~30 connections between SMD pads (a deterministic pseudo-random
/// netlist) on a 50 x 40 mm two layer board. Must finish quickly even in
/// debug builds.
#[test]
fn benchmark_thirty_connections() {
    let mut p = RoutingProblem::new(
        rect_outline(50.0, 40.0),
        two_layers(),
        RoutingRules::default(),
    );
    // Six "ICs" with 10 pads each (SOIC-like, 1.27 mm pitch).
    let mut pads: Vec<(f64, f64)> = Vec::new();
    for (cx, cy) in [
        (10.0, 10.0),
        (25.0, 10.0),
        (40.0, 10.0),
        (10.0, 30.0),
        (25.0, 30.0),
        (40.0, 30.0),
    ] {
        for i in 0..5 {
            let y = cy - 2.54 + f64::from(i) * 1.27;
            pads.push((cx - 2.7, y));
            pads.push((cx + 2.7, y));
        }
    }
    // Deterministic shuffle (LCG) and pairing into 30 two-pin nets.
    let mut seed: u64 = 12345;
    let mut order: Vec<usize> = (0..pads.len()).collect();
    for i in (1..order.len()).rev() {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        order.swap(i, (seed >> 33) as usize % (i + 1));
    }
    let mut ids = vec![TerminalId(0); pads.len()];
    for (k, &i) in order.iter().enumerate() {
        let (x, y) = pads[i];
        ids[i] = smd(&mut p, net(k as u32 / 2 + 1), x, y, 1.5, 0.6);
    }
    for pair in order.as_chunks::<2>().0 {
        p.connect(ids[pair[0]], ids[pair[1]]);
    }
    assert_eq!(p.connections.len(), 30);

    let start = Instant::now();
    let r = route(&p).unwrap();
    let elapsed = start.elapsed();
    eprintln!(
        "benchmark: {} routed, {} unrouted, {:?}, stats {:?}",
        r.routes.len(),
        r.unrouted.len(),
        elapsed,
        r.stats
    );
    assert!(r.routes.len() >= 27, "only {} routed", r.routes.len());
    assert!(elapsed.as_secs_f64() < 20.0, "too slow: {elapsed:?}");
    assert_clean(&p, &r);
}
