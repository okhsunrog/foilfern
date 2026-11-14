//! Port of libs/librepcb/editor/library/sym/fsm/symboleditorstate_measure.{h,cpp}
//! and libs/librepcb/editor/library/pkg/fsm/packageeditorstate_measure.{h,cpp}
//! (on top of the shared [`MeasureTool`]).

use super::{Cx, ElementHost, LibraryTool, State};
use crate::fsm::measure::MeasureTool;
use crate::fsm::{CursorShape, Key, KeyEvent, PointerEvent};

/// The measure state.
#[derive(Debug, Default)]
pub(crate) struct MeasureState {
    tool: MeasureTool,
}

impl<H: ElementHost> State<H> for MeasureState {
    fn entry(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        cx.out.tool = LibraryTool::Measure;
        cx.set_cursor(Some(CursorShape::Cross));
        self.tool
            .set_snap_candidates(H::measure_snap_candidates(cx.element(), cx.fpt()));
        let (grid, unit, pos) = (cx.grid(), cx.unit(), cx.cursor_pos());
        self.tool.enter(&mut cx.out.view, grid, unit, pos);
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        self.tool.leave(&mut cx.out.view);
        cx.set_cursor(None);
        true
    }

    fn key_pressed(&mut self, cx: &mut Cx<'_, '_, H>, e: KeyEvent) -> bool {
        if e.key == Key::Shift {
            self.tool.modifiers_changed(&mut cx.out.view, e.modifiers);
            return true;
        }
        false
    }

    fn key_released(&mut self, cx: &mut Cx<'_, '_, H>, e: KeyEvent) -> bool {
        State::<H>::key_pressed(self, cx, e)
    }

    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_, H>, e: PointerEvent) -> bool {
        self.tool
            .pointer_moved(&mut cx.out.view, e.pos, e.modifiers);
        true
    }

    fn left_pressed(&mut self, cx: &mut Cx<'_, '_, H>, _e: PointerEvent) -> bool {
        self.tool.left_pressed(&mut cx.out.view);
        true
    }

    fn copy(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        match self.tool.copy(&mut cx.out.view) {
            Some(text) => {
                cx.ctx.clipboard.set("text/plain", text.into_bytes());
                true
            }
            None => false,
        }
    }

    fn remove(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        self.tool.remove(&mut cx.out.view)
    }

    fn abort(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        self.tool.abort(&mut cx.out.view)
    }
}
