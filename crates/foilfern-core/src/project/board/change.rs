//! Item level change events of a board (upstream: the `Board` signals
//! `deviceAdded()`, `netSegmentAdded()`, `planeAdded()`, `zoneAdded()`,
//! `airWireAdded()`, ..., the `BI_NetSegment::elementsAdded()` /
//! `elementsRemoved()` signals and the items' `onEdited` signals).

use std::fmt;

use crate::project::error::EntityKind;
use crate::project::id::{ComponentInstanceId, NetSegmentId, NetSignalId, PlaneId};
use crate::types::Uuid;

/// Kind of a board item which is not addressed by a typed identifier, for
/// change events and error messages.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum BoardItemKind {
    /// A standalone pad of a net segment.
    Pad,
    /// A via of a net segment.
    Via,
    /// A junction of a net segment (upstream net point).
    Junction,
    /// A trace of a net segment (upstream net line).
    Trace,
    /// A keepout zone.
    Zone,
    /// A polygon.
    Polygon,
    /// A stroke text (of the board or of a device).
    StrokeText,
    /// A non-plated hole.
    Hole,
}

impl fmt::Display for BoardItemKind {
    /// The (untranslated) noun used in upstream messages.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Pad => "pad",
            Self::Via => "via",
            Self::Junction => "netpoint",
            Self::Trace => "netline",
            Self::Zone => "zone",
            Self::Polygon => "polygon",
            Self::StrokeText => "stroke text",
            Self::Hole => "hole",
        })
    }
}

impl From<BoardItemKind> for EntityKind {
    /// The entity kind used in error messages.
    fn from(kind: BoardItemKind) -> Self {
        match kind {
            BoardItemKind::Pad => Self::Pad,
            BoardItemKind::Via => Self::Via,
            BoardItemKind::Junction => Self::NetPoint,
            BoardItemKind::Trace => Self::Trace,
            BoardItemKind::Zone => Self::Zone,
            BoardItemKind::Polygon => Self::BoardPolygon,
            BoardItemKind::StrokeText => Self::StrokeText,
            BoardItemKind::Hole => Self::Hole,
        }
    }
}

/// UUIDs of net segment elements, per kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BoardSegmentItems {
    /// Standalone pads.
    pub pads: Vec<Uuid>,
    /// Vias.
    pub vias: Vec<Uuid>,
    /// Junctions.
    pub junctions: Vec<Uuid>,
    /// Traces.
    pub traces: Vec<Uuid>,
}

impl BoardSegmentItems {
    /// Whether no element is listed.
    pub fn is_empty(&self) -> bool {
        self.pads.is_empty()
            && self.vias.is_empty()
            && self.junctions.is_empty()
            && self.traces.is_empty()
    }
}

/// A change of an item of a board, see
/// [`Change::Board`](crate::project::Change::Board).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum BoardChange {
    /// The board settings (layers, design rules, DRC settings, ...)
    /// changed (upstream `designRulesModified()`,
    /// `innerLayerCountChanged()`, `preferredFootprintTagsChanged()`).
    Settings,
    /// The DRC message approvals changed.
    DrcApprovals,
    /// The layer visibility (user settings) changed.
    LayersVisibility,
    /// A device was added (upstream `deviceAdded()`).
    DeviceAdded(ComponentInstanceId),
    /// A device was removed (upstream `deviceRemoved()`).
    DeviceRemoved(ComponentInstanceId),
    /// Placement, library elements, attributes or stroke texts of a device
    /// changed (upstream `BI_Device::onEdited`, `attributesChanged()`).
    DeviceChanged(ComponentInstanceId),
    /// A net segment was added (upstream `netSegmentAdded()`).
    NetSegmentAdded(NetSegmentId),
    /// A net segment was removed (upstream `netSegmentRemoved()`).
    NetSegmentRemoved(NetSegmentId),
    /// The net of a net segment changed.
    NetSegmentNetChanged {
        /// The segment.
        segment: NetSegmentId,
        /// The previous net.
        from: Option<NetSignalId>,
        /// The new net.
        to: Option<NetSignalId>,
    },
    /// Elements were added to a net segment (upstream `elementsAdded()`).
    NetSegmentElementsAdded {
        /// The segment.
        segment: NetSegmentId,
        /// The added elements.
        items: BoardSegmentItems,
    },
    /// Elements were removed from a net segment (upstream
    /// `elementsRemoved()`).
    NetSegmentElementsRemoved {
        /// The segment.
        segment: NetSegmentId,
        /// The removed elements.
        items: BoardSegmentItems,
    },
    /// Elements of a net segment were modified (upstream `onEdited` of
    /// pads, vias, net points and net lines).
    NetSegmentElementsChanged {
        /// The segment.
        segment: NetSegmentId,
        /// The modified elements.
        items: BoardSegmentItems,
    },
    /// A plane was added (upstream `planeAdded()`).
    PlaneAdded(PlaneId),
    /// A plane was removed (upstream `planeRemoved()`).
    PlaneRemoved(PlaneId),
    /// A property of a plane changed (upstream `BI_Plane::onEdited`).
    PlaneChanged(PlaneId),
    /// A zone, polygon, stroke text or hole was added.
    ItemAdded {
        /// Kind of the item.
        kind: BoardItemKind,
        /// UUID of the item.
        uuid: Uuid,
    },
    /// A zone, polygon, stroke text or hole was removed.
    ItemRemoved {
        /// Kind of the item.
        kind: BoardItemKind,
        /// UUID of the item.
        uuid: Uuid,
    },
    /// A zone, polygon, stroke text or hole was modified.
    ItemChanged {
        /// Kind of the item.
        kind: BoardItemKind,
        /// UUID of the item.
        uuid: Uuid,
    },
    /// The air wires of some nets were rebuilt (upstream
    /// `airWireAdded()`/`airWireRemoved()`).
    AirWires(Vec<Option<NetSignalId>>),
    /// The calculated fragments of planes changed (not undoable, upstream
    /// `BI_Plane::onEdited(FragmentsChanged)`).
    PlaneFragments(Vec<PlaneId>),
}
