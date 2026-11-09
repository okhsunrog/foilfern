//! Port of libs/librepcb/editor/project/cmd/cmddragselectedschematicitems.{h,cpp}
//! and of the transformations of the item edit commands it uses
//! (`CmdSymbolInstanceEdit`, `CmdSymbolInstanceTextsReset`,
//! `CmdSchematicNetPointEdit`, `CmdSchematicNetLabelEdit`,
//! `CmdPolygonEdit`, `CmdTextEdit`, `CmdImageEdit`).
//!
//! Instead of modifying the items immediately and recording undo commands
//! at the end, the drag keeps the original items and the sequence of
//! operations (translate, rotate, mirror, snap) and computes the model
//! mutations for the current state; the FSM applies them to an open undo
//! group, replacing the previous preview. Replaying the operations on the
//! originals gives the same results as upstream's incremental updates.

use std::collections::BTreeSet;

use librepcb_core::geometry::{Image, Junction, NetLabel, Polygon, Text};
use librepcb_core::project::schematic::{Schematic, SchematicSymbol};
use librepcb_core::project::{
    BusSegmentId, BusSegmentRef, Mutation, NetSegmentId, NetSegmentRef, Project, SchematicId,
    SchematicMutation, SymbolId, SymbolRef,
};
use librepcb_core::types::{Angle, Length, Orientation, Point, PositiveLength};

use super::selection::SelectionQuery;

/// One operation of a drag (upstream: the `CmdDragSelectedSchematicItems`
/// methods).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Translate(Point),
    Rotate(Angle, Point),
    Mirror(Orientation, Point),
    SnapToGrid(PositiveLength),
}

fn map_point(p: Point, op: Op) -> Point {
    match op {
        Op::Translate(d) => p + d,
        Op::Rotate(a, c) => p.rotated(a, c),
        Op::Mirror(o, c) => p.mirrored(o, c),
        Op::SnapToGrid(g) => p.mapped_to_grid(g),
    }
}

/// `CmdSymbolInstanceEdit`.
fn transform_symbol(sym: &mut SchematicSymbol, op: Op) {
    sym.set_position(map_point(sym.position(), op));
    match op {
        Op::Rotate(a, _) => {
            sym.set_rotation(sym.rotation() + a);
        }
        Op::Mirror(o, _) => {
            sym.set_rotation(match o {
                Orientation::Horizontal => -sym.rotation(),
                Orientation::Vertical => Angle::DEG180 - sym.rotation(),
            });
            sym.set_mirrored(!sym.mirrored());
        }
        Op::Translate(_) | Op::SnapToGrid(_) => {}
    }
}

/// `CmdTextEdit`.
pub(crate) fn transform_text(text: &mut Text, op: Op) {
    text.set_position(map_point(text.position(), op));
    match op {
        Op::Rotate(a, _) => {
            text.set_rotation(text.rotation() + a);
        }
        Op::Mirror(o, _) => {
            // Upstream `CmdTextEdit::mirror(orientation, center)`.
            text.set_rotation(match o {
                Orientation::Horizontal => Angle::DEG180 - text.rotation(),
                Orientation::Vertical => -text.rotation(),
            });
            text.set_align(text.align().mirrored_v());
        }
        Op::Translate(_) | Op::SnapToGrid(_) => {}
    }
}

/// `CmdSchematicNetPointEdit`.
fn transform_junction(junction: &mut Junction, op: Op) {
    junction.set_position(map_point(junction.position(), op));
}

/// `CmdSchematicNetLabelEdit`.
pub(crate) fn transform_label(label: &mut NetLabel, op: Op) {
    label.set_position(map_point(label.position(), op));
    match op {
        Op::Rotate(a, _) => {
            label.set_rotation(label.rotation() + a);
        }
        Op::Mirror(o, _) => {
            label.set_mirrored(!label.mirrored());
            let mut rotation = label.rotation();
            if o == Orientation::Vertical {
                rotation += Angle::DEG180;
                label.set_rotation(label.rotation() + Angle::DEG180);
            }
            let rotation = rotation.mapped_to_0_360deg();
            if rotation == Angle::DEG90 || rotation == Angle::DEG270 {
                label.set_rotation(label.rotation() + Angle::DEG180);
            }
        }
        Op::Translate(_) | Op::SnapToGrid(_) => {}
    }
}

/// `CmdPolygonEdit`.
fn transform_polygon(polygon: &mut Polygon, op: Op) {
    let path = match op {
        Op::Translate(d) => polygon.path().translated(d),
        Op::Rotate(a, c) => polygon.path().rotated(a, c),
        Op::Mirror(o, c) => polygon.path().mirrored(o, c),
        Op::SnapToGrid(g) => polygon.path().mapped_to_grid(g),
    };
    polygon.set_path(path);
}

/// `CmdImageEdit`.
fn transform_image(image: &mut Image, op: Op) {
    match op {
        Op::Mirror(o, c) => {
            let offset = match o {
                Orientation::Horizontal => Point::new(-image.width().get(), Length::ZERO),
                Orientation::Vertical => Point::new(Length::ZERO, -image.height().get()),
            };
            image.set_position(
                image.position().mirrored(o, c) + offset.rotated(-image.rotation(), Point::ORIGIN),
            );
            image.set_rotation(-image.rotation());
        }
        _ => {
            image.set_position(map_point(image.position(), op));
            if let Op::Rotate(a, _) = op {
                image.set_rotation(image.rotation() + a);
            }
        }
    }
}

/// The items of a drag with their original state.
#[derive(Debug, Clone)]
pub(crate) struct DragSelection {
    schematic: SchematicId,
    grid: Option<PositiveLength>,
    symbols: Vec<SchematicSymbol>,
    junctions: Vec<(NetSegmentId, Junction)>,
    labels: Vec<(NetSegmentId, NetLabel)>,
    bus_junctions: Vec<(BusSegmentId, Junction)>,
    bus_labels: Vec<(BusSegmentId, NetLabel)>,
    polygons: Vec<Polygon>,
    texts: Vec<Text>,
    /// Texts of symbols which are not dragged themselves.
    symbol_texts: Vec<(SymbolId, Text)>,
    images: Vec<Image>,
    /// Net segments whose geometry changes (for the simplification).
    modified_segments: BTreeSet<NetSegmentId>,
    /// Bus segments whose geometry changes (for the simplification).
    modified_bus_segments: BTreeSet<BusSegmentId>,
    item_count: i64,
    center: Point,
    start: Point,
    delta: Point,
    ops: Vec<Op>,
    snapped: bool,
    mirrored: bool,
    rotated: bool,
    texts_reset: bool,
}

impl DragSelection {
    fn empty(schematic: SchematicId) -> Self {
        Self {
            schematic,
            grid: None,
            symbols: Vec::new(),
            junctions: Vec::new(),
            labels: Vec::new(),
            bus_junctions: Vec::new(),
            bus_labels: Vec::new(),
            polygons: Vec::new(),
            texts: Vec::new(),
            symbol_texts: Vec::new(),
            images: Vec::new(),
            modified_segments: BTreeSet::new(),
            modified_bus_segments: BTreeSet::new(),
            item_count: 0,
            center: Point::ORIGIN,
            start: Point::ORIGIN,
            delta: Point::ORIGIN,
            ops: Vec::new(),
            snapped: false,
            mirrored: false,
            rotated: false,
            texts_reset: false,
        }
    }

    /// Collects the selected items (upstream constructor), with the start
    /// position of the drag.
    pub fn new(s: &Schematic, query: &SelectionQuery, start: Point) -> Self {
        let mut query = query.clone();
        query.add_junctions_of_bus_lines(s, false);
        query.add_net_points_of_net_lines(s, false);
        let grid = s.properties().grid_interval;
        let mut d = Self {
            grid: Some(grid),
            start,
            ..Self::empty(s.id())
        };
        for (seg, _) in query.net_lines.iter().chain(&query.net_points) {
            d.modified_segments.insert(*seg);
        }
        for (seg, _) in query.bus_lines.iter().chain(&query.bus_junctions) {
            d.modified_bus_segments.insert(*seg);
        }
        for symbol in &query.symbols {
            for (seg_id, seg) in s.net_segments() {
                if seg.connected_pins().iter().any(|(sym, _)| sym == symbol) {
                    d.modified_segments.insert(*seg_id);
                }
            }
        }
        let mut center = Point::ORIGIN;
        for id in &query.symbols {
            let symbol = &s.symbols()[id];
            center += symbol.position();
            d.item_count += 1;
            d.symbols.push(symbol.clone());
        }
        for (seg, j) in &query.bus_junctions {
            let junction = &s.bus_segments()[seg].junctions()[j];
            center += junction.position();
            d.item_count += 1;
            d.bus_junctions.push((*seg, junction.clone()));
        }
        for (seg, l) in &query.bus_labels {
            let label = &s.bus_segments()[seg].labels()[l];
            center += label.position();
            d.item_count += 1;
            d.bus_labels.push((*seg, label.clone()));
        }
        for (seg, j) in &query.net_points {
            let junction = &s.net_segments()[seg].junctions()[j];
            center += junction.position();
            d.item_count += 1;
            d.junctions.push((*seg, junction.clone()));
        }
        for (seg, l) in &query.net_labels {
            let label = &s.net_segments()[seg].labels()[l];
            center += label.position();
            d.item_count += 1;
            d.labels.push((*seg, label.clone()));
        }
        for id in &query.polygons {
            let polygon = &s.polygons()[id];
            for v in polygon.path().vertices() {
                center += v.pos;
                d.item_count += 1;
            }
            d.polygons.push(polygon.clone());
        }
        for id in &query.texts {
            let text = &s.texts()[id];
            center += text.position();
            d.item_count += 1;
            d.texts.push(text.clone());
        }
        for (symbol, id) in &query.symbol_texts {
            // Texts of dragged symbols move with their symbol.
            if query.symbols.contains(symbol) {
                continue;
            }
            let text = &s.symbols()[symbol].texts()[id];
            center += text.position();
            d.item_count += 1;
            d.symbol_texts.push((*symbol, text.clone()));
        }
        for id in &query.images {
            let image = &s.images()[id];
            center += image.center();
            d.item_count += 1;
            d.images.push(image.clone());
        }
        // If only one item is selected, its exact position is the center.
        if d.item_count > 1 {
            center /= d.item_count;
            center = center.mapped_to_grid(grid);
        }
        d.center = center;
        d
    }

    /// A "drag" of a single symbol (placing a component).
    pub fn for_symbol(schematic: SchematicId, symbol: SchematicSymbol) -> Self {
        let pos = symbol.position();
        Self {
            schematic,
            center: pos,
            start: pos,
            item_count: 1,
            symbols: vec![symbol],
            ..Self::empty(schematic)
        }
    }

    /// A "drag" of a single net label (placing a label).
    pub fn for_label(schematic: SchematicId, segment: NetSegmentId, label: NetLabel) -> Self {
        let pos = label.position();
        Self {
            schematic,
            center: pos,
            start: pos,
            item_count: 1,
            labels: vec![(segment, label)],
            ..Self::empty(schematic)
        }
    }

    /// A "drag" of a single text (placing a text).
    pub fn for_text(schematic: SchematicId, text: Text) -> Self {
        let pos = text.position();
        Self {
            schematic,
            center: pos,
            start: pos,
            item_count: 1,
            texts: vec![text],
            ..Self::empty(schematic)
        }
    }

    /// Whether nothing is dragged.
    pub fn is_empty(&self) -> bool {
        self.item_count == 0
    }

    /// Net segments whose geometry is changed by the drag.
    pub fn modified_segments(&self) -> &BTreeSet<NetSegmentId> {
        &self.modified_segments
    }

    /// Bus segments whose geometry is changed by the drag.
    pub fn modified_bus_segments(&self) -> &BTreeSet<BusSegmentId> {
        &self.modified_bus_segments
    }

    /// A "drag" of a single bus label (placing a bus label).
    pub fn for_bus_label(schematic: SchematicId, segment: BusSegmentId, label: NetLabel) -> Self {
        let pos = label.position();
        Self {
            schematic,
            center: pos,
            start: pos,
            item_count: 1,
            bus_labels: vec![(segment, label)],
            ..Self::empty(schematic)
        }
    }

    /// Current position of the dragged items relative to the start.
    pub fn delta(&self) -> Point {
        self.delta
    }

    /// Current position of a single dragged item (placing tools).
    pub fn current_pos(&self) -> Point {
        self.center + self.delta
    }

    /// Moves the items to `pos` (upstream `setCurrentPosition()`; the
    /// offset is snapped to the grid unless `grid` is `None`).
    pub fn set_current_position(&mut self, pos: Point, grid: Option<PositiveLength>) {
        let mut delta = pos - self.start;
        if let Some(grid) = grid.or(self.grid) {
            delta = delta.mapped_to_grid(grid);
        }
        if delta != self.delta {
            let step = delta - self.delta;
            if let Some(Op::Translate(last)) = self.ops.last_mut() {
                *last += step;
            } else {
                self.ops.push(Op::Translate(step));
            }
            self.delta = delta;
        }
    }

    fn op_center(&self, around_current_position: bool, grid: PositiveLength) -> Point {
        if around_current_position && self.item_count > 1 {
            (self.start + self.delta).mapped_to_grid(grid)
        } else {
            self.center + self.delta
        }
    }

    /// Rotates the items (upstream `rotate()`).
    pub fn rotate(&mut self, angle: Angle, around_current_position: bool, grid: PositiveLength) {
        let center = self.op_center(around_current_position, grid);
        self.ops.push(Op::Rotate(angle, center));
        self.rotated = true;
    }

    /// Mirrors the items (upstream `mirror()`).
    pub fn mirror(
        &mut self,
        orientation: Orientation,
        around_current_position: bool,
        grid: PositiveLength,
    ) {
        let center = self.op_center(around_current_position, grid);
        self.ops.push(Op::Mirror(orientation, center));
        self.mirrored = !self.mirrored;
    }

    /// Snaps the items to the grid (upstream `snapToGrid()`).
    pub fn snap_to_grid(&mut self, grid: PositiveLength) {
        self.ops.push(Op::SnapToGrid(grid));
        self.snapped = true;
    }

    /// Resets the texts of the symbols (upstream `resetAllTexts()`).
    pub fn reset_all_texts(&mut self) {
        self.texts_reset = true;
    }

    /// Whether the drag changes anything (upstream `performExecute()`).
    pub fn has_changes(&self) -> bool {
        !self.delta.is_origin() || self.rotated || self.snapped || self.mirrored || self.texts_reset
    }

    /// The mutations bringing the items from their original state to the
    /// current state.
    pub fn mutations(&self, p: &Project) -> Vec<Mutation> {
        let sch = |m| Mutation::Schematic(m);
        let schematic = self.schematic;
        let mut result = Vec::new();
        for original in &self.symbols {
            let mut symbol = original.clone();
            let mut texts: Vec<Text> = symbol.texts().values().cloned().collect();
            for op in &self.ops {
                transform_symbol(&mut symbol, *op);
                for text in &mut texts {
                    transform_text(text, *op);
                }
            }
            let r = SymbolRef {
                schematic,
                symbol: symbol.id(),
            };
            if (symbol.position(), symbol.rotation(), symbol.mirrored())
                != (
                    original.position(),
                    original.rotation(),
                    original.mirrored(),
                )
            {
                result.push(sch(SchematicMutation::SetSymbolPlacement {
                    symbol: r,
                    position: symbol.position(),
                    rotation: symbol.rotation(),
                    mirrored: symbol.mirrored(),
                }));
            }
            if self.texts_reset {
                // Upstream `CmdSymbolInstanceTextsReset`.
                for uuid in original.texts().keys() {
                    result.push(sch(SchematicMutation::RemoveSymbolText {
                        symbol: r,
                        text: *uuid,
                    }));
                }
                if let Ok(resolved) = symbol.resolve(p.view()) {
                    for text in symbol.default_texts(resolved.lib_symbol) {
                        result.push(sch(SchematicMutation::AddSymbolText { symbol: r, text }));
                    }
                }
            } else {
                for text in texts {
                    if original.texts().get(&text.uuid()) != Some(&text) {
                        result.push(sch(SchematicMutation::UpdateSymbolText { symbol: r, text }));
                    }
                }
            }
        }
        for (segment, original) in &self.junctions {
            let mut junction = original.clone();
            for op in &self.ops {
                transform_junction(&mut junction, *op);
            }
            if junction.position() != original.position() {
                result.push(sch(SchematicMutation::SetNetPointPosition {
                    segment: NetSegmentRef {
                        schematic,
                        segment: *segment,
                    },
                    junction: junction.uuid(),
                    position: junction.position(),
                }));
            }
        }
        for (segment, original) in &self.labels {
            let mut label = original.clone();
            for op in &self.ops {
                transform_label(&mut label, *op);
            }
            if &label != original {
                result.push(sch(SchematicMutation::UpdateNetLabel {
                    segment: NetSegmentRef {
                        schematic,
                        segment: *segment,
                    },
                    label,
                }));
            }
        }
        for (segment, original) in &self.bus_junctions {
            let mut junction = original.clone();
            for op in &self.ops {
                transform_junction(&mut junction, *op);
            }
            if junction.position() != original.position() {
                result.push(sch(SchematicMutation::SetBusJunctionPosition {
                    segment: BusSegmentRef {
                        schematic,
                        segment: *segment,
                    },
                    junction: junction.uuid(),
                    position: junction.position(),
                }));
            }
        }
        for (segment, original) in &self.bus_labels {
            let mut label = original.clone();
            for op in &self.ops {
                transform_label(&mut label, *op);
            }
            if &label != original {
                result.push(sch(SchematicMutation::UpdateBusLabel {
                    segment: BusSegmentRef {
                        schematic,
                        segment: *segment,
                    },
                    label,
                }));
            }
        }
        for (symbol, original) in &self.symbol_texts {
            let mut text = original.clone();
            for op in &self.ops {
                transform_text(&mut text, *op);
            }
            if &text != original {
                result.push(sch(SchematicMutation::UpdateSymbolText {
                    symbol: SymbolRef {
                        schematic,
                        symbol: *symbol,
                    },
                    text,
                }));
            }
        }
        for original in &self.polygons {
            let mut polygon = original.clone();
            for op in &self.ops {
                transform_polygon(&mut polygon, *op);
            }
            if &polygon != original {
                result.push(sch(SchematicMutation::UpdatePolygon { schematic, polygon }));
            }
        }
        for original in &self.texts {
            let mut text = original.clone();
            for op in &self.ops {
                transform_text(&mut text, *op);
            }
            if &text != original {
                result.push(sch(SchematicMutation::UpdateText { schematic, text }));
            }
        }
        for original in &self.images {
            let mut image = original.clone();
            for op in &self.ops {
                transform_image(&mut image, *op);
            }
            if &image != original {
                result.push(sch(SchematicMutation::UpdateImage { schematic, image }));
            }
        }
        result
    }
}
