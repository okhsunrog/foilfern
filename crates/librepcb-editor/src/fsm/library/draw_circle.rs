//! Port of libs/librepcb/editor/library/sym/fsm/symboleditorstate_drawcircle.{h,cpp}
//! and libs/librepcb/editor/library/pkg/fsm/packageeditorstate_drawcircle.{h,cpp}.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::geometry::Circle;
use librepcb_core::types::{Layer, Length, Point, PositiveLength, UnsignedLength, Uuid};

use super::{Cx, ElementHost, LibraryTool, LibraryToolData, State};
use crate::fsm::measure::snap_position;
use crate::fsm::{CursorShape, PointerEvent};

/// The circle drawing state.
#[derive(Debug)]
pub(crate) struct DrawCircleState {
    layer: Layer,
    line_width: UnsignedLength,
    filled: bool,
    grab_area: bool,
    used_line_widths: BTreeMap<Layer, UnsignedLength>,
    /// The circle being drawn; an undo group is open.
    current: Option<(Uuid, Point)>,
    snap_candidates: BTreeSet<Point>,
}

impl DrawCircleState {
    pub(crate) fn new<H: ElementHost>() -> Self {
        let (layer, line_width, filled, grab_area) = H::circle_defaults();
        Self {
            layer,
            line_width,
            filled,
            grab_area,
            used_line_widths: BTreeMap::new(),
            current: None,
            snap_candidates: BTreeSet::new(),
        }
    }

    fn write_tool_data<H: ElementHost>(&self, cx: &mut Cx<'_, '_, H>) {
        let d = &mut cx.out.tool_data;
        d.layer = self.layer;
        d.available_layers = H::polygon_layers();
        d.line_width = self.line_width;
        d.filled = self.filled;
        d.grab_area = self.grab_area;
    }

    /// Upstream `updateSnapCandidates()`: circle centers.
    fn update_snap_candidates<H: ElementHost>(&mut self, cx: &Cx<'_, '_, H>) {
        self.snap_candidates = H::circles(cx.element(), cx.fpt())
            .map(|l| l.iter().map(|c| c.center()).collect())
            .unwrap_or_default();
    }

    fn start<H: ElementHost>(&mut self, cx: &mut Cx<'_, '_, H>, pos: Point) -> bool {
        if !cx.begin(H::add_text("circle")) {
            return false;
        }
        let uuid = Uuid::new_random();
        let circle = Circle::new(
            uuid,
            self.layer,
            self.line_width,
            self.filled,
            self.grab_area,
            pos,
            PositiveLength::new(Length::new(1)).unwrap_or_else(|_| unreachable!()),
        );
        let ok = cx.modify(|e, fpt| {
            H::circles_mut(e, fpt)
                .map(|l| {
                    l.push(circle);
                })
                .is_some()
        });
        if ok != Some(true) {
            cx.abort_group();
            return false;
        }
        self.current = Some((uuid, pos));
        cx.out.selection = [H::circle_item(uuid)].into_iter().collect();
        true
    }

    /// Upstream `updateCircleDiameter()`.
    fn update_diameter<H: ElementHost>(&self, cx: &mut Cx<'_, '_, H>, pos: Point) {
        let Some((uuid, center)) = self.current else {
            return;
        };
        let diameter = (*(pos - center).length() * 2).max(Length::new(1));
        let Ok(diameter) = PositiveLength::new(diameter) else {
            return;
        };
        cx.modify(|e, fpt| {
            if let Some(c) = H::circles_mut(e, fpt).and_then(|l| l.by_uuid_mut(&uuid)) {
                c.set_diameter(diameter);
            }
        });
    }

    fn finish<H: ElementHost>(&mut self, cx: &mut Cx<'_, '_, H>, pos: Point) -> bool {
        let Some((uuid, center)) = self.current else {
            return false;
        };
        if pos == center {
            return self.abort_command(cx);
        }
        self.update_diameter(cx, pos);
        self.current = None;
        cx.out.selection.remove(&H::circle_item(uuid));
        let ok = cx.commit();
        self.update_snap_candidates(cx);
        ok
    }

    fn abort_command<H: ElementHost>(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        if let Some((uuid, _)) = self.current.take() {
            cx.out.selection.remove(&H::circle_item(uuid));
        }
        let ok = cx.abort_group();
        self.update_snap_candidates(cx);
        ok
    }
}

impl<H: ElementHost> State<H> for DrawCircleState {
    fn entry(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        self.update_snap_candidates(cx);
        cx.out.tool = LibraryTool::DrawCircle;
        self.write_tool_data(cx);
        cx.set_cursor(Some(CursorShape::Cross));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        if self.current.is_some() && !self.abort_command(cx) {
            return false;
        }
        cx.set_cursor(None);
        true
    }

    fn abort(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        self.current.is_some() && self.abort_command(cx)
    }

    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_, H>, e: PointerEvent) -> bool {
        if self.current.is_some() {
            let pos = e.pos.mapped_to_grid(cx.grid());
            self.update_diameter(cx, pos);
        }
        true
    }

    fn left_pressed(&mut self, cx: &mut Cx<'_, '_, H>, e: PointerEvent) -> bool {
        if self.current.is_some() {
            self.finish(cx, e.pos.mapped_to_grid(cx.grid()))
        } else {
            // Not very nice to always snap (upstream comment).
            let (pos, _) = snap_position(e.pos, cx.grid(), &self.snap_candidates);
            self.start(cx, pos)
        }
    }

    fn tool_data_changed(&mut self, cx: &mut Cx<'_, '_, H>, old: &LibraryToolData) {
        let d = cx.out.tool_data.clone();
        if d.layer != old.layer {
            self.layer = d.layer;
            if H::IS_FOOTPRINT {
                self.line_width = if let Some(w) = self.used_line_widths.get(&d.layer) {
                    *w
                } else if d.layer.polygons_represent_areas() {
                    UnsignedLength::default()
                } else {
                    super::len(200_000)
                };
            }
        }
        if d.line_width != old.line_width {
            self.line_width = d.line_width;
            if H::IS_FOOTPRINT {
                self.used_line_widths.insert(self.layer, d.line_width);
            }
        }
        self.filled = d.filled;
        self.grab_area = d.grab_area;
        if let Some((uuid, _)) = self.current {
            let (layer, width, filled, grab) =
                (self.layer, self.line_width, self.filled, self.grab_area);
            cx.modify(|e, fpt| {
                if let Some(c) = H::circles_mut(e, fpt).and_then(|l| l.by_uuid_mut(&uuid)) {
                    c.set_layer(layer);
                    c.set_line_width(width);
                    c.set_is_filled(filled);
                    c.set_is_grab_area(grab);
                }
            });
        }
        self.write_tool_data(cx);
    }
}
