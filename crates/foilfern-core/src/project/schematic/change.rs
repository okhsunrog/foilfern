//! Item level change events of a schematic (upstream: the `Schematic`
//! signals `symbolAdded()`, `netSegmentAdded()`, ..., the segment signals
//! `netPointsAndNetLinesAdded()`, `netLabelAdded()`, ... and the items'
//! `onEdited` signals).
//!
//! Derived geometry is not journaled separately: when a symbol moves, the
//! net lines at its pins and the anchors of the labels of those segments
//! move too (upstream `SI_NetLine::updatePositions()`,
//! `updateAllNetLabelAnchors()`); consumers find them through
//! [`Schematic::pin_net_segment()`](super::Schematic::pin_net_segment).
//! Likewise, moving a junction or changing the elements of a segment moves
//! the anchors of its labels.

use crate::project::id::{BusSegmentId, NetSegmentId, SymbolId};
use crate::types::Uuid;

/// A change of an item of a schematic, see
/// [`Change::Schematic`](crate::project::Change::Schematic).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum SchematicChange {
    /// A symbol was added.
    SymbolAdded(SymbolId),
    /// A symbol was removed.
    SymbolRemoved(SymbolId),
    /// Position, rotation or mirror state of a symbol changed (upstream
    /// `SI_Symbol::onEdited(PositionChanged, ...)`).
    SymbolPlacementChanged(SymbolId),
    /// A text of a symbol was added, removed or modified (upstream
    /// `SI_Symbol::textAdded()`/`textRemoved()`, `SI_Text::onEdited`).
    SymbolTextsChanged(SymbolId),

    /// A net segment was added.
    NetSegmentAdded(NetSegmentId),
    /// A net segment was removed.
    NetSegmentRemoved(NetSegmentId),
    /// The net of an (empty) net segment changed.
    NetSegmentNetChanged(NetSegmentId),
    /// Junctions and lines were added to a net segment.
    NetSegmentElementsAdded {
        /// The segment.
        segment: NetSegmentId,
        /// The added junctions.
        junctions: Vec<Uuid>,
        /// The added lines.
        lines: Vec<Uuid>,
    },
    /// Junctions and lines were removed from a net segment.
    NetSegmentElementsRemoved {
        /// The segment.
        segment: NetSegmentId,
        /// The removed junctions.
        junctions: Vec<Uuid>,
        /// The removed lines.
        lines: Vec<Uuid>,
    },
    /// A junction of a net segment moved (the lines at it move too).
    NetPointChanged {
        /// The segment.
        segment: NetSegmentId,
        /// The junction.
        junction: Uuid,
    },
    /// The width of a net line changed.
    NetLineChanged {
        /// The segment.
        segment: NetSegmentId,
        /// The line.
        line: Uuid,
    },
    /// A net label was added.
    NetLabelAdded {
        /// The segment.
        segment: NetSegmentId,
        /// The label.
        label: Uuid,
    },
    /// A net label was removed.
    NetLabelRemoved {
        /// The segment.
        segment: NetSegmentId,
        /// The label.
        label: Uuid,
    },
    /// Position, rotation or mirror state of a net label changed.
    NetLabelChanged {
        /// The segment.
        segment: NetSegmentId,
        /// The label.
        label: Uuid,
    },

    /// A bus segment was added.
    BusSegmentAdded(BusSegmentId),
    /// A bus segment was removed.
    BusSegmentRemoved(BusSegmentId),
    /// The bus of an (empty) bus segment changed.
    BusSegmentBusChanged(BusSegmentId),
    /// Junctions and lines were added to a bus segment.
    BusSegmentElementsAdded {
        /// The segment.
        segment: BusSegmentId,
        /// The added junctions.
        junctions: Vec<Uuid>,
        /// The added lines.
        lines: Vec<Uuid>,
    },
    /// Junctions and lines were removed from a bus segment.
    BusSegmentElementsRemoved {
        /// The segment.
        segment: BusSegmentId,
        /// The removed junctions.
        junctions: Vec<Uuid>,
        /// The removed lines.
        lines: Vec<Uuid>,
    },
    /// A bus junction moved (the bus and net lines at it move too).
    BusJunctionChanged {
        /// The segment.
        segment: BusSegmentId,
        /// The junction.
        junction: Uuid,
    },
    /// The width of a bus line changed.
    BusLineChanged {
        /// The segment.
        segment: BusSegmentId,
        /// The line.
        line: Uuid,
    },
    /// A bus label was added.
    BusLabelAdded {
        /// The segment.
        segment: BusSegmentId,
        /// The label.
        label: Uuid,
    },
    /// A bus label was removed.
    BusLabelRemoved {
        /// The segment.
        segment: BusSegmentId,
        /// The label.
        label: Uuid,
    },
    /// Position, rotation or mirror state of a bus label changed.
    BusLabelChanged {
        /// The segment.
        segment: BusSegmentId,
        /// The label.
        label: Uuid,
    },

    /// A polygon was added.
    PolygonAdded(Uuid),
    /// A polygon was removed.
    PolygonRemoved(Uuid),
    /// A polygon was modified.
    PolygonChanged(Uuid),
    /// A text was added.
    TextAdded(Uuid),
    /// A text was removed.
    TextRemoved(Uuid),
    /// A text was modified.
    TextChanged(Uuid),
    /// An image was added.
    ImageAdded(Uuid),
    /// An image was removed.
    ImageRemoved(Uuid),
    /// An image was modified.
    ImageChanged(Uuid),
}
