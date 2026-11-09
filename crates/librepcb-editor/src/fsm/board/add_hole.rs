//! The hole tool: port of
//! libs/librepcb/editor/project/board/fsm/boardeditorstate_addhole.{h,cpp}.
//!
//! The hole follows the cursor (live preview in the group "Add hole to
//! board"); a click commits it and starts the next one.

use librepcb_core::geometry::NonEmptyPath;
use librepcb_core::project::board::{BoardHoleData, BoardItem};
use librepcb_core::project::{BoardMutation, Mutation};
use librepcb_core::types::{Length, MaskConfig, Point, PositiveLength, Uuid};
use librepcb_i18n::tr;

use super::context::Cx;
use super::output::{BoardToolData, ToolSetting};
use super::{BoardFsmInput, State};
use crate::fsm::CursorShape;

/// The hole tool (upstream `BoardEditorState_AddHole`).
#[derive(Debug)]
pub(super) struct AddHoleState {
    diameter: PositiveLength,
    placing: Option<(Uuid, Point)>,
}

impl Default for AddHoleState {
    fn default() -> Self {
        Self {
            // Invariant: 1 mm is positive.
            diameter: PositiveLength::new(Length::new(1_000_000)).expect("positive"),
            placing: None,
        }
    }
}

impl AddHoleState {
    fn preview(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        let Some((uuid, pos)) = self.placing else {
            return false;
        };
        let hole = BoardHoleData::new(
            uuid,
            self.diameter,
            NonEmptyPath::from_point(pos),
            MaskConfig::Automatic,
            false,
        );
        let board = cx.board_id();
        match cx.preview(
            0,
            vec![Mutation::Board(BoardMutation::AddItem {
                board,
                item: BoardItem::Hole(hole),
            })],
        ) {
            Ok(()) => true,
            Err(e) => {
                cx.error(e);
                self.abort(cx);
                false
            }
        }
    }

    fn add_hole(&mut self, cx: &mut Cx<'_, '_>, pos: Point) -> bool {
        if let Err(e) = cx.begin(tr!(
            "librepcb::editor::BoardEditorState_AddHole",
            "Add hole to board"
        )) {
            cx.error(e);
            return false;
        }
        self.placing = Some((Uuid::new_random(), pos));
        self.preview(cx)
    }

    fn abort(&mut self, cx: &mut Cx<'_, '_>) {
        self.placing = None;
        if cx.is_group_active() {
            cx.abort();
        }
    }
}

impl State for AddHoleState {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        let pos = cx.cursor_pos.mapped_to_grid(cx.grid());
        if !self.add_hole(cx, pos) {
            return false;
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
            BoardFsmInput::PointerMoved(e) => {
                let pos = e.pos.mapped_to_grid(cx.grid());
                match self.placing.as_mut() {
                    Some(p) => {
                        p.1 = pos;
                        self.preview(cx)
                    }
                    None => false,
                }
            }
            BoardFsmInput::LeftPressed(e) | BoardFsmInput::LeftDoubleClicked(e) => {
                let pos = e.pos.mapped_to_grid(cx.grid());
                if let Some(p) = self.placing.as_mut() {
                    p.1 = pos;
                    if self.preview(cx) {
                        self.placing = None;
                        if let Err(e) = cx.commit() {
                            cx.error(e);
                        }
                    }
                }
                self.add_hole(cx, pos);
                true
            }
            BoardFsmInput::ToolSetting(ToolSetting::HoleDiameter(d)) => {
                self.diameter = *d;
                self.preview(cx);
                true
            }
            _ => false,
        }
    }

    fn tool_data(&self, _cx: &Cx<'_, '_>) -> BoardToolData {
        BoardToolData {
            drill: Some(self.diameter),
            ..Default::default()
        }
    }
}
