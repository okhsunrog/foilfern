//! The selection of the board editor and queries on it (port of
//! libs/librepcb/editor/project/board/boardselectionquery.{h,cpp}; upstream
//! keeps the selection in the graphics items).
//!
//! Selecting a device implicitly selects its stroke texts (upstream
//! `BGI_StrokeText` follows the selection of its device).

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::geometry::TraceAnchor;
use librepcb_core::project::board::Board;
use librepcb_core::project::{ComponentInstanceId, NetSegmentId, PlaneId, Project};
use librepcb_core::types::Uuid;

use super::view::BoardItemRef;

/// The selected items of a board.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoardSelection {
    items: BTreeSet<BoardItemRef>,
    revision: u64,
}

impl BoardSelection {
    /// The selected items.
    pub fn items(&self) -> &BTreeSet<BoardItemRef> {
        &self.items
    }

    /// Whether nothing is selected.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// A number which changes whenever the selection changes.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether an item is selected (device texts also if their device is).
    pub fn contains(&self, item: BoardItemRef) -> bool {
        self.items.contains(&item)
            || matches!(item, BoardItemRef::DeviceStrokeText(c, _)
                if self.items.contains(&BoardItemRef::Device(c)))
    }

    /// Selects or deselects an item.
    pub fn set(&mut self, item: BoardItemRef, selected: bool) {
        let changed = if selected {
            self.items.insert(item)
        } else {
            let mut changed = self.items.remove(&item);
            if let BoardItemRef::Device(c) = item {
                let before = self.items.len();
                self.items
                    .retain(|i| !matches!(i, BoardItemRef::DeviceStrokeText(d, _) if *d == c));
                changed |= before != self.items.len();
            }
            changed
        };
        if changed {
            self.revision += 1;
        }
    }

    /// Clears the selection.
    pub fn clear(&mut self) {
        if !self.items.is_empty() {
            self.items.clear();
            self.revision += 1;
        }
    }

    /// Replaces the selection.
    pub fn replace(&mut self, items: impl IntoIterator<Item = BoardItemRef>) {
        let items: BTreeSet<BoardItemRef> = items.into_iter().collect();
        if items != self.items {
            self.items = items;
            self.revision += 1;
        }
    }

    /// Updates the net segments of the selected elements and removes items
    /// which no longer exist.
    pub fn update(&mut self, board: &Board) {
        let items: BTreeSet<BoardItemRef> = self
            .items
            .iter()
            .filter_map(|i| i.resolved(board))
            .collect();
        if items != self.items {
            self.items = items;
            self.revision += 1;
        }
    }
}

/// Selects all items of a board (upstream `BoardGraphicsScene::selectAll()`).
pub fn all_items(board: &Board) -> Vec<BoardItemRef> {
    let mut items = Vec::new();
    for (c, dev) in board.devices() {
        items.push(BoardItemRef::Device(*c));
        items.extend(
            dev.stroke_texts()
                .keys()
                .map(|u| BoardItemRef::DeviceStrokeText(*c, *u)),
        );
    }
    for seg in board.net_segments().values() {
        items.extend(segment_items(seg.id(), seg, true));
    }
    items.extend(board.planes().keys().map(|p| BoardItemRef::Plane(*p)));
    items.extend(board.zones().keys().map(|u| BoardItemRef::Zone(*u)));
    items.extend(board.polygons().keys().map(|u| BoardItemRef::Polygon(*u)));
    items.extend(
        board
            .stroke_texts()
            .keys()
            .map(|u| BoardItemRef::StrokeText(*u)),
    );
    items.extend(board.holes().keys().map(|u| BoardItemRef::Hole(*u)));
    items
}

/// The vias, junctions, traces (and pads if `pads`) of a net segment
/// (upstream `BoardGraphicsScene::selectNetSegment()` without pads).
pub fn segment_items(
    id: NetSegmentId,
    seg: &librepcb_core::project::board::BoardNetSegment,
    pads: bool,
) -> Vec<BoardItemRef> {
    let mut items = Vec::new();
    if pads {
        items.extend(seg.pads().keys().map(|u| BoardItemRef::Pad(id, *u)));
    }
    items.extend(seg.vias().keys().map(|u| BoardItemRef::Via(id, *u)));
    items.extend(
        seg.junctions()
            .keys()
            .map(|u| BoardItemRef::Junction(id, *u)),
    );
    items.extend(seg.traces().keys().map(|u| BoardItemRef::Trace(id, *u)));
    items
}

/// Net segment elements of a query result.
#[derive(Debug, Clone, Default)]
pub struct SegmentItems {
    /// Standalone pads.
    pub pads: BTreeSet<Uuid>,
    /// Vias.
    pub vias: BTreeSet<Uuid>,
    /// Junctions.
    pub junctions: BTreeSet<Uuid>,
    /// Traces.
    pub traces: BTreeSet<Uuid>,
}

/// A query on the selection (port of upstream `BoardSelectionQuery`):
/// collects the selected items of the requested kinds, optionally without
/// locked ones.
#[derive(Debug, Clone)]
pub struct SelectionQuery<'a> {
    project: &'a Project,
    board: &'a Board,
    selection: &'a BoardSelection,
    include_locked: bool,
    /// Devices.
    pub devices: BTreeSet<ComponentInstanceId>,
    /// Footprint pads.
    pub footprint_pads: BTreeSet<(ComponentInstanceId, Uuid)>,
    /// Standalone pads.
    pub pads: BTreeSet<(NetSegmentId, Uuid)>,
    /// Vias.
    pub vias: BTreeSet<(NetSegmentId, Uuid)>,
    /// Junctions.
    pub junctions: BTreeSet<(NetSegmentId, Uuid)>,
    /// Traces.
    pub traces: BTreeSet<(NetSegmentId, Uuid)>,
    /// Planes.
    pub planes: BTreeSet<PlaneId>,
    /// Zones.
    pub zones: BTreeSet<Uuid>,
    /// Polygons.
    pub polygons: BTreeSet<Uuid>,
    /// Board stroke texts.
    pub stroke_texts: BTreeSet<Uuid>,
    /// Device stroke texts.
    pub device_texts: BTreeSet<(ComponentInstanceId, Uuid)>,
    /// Holes.
    pub holes: BTreeSet<Uuid>,
}

impl<'a> SelectionQuery<'a> {
    /// Creates an empty query.
    pub fn new(
        project: &'a Project,
        board: &'a Board,
        selection: &'a BoardSelection,
        include_locked: bool,
    ) -> Self {
        Self {
            project,
            board,
            selection,
            include_locked,
            devices: BTreeSet::new(),
            footprint_pads: BTreeSet::new(),
            pads: BTreeSet::new(),
            vias: BTreeSet::new(),
            junctions: BTreeSet::new(),
            traces: BTreeSet::new(),
            planes: BTreeSet::new(),
            zones: BTreeSet::new(),
            polygons: BTreeSet::new(),
            stroke_texts: BTreeSet::new(),
            device_texts: BTreeSet::new(),
            holes: BTreeSet::new(),
        }
    }

    fn selected(&self) -> impl Iterator<Item = BoardItemRef> + '_ {
        self.selection
            .items()
            .iter()
            .filter_map(|i| i.resolved(self.board))
    }

    fn unlocked(&self, locked: bool) -> bool {
        !locked || self.include_locked
    }

    /// Number of collected items.
    pub fn count(&self) -> usize {
        self.devices.len()
            + self.footprint_pads.len()
            + self.pads.len()
            + self.vias.len()
            + self.junctions.len()
            + self.traces.len()
            + self.planes.len()
            + self.zones.len()
            + self.polygons.len()
            + self.stroke_texts.len()
            + self.device_texts.len()
            + self.holes.len()
    }

    /// Whether nothing was collected.
    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    /// Upstream `addDeviceInstancesOfSelectedFootprints()`.
    pub fn add_devices(&mut self) -> &mut Self {
        let found: Vec<_> = self
            .selected()
            .filter_map(|i| match i {
                BoardItemRef::Device(c) => self
                    .board
                    .device(c)
                    .filter(|d| self.unlocked(d.locked()))
                    .map(|_| c),
                _ => None,
            })
            .collect();
        self.devices.extend(found);
        self
    }

    /// Upstream `addDeviceInstancesAndTextsOfSelectedPads()`: adds the
    /// devices of selected footprint pads and their texts, returns these
    /// devices.
    pub fn add_devices_of_selected_pads(&mut self) -> BTreeSet<ComponentInstanceId> {
        let devices: BTreeSet<ComponentInstanceId> = self
            .selected()
            .filter_map(|i| match i {
                BoardItemRef::FootprintPad(c, _) => self
                    .board
                    .device(c)
                    .filter(|d| self.unlocked(d.locked()))
                    .map(|_| c),
                _ => None,
            })
            .collect();
        self.devices.extend(devices.iter().copied());
        for c in &devices {
            if let Some(d) = self.board.device(*c) {
                for (u, t) in d.stroke_texts() {
                    if self.unlocked(t.locked()) {
                        self.device_texts.insert((*c, *u));
                    }
                }
            }
        }
        devices
    }

    /// Upstream `addSelectedFootprintPads()`.
    pub fn add_footprint_pads(&mut self) -> &mut Self {
        let found: Vec<_> = self
            .selected()
            .filter_map(|i| match i {
                BoardItemRef::FootprintPad(c, u) => Some((c, u)),
                _ => None,
            })
            .collect();
        self.footprint_pads.extend(found);
        self
    }

    /// Upstream `addSelectedBoardPads()`.
    pub fn add_board_pads(&mut self) -> &mut Self {
        let found: Vec<_> = self
            .selected()
            .filter_map(|i| match i {
                BoardItemRef::Pad(s, u) => {
                    let pad = self.board.net_segment(s)?.pads().get(&u)?;
                    self.unlocked(pad.locked()).then_some((s, u))
                }
                _ => None,
            })
            .collect();
        self.pads.extend(found);
        self
    }

    /// Upstream `addSelectedVias()`.
    pub fn add_vias(&mut self) -> &mut Self {
        let found: Vec<_> = self
            .selected()
            .filter_map(|i| match i {
                BoardItemRef::Via(s, u) => Some((s, u)),
                _ => None,
            })
            .collect();
        self.vias.extend(found);
        self
    }

    /// Upstream `addSelectedNetPoints()`.
    pub fn add_junctions(&mut self) -> &mut Self {
        let found: Vec<_> = self
            .selected()
            .filter_map(|i| match i {
                BoardItemRef::Junction(s, u) => Some((s, u)),
                _ => None,
            })
            .collect();
        self.junctions.extend(found);
        self
    }

    /// Upstream `addSelectedNetLines()`.
    pub fn add_traces(&mut self) -> &mut Self {
        let found: Vec<_> = self
            .selected()
            .filter_map(|i| match i {
                BoardItemRef::Trace(s, u) => Some((s, u)),
                _ => None,
            })
            .collect();
        self.traces.extend(found);
        self
    }

    /// Upstream `addNetPointsOfNetLines()`.
    pub fn add_junctions_of_traces(&mut self, only_if_all_traces_selected: bool) -> &mut Self {
        let mut found = Vec::new();
        for (s, u) in &self.traces {
            let Some(seg) = self.board.net_segment(*s) else {
                continue;
            };
            let Some(t) = seg.traces().get(u) else {
                continue;
            };
            for a in [t.p1(), t.p2()] {
                if let TraceAnchor::Junction(j) = a {
                    let all = seg
                        .traces_at(a)
                        .all(|other| self.traces.contains(&(*s, other.uuid())));
                    if !only_if_all_traces_selected || all {
                        found.push((*s, j));
                    }
                }
            }
        }
        self.junctions.extend(found);
        self
    }

    /// Upstream `addSelectedPlanes()`.
    pub fn add_planes(&mut self) -> &mut Self {
        let found: Vec<_> = self
            .selected()
            .filter_map(|i| match i {
                BoardItemRef::Plane(p) => self
                    .board
                    .plane(p)
                    .filter(|x| self.unlocked(x.locked()))
                    .map(|_| p),
                _ => None,
            })
            .collect();
        self.planes.extend(found);
        self
    }

    /// Upstream `addSelectedZones()`.
    pub fn add_zones(&mut self) -> &mut Self {
        let found: Vec<_> = self
            .selected()
            .filter_map(|i| match i {
                BoardItemRef::Zone(u) => self
                    .board
                    .zones()
                    .get(&u)
                    .filter(|x| self.unlocked(x.locked()))
                    .map(|_| u),
                _ => None,
            })
            .collect();
        self.zones.extend(found);
        self
    }

    /// Upstream `addSelectedPolygons()`.
    pub fn add_polygons(&mut self) -> &mut Self {
        let found: Vec<_> = self
            .selected()
            .filter_map(|i| match i {
                BoardItemRef::Polygon(u) => self
                    .board
                    .polygons()
                    .get(&u)
                    .filter(|x| self.unlocked(x.locked()))
                    .map(|_| u),
                _ => None,
            })
            .collect();
        self.polygons.extend(found);
        self
    }

    /// Upstream `addSelectedBoardStrokeTexts()`.
    pub fn add_board_texts(&mut self) -> &mut Self {
        let found: Vec<_> = self
            .selected()
            .filter_map(|i| match i {
                BoardItemRef::StrokeText(u) => self
                    .board
                    .stroke_texts()
                    .get(&u)
                    .filter(|x| self.unlocked(x.locked()))
                    .map(|_| u),
                _ => None,
            })
            .collect();
        self.stroke_texts.extend(found);
        self
    }

    /// Upstream `addSelectedFootprintStrokeTexts()` (texts of selected
    /// devices are selected too).
    pub fn add_device_texts(&mut self) -> &mut Self {
        let mut found = Vec::new();
        for (c, d) in self.board.devices() {
            for (u, t) in d.stroke_texts() {
                if self
                    .selection
                    .contains(BoardItemRef::DeviceStrokeText(*c, *u))
                    && self.unlocked(t.locked())
                {
                    found.push((*c, *u));
                }
            }
        }
        self.device_texts.extend(found);
        self
    }

    /// Upstream `addSelectedHoles()`.
    pub fn add_holes(&mut self) -> &mut Self {
        let found: Vec<_> = self
            .selected()
            .filter_map(|i| match i {
                BoardItemRef::Hole(u) => self
                    .board
                    .holes()
                    .get(&u)
                    .filter(|x| self.unlocked(x.locked()))
                    .map(|_| u),
                _ => None,
            })
            .collect();
        self.holes.extend(found);
        self
    }

    /// Adds all kinds used by the drag and remove commands (without
    /// footprint pads).
    pub fn add_all(&mut self) -> &mut Self {
        self.add_devices()
            .add_board_pads()
            .add_vias()
            .add_junctions()
            .add_traces()
            .add_planes()
            .add_zones()
            .add_polygons()
            .add_board_texts()
            .add_device_texts()
            .add_holes()
    }

    /// Net segment elements grouped by segment (upstream
    /// `getNetSegmentItems()`).
    pub fn segment_items(&self) -> BTreeMap<NetSegmentId, SegmentItems> {
        let mut result: BTreeMap<NetSegmentId, SegmentItems> = BTreeMap::new();
        for (s, u) in &self.pads {
            result.entry(*s).or_default().pads.insert(*u);
        }
        for (s, u) in &self.vias {
            result.entry(*s).or_default().vias.insert(*u);
        }
        for (s, u) in &self.junctions {
            result.entry(*s).or_default().junctions.insert(*u);
        }
        for (s, u) in &self.traces {
            result.entry(*s).or_default().traces.insert(*u);
        }
        result
    }

    /// The project.
    pub fn project(&self) -> &'a Project {
        self.project
    }

    /// The board.
    pub fn board(&self) -> &'a Board {
        self.board
    }
}
