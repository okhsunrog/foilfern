//! Port of libs/librepcb/editor/project/schematic/fsm/schematiceditorstate_drawwire.{h,cpp}.
//!
//! The wire is built with the [`DrawWire`] command (which ports the model
//! changes of upstream's `startPositioning()`/`addNextNetPoint()`): while
//! positioning, the group of the current segment contains the preview
//! (from the fixed start anchor over the corner to the cursor, never
//! connected at the cursor side); a click replaces it by the final wire
//! connected to the anchor under the cursor and commits the group.
//!
//! Buses: wires can start and end at bus junctions and bus lines (which
//! are split). Upstream then shows a menu of the bus members to choose the
//! net from; here the FSM requests it ([`SchematicRequest::BusMemberMenu`])
//! and continues when the application calls
//! [`SchematicEditorFsm::choose_bus_member()`](super::SchematicEditorFsm::choose_bus_member).
//! With Control pressed, no menu is shown (upstream "Add New Bus Member").
//! A net label is added like upstream: at the end of a wire starting at a
//! bus, at the start of a wire ending at a bus.
//!
//! Differences to upstream: forced net names of pins are applied by the
//! command.

use std::collections::BTreeSet;

use librepcb_core::project::{BusSegmentId, NetSegmentId, NetSignalId};
use librepcb_core::types::{CircuitIdentifier, Point};
use librepcb_core::utils::toolbox;
use librepcb_i18n::tr;

use super::hit_test::{FindFlags, find_items_at, pin_position};
use super::simplify::SimplifySchematicSegments;
use super::{
    BusMemberChoice, BusMemberNet, Cx, SchematicItem, SchematicRequest, SchematicTool, State,
};
use crate::commands::{
    AddNetLabel, DrawWire, WireAnchor, WireMode, WireResult, net_label_orientation,
};
use crate::fsm::{CursorShape, Key, KeyEvent, PointerEvent};

/// A click on a bus waiting for the bus member menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingMenu {
    /// Starting the wire at a bus.
    Start { snap: bool },
    /// Ending the wire at a bus.
    End { snap: bool },
}

/// The draw wire state.
#[derive(Debug, Default)]
pub(crate) struct DrawWireState {
    /// Positioning: the fixed start anchor and its position.
    start: Option<(WireAnchor, Point)>,
    /// The net name chosen from the bus member menu at the start.
    start_net: Option<CircuitIdentifier>,
    /// Whether a net label follows the end of the wire (started at a bus).
    start_label: bool,
    /// The last preview.
    preview: Option<WireResult>,
    /// The segment of the last committed wire part (for the simplification
    /// when finishing).
    segments: BTreeSet<NetSegmentId>,
    /// A click on a bus waiting for the menu choice.
    pending: Option<PendingMenu>,
    cursor: Point,
}

/// The flags upstream's `findItem()` uses: only anchors.
fn anchor_flags() -> FindFlags {
    FindFlags {
        net_points: true,
        net_lines: true,
        symbol_pins: true,
        bus_junctions: true,
        bus_lines: true,
        ..FindFlags::NONE
    }
    .within_grid()
}

fn is_bus(item: Option<SchematicItem>) -> bool {
    matches!(
        item,
        Some(SchematicItem::BusJunction(..) | SchematicItem::BusLine(..))
    )
}

impl DrawWireState {
    fn is_positioning(&self) -> bool {
        self.start.is_some()
    }

    /// Returns the item to snap to and the snapped position (upstream
    /// `updateNetpointPositions()` item lookup).
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
            Some(SchematicItem::NetPoint(seg, j)) => {
                pos = s.net_segments()[&seg].junctions()[&j].position();
            }
            Some(SchematicItem::SymbolPin(symbol, pin)) => {
                if let Some(p) = pin_position(cx, s, symbol, pin) {
                    pos = p;
                }
            }
            Some(SchematicItem::NetLine(seg, l)) => {
                let segment = &s.net_segments()[&seg];
                let line = &segment.lines()[&l];
                let ap = |a| s.net_line_anchor_position(seg, a, cx.project().view());
                if let (Some(p1), Some(p2)) = (ap(line.p1()), ap(line.p2())) {
                    pos = toolbox::nearest_point_on_line(pos, p1, p2);
                }
            }
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

    /// Converts a hit item to a wire anchor.
    fn anchor_of(item: Option<SchematicItem>, pos: Point) -> WireAnchor {
        match item {
            Some(SchematicItem::NetPoint(segment, junction)) => {
                WireAnchor::Junction { segment, junction }
            }
            Some(SchematicItem::SymbolPin(symbol, pin)) => WireAnchor::SymbolPin { symbol, pin },
            Some(SchematicItem::NetLine(segment, line)) => WireAnchor::Line {
                segment,
                line,
                position: pos,
            },
            Some(SchematicItem::BusJunction(segment, junction)) => {
                WireAnchor::BusJunction { segment, junction }
            }
            Some(SchematicItem::BusLine(segment, line)) => WireAnchor::BusLine {
                segment,
                line,
                position: pos,
            },
            _ => WireAnchor::Point(pos),
        }
    }

    /// The wire mode: 45° at buses (upstream `updateNetpointPositions()`).
    fn wire_mode(&self, cx: &Cx<'_, '_>, target: Option<SchematicItem>) -> WireMode {
        if is_bus(target) {
            WireMode::Deg9045
        } else if matches!(
            self.start,
            Some((
                WireAnchor::BusJunction { .. } | WireAnchor::BusLine { .. },
                _
            ))
        ) {
            WireMode::Deg4590
        } else {
            cx.out.tool_data.wire_mode
        }
    }

    /// The corner points between the start and `pos`.
    fn corner(mode: WireMode, start: Point, pos: Point) -> Vec<Point> {
        let middle = mode.middle_point(start, pos);
        if middle != start && middle != pos {
            vec![middle]
        } else {
            Vec::new()
        }
    }

    /// Adds a net label to the segment of a wire at `pos`, oriented towards
    /// `dir_pos` (upstream `updateNetLabelPosition()`).
    fn add_label(cx: &mut Cx<'_, '_>, segment: NetSegmentId, pos: Point, dir_pos: Point) {
        let (rotation, mirrored) = net_label_orientation(pos, dir_pos);
        if let Err(e) = cx.ctx.editor.execute(AddNetLabel {
            segment,
            position: pos,
            rotation,
            mirrored,
        }) {
            log::warn!("Failed to add the net label: {e}");
        }
    }

    /// Replaces the preview (upstream `updateNetpointPositions()`).
    fn update_preview(&mut self, cx: &mut Cx<'_, '_>, snap: bool) -> Point {
        cx.ctx.editor.rollback_group_to(0);
        self.preview = None;
        let (item, pos) = self.snap_target(cx, snap);
        let Some((start, start_pos)) = self.start.clone() else {
            return pos;
        };
        cx.out.view.scene_cursor = None;
        if pos != start_pos {
            let points = Self::corner(self.wire_mode(cx, item), start_pos, pos);
            let dir_pos = points.first().copied().unwrap_or(start_pos);
            match cx.ctx.editor.execute(DrawWire {
                schematic: Some(cx.schematic),
                start,
                end: WireAnchor::Point(pos),
                points,
                net: self.start_net.clone(),
            }) {
                Ok(result) => {
                    if self.start_label {
                        Self::add_label(cx, result.segment, pos, dir_pos);
                    }
                    self.preview = Some(result);
                }
                Err(e) => log::debug!("Wire preview failed: {e}"),
            }
        }
        pos
    }

    /// Requests the bus member menu for a click on a bus (upstream
    /// `determineNetForBusMember()`).
    fn request_menu(
        &mut self,
        cx: &mut Cx<'_, '_>,
        item: Option<SchematicItem>,
        pending: PendingMenu,
    ) {
        let bus_segment: Option<BusSegmentId> = match item {
            Some(SchematicItem::BusJunction(seg, _) | SchematicItem::BusLine(seg, _)) => Some(seg),
            _ => None,
        };
        let nets = bus_segment
            .map(|seg| bus_member_nets(cx, seg))
            .unwrap_or_default();
        self.pending = Some(pending);
        cx.out.requests.push(SchematicRequest::BusMemberMenu {
            pos: self.cursor,
            nets,
        });
    }

    /// Starts drawing at the cursor (upstream `startPositioning()`).
    fn start_positioning(
        &mut self,
        cx: &mut Cx<'_, '_>,
        snap: bool,
        interactive: bool,
        choice: Option<BusMemberChoice>,
    ) -> bool {
        let (item, pos) = self.snap_target(cx, snap);
        if is_bus(item) && interactive && choice.is_none() {
            self.request_menu(cx, item, PendingMenu::Start { snap });
            return true;
        }
        let anchor = Self::anchor_of(item, pos);
        if let Err(e) = cx.ctx.editor.begin_group(tr!(
            "librepcb::editor::SchematicEditorState_DrawWire",
            "Draw Wire"
        )) {
            cx.error(e);
            return false;
        }
        self.start_net = None;
        self.start_label = false;
        if is_bus(item) && interactive {
            self.start_label = true;
            if let Some(BusMemberChoice::Net(net)) = choice {
                self.start_net = cx
                    .project()
                    .circuit()
                    .net_signal(net)
                    .map(|n| n.name().clone());
            }
        }
        self.start = Some((anchor, pos));
        self.preview = None;
        self.update_preview(cx, snap);
        true
    }

    /// Fixes the current point and continues or finishes (upstream
    /// `addNextNetPoint()`).
    fn add_next_point(
        &mut self,
        cx: &mut Cx<'_, '_>,
        snap: bool,
        interactive: bool,
        choice: Option<BusMemberChoice>,
    ) -> bool {
        let Some((start, start_pos)) = self.start.clone() else {
            return false;
        };
        // A wire ending at a bus gets a net label at its start (if its
        // segment has none yet) and possibly the net chosen from the menu.
        let current_segment = self.preview.as_ref().map(|p| p.segment);
        let segment_has_labels = current_segment.is_some_and(|seg| {
            self.start_label
                || cx
                    .sch()
                    .and_then(|s| s.net_segments().get(&seg))
                    .is_some_and(|s| !s.labels().is_empty())
        });
        let net_forced = current_segment.is_some_and(|seg| {
            cx.sch()
                .and_then(|s| s.net_segments().get(&seg))
                .is_some_and(|x| crate::fsm::find::forced_nets(cx.project()).contains(&x.net()))
        });
        // The preview is not an anchor (upstream: `findItem()` excludes the
        // positioning items).
        cx.ctx.editor.rollback_group_to(0);
        self.preview = None;
        let (item, pos) = self.snap_target(cx, snap);
        if pos == start_pos {
            self.abort_positioning(cx, true);
            return false;
        }
        let end_at_bus = is_bus(item) && current_segment.is_some() && !segment_has_labels;
        if end_at_bus && interactive && !net_forced && choice.is_none() {
            self.request_menu(cx, item, PendingMenu::End { snap });
            self.update_preview(cx, snap);
            return true;
        }
        let mut net = self.start_net.clone();
        if end_at_bus && let Some(BusMemberChoice::Net(n)) = choice {
            net = cx
                .project()
                .circuit()
                .net_signal(n)
                .map(|x| x.name().clone());
        }
        let end = Self::anchor_of(item, pos);
        let finish = !matches!(end, WireAnchor::Point(_));
        let points = Self::corner(self.wire_mode(cx, item), start_pos, pos);
        let dir_pos = points.first().copied().unwrap_or(start_pos);
        let first_point = points.first().copied().unwrap_or(pos);
        let result = cx.ctx.editor.execute(DrawWire {
            schematic: Some(cx.schematic),
            start,
            end,
            points,
            net,
        });
        let result = match result {
            Ok(result) => result,
            Err(e) => {
                cx.error(e);
                self.abort_positioning(cx, true);
                return false;
            }
        };
        if self.start_label {
            Self::add_label(cx, result.segment, pos, dir_pos);
        } else if end_at_bus {
            Self::add_label(cx, result.segment, start_pos, first_point);
        }
        if let Err(e) = cx.ctx.editor.commit_group() {
            cx.error(e);
        }
        self.segments.insert(result.segment);
        self.start = None;
        self.start_label = false;
        self.start_net = None;
        if finish {
            self.abort_positioning(cx, true);
            return false;
        }
        // Continue from the new end junction.
        let junction = cx.sch().and_then(|s| {
            let seg = s.net_segments().get(&result.segment)?;
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
            "librepcb::editor::SchematicEditorState_DrawWire",
            "Draw Wire"
        )) {
            cx.error(e);
            return false;
        }
        self.start = Some((
            WireAnchor::Junction {
                segment: result.segment,
                junction,
            },
            pos,
        ));
        self.update_preview(cx, snap);
        true
    }

    /// The answer of the bus member menu (`None`: canceled).
    pub fn choose_bus_member(
        &mut self,
        cx: &mut Cx<'_, '_>,
        choice: Option<BusMemberChoice>,
    ) -> bool {
        let Some(pending) = self.pending.take() else {
            return false;
        };
        let Some(choice) = choice else {
            // Upstream `UserCanceled`: nothing started, or keep drawing.
            return false;
        };
        match pending {
            PendingMenu::Start { snap } => self.start_positioning(cx, snap, true, Some(choice)),
            PendingMenu::End { snap } => self.add_next_point(cx, snap, true, Some(choice)),
        }
    }

    /// Stops drawing (upstream `abortPositioning()`): discards the preview
    /// and simplifies the drawn segments.
    fn abort_positioning(&mut self, cx: &mut Cx<'_, '_>, simplify: bool) -> bool {
        let mut ok = true;
        self.start = None;
        self.start_net = None;
        self.start_label = false;
        self.pending = None;
        self.preview = None;
        if cx.ctx.editor.undo_stack().is_group_active()
            && let Err(e) = cx.ctx.editor.abort_group()
        {
            cx.error(e);
            ok = false;
        }
        let segments = std::mem::take(&mut self.segments);
        if simplify && !segments.is_empty() {
            let segments: BTreeSet<NetSegmentId> = segments
                .into_iter()
                .filter(|s| cx.sch().is_some_and(|x| x.net_segments().contains_key(s)))
                .collect();
            if let Err(e) = cx.ctx.editor.execute(SimplifySchematicSegments {
                schematic: cx.schematic,
                segments,
                bus_segments: BTreeSet::new(),
            }) {
                log::error!("Failed to simplify net segments: {e}");
            }
        }
        ok
    }

    /// The wire mode was changed from the tool bar (upstream
    /// `setWireMode()`).
    pub fn wire_mode_changed(&mut self, cx: &mut Cx<'_, '_>) {
        if self.is_positioning() {
            self.update_preview(cx, true);
        }
    }
}

/// The nets connected to the bus of a bus segment, named nets first, then
/// by name (upstream `determineNetForBusMember()`); anonymous nets are
/// disabled.
fn bus_member_nets(cx: &Cx<'_, '_>, segment: BusSegmentId) -> Vec<BusMemberNet> {
    let p = cx.project();
    let Some(bus) = cx
        .sch()
        .and_then(|s| s.bus_segments().get(&segment))
        .map(|s| s.bus())
    else {
        return Vec::new();
    };
    let mut nets: BTreeSet<NetSignalId> = BTreeSet::new();
    for (schematic, bus_segment) in p.bus_uses(bus) {
        if let Some(s) = p.schematic(*schematic) {
            for ns in s.attached_net_segments(*bus_segment) {
                if let Some(seg) = s.net_segments().get(&ns) {
                    nets.insert(seg.net());
                }
            }
        }
    }
    let named = crate::fsm::find::named_nets(p);
    let mut result: Vec<BusMemberNet> = nets
        .into_iter()
        .filter_map(|id| {
            let n = p.circuit().net_signal(id)?;
            Some(BusMemberNet {
                net: id,
                name: n.name().to_string(),
                enabled: named.contains(&id),
            })
        })
        .collect();
    result.sort_by(|a, b| {
        b.enabled
            .cmp(&a.enabled)
            .then_with(|| toolbox::compare_numeric(&a.name, &b.name))
    });
    result
}

impl State for DrawWireState {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        cx.out.tool = SchematicTool::Wire;
        cx.set_cursor(Some(CursorShape::Cross));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        if self.is_positioning() || self.pending.is_some() {
            self.abort_positioning(cx, true);
        }
        cx.set_cursor(None);
        true
    }

    fn abort(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        if self.is_positioning() || self.pending.is_some() {
            return self.abort_positioning(cx, true);
        }
        false
    }

    fn key_pressed(&mut self, cx: &mut Cx<'_, '_>, e: KeyEvent) -> bool {
        if e.key == Key::Shift && self.is_positioning() && self.pending.is_none() {
            self.update_preview(cx, false);
            return true;
        }
        false
    }

    fn key_released(&mut self, cx: &mut Cx<'_, '_>, e: KeyEvent) -> bool {
        if e.key == Key::Shift && self.is_positioning() && self.pending.is_none() {
            self.update_preview(cx, true);
            return true;
        }
        false
    }

    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        if self.pending.is_some() {
            return true;
        }
        self.cursor = e.pos;
        if self.is_positioning() {
            self.update_preview(cx, !e.modifiers.shift);
            return true;
        }
        false
    }

    fn left_pressed(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        if self.pending.is_some() {
            return false;
        }
        self.cursor = e.pos;
        let snap = !e.modifiers.shift;
        let interactive = !e.modifiers.control;
        if self.is_positioning() {
            self.add_next_point(cx, snap, interactive, None)
        } else {
            self.start_positioning(cx, snap, interactive, None)
        }
    }

    fn left_double_clicked(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        if self.pending.is_some() {
            return false;
        }
        self.cursor = e.pos;
        if self.is_positioning() {
            return self.add_next_point(cx, !e.modifiers.shift, !e.modifiers.control, None);
        }
        false
    }

    fn right_released(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        if self.pending.is_some() {
            return true;
        }
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
            // Always accept the event while drawing (otherwise the FSM
            // aborts the tool).
            return true;
        }
        false
    }
}
