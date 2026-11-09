//! Port of libs/librepcb/editor/project/schematic/fsm/schematiceditorstate_measure.{h,cpp}
//! and of `MeasureTool::setSchematic()` (the snap candidates of a page).

use std::collections::BTreeSet;

use librepcb_core::project::Project;
use librepcb_core::project::schematic::Schematic;
use librepcb_core::types::Point;
use librepcb_core::utils::transform::Transform;

use super::{Cx, SchematicTool, State};
use crate::fsm::measure::{MeasureTool, snap_candidates_from_circle, snap_candidates_from_path};
use crate::fsm::{CursorShape, Key, KeyEvent, PointerEvent};

/// Snap candidates of a schematic page (upstream
/// `MeasureTool::setSchematic()`).
pub(crate) fn schematic_snap_candidates(p: &Project, s: &Schematic) -> BTreeSet<Point> {
    let mut candidates = BTreeSet::new();
    for symbol in s.symbols().values() {
        candidates.insert(symbol.position());
        let Ok(resolved) = symbol.resolve(p.view()) else {
            continue;
        };
        let t = Transform::from_obj(symbol);
        let lib = resolved.lib_symbol;
        for pin in lib.pins().iter() {
            candidates.insert(t.map(&pin.position()));
            let end = pin.position()
                + Point::new(pin.length().get(), librepcb_core::types::Length::ZERO)
                    .rotated(pin.rotation(), Point::ORIGIN);
            candidates.insert(t.map(&end));
        }
        for polygon in lib.polygons().iter() {
            candidates.extend(snap_candidates_from_path(&t.map(polygon.path())));
        }
        for circle in lib.circles().iter() {
            candidates.extend(snap_candidates_from_circle(
                t.map(&circle.center()),
                circle.diameter(),
            ));
        }
        for text in lib.texts().iter() {
            candidates.insert(t.map(&text.position()));
        }
    }
    for segment in s.net_segments().values() {
        for junction in segment.junctions().values() {
            candidates.insert(junction.position());
        }
        for label in segment.labels().values() {
            candidates.insert(label.position());
        }
    }
    for polygon in s.polygons().values() {
        candidates.extend(snap_candidates_from_path(polygon.path()));
    }
    for text in s.texts().values() {
        candidates.insert(text.position());
    }
    candidates
}

/// The measure state.
#[derive(Debug, Default)]
pub(crate) struct MeasureState {
    tool: MeasureTool,
}

impl State for MeasureState {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        let candidates = match cx.sch() {
            Some(s) => schematic_snap_candidates(cx.project(), s),
            None => return false,
        };
        self.tool.set_snap_candidates(candidates);
        let (grid, unit, pos) = (cx.grid(), cx.unit(), cx.cursor_pos());
        self.tool.enter(&mut cx.out.view, grid, unit, pos);
        cx.out.tool = SchematicTool::Measure;
        cx.set_cursor(Some(CursorShape::Cross));
        cx.out.selection.clear();
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.tool.leave(&mut cx.out.view);
        cx.set_cursor(None);
        true
    }

    fn copy(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        match self.tool.copy(&mut cx.out.view) {
            Some(text) => {
                cx.ctx.clipboard.set("text/plain", text.into_bytes());
                true
            }
            None => false,
        }
    }

    fn remove(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.tool.remove(&mut cx.out.view)
    }

    fn abort(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.tool.abort(&mut cx.out.view)
    }

    fn key_pressed(&mut self, cx: &mut Cx<'_, '_>, e: KeyEvent) -> bool {
        if e.key == Key::Shift {
            self.tool.modifiers_changed(&mut cx.out.view, e.modifiers);
            return true;
        }
        false
    }

    fn key_released(&mut self, cx: &mut Cx<'_, '_>, e: KeyEvent) -> bool {
        if e.key == Key::Shift {
            self.tool.modifiers_changed(&mut cx.out.view, e.modifiers);
            return true;
        }
        false
    }

    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        self.tool
            .pointer_moved(&mut cx.out.view, e.pos, e.modifiers);
        true
    }

    fn left_pressed(&mut self, cx: &mut Cx<'_, '_>, _e: PointerEvent) -> bool {
        self.tool.left_pressed(&mut cx.out.view);
        true
    }
}
