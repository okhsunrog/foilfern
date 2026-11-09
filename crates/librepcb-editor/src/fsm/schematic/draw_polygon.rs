//! Port of libs/librepcb/editor/project/schematic/fsm/schematiceditorstate_drawpolygon.{h,cpp}.

use librepcb_core::geometry::{Path, Polygon, Vertex};
use librepcb_core::project::{Mutation, SchematicMutation};
use librepcb_core::types::{Angle, Layer, Point, Uuid};
use librepcb_i18n::tr;

use super::add_text::allowed_geometry_layers;
use super::{Cx, SchematicTool, State};
use crate::commands::ApplyMutations;
use crate::fsm::{CursorShape, PointerEvent};

/// The draw polygon state.
#[derive(Debug, Default)]
pub(crate) struct DrawPolygonState {
    initialized: bool,
    /// The polygon being drawn (UUID) and its path at the start of the
    /// current segment.
    current: Option<(Uuid, Path)>,
    last_segment_pos: Point,
}

impl DrawPolygonState {
    fn polygon(cx: &Cx<'_, '_>, uuid: Uuid, path: Path) -> Polygon {
        let data = &cx.out.tool_data;
        Polygon::new(
            uuid,
            data.layer,
            data.line_width,
            data.filled,
            data.filled,
            path,
        )
    }

    /// Replaces the polygon in the open group with `path`.
    fn set_path(&self, cx: &mut Cx<'_, '_>, path: Path) {
        let Some((uuid, _)) = &self.current else {
            return;
        };
        cx.ctx.editor.rollback_group_to(0);
        let polygon = Self::polygon(cx, *uuid, path);
        let exists = cx.sch().is_some_and(|s| s.polygons().contains_key(uuid));
        let m = if exists {
            SchematicMutation::UpdatePolygon {
                schematic: cx.schematic,
                polygon,
            }
        } else {
            SchematicMutation::AddPolygon {
                schematic: cx.schematic,
                polygon,
            }
        };
        if let Err(e) = cx.ctx.editor.execute(ApplyMutations {
            text: None,
            mutations: vec![Mutation::Schematic(m)],
        }) {
            log::warn!("Failed to update the polygon: {e}");
        }
    }

    fn begin(cx: &mut Cx<'_, '_>) -> bool {
        match cx.ctx.editor.begin_group(tr!(
            "librepcb::editor::SchematicEditorState_DrawPolygon",
            "Draw schematic polygon"
        )) {
            Ok(()) => true,
            Err(e) => {
                cx.error(e);
                false
            }
        }
    }

    /// Upstream `startAddPolygon()`.
    fn start_add_polygon(&mut self, cx: &mut Cx<'_, '_>, pos: Point) -> bool {
        if !Self::begin(cx) {
            return false;
        }
        let path = Path::new(vec![
            Vertex::new(pos, Angle::DEG0),
            Vertex::new(pos, Angle::DEG0),
        ]);
        self.current = Some((Uuid::new_random(), Path::new(Vec::new())));
        self.set_path(cx, path);
        self.last_segment_pos = pos;
        true
    }

    /// Upstream `addSegment()`.
    fn add_segment(&mut self, cx: &mut Cx<'_, '_>, pos: Point) -> bool {
        // Abort if no segment drawn.
        if pos == self.last_segment_pos {
            self.abort_command(cx);
            return false;
        }
        let Some((uuid, _)) = self.current.clone() else {
            return false;
        };
        // Commit the current segment to allow undoing segment by segment.
        if let Err(e) = cx.ctx.editor.commit_group() {
            cx.error(e);
            self.abort_command(cx);
            return false;
        }
        let Some(path) = cx
            .sch()
            .and_then(|s| s.polygons().get(&uuid))
            .map(|p| p.path().clone())
        else {
            self.current = None;
            return false;
        };
        // If the polygon is now closed, stop here.
        if path.is_closed() {
            self.current = None;
            return true;
        }
        if !Self::begin(cx) {
            self.current = None;
            return false;
        }
        let mut new_path = path.clone();
        new_path.add_vertex(pos, Angle::DEG0);
        self.current = Some((uuid, path));
        self.set_path(cx, new_path);
        self.last_segment_pos = pos;
        true
    }

    /// Upstream `updateLastVertexPosition()`.
    fn update_last_vertex(&mut self, cx: &mut Cx<'_, '_>, pos: Point) -> bool {
        let Some(path) = self
            .current
            .as_ref()
            .and_then(|(uuid, _)| cx.sch()?.polygons().get(uuid))
            .map(|p| p.path().clone())
        else {
            return false;
        };
        let mut vertices = path.into_vertices();
        if let Some(last) = vertices.last_mut() {
            last.pos = pos;
        }
        self.set_path(cx, Path::new(vertices));
        true
    }

    fn abort_command(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.current = None;
        if cx.ctx.editor.undo_stack().is_group_active()
            && let Err(e) = cx.ctx.editor.abort_group()
        {
            cx.error(e);
            return false;
        }
        true
    }

    /// Layer, line width or fill changed in the tool bar.
    pub fn properties_changed(&mut self, cx: &mut Cx<'_, '_>) {
        let path = self
            .current
            .as_ref()
            .and_then(|(uuid, _)| cx.sch()?.polygons().get(uuid))
            .map(|p| p.path().clone());
        if let Some(path) = path {
            self.set_path(cx, path);
        }
    }
}

impl State for DrawPolygonState {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        if !self.initialized {
            // Upstream defaults: guide layer, 0.3 mm, not filled.
            self.initialized = true;
            cx.out.tool_data.layer = Layer::SCHEMATIC_GUIDE;
            cx.out.tool_data.filled = false;
        }
        cx.out.tool_data.available_layers = allowed_geometry_layers();
        cx.out.tool = SchematicTool::Polygon;
        cx.set_cursor(Some(CursorShape::Cross));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        if !self.abort_command(cx) {
            return false;
        }
        cx.set_cursor(None);
        true
    }

    fn abort(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        if self.current.is_some() {
            // Just finish the current polygon, not the tool.
            return self.abort_command(cx);
        }
        false
    }

    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        let pos = e.pos.mapped_to_grid(cx.grid());
        self.update_last_vertex(cx, pos)
    }

    fn left_pressed(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        let pos = e.pos.mapped_to_grid(cx.grid());
        if self.current.is_some() {
            self.add_segment(cx, pos);
        } else {
            self.start_add_polygon(cx, pos);
        }
        true
    }

    fn left_double_clicked(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        self.left_pressed(cx, e)
    }
}
