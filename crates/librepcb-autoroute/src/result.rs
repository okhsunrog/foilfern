//! The routing result: new traces and vias per routed connection, and the
//! connections which could not be routed.

use librepcb_core::types::{Layer, Point, PositiveLength};

use crate::problem::{NetId, TerminalId};

/// A straight trace segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    /// Copper layer.
    pub layer: Layer,
    /// Start point.
    pub start: Point,
    /// End point.
    pub end: Point,
    /// Trace width.
    pub width: PositiveLength,
}

/// A through via (spanning all copper layers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Via {
    /// Center.
    pub position: Point,
    /// Drill diameter.
    pub drill: PositiveLength,
    /// Outer diameter.
    pub diameter: PositiveLength,
}

/// Where a route starts or ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    /// At the position of a terminal.
    Terminal(TerminalId),
    /// On a via of another route of this result (at its position).
    Via(Point),
    /// On a trace of another route of this result (T-junction). The segment
    /// of the other route is split at this point, so it is always a segment
    /// endpoint.
    Junction {
        /// Layer of the joined trace.
        layer: Layer,
        /// The junction point.
        position: Point,
    },
}

/// A routed connection.
///
/// Segments and vias of all routes of a net form a graph whose nodes are
/// identified by their coordinates (and layer, except for vias): segment
/// endpoints at a terminal position attach to the terminal, endpoints at a
/// via position attach to the via, all other shared endpoints are junctions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// Index of the request in [`RoutingProblem::connections`](crate::RoutingProblem::connections).
    pub request: usize,
    /// The net.
    pub net: NetId,
    /// The terminals of the air wire this route connects. The route may
    /// actually start or end on copper already connected to them (see
    /// [`start`](Self::start) and [`end`](Self::end)).
    pub terminals: (TerminalId, TerminalId),
    /// Where the route starts.
    pub start: Endpoint,
    /// Where the route ends.
    pub end: Endpoint,
    /// The new trace segments.
    pub segments: Vec<Segment>,
    /// The new vias.
    pub vias: Vec<Via>,
}

/// Why a connection was not routed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UnroutedReason {
    /// A terminal has no free access point on an allowed layer.
    NoAccess,
    /// No path was found (blocked, or no room left).
    NoPath,
    /// Routing was cancelled before the connection was routed.
    Cancelled,
}

/// A connection which could not be routed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unrouted {
    /// Index of the request in [`RoutingProblem::connections`](crate::RoutingProblem::connections).
    pub request: usize,
    /// The net.
    pub net: NetId,
    /// The terminals of the air wire.
    pub terminals: (TerminalId, TerminalId),
    /// Why it could not be routed.
    pub reason: UnroutedReason,
}

/// Statistics of a routing run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RoutingStats {
    /// Grid pitch used (nanometers).
    pub grid_pitch: i64,
    /// Number of air wires (after splitting nets).
    pub air_wires: usize,
    /// Number of path searches.
    pub searches: usize,
    /// Number of routes ripped up for rerouting.
    pub rip_ups: usize,
}

/// Result of [`route()`](crate::route).
///
/// Air wires which got connected by other routes (e.g. a route of the same
/// net passing a T-junction) appear in neither list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoutingResult {
    /// The routed connections, in routing order.
    pub routes: Vec<Route>,
    /// The connections which could not be routed.
    pub unrouted: Vec<Unrouted>,
    /// Statistics.
    pub stats: RoutingStats,
}

impl RoutingResult {
    /// Returns whether all connections were routed.
    pub fn is_complete(&self) -> bool {
        self.unrouted.is_empty()
    }

    /// Iterates over all new segments.
    pub fn segments(&self) -> impl Iterator<Item = &Segment> {
        self.routes.iter().flat_map(|r| &r.segments)
    }

    /// Iterates over all new vias.
    pub fn vias(&self) -> impl Iterator<Item = &Via> {
        self.routes.iter().flat_map(|r| &r.vias)
    }
}

/// Progress report passed to the callback of
/// [`route_with_progress()`](crate::route_with_progress).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Air wires currently connected.
    pub connected: usize,
    /// Total number of air wires.
    pub total: usize,
    /// Path searches done so far.
    pub searches: usize,
}
