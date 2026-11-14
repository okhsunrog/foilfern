//! Port of libs/librepcb/core/project/board/items/bi_airwire.{h,cpp}.
//!
//! Air wires are derived data (see
//! [`BoardDerived`](super::BoardDerived)), computed per net by the
//! [`BoardAirWiresBuilder`](super::BoardAirWiresBuilder) and never saved.
//! Unlike upstream, an air wire stores the positions of its anchors (at the
//! time it was built) instead of pointers to the anchor items.

use crate::geometry::TraceAnchor;
use crate::project::id::NetSignalId;
use crate::types::Point;

/// An air wire (unrouted connection) of a board.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AirWire {
    net: NetSignalId,
    p1: TraceAnchor,
    p2: TraceAnchor,
    p1_position: Point,
    p2_position: Point,
}

impl AirWire {
    /// Creates an air wire between two anchors at the given positions.
    pub fn new(net: NetSignalId, p1: (TraceAnchor, Point), p2: (TraceAnchor, Point)) -> Self {
        Self {
            net,
            p1: p1.0,
            p2: p2.0,
            p1_position: p1.1,
            p2_position: p2.1,
        }
    }

    /// Returns the net signal.
    pub fn net(&self) -> NetSignalId {
        self.net
    }

    /// Returns the first anchor.
    pub fn p1(&self) -> TraceAnchor {
        self.p1
    }

    /// Returns the second anchor.
    pub fn p2(&self) -> TraceAnchor {
        self.p2
    }

    /// Returns the position of the first anchor.
    pub fn p1_position(&self) -> Point {
        self.p1_position
    }

    /// Returns the position of the second anchor.
    pub fn p2_position(&self) -> Point {
        self.p2_position
    }

    /// Whether both anchors are at the same position (a connection between
    /// layers, upstream `isVertical()`).
    pub fn is_vertical(&self) -> bool {
        self.p1_position == self.p2_position
    }
}
