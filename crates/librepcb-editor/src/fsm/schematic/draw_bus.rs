//! Port of libs/librepcb/editor/project/schematic/fsm/schematiceditorstate_drawbus.{h,cpp}.
//!
//! Works like the draw wire state (see `draw_wire`): the bus lines are
//! built with the [`DrawBus`] command; while positioning, the group of the
//! current segment contains the preview (from the fixed start anchor over
//! the corner to the cursor); a click replaces it by the final lines
//! connected to the bus junction or line under the cursor and commits the
//! group. A new bus segment belongs to the bus chosen in the tool bar (and
//! gets a bus label at its start) or to a new bus with an automatic name.

use std::collections::BTreeSet;

use librepcb_core::project::BusSegmentId;
use librepcb_core::types::Point;
use librepcb_core::utils::toolbox;
use librepcb_i18n::tr;

use super::hit_test::{FindFlags, find_items_at};
use super::simplify::SimplifySchematicSegments;
use super::{Cx, SchematicItem, SchematicTool, State};
use crate::commands::{BusAnchor, BusResult, DrawBus, WireMode};
use crate::fsm::{CursorShape, Key, KeyEvent, PointerEvent};

/// The draw bus state.
#[derive(Debug, Default)]
pub(crate) struct DrawBusState {
    /// Positioning: the fixed start anchor and its position.
    start: Option<(BusAnchor, Point)>,
    /// The last preview.
    preview: Option<BusResult>,
    /// The segments of the committed parts (for the simplification).
    segments: BTreeSet<BusSegmentId>,
    cursor: Point,
}

/// The flags upstream's `findItem()` uses: bus junctions and lines.
fn anchor_flags() -> FindFlags {
    FindFlags {
        bus_junctions: true,
        bus_lines: true,
        ..FindFlags::NONE
    }
    .within_grid()
}

impl DrawBusState {
    fn is_positioning(&self) -> bool {
        self.start.is_some()
    }

    /// Returns the item to snap to and the snapped position (upstream
    /// `updateJunctionPositions()` item lookup).
    fn snap_target(&self, cx: &Cx<'_, '_>, snap: bool) -> (Option<SchematicItem>, Point) {
        let mut pos = self.cursor.mapped_to_grid(cx.grid());
        if !snap {
            return (None, pos);
        }
        let Some(s) = cx.sch() else {
            return (None, pos);
        };
        let item = find_items_at(cx, self.cursor, anchor_flags(), &[])
            .into_iter()
            .next();
        match item {
            Some(SchematicItem::BusJunction(seg, j)) => {
                pos = s.bus_segments()[&seg].junctions()[&j].position();
            }
            Some(SchematicItem::BusLine(seg, l)) => {
                let segment = &s.bus_segments()[&seg];
                let line = &segment.lines()[&l];
                if let (Some(p1), Some(p2)) = (
                    segment.junction_position(line.p1()),
                    segment.junction_position(line.p2()),
                ) {
                    pos = toolbox::nearest_point_on_line(pos, p1, p2);
                }
            }
            _ => {}
        }
        (item, pos)
    }

    fn anchor_of(item: Option<SchematicItem>, pos: Point) -> BusAnchor {
        match item {
            Some(SchematicItem::BusJunction(segment, junction)) => {
                BusAnchor::Junction { segment, junction }
            }
            Some(SchematicItem::BusLine(segment, line)) => BusAnchor::Line {
                segment,
                line,
                position: pos,
            },
            _ => BusAnchor::Point(pos),
        }
    }

    fn corner(cx: &Cx<'_, '_>, start: Point, pos: Point) -> Vec<Point> {
        let middle = cx.out.tool_data.wire_mode.middle_point(start, pos);
        if middle != start && middle != pos {
            vec![middle]
        } else {
            Vec::new()
        }
    }

    /// Replaces the preview (upstream `updateJunctionPositions()`).
    fn update_preview(&mut self, cx: &mut Cx<'_, '_>, snap: bool) -> Point {
        cx.ctx.editor.rollback_group_to(0);
        self.preview = None;
        let (_, pos) = self.snap_target(cx, snap);
        let Some((start, start_pos)) = self.start.clone() else {
            return pos;
        };
        if pos != start_pos {
            let points = Self::corner(cx, start_pos, pos);
            match cx.ctx.editor.execute(DrawBus {
                schematic: Some(cx.schematic),
                start,
                end: BusAnchor::Point(pos),
                points,
                bus: cx.out.tool_data.bus,
            }) {
                Ok(result) => self.preview = Some(result),
                Err(e) => log::debug!("Bus preview failed: {e}"),
            }
        }
        pos
    }

    /// Upstream `startPositioning()`.
    fn start_positioning(&mut self, cx: &mut Cx<'_, '_>, snap: bool) -> bool {
        let (item, pos) = self.snap_target(cx, snap);
        let anchor = Self::anchor_of(item, pos);
        if let Err(e) = cx.ctx.editor.begin_group(tr!(
            "librepcb::editor::SchematicEditorState_DrawBus",
            "Draw Wire"
        )) {
            cx.error(e);
            return false;
        }
        self.start = Some((anchor, pos));
        self.preview = None;
        self.update_preview(cx, snap);
        true
    }

    /// Upstream `addNextJunction()`.
    fn add_next_junction(&mut self, cx: &mut Cx<'_, '_>, snap: bool) -> bool {
        let Some((start, start_pos)) = self.start.clone() else {
            return false;
        };
        cx.ctx.editor.rollback_group_to(0);
        self.preview = None;
        let (item, pos) = self.snap_target(cx, snap);
        if pos == start_pos {
            self.abort_positioning(cx, true);
            return false;
        }
        let end = Self::anchor_of(item, pos);
        let finish = !matches!(end, BusAnchor::Point(_));
        let points = Self::corner(cx, start_pos, pos);
        let result = match cx.ctx.editor.execute(DrawBus {
            schematic: Some(cx.schematic),
            start,
            end,
            points,
            bus: cx.out.tool_data.bus,
        }) {
            Ok(result) => result,
            Err(e) => {
                cx.error(e);
                self.abort_positioning(cx, true);
                return false;
            }
        };
        if let Err(e) = cx.ctx.editor.commit_group() {
            cx.error(e);
        }
        self.segments.insert(result.segment);
        self.start = None;
        if finish {
            self.abort_positioning(cx, true);
            return false;
        }
        let junction = cx.sch().and_then(|s| {
            let seg = s.bus_segments().get(&result.segment)?;
            result
                .junctions
                .iter()
                .find(|j| seg.junctions().get(*j).is_some_and(|x| x.position() == pos))
                .copied()
        });
        let Some(junction) = junction else {
            self.abort_positioning(cx, true);
            return false;
        };
        if let Err(e) = cx.ctx.editor.begin_group(tr!(
            "librepcb::editor::SchematicEditorState_DrawBus",
            "Draw Wire"
        )) {
            cx.error(e);
            return false;
        }
        self.start = Some((
            BusAnchor::Junction {
                segment: result.segment,
                junction,
            },
            pos,
        ));
        self.update_preview(cx, snap);
        true
    }

    /// Upstream `abortPositioning()`: discards the preview and simplifies
    /// the drawn segments.
    fn abort_positioning(&mut self, cx: &mut Cx<'_, '_>, simplify: bool) -> bool {
        let mut ok = true;
        self.start = None;
        self.preview = None;
        if cx.ctx.editor.undo_stack().is_group_active()
            && let Err(e) = cx.ctx.editor.abort_group()
        {
            cx.error(e);
            ok = false;
        }
        let segments = std::mem::take(&mut self.segments);
        if simplify && !segments.is_empty() {
            let bus_segments: BTreeSet<BusSegmentId> = segments
                .into_iter()
                .filter(|s| cx.sch().is_some_and(|x| x.bus_segments().contains_key(s)))
                .collect();
            if let Err(e) = cx.ctx.editor.execute(SimplifySchematicSegments {
                schematic: cx.schematic,
                segments: BTreeSet::new(),
                bus_segments,
            }) {
                log::error!("Failed to simplify schematic segments: {e}");
            }
        }
        ok
    }

    /// The wire mode was changed from the tool bar.
    pub fn wire_mode_changed(&mut self, cx: &mut Cx<'_, '_>) {
        if self.is_positioning() {
            self.update_preview(cx, true);
        }
    }
}

impl State for DrawBusState {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        cx.out.tool = SchematicTool::Bus;
        // Upstream: the preselected bus is reset when entering the tool.
        cx.out.tool_data.bus = None;
        cx.set_cursor(Some(CursorShape::Cross));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        if self.is_positioning() {
            self.abort_positioning(cx, true);
        }
        cx.set_cursor(None);
        true
    }

    fn abort(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        if self.is_positioning() {
            return self.abort_positioning(cx, true);
        }
        false
    }

    fn key_pressed(&mut self, cx: &mut Cx<'_, '_>, e: KeyEvent) -> bool {
        if e.key == Key::Shift && self.is_positioning() {
            self.update_preview(cx, false);
            return true;
        }
        false
    }

    fn key_released(&mut self, cx: &mut Cx<'_, '_>, e: KeyEvent) -> bool {
        if e.key == Key::Shift && self.is_positioning() {
            self.update_preview(cx, true);
            return true;
        }
        false
    }

    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        self.cursor = e.pos;
        if self.is_positioning() {
            self.update_preview(cx, !e.modifiers.shift);
            return true;
        }
        false
    }

    fn left_pressed(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        self.cursor = e.pos;
        let snap = !e.modifiers.shift;
        if self.is_positioning() {
            self.add_next_junction(cx, snap)
        } else {
            self.start_positioning(cx, snap)
        }
    }

    fn left_double_clicked(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        self.cursor = e.pos;
        if self.is_positioning() {
            return self.add_next_junction(cx, !e.modifiers.shift);
        }
        false
    }

    fn right_released(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        self.cursor = e.pos;
        if self.is_positioning() {
            let mode = &mut cx.out.tool_data.wire_mode;
            *mode = match *mode {
                WireMode::HV => WireMode::VH,
                WireMode::VH => WireMode::Deg9045,
                WireMode::Deg9045 => WireMode::Deg4590,
                WireMode::Deg4590 => WireMode::Straight,
                WireMode::Straight => WireMode::HV,
            };
            self.update_preview(cx, true);
            return true;
        }
        false
    }

    fn update(&mut self, cx: &mut Cx<'_, '_>) {
        let mut buses: Vec<(librepcb_core::project::BusId, String)> = cx
            .project()
            .circuit()
            .buses()
            .iter()
            .map(|(id, b)| (*id, b.name().to_string()))
            .collect();
        buses.sort_by(|a, b| toolbox::compare_numeric(&a.1, &b.1));
        cx.out.tool_data.buses = buses;
        if cx
            .out
            .tool_data
            .bus
            .is_some_and(|b| cx.project().circuit().bus(b).is_none())
        {
            cx.out.tool_data.bus = None;
        }
    }
}
