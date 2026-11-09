//! The polygon, plane and zone tools: port of
//! libs/librepcb/editor/project/board/fsm/boardeditorstate_drawpolygon,
//! boardeditorstate_drawplane and boardeditorstate_drawzone (`.{h,cpp}`).
//!
//! The first click adds the item with two vertices at the position; the
//! last vertex follows the cursor (live preview); every further click
//! commits the group (vertex by vertex undo, like upstream) and adds a new
//! vertex. Clicking the last position again (or closing a polygon)
//! finishes the item.

use std::collections::BTreeSet;

use librepcb_core::geometry::{Path, Vertex, ZoneRules};
use librepcb_core::project::board::{
    BoardItem, BoardPlane, BoardPolygonData, BoardZoneData, PlaneConnectStyle,
};
use librepcb_core::project::{BoardMutation, Mutation, NetSignalId};
use librepcb_core::types::{Angle, Layer, Length, Point, PositiveLength, UnsignedLength, Uuid};
use librepcb_i18n::tr;

use super::add_via::sorted_nets;
use super::context::Cx;
use super::output::{BoardToolData, ToolNet, ToolSetting};
use super::{BoardFsmInput, State};
use crate::error::Result;
use crate::fsm::CursorShape;

/// The item being drawn.
#[derive(Debug, Clone)]
struct Drawing {
    uuid: Uuid,
    path: Path,
    /// Group length before the (preview) outline update.
    mark: usize,
    last_pos: Point,
}

/// Layers allowed for board polygons and texts (upstream
/// `getAllowedGeometryLayers()`).
pub(super) fn geometry_layers(cx: &Cx<'_, '_>) -> Vec<Layer> {
    let mut layers: BTreeSet<Layer> = [
        Layer::BOARD_SHEET_FRAMES,
        Layer::BOARD_OUTLINES,
        Layer::BOARD_CUTOUTS,
        Layer::BOARD_PLATED_CUTOUTS,
        Layer::BOARD_MEASURES,
        Layer::BOARD_ALIGNMENT,
        Layer::BOARD_DOCUMENTATION,
        Layer::BOARD_COMMENTS,
        Layer::BOARD_GUIDE,
        Layer::TOP_NAMES,
        Layer::TOP_VALUES,
        Layer::TOP_LEGEND,
        Layer::TOP_DOCUMENTATION,
        Layer::TOP_COPPER,
        Layer::TOP_GLUE,
        Layer::TOP_SOLDER_PASTE,
        Layer::TOP_STOP_MASK,
        Layer::BOT_NAMES,
        Layer::BOT_VALUES,
        Layer::BOT_LEGEND,
        Layer::BOT_DOCUMENTATION,
        Layer::BOT_COPPER,
        Layer::BOT_GLUE,
        Layer::BOT_SOLDER_PASTE,
        Layer::BOT_STOP_MASK,
    ]
    .into_iter()
    .collect();
    layers.extend(cx.copper_layers());
    layers.into_iter().collect()
}

/// The polygon tool (upstream `BoardEditorState_DrawPolygon`).
#[derive(Debug)]
pub(super) struct DrawPolygonState {
    layer: Layer,
    line_width: UnsignedLength,
    filled: bool,
    angle: Angle,
    drawing: Option<Drawing>,
}

impl Default for DrawPolygonState {
    fn default() -> Self {
        Self {
            layer: Layer::BOARD_OUTLINES,
            line_width: UnsignedLength::ZERO,
            filled: false,
            angle: Angle::DEG0,
            drawing: None,
        }
    }
}

impl DrawPolygonState {
    fn polygon(&self, d: &Drawing) -> BoardPolygonData {
        BoardPolygonData::new(
            d.uuid,
            self.layer,
            self.line_width,
            d.path.clone(),
            self.filled,
            self.filled,
            false,
        )
    }

    fn preview(&mut self, cx: &mut Cx<'_, '_>) {
        let Some(d) = self.drawing.clone() else {
            return;
        };
        let board = cx.board_id();
        let item = BoardItem::Polygon(self.polygon(&d));
        if let Err(e) = cx.preview(
            d.mark,
            vec![Mutation::Board(BoardMutation::UpdateItem { board, item })],
        ) {
            cx.error(e);
            self.abort(cx);
        }
    }

    fn start(&mut self, cx: &mut Cx<'_, '_>, pos: Point) {
        if let Err(e) = cx.begin(tr!(
            "librepcb::editor::BoardEditorState_DrawPolygon",
            "Draw board polygon"
        )) {
            cx.error(e);
            return;
        }
        let d = Drawing {
            uuid: Uuid::new_random(),
            path: Path::new(vec![Vertex::new(pos, self.angle), Vertex::at(pos)]),
            mark: 0,
            last_pos: pos,
        };
        let board = cx.board_id();
        let item = BoardItem::Polygon(self.polygon(&d));
        match cx.apply(
            String::new(),
            vec![Mutation::Board(BoardMutation::AddItem { board, item })],
        ) {
            Ok(()) => {
                self.drawing = Some(Drawing {
                    mark: cx.group_len(),
                    ..d
                });
            }
            Err(e) => {
                cx.error(e);
                cx.abort();
            }
        }
    }

    fn add_segment(&mut self, cx: &mut Cx<'_, '_>, pos: Point) {
        let Some(d) = self.drawing.clone() else {
            return;
        };
        if pos == d.last_pos {
            self.abort(cx);
            return;
        }
        // Finish the group to allow undoing segment by segment.
        if let Err(e) = cx.commit() {
            cx.error(e);
            self.abort(cx);
            return;
        }
        if d.path.is_closed() {
            self.drawing = None;
            return;
        }
        if let Err(e) = cx.begin(tr!(
            "librepcb::editor::BoardEditorState_DrawPolygon",
            "Draw board polygon"
        )) {
            cx.error(e);
            self.drawing = None;
            return;
        }
        let mut vertices = d.path.vertices().to_vec();
        if let Some(last) = vertices.last_mut() {
            last.angle = self.angle;
        }
        vertices.push(Vertex::at(pos));
        self.drawing = Some(Drawing {
            path: Path::new(vertices),
            mark: cx.group_len(),
            last_pos: pos,
            ..d
        });
        self.preview(cx);
    }

    fn abort(&mut self, cx: &mut Cx<'_, '_>) {
        self.drawing = None;
        if cx.is_group_active() {
            cx.abort();
        }
    }
}

impl State for DrawPolygonState {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        cx.set_cursor(Some(CursorShape::Cross));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.abort(cx);
        cx.set_cursor(None);
        true
    }

    fn process(&mut self, cx: &mut Cx<'_, '_>, input: &BoardFsmInput) -> bool {
        match input {
            BoardFsmInput::Abort if self.drawing.is_some() => {
                self.abort(cx);
                true
            }
            BoardFsmInput::PointerMoved(e) => {
                let pos = e.pos.mapped_to_grid(cx.grid());
                let Some(d) = self.drawing.as_mut() else {
                    return false;
                };
                if let Some(last) = d.path.vertices_mut().last_mut() {
                    last.pos = pos;
                }
                self.preview(cx);
                true
            }
            BoardFsmInput::LeftPressed(e) | BoardFsmInput::LeftDoubleClicked(e) => {
                let pos = e.pos.mapped_to_grid(cx.grid());
                if self.drawing.is_some() {
                    self.add_segment(cx, pos);
                } else {
                    self.start(cx, pos);
                }
                true
            }
            BoardFsmInput::ToolSetting(setting) => {
                match setting {
                    ToolSetting::Layer(l) => self.layer = *l,
                    ToolSetting::LineWidth(w) => self.line_width = *w,
                    ToolSetting::Filled(f) => self.filled = *f,
                    ToolSetting::Angle(a) => {
                        self.angle = *a;
                        if let Some(d) = self.drawing.as_mut() {
                            let n = d.path.vertices().len();
                            if n > 1 {
                                d.path.vertices_mut()[n - 2].angle = *a;
                            }
                        }
                    }
                    _ => return false,
                }
                self.preview(cx);
                true
            }
            _ => false,
        }
    }

    fn tool_data(&self, cx: &Cx<'_, '_>) -> BoardToolData {
        BoardToolData {
            layers: geometry_layers(cx),
            layer: Some(self.layer),
            line_width: Some(self.line_width),
            angle: self.angle,
            filled: self.filled,
            ..Default::default()
        }
    }
}

/// The plane tool (upstream `BoardEditorState_DrawPlane`).
#[derive(Debug)]
pub(super) struct DrawPlaneState {
    auto_net: bool,
    net: Option<NetSignalId>,
    layer: Layer,
    drawing: Option<Drawing>,
}

impl Default for DrawPlaneState {
    fn default() -> Self {
        Self {
            auto_net: true,
            net: None,
            layer: Layer::TOP_COPPER,
            drawing: None,
        }
    }
}

impl DrawPlaneState {
    /// The plane with the settings of upstream `updatePlaneSettings()`.
    fn plane(&self, cx: &Cx<'_, '_>, uuid: Uuid, outline: Path) -> BoardPlane {
        let mut plane = BoardPlane::new(uuid, self.layer, self.net, outline);
        plane.set_connect_style(PlaneConnectStyle::ThermalRelief);
        let p = cx.project();
        let Ok(board) = cx.board() else {
            return plane;
        };
        let nc = self
            .net
            .and_then(|n| p.circuit().net_signal(n))
            .and_then(|n| p.circuit().net_class(n.net_class()));
        let drc = &board.settings().drc_settings;
        let ul = |nm: i64| UnsignedLength::new(Length::new(nm)).unwrap_or_default();
        // A reasonable minimum width: the default trace width, at most
        // 0.2 mm, at least the DRC minimum copper width.
        let default_width = nc
            .and_then(|c| c.default_trace_width())
            .unwrap_or_else(|| board.design_rules().default_trace_width());
        let mut min_width = UnsignedLength::from(default_width).min(ul(200_000));
        min_width = min_width.max(drc.min_copper_width());
        plane.set_min_width(min_width);
        plane.set_thermal_spoke_width(
            PositiveLength::new(*min_width)
                .unwrap_or_else(|_| PositiveLength::new(Length::new(200_000)).expect("positive")),
        );
        let clearance = |a: UnsignedLength, b: UnsignedLength, fallback: i64| {
            let v = a.max(b);
            if *v > Length::ZERO { v } else { ul(fallback) }
        };
        let nc_clearance = nc
            .map(|c| c.min_copper_copper_clearance())
            .unwrap_or(UnsignedLength::ZERO);
        plane.set_min_clearance_to_copper(clearance(
            nc_clearance,
            drc.min_copper_copper_clearance(),
            250_000,
        ));
        plane.set_min_clearance_to_board(clearance(
            nc_clearance,
            drc.min_copper_board_clearance(),
            300_000,
        ));
        plane.set_min_clearance_to_npth(clearance(
            nc_clearance,
            drc.min_copper_npth_clearance(),
            300_000,
        ));
        plane
    }

    fn preview(&mut self, cx: &mut Cx<'_, '_>) {
        let Some(d) = self.drawing.clone() else {
            return;
        };
        let board = cx.board_id();
        let plane = self.plane(cx, d.uuid, d.path.clone());
        if let Err(e) = cx.preview(
            d.mark,
            vec![Mutation::Board(BoardMutation::UpdatePlane { board, plane })],
        ) {
            cx.error(e);
            self.abort(cx);
        }
    }

    /// Upstream `startAddPlane()`; `None`: the automatic outline, committed
    /// immediately.
    fn start(&mut self, cx: &mut Cx<'_, '_>, pos: Option<Point>) -> bool {
        if let Err(e) = cx.begin(tr!(
            "librepcb::editor::BoardEditorState_DrawPlane",
            "Draw Board Plane"
        )) {
            cx.error(e);
            return false;
        }
        let result = (|| -> Result<()> {
            let path = match pos {
                Some(p) => Path::new(vec![Vertex::at(p), Vertex::at(p)]),
                None => auto_plane_outline(cx)?,
            };
            let uuid = Uuid::new_random();
            let board = cx.board_id();
            let plane = self.plane(cx, uuid, path.clone());
            cx.apply(
                String::new(),
                vec![Mutation::Board(BoardMutation::AddPlane { board, plane })],
            )?;
            if pos.is_none() {
                cx.commit()?;
            } else {
                self.drawing = Some(Drawing {
                    uuid,
                    last_pos: path.vertices().last().map(|v| v.pos).unwrap_or_default(),
                    path,
                    mark: cx.group_len(),
                });
            }
            Ok(())
        })();
        match result {
            Ok(()) => true,
            Err(e) => {
                cx.error(e);
                self.abort(cx);
                false
            }
        }
    }

    fn add_segment(&mut self, cx: &mut Cx<'_, '_>, pos: Point) {
        let Some(d) = self.drawing.clone() else {
            return;
        };
        if pos == d.last_pos {
            self.abort(cx);
            return;
        }
        let mut mark = d.mark;
        if d.path.vertices().len() > 2 {
            if let Err(e) = cx.commit() {
                cx.error(e);
                self.abort(cx);
                return;
            }
            if let Err(e) = cx.begin(tr!(
                "librepcb::editor::BoardEditorState_DrawPlane",
                "Draw board plane"
            )) {
                cx.error(e);
                self.drawing = None;
                return;
            }
            mark = cx.group_len();
        }
        let mut path = d.path.clone();
        path.add_vertex(pos, Angle::DEG0);
        self.drawing = Some(Drawing {
            path,
            mark,
            last_pos: pos,
            ..d
        });
        self.preview(cx);
    }

    fn abort(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.drawing = None;
        if cx.is_group_active() {
            cx.abort();
        }
        true
    }

    /// Upstream `autoAdd()`.
    fn auto_add(&mut self, cx: &mut Cx<'_, '_>) {
        self.abort(cx);
        if self.start(cx, None) {
            // Continue with the next layer; leave after the bottom layer.
            let copper = cx.copper_layers();
            let next = Layer::copper(self.layer.copper_number() + 1)
                .filter(|l| copper.contains(l))
                .unwrap_or(Layer::BOT_COPPER);
            if next != self.layer {
                self.layer = next;
            } else {
                cx.leave_requested = true;
            }
        }
    }
}

/// The automatic plane outline (upstream `determineAutoPlaneOutline()`).
fn auto_plane_outline(cx: &Cx<'_, '_>) -> Result<Path> {
    crate::commands::board::auto_plane_outline(cx.project(), cx.board_id(), *cx.grid())
}

impl State for DrawPlaneState {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        // The net with the most elements.
        let exists = self
            .net
            .is_some_and(|n| cx.project().circuit().net_signal(n).is_some());
        if self.auto_net || (self.net.is_some() && !exists) {
            self.net = cx.project().net_signal_with_most_elements();
            self.auto_net = true;
        }
        cx.set_cursor(Some(CursorShape::Cross));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.abort(cx);
        cx.set_cursor(None);
        true
    }

    fn process(&mut self, cx: &mut Cx<'_, '_>, input: &BoardFsmInput) -> bool {
        match input {
            BoardFsmInput::Abort if self.drawing.is_some() => self.abort(cx),
            BoardFsmInput::AutoAddPlane => {
                self.auto_add(cx);
                true
            }
            BoardFsmInput::PointerMoved(e) => {
                let pos = e.pos.mapped_to_grid(cx.grid());
                let Some(d) = self.drawing.as_mut() else {
                    return false;
                };
                if let Some(last) = d.path.vertices_mut().last_mut() {
                    last.pos = pos;
                }
                self.preview(cx);
                true
            }
            BoardFsmInput::LeftPressed(e) | BoardFsmInput::LeftDoubleClicked(e) => {
                let pos = e.pos.mapped_to_grid(cx.grid());
                if self.drawing.is_some() {
                    self.add_segment(cx, pos);
                } else {
                    self.start(cx, Some(pos));
                }
                true
            }
            BoardFsmInput::ToolSetting(setting) => {
                match setting {
                    ToolSetting::Net(ToolNet { net, .. }) => {
                        self.net = *net;
                        self.auto_net = false;
                    }
                    ToolSetting::Layer(l) => self.layer = *l,
                    _ => return false,
                }
                self.preview(cx);
                true
            }
            _ => false,
        }
    }

    fn tool_data(&self, cx: &Cx<'_, '_>) -> BoardToolData {
        BoardToolData {
            nets: sorted_nets(cx),
            net: ToolNet {
                auto: false,
                net: self.net,
            },
            layers: cx.copper_layers(),
            layer: Some(self.layer),
            ..Default::default()
        }
    }
}

/// The zone tool (upstream `BoardEditorState_DrawZone`).
#[derive(Debug)]
pub(super) struct DrawZoneState {
    layers: BTreeSet<Layer>,
    rules: ZoneRules,
    angle: Angle,
    drawing: Option<Drawing>,
}

impl Default for DrawZoneState {
    fn default() -> Self {
        Self {
            layers: BTreeSet::from([Layer::TOP_COPPER]),
            rules: ZoneRules::all(),
            angle: Angle::DEG0,
            drawing: None,
        }
    }
}

impl DrawZoneState {
    fn zone(&self, d: &Drawing) -> Result<BoardZoneData> {
        Ok(BoardZoneData::new(
            d.uuid,
            self.layers.clone(),
            self.rules,
            d.path.clone(),
            false,
        )?)
    }

    fn preview(&mut self, cx: &mut Cx<'_, '_>) {
        let Some(d) = self.drawing.clone() else {
            return;
        };
        let board = cx.board_id();
        let result = self.zone(&d).and_then(|z| {
            cx.preview(
                d.mark,
                vec![Mutation::Board(BoardMutation::UpdateItem {
                    board,
                    item: BoardItem::Zone(z),
                })],
            )
        });
        if let Err(e) = result {
            cx.error(e);
            self.abort(cx);
        }
    }

    fn start(&mut self, cx: &mut Cx<'_, '_>, pos: Point) {
        if let Err(e) = cx.begin(tr!(
            "librepcb::editor::BoardEditorState_DrawZone",
            "Draw board zone"
        )) {
            cx.error(e);
            return;
        }
        let d = Drawing {
            uuid: Uuid::new_random(),
            path: Path::new(vec![Vertex::new(pos, self.angle), Vertex::at(pos)]),
            mark: 0,
            last_pos: pos,
        };
        let board = cx.board_id();
        let result = self.zone(&d).and_then(|z| {
            cx.apply(
                String::new(),
                vec![Mutation::Board(BoardMutation::AddItem {
                    board,
                    item: BoardItem::Zone(z),
                })],
            )
        });
        match result {
            Ok(()) => {
                self.drawing = Some(Drawing {
                    mark: cx.group_len(),
                    ..d
                })
            }
            Err(e) => {
                cx.error(e);
                cx.abort();
            }
        }
    }

    fn add_segment(&mut self, cx: &mut Cx<'_, '_>, pos: Point) {
        let Some(d) = self.drawing.clone() else {
            return;
        };
        let mut path = d.path.clone();
        let mut check = path.clone();
        check.clean();
        check.close();
        let has_area = !check.is_zero_area();
        let finish = path.is_closed() || pos == d.last_pos;
        if finish && !has_area {
            self.abort(cx);
            return;
        }
        let mut mark = d.mark;
        if has_area {
            path.clean();
            path.open();
            self.drawing = Some(Drawing {
                path: path.clone(),
                ..d.clone()
            });
            self.preview(cx);
            if let Err(e) = cx.commit() {
                cx.error(e);
                self.abort(cx);
                return;
            }
            if let Err(e) = cx.begin(tr!(
                "librepcb::editor::BoardEditorState_DrawZone",
                "Draw Board Zone"
            )) {
                cx.error(e);
                self.drawing = None;
                return;
            }
            mark = cx.group_len();
        }
        if finish {
            self.abort(cx);
            return;
        }
        let mut vertices = path.vertices().to_vec();
        if let Some(last) = vertices.last_mut() {
            last.angle = self.angle;
        }
        vertices.push(Vertex::at(pos));
        self.drawing = Some(Drawing {
            path: Path::new(vertices),
            mark,
            last_pos: pos,
            ..d
        });
        self.preview(cx);
    }

    fn abort(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.drawing = None;
        if cx.is_group_active() {
            cx.abort();
        }
        true
    }
}

impl State for DrawZoneState {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        cx.set_cursor(Some(CursorShape::Cross));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.abort(cx);
        cx.set_cursor(None);
        true
    }

    fn process(&mut self, cx: &mut Cx<'_, '_>, input: &BoardFsmInput) -> bool {
        match input {
            BoardFsmInput::Abort if self.drawing.is_some() => self.abort(cx),
            BoardFsmInput::PointerMoved(e) => {
                let pos = e.pos.mapped_to_grid(cx.grid());
                let Some(d) = self.drawing.as_mut() else {
                    return false;
                };
                if let Some(last) = d.path.vertices_mut().last_mut() {
                    last.pos = pos;
                }
                self.preview(cx);
                true
            }
            BoardFsmInput::LeftPressed(e) | BoardFsmInput::LeftDoubleClicked(e) => {
                let pos = e.pos.mapped_to_grid(cx.grid());
                if self.drawing.is_some() {
                    self.add_segment(cx, pos);
                } else {
                    self.start(cx, pos);
                }
                true
            }
            BoardFsmInput::ToolSetting(setting) => {
                match setting {
                    ToolSetting::Layer(l) => self.layers = BTreeSet::from([*l]),
                    ToolSetting::ZoneRules(r) => self.rules = *r,
                    ToolSetting::Angle(a) => {
                        self.angle = *a;
                        if let Some(d) = self.drawing.as_mut() {
                            let n = d.path.vertices().len();
                            if n > 1 {
                                d.path.vertices_mut()[n - 2].angle = *a;
                            }
                        }
                    }
                    _ => return false,
                }
                self.preview(cx);
                true
            }
            _ => false,
        }
    }

    fn tool_data(&self, cx: &Cx<'_, '_>) -> BoardToolData {
        BoardToolData {
            layers: cx.copper_layers(),
            layer: self.layers.iter().next().copied(),
            angle: self.angle,
            zone_rules: self.rules,
            ..Default::default()
        }
    }
}
