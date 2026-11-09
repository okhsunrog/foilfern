//! The routing problem: board outline, copper layers, obstacles, terminals,
//! connections to route and design rules.
//!
//! Everything is plain data in board coordinates (integer nanometers). The
//! problem does not know about the project model; the caller extracts it from
//! a board and applies the [`RoutingResult`](crate::RoutingResult).

use std::collections::BTreeMap;

use librepcb_core::types::{Layer, Length, Point, PositiveLength, Uuid};

/// Identifier of a net (the UUID of the net signal).
pub type NetId = Uuid;

/// A copper (or keepout) shape.
///
/// Arcs are not supported; the caller flattens curved outlines to polygons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// A filled polygon (implicitly closed, the last vertex need not repeat
    /// the first one). Self-intersecting polygons use the even-odd rule.
    Polygon(Vec<Point>),
    /// A filled circle.
    Circle {
        /// Center of the circle.
        center: Point,
        /// Diameter of the circle.
        diameter: PositiveLength,
    },
    /// A straight line with round caps, e.g. a trace or an obround pad.
    Segment {
        /// Start point of the center line.
        start: Point,
        /// End point of the center line.
        end: Point,
        /// Width of the line (diameter of the caps).
        width: PositiveLength,
    },
}

impl Shape {
    /// Creates an axis-aligned rectangle centered at `center`.
    pub fn rect(center: Point, width: Length, height: Length) -> Self {
        let (hw, hh) = (width / 2, height / 2);
        Self::Polygon(vec![
            Point::new(center.x - hw, center.y - hh),
            Point::new(center.x + hw, center.y - hh),
            Point::new(center.x + hw, center.y + hh),
            Point::new(center.x - hw, center.y + hh),
        ])
    }
}

/// The copper layers an object is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayerScope {
    /// All copper layers of the board (THT pads, holes, vias).
    All,
    /// Only the listed layers (layers which are not copper layers of the
    /// problem are ignored).
    Only(Vec<Layer>),
}

impl LayerScope {
    /// A single layer.
    pub fn single(layer: Layer) -> Self {
        Self::Only(vec![layer])
    }
}

/// Something the router must keep clear of: copper of existing traces,
/// vias, planes and pads which are not terminals, or keepout areas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Obstacle {
    /// The shape (including the width of traces).
    pub shape: Shape,
    /// The layers the obstacle is on.
    pub layers: LayerScope,
    /// The net of copper obstacles. Routes of this net may touch or cross it
    /// (without being considered connected to it). `None` blocks all nets
    /// (keepouts, copper without net).
    pub net: Option<NetId>,
    /// Clearance required around this obstacle if larger than
    /// [`RoutingRules::clearance`] (e.g. a pad specific copper clearance).
    pub clearance: Option<Length>,
}

impl Obstacle {
    /// Creates a copper obstacle on the given layers.
    pub fn copper(shape: Shape, layers: LayerScope, net: Option<NetId>) -> Self {
        Self {
            shape,
            layers,
            net,
            clearance: None,
        }
    }

    /// Creates a keepout area which blocks all nets on the given layers.
    pub fn keepout(shape: Shape, layers: LayerScope) -> Self {
        Self::copper(shape, layers, None)
    }
}

/// A drilled hole (always through all layers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hole {
    /// A [`Shape::Circle`] for round holes, a [`Shape::Segment`] for slots.
    pub shape: Shape,
    /// Whether the hole is plated. The copper around a plated hole must be
    /// given separately (as terminal or obstacle); the hole itself only keeps
    /// vias at the drill-to-drill clearance. Non-plated holes keep all copper
    /// at [`RoutingRules::npth_clearance`].
    pub plated: bool,
}

/// A point routes connect to: a pad, or a junction or via of an existing,
/// partially routed net.
///
/// Terminals are copper of their net: routes of other nets keep clear of
/// them, so they must not be added again as [`Obstacle`]s. Vias are never
/// placed on terminals (no via-in-pad).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terminal {
    /// The net of the terminal.
    pub net: NetId,
    /// The anchor point where routes end (the pad position). It should lie
    /// inside `shape`.
    pub position: Point,
    /// The copper shape (pad shape in board coordinates).
    pub shape: Shape,
    /// The layers where routes may connect (THT: all, SMD: one).
    pub layers: LayerScope,
    /// Clearance required around this terminal if larger than
    /// [`RoutingRules::clearance`].
    pub clearance: Option<Length>,
}

/// Index of a terminal in [`RoutingProblem::terminals`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TerminalId(pub usize);

/// A connection to route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    /// Connect two terminals of the same net (an air wire).
    Terminals(TerminalId, TerminalId),
    /// Connect all terminals of a net. It is split into air wires (minimum
    /// spanning tree) which are routed like [`Connection::Terminals`].
    Net(NetId),
}

/// Preferred trace direction of a layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreferredDirection {
    /// Along the x axis.
    Horizontal,
    /// Along the y axis.
    Vertical,
}

/// Design rules and router settings.
///
/// The defaults correspond to the upstream default DRC settings (copper
/// clearance 0.2 mm, board clearance 0.3 mm, NPTH clearance 0.25 mm,
/// drill-drill clearance 0.35 mm, drill-board clearance 0.5 mm, annular
/// ring 0.2 mm, PTH drill 0.3 mm).
#[derive(Debug, Clone, PartialEq)]
pub struct RoutingRules {
    /// Trace width of nets without entry in `net_trace_widths`.
    pub trace_width: PositiveLength,
    /// Trace widths of specific nets.
    pub net_trace_widths: BTreeMap<NetId, PositiveLength>,
    /// Copper to copper clearance between different nets.
    pub clearance: Length,
    /// Copper to board edge clearance.
    pub board_clearance: Length,
    /// Copper to non-plated hole clearance.
    pub npth_clearance: Length,
    /// Drill to drill clearance (vias to vias and holes).
    pub drill_clearance: Length,
    /// Drill to board edge clearance.
    pub drill_board_clearance: Length,
    /// Drill diameter of new vias.
    pub via_drill: PositiveLength,
    /// Outer diameter of new vias.
    pub via_diameter: PositiveLength,
    /// Cost of a via, expressed as the trace length it is worth.
    pub via_cost: Length,
    /// Layers routes may use; `None` allows all copper layers. Vias always go
    /// through all copper layers.
    pub allowed_layers: Option<Vec<Layer>>,
    /// Preferred trace direction per layer. Moves against it cost more.
    pub preferred_directions: BTreeMap<Layer, PreferredDirection>,
    /// Pitch of the routing grid; `None` chooses 0.1 mm, coarser for large
    /// boards (to bound memory and time).
    pub grid_pitch: Option<PositiveLength>,
    /// How often a single connection may be ripped up to make room for
    /// another one (rip-up and reroute).
    pub max_rip_ups: u32,
}

impl Default for RoutingRules {
    fn default() -> Self {
        Self {
            trace_width: positive(250_000),
            net_trace_widths: BTreeMap::new(),
            clearance: Length::new(200_000),
            board_clearance: Length::new(300_000),
            npth_clearance: Length::new(250_000),
            drill_clearance: Length::new(350_000),
            drill_board_clearance: Length::new(500_000),
            via_drill: positive(300_000),
            via_diameter: positive(700_000),
            via_cost: Length::new(2_000_000),
            allowed_layers: None,
            preferred_directions: BTreeMap::new(),
            grid_pitch: None,
            max_rip_ups: 3,
        }
    }
}

impl RoutingRules {
    /// Returns the trace width of a net.
    pub fn trace_width_of(&self, net: &NetId) -> PositiveLength {
        self.net_trace_widths
            .get(net)
            .copied()
            .unwrap_or(self.trace_width)
    }
}

fn positive(nm: i64) -> PositiveLength {
    // Invariant: only called with positive constants.
    PositiveLength::new(Length::new(nm)).expect("constant is positive")
}

/// The complete description of a routing task.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RoutingProblem {
    /// Board outline polygons (outer outline and cutouts, combined with the
    /// even-odd rule).
    pub outline: Vec<Vec<Point>>,
    /// Copper layers of the board, ordered from top to bottom.
    pub copper_layers: Vec<Layer>,
    /// Obstacles (existing copper, keepouts).
    pub obstacles: Vec<Obstacle>,
    /// Drilled holes.
    pub holes: Vec<Hole>,
    /// Terminals, referenced by [`TerminalId`] (index in this list).
    pub terminals: Vec<Terminal>,
    /// The connections to route.
    pub connections: Vec<Connection>,
    /// Design rules and settings.
    pub rules: RoutingRules,
}

impl RoutingProblem {
    /// Creates a problem without obstacles, terminals and connections.
    pub fn new(outline: Vec<Vec<Point>>, copper_layers: Vec<Layer>, rules: RoutingRules) -> Self {
        Self {
            outline,
            copper_layers,
            rules,
            ..Self::default()
        }
    }

    /// Adds a terminal and returns its identifier.
    pub fn add_terminal(&mut self, terminal: Terminal) -> TerminalId {
        self.terminals.push(terminal);
        TerminalId(self.terminals.len() - 1)
    }

    /// Adds an obstacle.
    pub fn add_obstacle(&mut self, obstacle: Obstacle) {
        self.obstacles.push(obstacle);
    }

    /// Adds a hole.
    pub fn add_hole(&mut self, hole: Hole) {
        self.holes.push(hole);
    }

    /// Adds a connection between two terminals.
    pub fn connect(&mut self, a: TerminalId, b: TerminalId) {
        self.connections.push(Connection::Terminals(a, b));
    }

    /// Adds a connection of all terminals of a net.
    pub fn connect_net(&mut self, net: NetId) {
        self.connections.push(Connection::Net(net));
    }
}
