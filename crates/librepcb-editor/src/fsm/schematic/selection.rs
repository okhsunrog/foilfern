//! Port of libs/librepcb/editor/project/schematic/schematicselectionquery.{h,cpp}:
//! the selected model objects of a schematic page, resolved from the FSM's
//! selection set (only items which still exist).

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::geometry::NetLineAnchor;
use librepcb_core::project::schematic::Schematic;
use librepcb_core::project::{BusSegmentId, NetSegmentId, SymbolId};
use librepcb_core::types::Uuid;

use super::SchematicItem;

/// The selected items of a page by kind (upstream
/// `SchematicSelectionQuery`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SelectionQuery {
    pub symbols: BTreeSet<SymbolId>,
    pub pins: BTreeSet<(SymbolId, Uuid)>,
    pub net_points: BTreeSet<(NetSegmentId, Uuid)>,
    pub net_lines: BTreeSet<(NetSegmentId, Uuid)>,
    pub net_labels: BTreeSet<(NetSegmentId, Uuid)>,
    pub bus_junctions: BTreeSet<(BusSegmentId, Uuid)>,
    pub bus_lines: BTreeSet<(BusSegmentId, Uuid)>,
    pub bus_labels: BTreeSet<(BusSegmentId, Uuid)>,
    pub polygons: BTreeSet<Uuid>,
    pub texts: BTreeSet<Uuid>,
    /// Texts of symbols selected on their own (upstream
    /// `addSelectedSymbolTexts()`).
    pub symbol_texts: BTreeSet<(SymbolId, Uuid)>,
    pub images: BTreeSet<Uuid>,
}

/// The selected junctions, lines and labels of one net segment (upstream
/// `SchematicSelectionQuery::NetSegmentItems`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NetSegmentItems {
    pub net_points: BTreeSet<Uuid>,
    pub net_lines: BTreeSet<Uuid>,
    pub net_labels: BTreeSet<Uuid>,
}

impl SelectionQuery {
    /// Collects the selected items which exist on the page (upstream
    /// `addSelected*()`).
    pub fn new(selection: &BTreeSet<SchematicItem>, s: &Schematic) -> Self {
        let mut q = Self::default();
        for item in selection {
            if !item_exists(item, s) {
                continue;
            }
            match *item {
                SchematicItem::Symbol(id) => {
                    q.symbols.insert(id);
                }
                SchematicItem::SymbolPin(id, pin) => {
                    q.pins.insert((id, pin));
                }
                SchematicItem::SymbolText(id, text) => {
                    q.symbol_texts.insert((id, text));
                }
                SchematicItem::NetPoint(seg, j) => {
                    q.net_points.insert((seg, j));
                }
                SchematicItem::NetLine(seg, l) => {
                    q.net_lines.insert((seg, l));
                }
                SchematicItem::NetLabel(seg, l) => {
                    q.net_labels.insert((seg, l));
                }
                SchematicItem::BusJunction(seg, j) => {
                    q.bus_junctions.insert((seg, j));
                }
                SchematicItem::BusLine(seg, l) => {
                    q.bus_lines.insert((seg, l));
                }
                SchematicItem::BusLabel(seg, l) => {
                    q.bus_labels.insert((seg, l));
                }
                SchematicItem::Polygon(id) => {
                    q.polygons.insert(id);
                }
                SchematicItem::Text(id) => {
                    q.texts.insert(id);
                }
                SchematicItem::Image(id) => {
                    q.images.insert(id);
                }
            }
        }
        q
    }

    /// Adds the junctions of the selected lines (upstream
    /// `addNetPointsOfNetLines()`); with `only_if_all_lines_selected`, only
    /// junctions whose lines are all selected.
    pub fn add_net_points_of_net_lines(&mut self, s: &Schematic, only_if_all_lines_selected: bool) {
        let mut add = BTreeSet::new();
        for (seg_id, line) in &self.net_lines {
            let Some(seg) = s.net_segments().get(seg_id) else {
                continue;
            };
            let Some(line) = seg.lines().get(line) else {
                continue;
            };
            for anchor in [line.p1(), line.p2()] {
                if let NetLineAnchor::Junction(j) = anchor {
                    let all = seg
                        .lines_at(anchor)
                        .all(|l| self.net_lines.contains(&(*seg_id, l.uuid())));
                    if !only_if_all_lines_selected || all {
                        add.insert((*seg_id, j));
                    }
                }
            }
        }
        self.net_points.extend(add);
    }

    /// Adds the junctions of the selected bus lines (upstream
    /// `addJunctionsOfBusLines()`).
    pub fn add_junctions_of_bus_lines(&mut self, s: &Schematic, only_if_all_lines_selected: bool) {
        let mut add = BTreeSet::new();
        for (seg_id, line) in &self.bus_lines {
            let Some(seg) = s.bus_segments().get(seg_id) else {
                continue;
            };
            let Some(line) = seg.lines().get(line) else {
                continue;
            };
            for anchor in [line.p1(), line.p2()] {
                if let NetLineAnchor::Junction(j) = anchor {
                    let all = seg
                        .lines_at(j)
                        .all(|l| self.bus_lines.contains(&(*seg_id, l.uuid())));
                    if !only_if_all_lines_selected || all {
                        add.insert((*seg_id, j));
                    }
                }
            }
        }
        self.bus_junctions.extend(add);
    }

    /// The selected junctions, lines and labels per net segment (upstream
    /// `getNetSegmentItems()`).
    pub fn net_segment_items(&self) -> BTreeMap<NetSegmentId, NetSegmentItems> {
        let mut result: BTreeMap<NetSegmentId, NetSegmentItems> = BTreeMap::new();
        for (seg, j) in &self.net_points {
            result.entry(*seg).or_default().net_points.insert(*j);
        }
        for (seg, l) in &self.net_lines {
            result.entry(*seg).or_default().net_lines.insert(*l);
        }
        for (seg, l) in &self.net_labels {
            result.entry(*seg).or_default().net_labels.insert(*l);
        }
        result
    }

    /// Whether items are selected which can be removed, rotated, mirrored,
    /// copied (upstream: the condition of the cut/copy/remove features).
    pub fn has_modifiable_items(&self) -> bool {
        !self.symbols.is_empty()
            || !self.net_lines.is_empty()
            || !self.net_labels.is_empty()
            || !self.bus_lines.is_empty()
            || !self.bus_labels.is_empty()
            || !self.polygons.is_empty()
            || !self.texts.is_empty()
            || !self.symbol_texts.is_empty()
            || !self.images.is_empty()
    }

    /// Number of selected items (upstream `getResultCount()`).
    pub fn count(&self) -> usize {
        self.symbols.len()
            + self.pins.len()
            + self.net_points.len()
            + self.net_lines.len()
            + self.net_labels.len()
            + self.bus_junctions.len()
            + self.bus_lines.len()
            + self.bus_labels.len()
            + self.polygons.len()
            + self.texts.len()
            + self.symbol_texts.len()
            + self.images.len()
    }
}

/// Returns whether an item still exists on the page.
pub(crate) fn item_exists(item: &SchematicItem, s: &Schematic) -> bool {
    let seg = |id: &NetSegmentId| s.net_segments().get(id);
    let bus = |id: &BusSegmentId| s.bus_segments().get(id);
    match item {
        SchematicItem::Symbol(id) => s.symbols().contains_key(id),
        SchematicItem::SymbolPin(id, _) => s.symbols().contains_key(id),
        SchematicItem::SymbolText(id, t) => s
            .symbols()
            .get(id)
            .is_some_and(|x| x.texts().contains_key(t)),
        SchematicItem::NetPoint(id, j) => seg(id).is_some_and(|x| x.junctions().contains_key(j)),
        SchematicItem::NetLine(id, l) => seg(id).is_some_and(|x| x.lines().contains_key(l)),
        SchematicItem::NetLabel(id, l) => seg(id).is_some_and(|x| x.labels().contains_key(l)),
        SchematicItem::BusJunction(id, j) => bus(id).is_some_and(|x| x.junctions().contains_key(j)),
        SchematicItem::BusLine(id, l) => bus(id).is_some_and(|x| x.lines().contains_key(l)),
        SchematicItem::BusLabel(id, l) => bus(id).is_some_and(|x| x.labels().contains_key(l)),
        SchematicItem::Polygon(id) => s.polygons().contains_key(id),
        SchematicItem::Text(id) => s.texts().contains_key(id),
        SchematicItem::Image(id) => s.images().contains_key(id),
    }
}

/// All items of a page which can be selected (upstream `selectAll()`);
/// texts of symbols are moved with their symbols and therefore not listed.
pub(crate) fn all_items(s: &Schematic) -> BTreeSet<SchematicItem> {
    let mut items = BTreeSet::new();
    for id in s.symbols().keys() {
        items.insert(SchematicItem::Symbol(*id));
    }
    for (id, seg) in s.net_segments() {
        for j in seg.junctions().keys() {
            items.insert(SchematicItem::NetPoint(*id, *j));
        }
        for l in seg.lines().keys() {
            items.insert(SchematicItem::NetLine(*id, *l));
        }
        for l in seg.labels().keys() {
            items.insert(SchematicItem::NetLabel(*id, *l));
        }
    }
    for (id, seg) in s.bus_segments() {
        for j in seg.junctions().keys() {
            items.insert(SchematicItem::BusJunction(*id, *j));
        }
        for l in seg.lines().keys() {
            items.insert(SchematicItem::BusLine(*id, *l));
        }
        for l in seg.labels().keys() {
            items.insert(SchematicItem::BusLabel(*id, *l));
        }
    }
    for id in s.polygons().keys() {
        items.insert(SchematicItem::Polygon(*id));
    }
    for id in s.texts().keys() {
        items.insert(SchematicItem::Text(*id));
    }
    for id in s.images().keys() {
        items.insert(SchematicItem::Image(*id));
    }
    items
}
