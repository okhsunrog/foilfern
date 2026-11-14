//! Port of libs/librepcb/core/project/schematic/items/si_netsegment.{h,cpp}
//! (with `si_netpoint`, `si_netline` and `si_netlabel`, which become the
//! geometry types [`Junction`], [`NetLine`] and [`NetLabel`] stored in the
//! segment).
//!
//! Differences to upstream: net lines reference their anchors by
//! [`NetLineAnchor`] (junction of this segment, bus junction, symbol pin)
//! instead of pointers; anchor positions are resolved by the schematic
//! ([`Schematic::net_line_anchor_position()`](super::Schematic::net_line_anchor_position)).
//! The registration of lines at their anchors (`registerNetLine()`) is
//! the schematic's anchor index (pins, bus junctions) or a scan of the
//! segment's lines ([`lines_at()`](SchematicNetSegment::lines_at)). The
//! adding/removing of elements with its validation lives in the project
//! mutations; the Qt signals are the change journal. Not ported:
//! `getForcedNetNames()` (needs attribute substitution).

use std::collections::{BTreeMap, BTreeSet};

use petgraph::unionfind::UnionFind;

use super::uuid_map;
use crate::geometry::{Junction, NetLabel, NetLine, NetLineAnchor};
use crate::project::id::{BusSegmentId, NetSegmentId, NetSignalId, SymbolId};
use crate::serialization::{HasUuid, List, SerializeObject};
use crate::types::{Length, Point, UnsignedLength, Uuid};
use crate::utils::toolbox;

/// A net segment of a schematic: connected junctions and net lines of one
/// net, plus its net labels.
///
/// Elements are keyed by UUID (upstream `QMap`, file order).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SchematicNetSegment {
    uuid: Uuid,
    net: NetSignalId,
    #[serde(deserialize_with = "uuid_map::deserialize")]
    junctions: BTreeMap<Uuid, Junction>,
    #[serde(deserialize_with = "uuid_map::deserialize")]
    lines: BTreeMap<Uuid, NetLine>,
    #[serde(deserialize_with = "uuid_map::deserialize")]
    labels: BTreeMap<Uuid, NetLabel>,
}

impl SchematicNetSegment {
    /// Default width of net lines (upstream `SI_NetLine::getDefaultWidth()`).
    pub const DEFAULT_LINE_WIDTH: Length = Length::new(158_750);

    /// Creates an empty segment of `net`.
    pub fn new(uuid: Uuid, net: NetSignalId) -> Self {
        Self {
            uuid,
            net,
            junctions: BTreeMap::new(),
            lines: BTreeMap::new(),
            labels: BTreeMap::new(),
        }
    }

    /// Returns the default width of net lines.
    pub fn default_line_width() -> UnsignedLength {
        UnsignedLength::new(Self::DEFAULT_LINE_WIDTH).expect("constant is not negative")
    }

    /// Returns the UUID.
    pub fn uuid(&self) -> Uuid {
        self.uuid
    }

    /// Returns the typed identifier.
    pub fn id(&self) -> NetSegmentId {
        NetSegmentId(self.uuid)
    }

    /// Returns the net signal.
    pub fn net(&self) -> NetSignalId {
        self.net
    }

    pub(crate) fn set_net(&mut self, net: NetSignalId) {
        self.net = net;
    }

    /// Returns the junctions (upstream net points).
    pub fn junctions(&self) -> &BTreeMap<Uuid, Junction> {
        &self.junctions
    }

    /// Returns the net lines.
    pub fn lines(&self) -> &BTreeMap<Uuid, NetLine> {
        &self.lines
    }

    /// Returns the net labels.
    pub fn labels(&self) -> &BTreeMap<Uuid, NetLabel> {
        &self.labels
    }

    /// Adds or replaces a junction (for building a segment before adding
    /// it to a schematic), returns the previous one with the same UUID.
    pub fn insert_junction(&mut self, junction: Junction) -> Option<Junction> {
        self.junctions.insert(junction.uuid(), junction)
    }

    /// Adds or replaces a net line, see [`insert_junction()`](Self::insert_junction).
    pub fn insert_line(&mut self, line: NetLine) -> Option<NetLine> {
        self.lines.insert(line.uuid(), line)
    }

    /// Adds or replaces a net label, see [`insert_junction()`](Self::insert_junction).
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

    /// Returns the lines connected to `anchor` (a junction of this segment,
    /// a bus junction or a pin); upstream `getNetLines()` of the anchor.
    pub fn lines_at(&self, anchor: NetLineAnchor) -> impl Iterator<Item = &NetLine> {
        self.lines
            .values()
            .filter(move |l| l.p1() == anchor || l.p2() == anchor)
    }

    /// Whether a junction dot is shown at `anchor`: more than two lines at a
    /// junction of this segment, more than one line at a pin (upstream
    /// `SI_NetPoint`/`SI_SymbolPin::isVisibleJunction()`). Bus junctions
    /// are drawn by their bus segment
    /// ([`SchematicBusSegment::is_visible_junction()`](super::SchematicBusSegment::is_visible_junction)).
    pub fn is_visible_junction(&self, anchor: NetLineAnchor) -> bool {
        let count = self.lines_at(anchor).count();
        match anchor {
            NetLineAnchor::Junction(_) => count > 2,
            NetLineAnchor::Pin { .. } => count > 1,
            NetLineAnchor::BusJunction { .. } => false,
        }
    }

    /// Returns all symbol pins connected by lines (upstream
    /// `getAllConnectedPins()`).
    pub fn connected_pins(&self) -> BTreeSet<(SymbolId, Uuid)> {
        self.anchors()
            .filter_map(|a| match a {
                NetLineAnchor::Pin { symbol, pin } => Some((SymbolId(symbol), pin)),
                _ => None,
            })
            .collect()
    }

    /// Returns all bus junctions connected by lines (upstream
    /// `getAllConnectedBusJunctions()`).
    pub fn connected_bus_junctions(&self) -> BTreeSet<(BusSegmentId, Uuid)> {
        self.anchors()
            .filter_map(|a| match a {
                NetLineAnchor::BusJunction { segment, junction } => {
                    Some((BusSegmentId(segment), junction))
                }
                _ => None,
            })
            .collect()
    }

    /// Returns all bus segments connected by lines (upstream
    /// `getAllConnectedBusSegments()`).
    pub fn connected_bus_segments(&self) -> BTreeSet<BusSegmentId> {
        self.connected_bus_junctions()
            .into_iter()
            .map(|(s, _)| s)
            .collect()
    }

    /// Whether all junctions and lines are connected together (upstream
    /// `areAllNetPointsConnectedTogether()`); an empty segment is cohesive.
    pub fn is_cohesive(&self) -> bool {
        are_connected(self.junctions.keys().copied(), self.lines.values())
    }

    /// Returns the point on the lines nearest to `p` (upstream
    /// `calcNearestPoint()`), or `p` itself if the segment has no lines.
    /// `position` resolves the anchor positions; lines with unresolvable
    /// anchors are ignored.
    pub fn nearest_point(
        &self,
        p: Point,
        position: impl Fn(NetLineAnchor) -> Option<Point>,
    ) -> Point {
        nearest_point(self.lines.values(), p, position)
    }

    /// All line endpoints (with duplicates).
    fn anchors(&self) -> impl Iterator<Item = NetLineAnchor> + '_ {
        self.lines.values().flat_map(|l| [l.p1(), l.p2()])
    }
}

impl HasUuid for SchematicNetSegment {
    fn uuid(&self) -> Uuid {
        self.uuid
    }
}

impl SerializeObject for SchematicNetSegment {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.ensure_line_break();
        root.append_child("net", &self.net);
        root.ensure_line_break();
        serialize_elements(root, &self.junctions, &self.lines, &self.labels);
    }
}

/// Writes junctions, lines and labels like upstream `SI_NetSegment` and
/// `SI_BusSegment::serialize()`.
pub(super) fn serialize_elements(
    root: &mut List,
    junctions: &BTreeMap<Uuid, Junction>,
    lines: &BTreeMap<Uuid, NetLine>,
    labels: &BTreeMap<Uuid, NetLabel>,
) {
    for obj in junctions.values() {
        root.ensure_line_break();
        obj.serialize(root.append_list("junction"));
    }
    root.ensure_line_break();
    for obj in lines.values() {
        root.ensure_line_break();
        obj.serialize(root.append_list("line"));
    }
    root.ensure_line_break();
    for obj in labels.values() {
        root.ensure_line_break();
        obj.serialize(root.append_list("label"));
    }
    root.ensure_line_break();
}

/// Whether the given junctions and all lines form one connected graph
/// (anchors are nodes, lines are edges); nothing at all is connected too.
///
/// Uses a union-find over the anchors instead of upstream's recursive
/// traversal from the first element; the result is the same (every
/// junction and every line reachable from any element).
pub(super) fn are_connected<'a>(
    junctions: impl IntoIterator<Item = Uuid>,
    lines: impl IntoIterator<Item = &'a NetLine>,
) -> bool {
    fn index(a: NetLineAnchor, nodes: &mut BTreeMap<NetLineAnchor, usize>) -> usize {
        let next = nodes.len();
        *nodes.entry(a).or_insert(next)
    }
    let mut nodes: BTreeMap<NetLineAnchor, usize> = BTreeMap::new();
    let junctions: Vec<usize> = junctions
        .into_iter()
        .map(|j| index(NetLineAnchor::Junction(j), &mut nodes))
        .collect();
    let edges: Vec<(usize, usize)> = lines
        .into_iter()
        .map(|l| (index(l.p1(), &mut nodes), index(l.p2(), &mut nodes)))
        .collect();
    let mut sets = UnionFind::<usize>::new(nodes.len());
    for (a, b) in &edges {
        sets.union(*a, *b);
    }
    let mut roots = junctions
        .iter()
        .copied()
        .chain(edges.iter().map(|(a, _)| *a))
        .map(|n| sets.find(n));
    match roots.next() {
        Some(first) => roots.all(|r| r == first),
        None => true,
    }
}

/// Upstream `calcNearestPoint()` of net and bus segments: the first line
/// (in UUID order) with the smallest distance wins.
pub(super) fn nearest_point<'a>(
    lines: impl IntoIterator<Item = &'a NetLine>,
    p: Point,
    position: impl Fn(NetLineAnchor) -> Option<Point>,
) -> Point {
    let mut best: Option<(UnsignedLength, Point)> = None;
    for line in lines {
        let (Some(p1), Some(p2)) = (position(line.p1()), position(line.p2())) else {
            continue;
        };
        let (distance, nearest) = toolbox::shortest_distance_between_point_and_line(p, p1, p2);
        if best.is_none_or(|(d, _)| distance < d) {
            best = Some((distance, nearest));
        }
    }
    best.map_or(p, |(_, nearest)| nearest)
}
