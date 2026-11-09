//! Built-in autorouter for simple boards (2–4 copper layers, a few dozen
//! nets).
//!
//! This is original code without upstream counterpart (upstream LibrePCB
//! only exports Specctra DSN for external autorouters). It is a pure
//! algorithm on an explicit [`RoutingProblem`]: it does not know the project
//! model. The caller extracts the problem from a board (outline, obstacles,
//! terminals from pads and air wires, design rules) and applies the
//! [`RoutingResult`] as traces and vias.
//!
//! # Algorithm
//!
//! 1. **Setup.** All copper, keepouts, holes and board edges become items
//!    with exact geometry (point/segment/polygon cores plus distances, see
//!    `items`) in an R*-tree ([`rstar`]). Nets given as whole
//!    ([`Connection::Net`]) are split into air wires with the upstream
//!    minimum spanning tree algorithm
//!    ([`AirWiresBuilder`](librepcb_core::algorithm::AirWiresBuilder)).
//! 2. **Grid.** A uniform grid (default pitch 0.1 mm, coarser for large
//!    boards) per routing layer. For each trace width a map marks the nodes
//!    a trace center line must not use: items inflated by clearance + half
//!    width (plus a margin that makes moves between free nodes safe), with
//!    the net that may still use them. A via map does the same for via
//!    centers (clearance + via radius, drill clearances, board edge).
//! 3. **Search.** Air wires are routed shortest first with A* over
//!    `(node, layer, direction)` states: 8 directions (45° routing), bend
//!    costs, no acute angles, preferred layer directions, and vias as layer
//!    changes with a configurable cost. A search runs from all copper already
//!    connected to one terminal (its pads, and earlier routes including
//!    their vias, so T-junctions happen naturally) to the copper connected to
//!    the other one.
//! 4. **Rip-up and reroute.** If no path exists, the search is repeated
//!    with nodes used by other routes passable at a high penalty; the routes
//!    in the way are ripped up (with routes attached to them) and queued
//!    again. Each wire can be ripped up only
//!    [`max_rip_ups`](RoutingRules::max_rip_ups) times, which guarantees
//!    termination. Failed wires get a final retry at the end.
//! 5. **Simplification.** Collinear steps are merged, then points are
//!    removed greedily where a direct 45°/90° segment is free (checked
//!    exactly).
//! 6. **Verification.** Every segment and via is checked against the exact
//!    geometry of all other nets' copper, holes and the board edge before it
//!    is committed, and all routes are checked again at the end. The grid is
//!    only a search structure; no clearance violation is ever produced from
//!    rasterization errors.
//!
//! The router is deterministic: the same problem gives the same result.
//!
//! # Limitations
//!
//! - Through vias only (no blind/buried vias), one via size.
//! - Grid-based: gaps narrower than the grid pitch may be missed, and
//!   off-grid terminals are reached with a short stub from their position.
//! - No via-in-pad; no length matching, differential pairs or plane
//!   connections; nets are routed independently of each other's
//!   criticality.

mod error;
mod geometry;
mod grid;
mod items;
mod problem;
mod result;
mod router;
mod search;

use std::ops::ControlFlow;

pub use error::{Error, Result};
pub use problem::{
    Connection, Hole, LayerScope, NetId, Obstacle, PreferredDirection, RoutingProblem,
    RoutingRules, Shape, Terminal, TerminalId,
};
pub use result::{
    Endpoint, Progress, Route, RoutingResult, RoutingStats, Segment, Unrouted, UnroutedReason, Via,
};

static_assertions::assert_impl_all!(RoutingProblem: Send, Sync);
static_assertions::assert_impl_all!(RoutingResult: Send, Sync);

/// Routes all connections of the problem.
///
/// Returns an error only for invalid problems; connections which cannot be
/// routed are listed in [`RoutingResult::unrouted`].
pub fn route(problem: &RoutingProblem) -> Result<RoutingResult> {
    route_with_progress(problem, |_| ControlFlow::Continue(()))
}

/// Like [`route()`], reporting progress before each path search. Returning
/// [`ControlFlow::Break`] cancels routing; the routes found so far are
/// returned and the remaining connections are reported as
/// [`UnroutedReason::Cancelled`].
pub fn route_with_progress(
    problem: &RoutingProblem,
    mut progress: impl FnMut(Progress) -> ControlFlow<()>,
) -> Result<RoutingResult> {
    Ok(router::Router::new(problem)?.run(&mut progress))
}
