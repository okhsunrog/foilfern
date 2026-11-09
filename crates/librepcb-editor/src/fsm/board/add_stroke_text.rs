//! The text tool: port of
//! libs/librepcb/editor/project/board/fsm/boardeditorstate_addstroketext.{h,cpp}.
//!
//! The text follows the cursor (live preview in the group "Add text to
//! board"); rotate/flip (and the right button) modify it; a click commits
//! it and starts the next one.

use librepcb_core::project::board::{BoardItem, BoardStrokeTextData};
use librepcb_core::project::{BoardMutation, Mutation};
use librepcb_core::types::{
    Alignment, Angle, HAlign, Layer, Length, Orientation, Point, PositiveLength, StrokeTextSpacing,
    UnsignedLength, Uuid, VAlign,
};
use librepcb_i18n::tr;

use super::context::Cx;
use super::draw_polygon::geometry_layers;
use super::output::{BoardToolData, ToolSetting};
use super::transform::mirror_text;
use super::{BoardFsmInput, State};
use crate::fsm::CursorShape;
use crate::fsm::Features;

/// The text tool (upstream `BoardEditorState_AddStrokeText`).
#[derive(Debug)]
pub(super) struct AddStrokeTextState {
    /// The properties of the next text (position is set when placing).
    props: BoardStrokeTextData,
    placing: bool,
}

impl Default for AddStrokeTextState {
    fn default() -> Self {
        Self {
            props: BoardStrokeTextData::new(
                Uuid::new_random(),
                Layer::BOARD_DOCUMENTATION,
                "{{PROJECT}}".to_owned(),
                Point::ORIGIN,
                Angle::DEG0,
                // Invariant: 1.5 mm is positive.
                PositiveLength::new(Length::new(1_500_000)).expect("positive"),
                UnsignedLength::new(Length::new(200_000)).unwrap_or_default(),
                StrokeTextSpacing::Auto,
                StrokeTextSpacing::Auto,
                Alignment::new(HAlign::Left, VAlign::Bottom),
                false,
                true,
                false,
            ),
            placing: false,
        }
    }
}

impl AddStrokeTextState {
    fn preview(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        if !self.placing {
            return false;
        }
        let board = cx.board_id();
        match cx.preview(
            0,
            vec![Mutation::Board(BoardMutation::AddItem {
                board,
                item: BoardItem::StrokeText(self.props.clone()),
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

    fn add_text(&mut self, cx: &mut Cx<'_, '_>, pos: Point) -> bool {
        if let Err(e) = cx.begin(tr!(
            "librepcb::editor::BoardEditorState_AddStrokeText",
            "Add text to board"
        )) {
            cx.error(e);
            return false;
        }
        self.props = self.props.with_uuid(Uuid::new_random());
        self.props.set_position(pos);
        self.placing = true;
        self.preview(cx)
    }

    fn abort(&mut self, cx: &mut Cx<'_, '_>) {
        self.placing = false;
        if cx.is_group_active() {
            cx.abort();
        }
    }

    fn rotate(&mut self, cx: &mut Cx<'_, '_>, angle: Angle) -> bool {
        if !self.placing {
            return false;
        }
        self.props.set_rotation(self.props.rotation() + angle);
        self.preview(cx);
        true
    }

    fn flip(&mut self, cx: &mut Cx<'_, '_>, orientation: Orientation) -> bool {
        if !self.placing {
            return false;
        }
        let pos = self.props.position();
        let inner = Some(cx.inner_layer_count());
        mirror_text(&mut self.props, orientation, pos, inner);
        self.preview(cx);
        true
    }
}

impl State for AddStrokeTextState {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        let pos = cx.cursor_pos.mapped_to_grid(cx.grid());
        if !self.add_text(cx, pos) {
            return false;
        }
        cx.out.view.features = Features {
            rotate: true,
            flip: true,
            ..Default::default()
        };
        cx.set_cursor(Some(CursorShape::Cross));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.abort(cx);
        cx.out.view.features = Features::default();
        cx.set_cursor(None);
        true
    }

    fn process(&mut self, cx: &mut Cx<'_, '_>, input: &BoardFsmInput) -> bool {
        match input {
            BoardFsmInput::Rotate(angle) => self.rotate(cx, *angle),
            BoardFsmInput::Flip(o) => self.flip(cx, *o),
            BoardFsmInput::PointerMoved(e) => {
                if !self.placing {
                    return false;
                }
                self.props.set_position(e.pos.mapped_to_grid(cx.grid()));
                self.preview(cx)
            }
            BoardFsmInput::LeftPressed(e) | BoardFsmInput::LeftDoubleClicked(e) => {
                let pos = e.pos.mapped_to_grid(cx.grid());
                if self.placing {
                    self.props.set_position(pos);
                    if self.preview(cx) {
                        self.placing = false;
                        if let Err(e) = cx.commit() {
                            cx.error(e);
                        }
                    }
                }
                self.add_text(cx, pos);
                true
            }
            BoardFsmInput::RightReleased(_) => {
                self.rotate(cx, Angle::DEG90);
                // Always accept the event while placing, else the FSM
                // aborts the tool.
                self.placing
            }
            BoardFsmInput::ToolSetting(setting) => {
                match setting {
                    ToolSetting::Layer(l) => {
                        self.props.set_layer(*l);
                    }
                    ToolSetting::TextHeight(h) => {
                        self.props.set_height(*h);
                    }
                    ToolSetting::Text(t) => {
                        self.props.set_text(t.clone());
                    }
                    ToolSetting::Mirrored(m) => {
                        self.props.set_mirrored(*m);
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
            layer: Some(self.props.layer()),
            size: Some(self.props.height()),
            value: self.props.text().clone(),
            value_suggestions: ["{{BOARD}}", "{{PROJECT}}", "{{AUTHOR}}", "{{VERSION}}"]
                .into_iter()
                .map(String::from)
                .collect(),
            mirrored: self.props.mirrored(),
            ..Default::default()
        }
    }
}
