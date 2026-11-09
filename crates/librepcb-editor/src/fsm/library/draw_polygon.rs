//! Port of libs/librepcb/editor/library/sym/fsm/symboleditorstate_drawpolygonbase.{h,cpp}
//! and libs/librepcb/editor/library/pkg/fsm/packageeditorstate_drawpolygonbase.{h,cpp}
//! (with the derived line, rect, polygon and arc states).

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::geometry::{Path, Polygon, Vertex};
use librepcb_core::types::{Angle, Layer, Length, Point, UnsignedLength, Uuid};
use librepcb_core::utils::toolbox;
use librepcb_i18n::tr;

use super::{
    Cx, ElementHost, LibraryTool, LibraryToolData, PolygonMode, State, format_angle, format_length,
};
use crate::fsm::measure::{snap_candidates_from_circle, snap_candidates_from_path, snap_position};
use crate::fsm::{CursorShape, Key, KeyEvent, Modifiers, PointerEvent, SceneCursor};

/// The polygon drawing state.
#[derive(Debug)]
pub(crate) struct DrawPolygonState {
    mode: PolygonMode,
    layer: Layer,
    line_width: UnsignedLength,
    filled: bool,
    grab_area: bool,
    last_angle: Angle,
    /// Line widths used per layer (packages).
    used_line_widths: BTreeMap<Layer, UnsignedLength>,
    /// The polygon being drawn (its UUID); an undo group is open.
    current: Option<Uuid>,
    arc_center: Point,
    arc_in_second_state: bool,
    last_scene_pos: Point,
    cursor_pos: Point,
    snap_candidates: BTreeSet<Point>,
}

impl DrawPolygonState {
    pub(crate) fn new<H: ElementHost>(mode: PolygonMode) -> Self {
        let (layer, line_width, filled, grab_area) = H::polygon_defaults(mode);
        Self {
            mode,
            layer,
            line_width,
            filled,
            grab_area,
            last_angle: Angle::DEG0,
            used_line_widths: BTreeMap::new(),
            current: None,
            arc_center: Point::ORIGIN,
            arc_in_second_state: false,
            last_scene_pos: Point::ORIGIN,
            cursor_pos: Point::ORIGIN,
            snap_candidates: BTreeSet::new(),
        }
    }

    fn tool(&self) -> LibraryTool {
        match self.mode {
            PolygonMode::Line => LibraryTool::DrawLine,
            PolygonMode::Rect => LibraryTool::DrawRect,
            PolygonMode::Polygon => LibraryTool::DrawPolygon,
            PolygonMode::Arc => LibraryTool::DrawArc,
        }
    }

    fn write_tool_data<H: ElementHost>(&self, cx: &mut Cx<'_, '_, H>) {
        let d = &mut cx.out.tool_data;
        d.layer = self.layer;
        d.available_layers = H::polygon_layers();
        d.line_width = self.line_width;
        d.filled = self.filled;
        d.grab_area = self.grab_area;
        d.angle = self.last_angle;
    }

    fn path<H: ElementHost>(&self, cx: &Cx<'_, '_, H>) -> Option<Path> {
        let uuid = self.current?;
        H::polygons(cx.element(), cx.fpt())?
            .by_uuid(&uuid)
            .map(|p| p.path().clone())
    }

    fn set_path<H: ElementHost>(&self, cx: &mut Cx<'_, '_, H>, path: Path) {
        let Some(uuid) = self.current else {
            return;
        };
        cx.modify(|e, fpt| {
            if let Some(p) = H::polygons_mut(e, fpt).and_then(|l| l.by_uuid_mut(&uuid)) {
                p.set_path(path);
            }
        });
    }

    /// Upstream `start()`.
    fn start<H: ElementHost>(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        if self.mode == PolygonMode::Arc {
            self.last_angle = Angle::DEG0;
            self.arc_center = self.cursor_pos;
            self.arc_in_second_state = false;
        }
        let mut path = Path::new(Vec::new());
        if self.mode == PolygonMode::Arc {
            for _ in 0..3 {
                path.add_vertex(self.cursor_pos, Angle::DEG0);
            }
        } else {
            let count = if self.mode == PolygonMode::Rect { 5 } else { 2 };
            for i in 0..count {
                let angle = if i == 0 { self.last_angle } else { Angle::DEG0 };
                path.add_vertex(self.cursor_pos, angle);
            }
        }
        if !cx.begin(H::add_text("polygon")) {
            return false;
        }
        let uuid = Uuid::new_random();
        let polygon = Polygon::new(
            uuid,
            self.layer,
            self.line_width,
            self.filled,
            self.grab_area,
            path,
        );
        let added = cx.modify(|e, fpt| {
            H::polygons_mut(e, fpt)
                .map(|l| {
                    l.push(polygon);
                })
                .is_some()
        });
        if added != Some(true) {
            cx.abort_group();
            return false;
        }
        self.current = Some(uuid);
        cx.out.selection = [H::polygon_item(uuid)].into_iter().collect();
        self.update_overlay_text(cx);
        self.update_status_bar_message(cx);
        true
    }

    /// Upstream `abort()`.
    fn abort_command<H: ElementHost>(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        if let Some(uuid) = self.current.take() {
            cx.out.selection.remove(&H::polygon_item(uuid));
        }
        let ok = cx.abort_group();
        self.update_overlay_text(cx);
        self.update_status_bar_message(cx);
        self.update_snap_candidates(cx);
        ok
    }

    /// Upstream `addNextSegment()`.
    fn add_next_segment<H: ElementHost>(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        let Some(path) = self.path(cx) else {
            return self.abort_command(cx);
        };
        let mut vertices = path.into_vertices();
        let n = vertices.len();
        // If no line was drawn, abort now.
        let is_empty = match self.mode {
            PolygonMode::Rect => {
                let size = vertices[n - 3].pos - vertices[0].pos;
                size.x == Length::ZERO || size.y == Length::ZERO
            }
            PolygonMode::Arc => {
                if self.arc_in_second_state {
                    vertices[n - 1].pos == vertices[0].pos
                } else {
                    vertices[n - 1].pos == self.arc_center
                }
            }
            _ => vertices[n - 1].pos == vertices[n - 2].pos,
        };
        if is_empty {
            return self.abort_command(cx);
        }

        // If the first part of an arc was drawn, start the second part now.
        if self.mode == PolygonMode::Arc && !self.arc_in_second_state {
            self.arc_in_second_state = true;
            self.update_polygon_path(cx);
            self.update_overlay_text(cx);
            self.update_status_bar_message(cx);
            return true;
        }

        // Commit current polygon segment.
        self.set_path(cx, Path::new(vertices.clone()));
        if !cx.commit() {
            return false;
        }

        // If the polygon is completed, abort now.
        let closed = vertices.first().map(|v| v.pos) == vertices.last().map(|v| v.pos);
        if matches!(self.mode, PolygonMode::Rect | PolygonMode::Arc) || closed {
            return self.abort_command(cx);
        }

        // Add next polygon segment.
        if !cx.begin(H::add_text("polygon")) {
            self.current = None;
            return false;
        }
        if let Some(last) = vertices.last_mut() {
            last.angle = self.last_angle;
        }
        vertices.push(Vertex::new(self.cursor_pos, Angle::DEG0));
        self.set_path(cx, Path::new(vertices));
        self.update_snap_candidates(cx);
        self.update_overlay_text(cx);
        self.update_status_bar_message(cx);
        true
    }

    /// Upstream `updateCursorPosition()`.
    fn update_cursor_position<H: ElementHost>(&mut self, cx: &mut Cx<'_, '_, H>, m: Modifiers) {
        self.cursor_pos = self.last_scene_pos;
        let mut snapped = false;
        if !m.shift && !m.control {
            (self.cursor_pos, snapped) =
                snap_position(self.cursor_pos, cx.grid(), &self.snap_candidates);
        } else if !m.control {
            self.cursor_pos = self.cursor_pos.mapped_to_grid(cx.grid());
        }
        cx.out.view.scene_cursor = Some(SceneCursor {
            pos: self.cursor_pos,
            cross: true,
            circle: snapped,
        });
        if self.current.is_some() {
            self.update_polygon_path(cx);
        }
        self.update_overlay_text(cx);
    }

    /// Upstream `updatePolygonPath()`.
    fn update_polygon_path<H: ElementHost>(&mut self, cx: &mut Cx<'_, '_, H>) {
        let Some(path) = self.path(cx) else {
            return;
        };
        let mut v = path.into_vertices();
        let n = v.len();
        let cursor = self.cursor_pos;
        match self.mode {
            PolygonMode::Rect if n >= 5 => {
                v[n - 4].pos = Point::new(cursor.x, v[n - 5].pos.y);
                v[n - 3].pos = cursor;
                v[n - 2].pos = Point::new(v[n - 5].pos.x, cursor.y);
            }
            PolygonMode::Arc if !self.arc_in_second_state => {
                // Draw 2 arcs with 180° each to result in an accurate 360°
                // circle which helps to place the start point of the arc.
                v = vec![
                    Vertex::new(cursor, Angle::DEG180),
                    Vertex::new(
                        cursor.rotated(Angle::DEG180, self.arc_center),
                        Angle::DEG180,
                    ),
                    Vertex::new(cursor, Angle::DEG0),
                ];
            }
            PolygonMode::Arc => {
                // Place the end point of the arc; the direction is taken
                // from the previous angle.
                let start = v[0].pos;
                let mut angle =
                    toolbox::arc_angle(start, cursor, self.arc_center).mapped_to_180deg();
                if (self.last_angle > Angle::DEG90 && angle < Angle::DEG0)
                    || (self.last_angle < -Angle::DEG90 && angle > Angle::DEG0)
                {
                    angle = angle.inverted();
                }
                v.truncate(1);
                if angle.abs() > Angle::DEG270 {
                    // Two segments to avoid inaccuracy of too large angles.
                    let half = angle / 2;
                    v[0].angle = angle - half;
                    v.push(Vertex::new(start.rotated(half, self.arc_center), half));
                    v.push(Vertex::new(
                        start.rotated(angle, self.arc_center),
                        Angle::DEG0,
                    ));
                } else {
                    v[0].angle = angle;
                    v.push(Vertex::new(
                        start.rotated(angle, self.arc_center),
                        Angle::DEG0,
                    ));
                }
                self.last_angle = angle;
            }
            _ if n >= 2 => {
                v[n - 1].pos = cursor;
            }
            _ => {}
        }
        self.set_path(cx, Path::new(v));
    }

    /// Upstream `updateOverlayText()` (plain text).
    fn update_overlay_text<H: ElementHost>(&self, cx: &mut Cx<'_, '_, H>) {
        let unit = cx.unit();
        let vertices = self.path(cx).map(Path::into_vertices).unwrap_or_default();
        let n = vertices.len();
        let cursor = self.cursor_pos;
        let fl = |name: &str, v: Length| format_length(unit, name, v);
        let lines: Vec<String> = match self.mode {
            PolygonMode::Line | PolygonMode::Polygon => {
                let p0 = if n >= 2 { vertices[n - 2].pos } else { cursor };
                let p1 = if n >= 2 { vertices[n - 1].pos } else { cursor };
                let diff = p1 - p0;
                let (dx, dy) = diff.to_mm();
                let angle = Angle::from_rad(libm::atan2(dy, dx)).unwrap_or(Angle::DEG0);
                vec![
                    fl("X0", p0.x),
                    fl("Y0", p0.y),
                    fl("X1", p1.x),
                    fl("Y1", p1.y),
                    String::new(),
                    fl("Δ", *diff.length()),
                    format_angle(unit, "∠", angle),
                ]
            }
            PolygonMode::Rect => {
                let p0 = if n >= 3 { vertices[0].pos } else { cursor };
                let p1 = if n >= 3 { vertices[2].pos } else { cursor };
                vec![
                    fl("X0", p0.x),
                    fl("Y0", p0.y),
                    fl("X1", p1.x),
                    fl("Y1", p1.y),
                    String::new(),
                    fl("ΔX", (p1.x - p0.x).abs()),
                    fl("ΔY", (p1.y - p0.y).abs()),
                ]
            }
            PolygonMode::Arc => {
                let center = if n >= 2 { self.arc_center } else { cursor };
                let p0 = vertices.first().map_or(cursor, |v| v.pos);
                let p1 = vertices.last().map_or(cursor, |v| v.pos);
                let radius = if n >= 2 {
                    *(p0 - self.arc_center).length()
                } else {
                    Length::ZERO
                };
                let angle = vertices.iter().fold(Angle::DEG0, |acc, v| acc + v.angle);
                vec![
                    fl("X·", center.x),
                    fl("Y·", center.y),
                    fl("X0", p0.x),
                    fl("Y0", p0.y),
                    fl("X1", p1.x),
                    fl("Y1", p1.y),
                    String::new(),
                    fl("r", radius),
                    fl("⌀", radius * 2),
                    format_angle(unit, "∠", angle),
                ]
            }
        };
        cx.out.view.info_box = lines.join("\n");
    }

    /// Upstream `updateStatusBarMessage()`.
    fn update_status_bar_message<H: ElementHost>(&self, cx: &mut Cx<'_, '_, H>) {
        let ctx = if H::IS_FOOTPRINT {
            "librepcb::editor::PackageEditorState_DrawPolygonBase"
        } else {
            "librepcb::editor::SymbolEditorState_DrawPolygonBase"
        };
        let note = format!(
            " {}",
            tr!(
                ctx,
                "(press {0} to disable snap, {1} to abort)",
                "Shift/Ctrl",
                tr!(ctx, "right click")
            )
        );
        let active = self.current.is_some();
        let msg = match self.mode {
            PolygonMode::Rect if !active => "Click to specify the first edge",
            PolygonMode::Rect => "Click to specify the second edge",
            PolygonMode::Arc if !active => "Click to specify the arc center",
            PolygonMode::Arc if !self.arc_in_second_state => "Click to specify the start point",
            PolygonMode::Arc => "Click to specify the end point",
            _ if !active => "Click to specify the first point",
            _ => "Click to specify the next point",
        };
        cx.out.view.set_status(tr!(ctx, msg) + &note, None);
    }

    /// Upstream `updateSnapCandidates()`: vertices of polygons and circles
    /// on the same layer.
    fn update_snap_candidates<H: ElementHost>(&mut self, cx: &Cx<'_, '_, H>) {
        self.snap_candidates.clear();
        if let Some(polygons) = H::polygons(cx.element(), cx.fpt()) {
            for p in polygons.iter() {
                if p.layer() == self.layer {
                    self.snap_candidates
                        .extend(snap_candidates_from_path(p.path()));
                }
            }
        }
        if let Some(circles) = H::circles(cx.element(), cx.fpt()) {
            for c in circles.iter() {
                if c.layer() == self.layer {
                    self.snap_candidates
                        .extend(snap_candidates_from_circle(c.center(), c.diameter()));
                }
            }
        }
    }
}

impl<H: ElementHost> State<H> for DrawPolygonState {
    fn entry(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        self.last_scene_pos = cx.cursor_pos().mapped_to_grid(cx.grid());
        self.update_snap_candidates(cx);
        self.update_cursor_position(cx, Modifiers::NONE);
        self.update_status_bar_message(cx);
        cx.out.tool = self.tool();
        self.write_tool_data(cx);
        cx.set_cursor(Some(CursorShape::Cross));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        if !self.abort_command(cx) {
            return false;
        }
        cx.set_cursor(None);
        cx.out.view.scene_cursor = None;
        cx.out.view.info_box.clear();
        cx.out.view.set_status(String::new(), None);
        true
    }

    fn abort(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        if self.current.is_some() {
            self.abort_command(cx)
        } else {
            false
        }
    }

    fn key_pressed(&mut self, cx: &mut Cx<'_, '_, H>, e: KeyEvent) -> bool {
        if matches!(e.key, Key::Shift | Key::Control) {
            self.update_cursor_position(cx, e.modifiers);
            return true;
        }
        false
    }

    fn key_released(&mut self, cx: &mut Cx<'_, '_, H>, e: KeyEvent) -> bool {
        self.key_pressed(cx, e)
    }

    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_, H>, e: PointerEvent) -> bool {
        self.last_scene_pos = e.pos;
        self.update_cursor_position(cx, e.modifiers);
        true
    }

    fn left_pressed(&mut self, cx: &mut Cx<'_, '_, H>, e: PointerEvent) -> bool {
        self.last_scene_pos = e.pos;
        self.update_cursor_position(cx, e.modifiers);
        if self.current.is_some() {
            self.add_next_segment(cx)
        } else {
            self.start(cx)
        }
    }

    fn left_double_clicked(&mut self, cx: &mut Cx<'_, '_, H>, e: PointerEvent) -> bool {
        self.left_pressed(cx, e)
    }

    fn tool_data_changed(&mut self, cx: &mut Cx<'_, '_, H>, old: &LibraryToolData) {
        let d = cx.out.tool_data.clone();
        if d.layer != old.layer {
            self.layer = d.layer;
            if H::IS_FOOTPRINT {
                // Upstream PackageEditorState_DrawPolygonBase::setLayer().
                self.line_width = if let Some(w) = self.used_line_widths.get(&d.layer) {
                    *w
                } else if d.layer.polygons_represent_areas() {
                    UnsignedLength::default()
                } else {
                    super::len(200_000)
                };
            }
            self.update_snap_candidates(cx);
        }
        if d.line_width != old.line_width {
            self.line_width = d.line_width;
            if H::IS_FOOTPRINT {
                self.used_line_widths.insert(self.layer, d.line_width);
            }
        }
        self.filled = d.filled;
        self.grab_area = d.grab_area;
        if d.angle != old.angle {
            self.last_angle = d.angle;
            if let Some(path) = self.path(cx) {
                let mut v = path.into_vertices();
                let n = v.len();
                if n > 1 {
                    v[n - 2].angle = self.last_angle;
                    self.set_path(cx, Path::new(v));
                }
            }
        }
        if let Some(uuid) = self.current {
            let (layer, width, filled, grab) =
                (self.layer, self.line_width, self.filled, self.grab_area);
            cx.modify(|e, fpt| {
                if let Some(p) = H::polygons_mut(e, fpt).and_then(|l| l.by_uuid_mut(&uuid)) {
                    p.set_layer(layer);
                    p.set_line_width(width);
                    p.set_is_filled(filled);
                    p.set_is_grab_area(grab);
                }
            });
        }
        self.write_tool_data(cx);
    }
}
