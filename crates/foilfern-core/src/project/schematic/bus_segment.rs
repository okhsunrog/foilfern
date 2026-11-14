//! Port of libs/librepcb/core/project/schematic/items/si_bussegment.{h,cpp}
//! (with `si_busjunction`, `si_busline` and `si_buslabel`, which become the
//! geometry types [`Junction`], [`NetLine`] and [`NetLabel`] stored in the
//! segment, like upstream stores them internally).
//!
//! Differences to upstream: bus lines reference their junctions by
//! [`NetLineAnchor::Junction`] of this segment instead of pointers; the net
//! lines attached to bus junctions are found through the schematic's anchor
//! index. The adding/removing of elements with its validation lives in the
//! project mutations; the Qt signals are the change journal.

use std::collections::BTreeMap;

use super::net_segment::{are_connected, nearest_point, serialize_elements};
use super::uuid_map;
use crate::geometry::{Junction, NetLabel, NetLine, NetLineAnchor};
use crate::project::id::{BusId, BusSegmentId};
use crate::serialization::{HasUuid, List, SerializeObject};
use crate::types::{Length, Point, UnsignedLength, Uuid};

/// A bus segment of a schematic: connected junctions and lines of one bus,
/// plus its labels. Net lines of net segments may end at its junctions.
///
/// Elements are keyed by UUID (upstream `QMap`, file order).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SchematicBusSegment {
    uuid: Uuid,
    bus: BusId,
    #[serde(deserialize_with = "uuid_map::deserialize")]
    junctions: BTreeMap<Uuid, Junction>,
    #[serde(deserialize_with = "uuid_map::deserialize")]
    lines: BTreeMap<Uuid, NetLine>,
    #[serde(deserialize_with = "uuid_map::deserialize")]
    labels: BTreeMap<Uuid, NetLabel>,
}

impl SchematicBusSegment {
    /// Default width of bus lines (upstream `SI_BusLine::getDefaultWidth()`).
    pub const DEFAULT_LINE_WIDTH: Length = Length::new(400_000);

    /// Creates an empty segment of `bus`.
    pub fn new(uuid: Uuid, bus: BusId) -> Self {
        Self {
            uuid,
            bus,
            junctions: BTreeMap::new(),
            lines: BTreeMap::new(),
            labels: BTreeMap::new(),
        }
    }

    /// Returns the default width of bus lines.
    pub fn default_line_width() -> UnsignedLength {
        UnsignedLength::new(Self::DEFAULT_LINE_WIDTH).expect("constant is not negative")
    }

    /// Returns the UUID.
    pub fn uuid(&self) -> Uuid {
        self.uuid
    }

    /// Returns the typed identifier.
    pub fn id(&self) -> BusSegmentId {
        BusSegmentId(self.uuid)
    }

    /// Returns the bus.
    pub fn bus(&self) -> BusId {
        self.bus
    }

    pub(crate) fn set_bus(&mut self, bus: BusId) {
        self.bus = bus;
    }

    /// Returns the junctions.
    pub fn junctions(&self) -> &BTreeMap<Uuid, Junction> {
        &self.junctions
    }

    /// Returns the bus lines (anchors are always
    /// [`NetLineAnchor::Junction`]s of this segment).
    pub fn lines(&self) -> &BTreeMap<Uuid, NetLine> {
        &self.lines
    }

    /// Returns the labels.
    pub fn labels(&self) -> &BTreeMap<Uuid, NetLabel> {
        &self.labels
    }

    /// Adds or replaces a junction (for building a segment before adding
    /// it to a schematic), returns the previous one with the same UUID.
    pub fn insert_junction(&mut self, junction: Junction) -> Option<Junction> {
        self.junctions.insert(junction.uuid(), junction)
    }

    /// Adds or replaces a bus line, see [`insert_junction()`](Self::insert_junction).
    pub fn insert_line(&mut self, line: NetLine) -> Option<NetLine> {
        self.lines.insert(line.uuid(), line)
    }

    /// Adds or replaces a label, see [`insert_junction()`](Self::insert_junction).
    pub fn insert_label(&mut self, label: NetLabel) -> Option<NetLabel> {
        self.labels.insert(label.uuid(), label)
    }

    pub(crate) fn junctions_mut(&mut self) -> &mut BTreeMap<Uuid, Junction> {
        &mut self.junctions
    }

    pub(crate) fn lines_mut(&mut self) -> &mut BTreeMap<Uuid, NetLine> {
        &mut self.lines
    }

    pub(crate) fn labels_mut(&mut self) -> &mut BTreeMap<Uuid, NetLabel> {
        &mut self.labels
    }

    /// Whether the segment contains any junction, line or label (upstream
    /// `isUsed()`).
    pub fn is_used(&self) -> bool {
        !(self.junctions.is_empty() && self.lines.is_empty() && self.labels.is_empty())
    }

    /// Returns the bus lines connected to the junction `junction`.
    pub fn lines_at(&self, junction: Uuid) -> impl Iterator<Item = &NetLine> {
        let anchor = NetLineAnchor::Junction(junction);
        self.lines
            .values()
            .filter(move |l| l.p1() == anchor || l.p2() == anchor)
    }

    /// Whether a junction dot is shown at the junction: more than two bus
    /// lines (upstream `SI_BusJunction::isVisibleJunction()`).
    pub fn is_visible_junction(&self, junction: Uuid) -> bool {
        self.lines_at(junction).count() > 2
    }

    /// Whether all junctions and lines are connected together (upstream
    /// `areAllJunctionsConnectedTogether()`); an empty segment is cohesive.
    pub fn is_cohesive(&self) -> bool {
        are_connected(self.junctions.keys().copied(), self.lines.values())
    }

    /// Returns the position of a junction of this segment.
    pub fn junction_position(&self, anchor: NetLineAnchor) -> Option<Point> {
        match anchor {
            NetLineAnchor::Junction(j) => self.junctions.get(&j).map(Junction::position),
            _ => None,
        }
    }

    /// Returns the point on the bus lines nearest to `p` (upstream
    /// `calcNearestPoint()`), or `p` itself if there are no lines. This is
    /// the anchor of bus labels.
    pub fn nearest_point(&self, p: Point) -> Point {
        nearest_point(self.lines.values(), p, |a| self.junction_position(a))
    }
}

impl HasUuid for SchematicBusSegment {
    fn uuid(&self) -> Uuid {
        self.uuid
    }
}

impl SerializeObject for SchematicBusSegment {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.ensure_line_break();
        root.append_child("bus", &self.bus);
        root.ensure_line_break();
        serialize_elements(root, &self.junctions, &self.lines, &self.labels);
    }
}
