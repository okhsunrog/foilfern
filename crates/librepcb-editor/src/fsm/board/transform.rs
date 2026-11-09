//! Moving, rotating, flipping and modifying the selected board items: port
//! of libs/librepcb/editor/project/cmd/cmddragselectedboarditems.{h,cpp}
//! and cmdflipselectedboarditems.{h,cpp} (with the transformations of the
//! `CmdBoard*Edit`, `CmdDeviceInstanceEdit` and `CmdDeviceStrokeTextsReset`
//! commands they use).
//!
//! [`DragItems`] holds the original and the current state of every
//! affected item, like the upstream edit commands hold their new values;
//! [`DragItems::mutations()`] returns the model mutations which bring the
//! board from the original to the current state. The FSM applies them as a
//! live preview (rolled back and re-applied on every change) inside one
//! undo group.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::geometry::{ComponentSide, Junction, NonEmptyPath, Trace, Via};
use librepcb_core::project::board::{
    BoardDevice, BoardHoleData, BoardItem, BoardNetSegment, BoardPadData, BoardPlane,
    BoardPolygonData, BoardSegmentElements, BoardStrokeTextData, BoardZoneData,
};
use librepcb_core::project::{
    BoardId, BoardMutation, BoardNetSegmentRef, ComponentInstanceId, Mutation, NetSegmentId,
    PlaneId, Project,
};
use librepcb_core::types::{
    Angle, Layer, Orientation, Point, PositiveLength, UnsignedLength, Uuid,
};

use super::selection::SelectionQuery;
use crate::error::{Error, Result};

/// A device of a drag operation: the device itself is transformed if
/// `moved`, and its selected texts are transformed in `current`.
#[derive(Debug, Clone)]
struct DeviceState {
    original: BoardDevice,
    current: BoardDevice,
    moved: bool,
    texts: BTreeSet<Uuid>,
}

/// The items of one net segment of a drag operation.
#[derive(Debug, Clone, Default)]
struct SegmentState {
    pads: BTreeMap<Uuid, (BoardPadData, BoardPadData)>,
    vias: BTreeMap<Uuid, (Via, Via)>,
    junctions: BTreeMap<Uuid, (Junction, Junction)>,
    traces: BTreeMap<Uuid, (Trace, Trace)>,
}

/// The selected items being dragged or modified (port of upstream
/// `CmdDragSelectedBoardItems`).
#[derive(Debug, Clone)]
pub struct DragItems {
    board: BoardId,
    inner_layers: usize,
    grid: PositiveLength,
    devices: BTreeMap<ComponentInstanceId, DeviceState>,
    segments: BTreeMap<NetSegmentId, SegmentState>,
    planes: BTreeMap<PlaneId, (BoardPlane, BoardPlane)>,
    zones: BTreeMap<Uuid, (BoardZoneData, BoardZoneData)>,
    polygons: BTreeMap<Uuid, (BoardPolygonData, BoardPolygonData)>,
    texts: BTreeMap<Uuid, (BoardStrokeTextData, BoardStrokeTextData)>,
    holes: BTreeMap<Uuid, (BoardHoleData, BoardHoleData)>,
    item_count: i64,
    start_pos: Point,
    delta: Point,
    center: Point,
    delta_angle: Angle,
    snapped: bool,
    locked_changed: bool,
    width_changed: bool,
    texts_reset: bool,
    auto_selected_devices: BTreeSet<ComponentInstanceId>,
    has_traces: bool,
    has_polygons: bool,
    has_texts: bool,
}

impl DragItems {
    /// Collects the selected items like the upstream constructor
    /// (`include_traces`: also transform selected traces, e.g. for changing
    /// their width; their junctions are always moved).
    pub fn new(query: &mut SelectionQuery<'_>, include_traces: bool, start_pos: Point) -> Self {
        query
            .add_devices()
            .add_board_pads()
            .add_vias()
            .add_junctions()
            .add_traces()
            .add_junctions_of_traces(false)
            .add_planes()
            .add_zones()
            .add_polygons()
            .add_board_texts()
            .add_device_texts()
            .add_holes();
        // Individual footprint pads cannot be dragged: drag their devices.
        let auto_selected = query.add_devices_of_selected_pads();
        let _ = include_traces; // upstream adds the selected traces in any case
        let board = query.board();
        let mut items = Self {
            board: board.id(),
            inner_layers: board.settings().inner_layer_count as usize,
            grid: board.settings().grid_interval,
            devices: BTreeMap::new(),
            segments: BTreeMap::new(),
            planes: BTreeMap::new(),
            zones: BTreeMap::new(),
            polygons: BTreeMap::new(),
            texts: BTreeMap::new(),
            holes: BTreeMap::new(),
            item_count: 0,
            start_pos,
            delta: Point::ORIGIN,
            center: Point::ORIGIN,
            delta_angle: Angle::DEG0,
            snapped: false,
            locked_changed: false,
            width_changed: false,
            texts_reset: false,
            auto_selected_devices: auto_selected,
            has_traces: !query.traces.is_empty(),
            has_polygons: !query.polygons.is_empty(),
            has_texts: !query.stroke_texts.is_empty() || !query.device_texts.is_empty(),
        };
        let mut center = Point::ORIGIN;
        for c in &query.devices {
            if let Some(d) = board.device(*c) {
                center += d.position();
                items.item_count += 1;
                items.devices.insert(
                    *c,
                    DeviceState {
                        original: d.clone(),
                        current: d.clone(),
                        moved: true,
                        texts: BTreeSet::new(),
                    },
                );
            }
        }
        for (s, u) in &query.pads {
            if let Some(p) = board.net_segment(*s).and_then(|seg| seg.pads().get(u)) {
                center += p.pad().position();
                items.item_count += 1;
                items
                    .segments
                    .entry(*s)
                    .or_default()
                    .pads
                    .insert(*u, (p.clone(), p.clone()));
            }
        }
        for (s, u) in &query.vias {
            if let Some(v) = board.net_segment(*s).and_then(|seg| seg.vias().get(u)) {
                center += v.position();
                items.item_count += 1;
                items
                    .segments
                    .entry(*s)
                    .or_default()
                    .vias
                    .insert(*u, (v.clone(), v.clone()));
            }
        }
        for (s, u) in &query.junctions {
            if let Some(j) = board.net_segment(*s).and_then(|seg| seg.junctions().get(u)) {
                center += j.position();
                items.item_count += 1;
                items
                    .segments
                    .entry(*s)
                    .or_default()
                    .junctions
                    .insert(*u, (j.clone(), j.clone()));
            }
        }
        let p = query.project();
        for (s, u) in &query.traces {
            let Some(seg) = board.net_segment(*s) else {
                continue;
            };
            if let Some(t) = seg.traces().get(u) {
                for a in [t.p1(), t.p2()] {
                    if let Some(pos) = board.anchor_position(seg, a, p.library(), p.circuit()) {
                        center += pos;
                    }
                }
                items.item_count += 2;
                items
                    .segments
                    .entry(*s)
                    .or_default()
                    .traces
                    .insert(*u, (t.clone(), t.clone()));
            }
        }
        for id in &query.planes {
            if let Some(plane) = board.plane(*id) {
                for v in plane.outline().vertices() {
                    center += v.pos;
                    items.item_count += 1;
                }
                items.planes.insert(*id, (plane.clone(), plane.clone()));
            }
        }
        for u in &query.zones {
            if let Some(z) = board.zones().get(u) {
                for v in z.outline().vertices() {
                    center += v.pos;
                    items.item_count += 1;
                }
                items.zones.insert(*u, (z.clone(), z.clone()));
            }
        }
        for u in &query.polygons {
            if let Some(poly) = board.polygons().get(u) {
                for v in poly.path().vertices() {
                    center += v.pos;
                    items.item_count += 1;
                }
                items.polygons.insert(*u, (poly.clone(), poly.clone()));
            }
        }
        for u in &query.stroke_texts {
            if let Some(t) = board.stroke_texts().get(u) {
                center += t.position();
                items.item_count += 1;
                items.texts.insert(*u, (t.clone(), t.clone()));
            }
        }
        for (c, u) in &query.device_texts {
            let Some(d) = board.device(*c) else {
                continue;
            };
            let Some(t) = d.stroke_texts().get(u) else {
                continue;
            };
            // Do not count texts of devices if the device is selected too.
            if !query.devices.contains(c) {
                center += t.position();
                items.item_count += 1;
            }
            items
                .devices
                .entry(*c)
                .or_insert_with(|| DeviceState {
                    original: d.clone(),
                    current: d.clone(),
                    moved: false,
                    texts: BTreeSet::new(),
                })
                .texts
                .insert(*u);
        }
        for u in &query.holes {
            if let Some(h) = board.holes().get(u) {
                center += h.path().first().pos;
                items.item_count += 1;
                items.holes.insert(*u, (h.clone(), h.clone()));
            }
        }
        // If only one item is selected, use its exact position as center.
        if items.item_count > 1 {
            center /= items.item_count;
            center = center.mapped_to_grid(items.grid);
        }
        items.center = center;
        items
    }

    /// Whether any item is affected.
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
            && self.segments.is_empty()
            && self.planes.is_empty()
            && self.zones.is_empty()
            && self.polygons.is_empty()
            && self.texts.is_empty()
            && self.holes.is_empty()
    }

    /// Devices which were added because one of their pads is selected
    /// (upstream `selectDevicesOfPads()`).
    pub fn auto_selected_devices(&self) -> &BTreeSet<ComponentInstanceId> {
        &self.auto_selected_devices
    }

    /// Whether traces are affected.
    pub fn has_traces(&self) -> bool {
        self.has_traces
    }

    /// Whether polygons are affected.
    pub fn has_polygons(&self) -> bool {
        self.has_polygons
    }

    /// Whether stroke texts are affected.
    pub fn has_texts(&self) -> bool {
        self.has_texts
    }

    /// Whether anything was changed (upstream: the command is discarded
    /// otherwise).
    pub fn has_changes(&self) -> bool {
        !self.delta.is_origin()
            || self.delta_angle != Angle::DEG0
            || self.snapped
            || self.texts_reset
            || self.locked_changed
            || self.width_changed
    }

    /// The median line width of the affected traces, polygons and texts
    /// (upstream `getMedianLineWidth()`).
    pub fn median_line_width(&self) -> UnsignedLength {
        let mut values: Vec<UnsignedLength> = Vec::new();
        for seg in self.segments.values() {
            values.extend(
                seg.traces
                    .values()
                    .map(|(t, _)| UnsignedLength::from(t.width())),
            );
        }
        values.extend(self.polygons.values().map(|(p, _)| p.line_width()));
        values.extend(self.texts.values().map(|(t, _)| t.stroke_width()));
        for d in self.devices.values() {
            values.extend(
                d.texts
                    .iter()
                    .filter_map(|u| d.original.stroke_texts().get(u))
                    .map(|t| t.stroke_width()),
            );
        }
        values.sort();
        values
            .get(values.len() / 2)
            .copied()
            .unwrap_or(UnsignedLength::ZERO)
    }

    fn for_each_device_text(d: &mut DeviceState, mut f: impl FnMut(&mut BoardStrokeTextData)) {
        for u in &d.texts {
            if let Some(mut t) = d.current.stroke_texts().get(u).cloned() {
                f(&mut t);
                d.current.insert_stroke_text(t);
            }
        }
    }

    /// Moves the items to `pos` relative to the start position (upstream
    /// `setCurrentPosition()`; the delta is mapped to the grid if
    /// `grid_increment`).
    pub fn set_current_position(&mut self, pos: Point, grid_increment: bool) {
        let mut delta = pos - self.start_pos;
        if grid_increment {
            delta = delta.mapped_to_grid(self.grid);
        }
        if delta == self.delta {
            return;
        }
        let d = delta - self.delta;
        self.translate(d);
        self.delta = delta;
    }

    fn translate(&mut self, d: Point) {
        for dev in self.devices.values_mut() {
            if dev.moved {
                dev.current.set_position(dev.current.position() + d);
            }
            Self::for_each_device_text(dev, |t| {
                t.set_position(t.position() + d);
            });
        }
        for seg in self.segments.values_mut() {
            for (_, p) in seg.pads.values_mut() {
                let pos = p.pad().position() + d;
                p.pad_mut().set_position(pos);
            }
            for (_, v) in seg.vias.values_mut() {
                v.set_position(v.position() + d);
            }
            for (_, j) in seg.junctions.values_mut() {
                j.set_position(j.position() + d);
            }
        }
        for (_, p) in self.planes.values_mut() {
            p.set_outline(p.outline().translated(d));
        }
        for (_, z) in self.zones.values_mut() {
            z.set_outline(z.outline().translated(d));
        }
        for (_, p) in self.polygons.values_mut() {
            p.set_path(p.path().translated(d));
        }
        for (_, t) in self.texts.values_mut() {
            t.set_position(t.position() + d);
        }
        for (_, h) in self.holes.values_mut() {
            if let Ok(path) = NonEmptyPath::new(h.path().get().translated(d)) {
                h.set_path(path);
            }
        }
    }

    /// Rotates the items (upstream `rotate()`): around the current cursor
    /// position (mapped to the grid) if `around_current_position` and
    /// several items are dragged, else around their center.
    pub fn rotate(&mut self, angle: Angle, around_current_position: bool) {
        let center = if around_current_position && self.item_count > 1 {
            (self.start_pos + self.delta).mapped_to_grid(self.grid)
        } else {
            self.center + self.delta
        };
        for dev in self.devices.values_mut() {
            if dev.moved {
                dev.current
                    .set_position(dev.current.position().rotated(angle, center));
                dev.current.set_rotation(dev.current.rotation() + angle);
            }
            Self::for_each_device_text(dev, |t| {
                t.set_position(t.position().rotated(angle, center));
                t.set_rotation(t.rotation() + angle);
            });
        }
        for seg in self.segments.values_mut() {
            for (_, p) in seg.pads.values_mut() {
                let pad = p.pad_mut();
                pad.set_position(pad.position().rotated(angle, center));
                pad.set_rotation(pad.rotation() + angle);
            }
            for (_, v) in seg.vias.values_mut() {
                v.set_position(v.position().rotated(angle, center));
            }
            for (_, j) in seg.junctions.values_mut() {
                j.set_position(j.position().rotated(angle, center));
            }
        }
        for (_, p) in self.planes.values_mut() {
            p.set_outline(p.outline().rotated(angle, center));
        }
        for (_, z) in self.zones.values_mut() {
            z.set_outline(z.outline().rotated(angle, center));
        }
        for (_, p) in self.polygons.values_mut() {
            p.set_path(p.path().rotated(angle, center));
        }
        for (_, t) in self.texts.values_mut() {
            t.set_position(t.position().rotated(angle, center));
            t.set_rotation(t.rotation() + angle);
        }
        for (_, h) in self.holes.values_mut() {
            if let Ok(path) = NonEmptyPath::new(h.path().get().rotated(angle, center)) {
                h.set_path(path);
            }
        }
        self.delta_angle += angle;
    }

    /// Snaps the items to the grid (upstream `snapToGrid()`).
    pub fn snap_to_grid(&mut self) {
        let grid = self.grid;
        for dev in self.devices.values_mut() {
            if dev.moved {
                dev.current
                    .set_position(dev.current.position().mapped_to_grid(grid));
            }
            Self::for_each_device_text(dev, |t| {
                t.set_position(t.position().mapped_to_grid(grid));
            });
        }
        for seg in self.segments.values_mut() {
            for (_, p) in seg.pads.values_mut() {
                let pos = p.pad().position().mapped_to_grid(grid);
                p.pad_mut().set_position(pos);
            }
            for (_, v) in seg.vias.values_mut() {
                v.set_position(v.position().mapped_to_grid(grid));
            }
            for (_, j) in seg.junctions.values_mut() {
                j.set_position(j.position().mapped_to_grid(grid));
            }
        }
        for (_, p) in self.planes.values_mut() {
            p.set_outline(p.outline().mapped_to_grid(grid));
        }
        for (_, z) in self.zones.values_mut() {
            z.set_outline(z.outline().mapped_to_grid(grid));
        }
        for (_, p) in self.polygons.values_mut() {
            p.set_path(p.path().mapped_to_grid(grid));
        }
        for (_, t) in self.texts.values_mut() {
            t.set_position(t.position().mapped_to_grid(grid));
        }
        for (_, h) in self.holes.values_mut() {
            let p0 = h.path().first().pos;
            let d = p0.mapped_to_grid(grid) - p0;
            if let Ok(path) = NonEmptyPath::new(h.path().get().translated(d)) {
                h.set_path(path);
            }
        }
        self.snapped = true;
    }

    /// Locks or unlocks the items (upstream `setLocked()`).
    pub fn set_locked(&mut self, locked: bool) {
        for dev in self.devices.values_mut() {
            if dev.moved {
                dev.current.set_locked(locked);
            }
            Self::for_each_device_text(dev, |t| {
                t.set_locked(locked);
            });
        }
        for seg in self.segments.values_mut() {
            for (_, p) in seg.pads.values_mut() {
                p.set_locked(locked);
            }
        }
        for (_, p) in self.planes.values_mut() {
            p.set_locked(locked);
        }
        for (_, z) in self.zones.values_mut() {
            z.set_locked(locked);
        }
        for (_, p) in self.polygons.values_mut() {
            p.set_locked(locked);
        }
        for (_, t) in self.texts.values_mut() {
            t.set_locked(locked);
        }
        for (_, h) in self.holes.values_mut() {
            h.set_locked(locked);
        }
        self.locked_changed = true;
    }

    /// Sets the width of traces, polygon lines and text strokes (upstream
    /// `setLineWidth()`; traces only if `width` is positive).
    pub fn set_line_width(&mut self, width: UnsignedLength) {
        if let Ok(w) = PositiveLength::new(*width) {
            for seg in self.segments.values_mut() {
                for (_, t) in seg.traces.values_mut() {
                    t.set_width(w);
                }
            }
        }
        for (_, p) in self.polygons.values_mut() {
            p.set_line_width(width);
        }
        for (_, t) in self.texts.values_mut() {
            t.set_stroke_width(width);
        }
        for dev in self.devices.values_mut() {
            Self::for_each_device_text(dev, |t| {
                t.set_stroke_width(width);
            });
        }
        self.width_changed = true;
    }

    /// Resets the texts of the devices to the footprint's defaults
    /// (upstream `resetAllTexts()` / `CmdDeviceStrokeTextsReset`).
    pub fn reset_all_texts(&mut self) {
        self.texts_reset = true;
    }

    /// The mutations bringing the board from the original to the current
    /// state (empty if nothing changed).
    pub fn mutations(&self, project: &Project) -> Result<Vec<Mutation>> {
        let board = self.board;
        let mut out = Vec::new();
        if !self.has_changes() {
            return Ok(out);
        }
        for dev in self.devices.values() {
            let mut current = dev.current.clone();
            if self.texts_reset && dev.moved {
                // upstream `CmdDeviceStrokeTextsReset` (after the move).
                let lib = project.library();
                let package = lib
                    .device(&current.lib_device())
                    .and_then(|d| lib.package(&d.package_uuid()));
                if let Some(footprint) =
                    package.and_then(|p| p.footprints().by_uuid(&current.lib_footprint()))
                {
                    let texts = current.default_stroke_texts(footprint);
                    let old: Vec<Uuid> = current.stroke_texts().keys().copied().collect();
                    for u in old {
                        current.remove_stroke_text(&u);
                    }
                    for t in &texts {
                        current.insert_stroke_text(BoardStrokeTextData::from_stroke_text(
                            t,
                            current.locked(),
                        ));
                    }
                }
            }
            if current != dev.original {
                out.push(Mutation::Board(BoardMutation::UpdateDevice {
                    board,
                    device: current,
                }));
            }
        }
        for (s, seg) in &self.segments {
            let mut elements = BoardSegmentElements::default();
            elements.pads.extend(
                seg.pads
                    .values()
                    .filter(|(a, b)| a != b)
                    .map(|(_, b)| b.clone()),
            );
            elements.vias.extend(
                seg.vias
                    .values()
                    .filter(|(a, b)| a != b)
                    .map(|(_, b)| b.clone()),
            );
            elements.junctions.extend(
                seg.junctions
                    .values()
                    .filter(|(a, b)| a != b)
                    .map(|(_, b)| b.clone()),
            );
            elements.traces.extend(
                seg.traces
                    .values()
                    .filter(|(a, b)| a != b)
                    .map(|(_, b)| b.clone()),
            );
            if !elements.is_empty() {
                out.push(Mutation::Board(BoardMutation::UpdateNetSegmentElements {
                    segment: BoardNetSegmentRef { board, segment: *s },
                    elements,
                }));
            }
        }
        for (a, b) in self.planes.values() {
            if a != b {
                out.push(Mutation::Board(BoardMutation::UpdatePlane {
                    board,
                    plane: b.clone(),
                }));
            }
        }
        let mut item = |changed: bool, item: BoardItem| {
            if changed {
                out.push(Mutation::Board(BoardMutation::UpdateItem { board, item }));
            }
        };
        for (a, b) in self.zones.values() {
            item(a != b, BoardItem::Zone(b.clone()));
        }
        for (a, b) in self.polygons.values() {
            item(a != b, BoardItem::Polygon(b.clone()));
        }
        for (a, b) in self.texts.values() {
            item(a != b, BoardItem::StrokeText(b.clone()));
        }
        for (a, b) in self.holes.values() {
            item(a != b, BoardItem::Hole(b.clone()));
        }
        let _ = self.inner_layers;
        Ok(out)
    }
}

/// Returns the mutations flipping the selected items (port of upstream
/// `CmdFlipSelectedBoardItems::performExecute()`): the affected net
/// segments are removed, the items mirrored at the center of all items and
/// the segments added again. Empty if nothing is selected.
pub fn flip_mutations(
    query: &mut SelectionQuery<'_>,
    orientation: Orientation,
) -> Result<Vec<Mutation>> {
    query
        .add_devices()
        .add_board_pads()
        .add_traces()
        .add_vias()
        .add_planes()
        .add_zones()
        .add_polygons()
        .add_board_texts()
        .add_device_texts()
        .add_holes()
        .add_junctions_of_traces(false);
    let p = query.project();
    let board = query.board();
    let board_id = board.id();
    let inner = Some(board.settings().inner_layer_count as usize);

    // Center of all elements.
    let mut center = Point::ORIGIN;
    let mut count: i64 = 0;
    let mut add = |pos: Point| {
        center += pos;
        count += 1;
    };
    for c in &query.devices {
        if let Some(d) = board.device(*c) {
            add(d.position());
        }
    }
    for (s, u) in &query.pads {
        if let Some(pad) = board.net_segment(*s).and_then(|seg| seg.pads().get(u)) {
            add(pad.pad().position());
        }
    }
    for (s, u) in &query.traces {
        if let Some(seg) = board.net_segment(*s)
            && let Some(t) = seg.traces().get(u)
        {
            for a in [t.p1(), t.p2()] {
                add(board
                    .anchor_position(seg, a, p.library(), p.circuit())
                    .unwrap_or_default());
            }
        }
    }
    for (s, u) in &query.junctions {
        if let Some(j) = board.net_segment(*s).and_then(|seg| seg.junctions().get(u)) {
            add(j.position());
        }
    }
    for (s, u) in &query.vias {
        if let Some(v) = board.net_segment(*s).and_then(|seg| seg.vias().get(u)) {
            add(v.position());
        }
    }
    let unique = |vertices: &[librepcb_core::geometry::Vertex]| -> Vec<Point> {
        let mut seen: Vec<librepcb_core::geometry::Vertex> = Vec::new();
        for v in vertices {
            if !seen.contains(v) {
                seen.push(*v);
            }
        }
        seen.into_iter().map(|v| v.pos).collect()
    };
    for id in &query.planes {
        if let Some(plane) = board.plane(*id) {
            for pos in unique(plane.outline().vertices()) {
                add(pos);
            }
        }
    }
    for u in &query.zones {
        if let Some(z) = board.zones().get(u) {
            for v in z.outline().vertices() {
                add(v.pos);
            }
        }
    }
    for u in &query.polygons {
        if let Some(poly) = board.polygons().get(u) {
            for pos in unique(poly.path().vertices()) {
                add(pos);
            }
        }
    }
    for u in &query.stroke_texts {
        if let Some(t) = board.stroke_texts().get(u) {
            add(t.position());
        }
    }
    for (c, u) in &query.device_texts {
        if !query.devices.contains(c)
            && let Some(t) = board.device(*c).and_then(|d| d.stroke_texts().get(u))
        {
            add(t.position());
        }
    }
    for u in &query.holes {
        if let Some(h) = board.holes().get(u) {
            add(h.path().first().pos);
        }
    }
    if count == 0 {
        return Ok(Vec::new());
    }
    center /= count;

    // Affected net segments.
    let mut segments: BTreeSet<NetSegmentId> = BTreeSet::new();
    segments.extend(query.traces.iter().map(|(s, _)| *s));
    segments.extend(query.vias.iter().map(|(s, _)| *s));
    segments.extend(query.pads.iter().map(|(s, _)| *s));
    for c in &query.devices {
        for seg in board.net_segments().values() {
            if seg.footprint_pads().iter().any(|(d, _)| d == c) {
                segments.insert(seg.id());
            }
        }
    }
    let mut new_segments: BTreeMap<NetSegmentId, BoardNetSegment> = segments
        .iter()
        .filter_map(|s| board.net_segment(*s).map(|seg| (*s, seg.clone())))
        .collect();

    let mut out = Vec::new();
    for s in new_segments.keys() {
        out.push(Mutation::Board(BoardMutation::RemoveNetSegment(
            BoardNetSegmentRef {
                board: board_id,
                segment: *s,
            },
        )));
    }

    // Devices with their selected texts.
    let mut devices: BTreeMap<ComponentInstanceId, BoardDevice> = BTreeMap::new();
    for c in &query.devices {
        if let Some(d) = board.device(*c) {
            let mut d = d.clone();
            d.set_mirrored(!d.mirrored());
            d.set_position(d.position().mirrored(orientation, center));
            d.set_rotation(match orientation {
                Orientation::Horizontal => -d.rotation(),
                Orientation::Vertical => Angle::DEG180 - d.rotation(),
            });
            devices.insert(*c, d);
        }
    }
    for (c, u) in &query.device_texts {
        let Some(dev) = board.device(*c) else {
            continue;
        };
        let d = devices.entry(*c).or_insert_with(|| dev.clone());
        if let Some(mut t) = d.stroke_texts().get(u).cloned() {
            mirror_text(&mut t, orientation, center, inner);
            d.insert_stroke_text(t);
        }
    }
    for d in devices.into_values() {
        out.push(Mutation::Board(BoardMutation::UpdateDevice {
            board: board_id,
            device: d,
        }));
    }

    // Net segment elements.
    for (s, u) in &query.traces {
        if let Some(seg) = new_segments.get_mut(s)
            && let Some(mut t) = seg.traces().get(u).cloned()
        {
            t.set_layer(t.layer().mirrored(inner));
            seg.insert_trace(t);
        }
    }
    for (s, u) in &query.junctions {
        if let Some(seg) = new_segments.get_mut(s)
            && let Some(mut j) = seg.junctions().get(u).cloned()
        {
            j.set_position(j.position().mirrored(orientation, center));
            seg.insert_junction(j);
        }
    }
    for (s, u) in &query.vias {
        if let Some(seg) = new_segments.get_mut(s)
            && let Some(mut v) = seg.vias().get(u).cloned()
        {
            v.set_position(v.position().mirrored(orientation, center));
            let (start, end) = (v.start_layer(), v.end_layer());
            v.set_layers(end.mirrored(inner), start.mirrored(inner))
                .map_err(|e| Error::InvalidArgument(e.to_string()))?;
            seg.insert_via(v);
        }
    }
    for (s, u) in &query.pads {
        if let Some(seg) = new_segments.get_mut(s)
            && let Some(mut pad) = seg.pads().get(u).cloned()
        {
            let pp = pad.pad_mut();
            pp.set_component_side(match pp.component_side() {
                ComponentSide::Top => ComponentSide::Bottom,
                ComponentSide::Bottom => ComponentSide::Top,
            });
            pp.set_position(pp.position().mirrored(orientation, center));
            pp.set_rotation(match orientation {
                Orientation::Horizontal => -pp.rotation(),
                Orientation::Vertical => Angle::DEG180 - pp.rotation(),
            });
            seg.insert_pad(pad);
        }
    }

    // Planes, zones, polygons, texts, holes.
    for id in &query.planes {
        if let Some(plane) = board.plane(*id) {
            let mut plane = plane.clone();
            plane.set_layer(plane.layer().mirrored(inner));
            plane.set_outline(plane.outline().mirrored(orientation, center));
            out.push(Mutation::Board(BoardMutation::UpdatePlane {
                board: board_id,
                plane,
            }));
        }
    }
    for u in &query.zones {
        if let Some(z) = board.zones().get(u) {
            let mut z = z.clone();
            z.set_outline(z.outline().mirrored(orientation, center));
            let layers: BTreeSet<Layer> = z.layers().iter().map(|l| l.mirrored(inner)).collect();
            z.set_layers(layers)
                .map_err(|e| Error::InvalidArgument(e.to_string()))?;
            out.push(Mutation::Board(BoardMutation::UpdateItem {
                board: board_id,
                item: BoardItem::Zone(z),
            }));
        }
    }
    for u in &query.polygons {
        if let Some(poly) = board.polygons().get(u) {
            let mut poly = poly.clone();
            poly.set_path(poly.path().mirrored(orientation, center));
            poly.set_layer(poly.layer().mirrored(inner));
            out.push(Mutation::Board(BoardMutation::UpdateItem {
                board: board_id,
                item: BoardItem::Polygon(poly),
            }));
        }
    }
    for u in &query.stroke_texts {
        if let Some(t) = board.stroke_texts().get(u) {
            let mut t = t.clone();
            mirror_text(&mut t, orientation, center, inner);
            out.push(Mutation::Board(BoardMutation::UpdateItem {
                board: board_id,
                item: BoardItem::StrokeText(t),
            }));
        }
    }
    for u in &query.holes {
        if let Some(h) = board.holes().get(u) {
            let mut h = h.clone();
            if let Ok(path) = NonEmptyPath::new(h.path().get().mirrored(orientation, center)) {
                h.set_path(path);
            }
            out.push(Mutation::Board(BoardMutation::UpdateItem {
                board: board_id,
                item: BoardItem::Hole(h),
            }));
        }
    }

    // Reconnect the net segments.
    for segment in new_segments.into_values() {
        out.push(Mutation::Board(BoardMutation::AddNetSegment {
            board: board_id,
            segment,
        }));
    }
    Ok(out)
}

/// Mirrors a stroke text at `center` and to the other board side (upstream
/// `CmdBoardStrokeTextEdit::mirrorGeometry()` + `mirrorLayer()`).
pub(crate) fn mirror_text(
    t: &mut BoardStrokeTextData,
    orientation: Orientation,
    center: Point,
    inner_layers: Option<usize>,
) {
    t.set_position(t.position().mirrored(orientation, center));
    t.set_rotation(match orientation {
        Orientation::Vertical => Angle::DEG180 - t.rotation(),
        Orientation::Horizontal => -t.rotation(),
    });
    t.set_align(t.align().mirrored_h());
    // mirrorLayer()
    t.set_layer(t.layer().mirrored(inner_layers));
    t.set_mirrored(!t.mirrored());
    t.set_align(t.align().mirrored_h());
}
