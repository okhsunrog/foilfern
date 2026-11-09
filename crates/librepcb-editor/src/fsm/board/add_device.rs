//! The device placement tool: port of
//! libs/librepcb/editor/project/board/fsm/boardeditorstate_adddevice.{h,cpp}
//! (started from the "place devices" panel with
//! [`BoardFsmInput::AddDevice`]).
//!
//! The device is added at the cursor ([`crate::commands::AddDevice`]:
//! library elements, assembly option) and follows the cursor as a live
//! preview; rotate/flip (and the right button) modify it; a click places it
//! and leaves the tool.

use librepcb_core::project::board::BoardDevice;
use librepcb_core::project::{BoardMutation, ComponentInstanceId, Mutation};
use librepcb_core::types::{Angle, Orientation, Point, Uuid};
use librepcb_i18n::tr;

use super::context::Cx;
use super::output::{BoardRequest, BoardToolData};
use super::transform::mirror_text;
use super::{BoardFsmInput, State};
use crate::commands::AddDevice;
use crate::fsm::Features;

/// The device being placed.
#[derive(Debug, Clone)]
struct Placing {
    device: BoardDevice,
    mark: usize,
}

/// The device placement tool (upstream `BoardEditorState_AddDevice`).
#[derive(Debug, Default)]
pub(super) struct AddDeviceState {
    placing: Option<Placing>,
}

impl AddDeviceState {
    fn add_device(
        &mut self,
        cx: &mut Cx<'_, '_>,
        component: ComponentInstanceId,
        device: Uuid,
        footprint: Option<Uuid>,
    ) -> bool {
        if cx
            .project()
            .circuit()
            .component_instance(component)
            .is_none()
        {
            return false;
        }
        if let Err(e) = cx.begin(tr!(
            "librepcb::editor::BoardEditorState_AddDevice",
            "Add device to board"
        )) {
            cx.error(e);
            return false;
        }
        let pos = cx.cursor_pos.mapped_to_grid(cx.grid());
        let board = cx.board_id();
        let result = cx.exec(AddDevice {
            component: component.into(),
            board: Some(board),
            device: Some(device),
            footprint,
            position: pos,
            rotation: Angle::DEG0,
            mirrored: false,
        });
        let added = result
            .ok()
            .and_then(|_| cx.board().ok().and_then(|b| b.device(component)).cloned());
        match added {
            Some(device) => {
                self.placing = Some(Placing {
                    device,
                    mark: cx.group_len(),
                });
                true
            }
            None => {
                cx.error(tr!(
                    "librepcb::editor::BoardEditorState_AddDevice",
                    "Could not add device:\n\n{0}",
                    tr!(
                        "librepcb::editor::BoardEditorState_AddDevice",
                        "Add device to board"
                    )
                ));
                self.abort(cx);
                false
            }
        }
    }

    fn preview(&mut self, cx: &mut Cx<'_, '_>) {
        let Some(p) = self.placing.clone() else {
            return;
        };
        let board = cx.board_id();
        if let Err(e) = cx.preview(
            p.mark,
            vec![Mutation::Board(BoardMutation::UpdateDevice {
                board,
                device: p.device,
            })],
        ) {
            cx.error(e);
        }
    }

    fn translate_to(&mut self, pos: Point) {
        if let Some(p) = self.placing.as_mut() {
            let d = pos - p.device.position();
            p.device.set_position(pos);
            let texts: Vec<_> = p.device.stroke_texts().values().cloned().collect();
            for mut t in texts {
                t.set_position(t.position() + d);
                p.device.insert_stroke_text(t);
            }
        }
    }

    /// Upstream `rotateDevice()` (around the device position).
    fn rotate(&mut self, cx: &mut Cx<'_, '_>, angle: Angle) -> bool {
        let Some(p) = self.placing.as_mut() else {
            return false;
        };
        let center = p.device.position();
        p.device.set_rotation(p.device.rotation() + angle);
        let texts: Vec<_> = p.device.stroke_texts().values().cloned().collect();
        for mut t in texts {
            t.set_position(t.position().rotated(angle, center));
            t.set_rotation(t.rotation() + angle);
            p.device.insert_stroke_text(t);
        }
        self.preview(cx);
        true
    }

    /// Upstream `mirrorDevice()` (at the device position).
    fn mirror(&mut self, cx: &mut Cx<'_, '_>, orientation: Orientation) -> bool {
        let inner = Some(cx.inner_layer_count());
        let Some(p) = self.placing.as_mut() else {
            return false;
        };
        let center = p.device.position();
        let d = &mut p.device;
        d.set_mirrored(!d.mirrored());
        d.set_position(d.position().mirrored(orientation, center));
        d.set_rotation(match orientation {
            Orientation::Horizontal => -d.rotation(),
            Orientation::Vertical => Angle::DEG180 - d.rotation(),
        });
        let texts: Vec<_> = d.stroke_texts().values().cloned().collect();
        for mut t in texts {
            mirror_text(&mut t, orientation, center, inner);
            d.insert_stroke_text(t);
        }
        self.preview(cx);
        true
    }

    fn abort(&mut self, cx: &mut Cx<'_, '_>) {
        self.placing = None;
        if cx.is_group_active() {
            cx.abort();
        }
    }
}

impl State for AddDeviceState {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        cx.out.view.features = Features {
            rotate: true,
            flip: true,
            ..Default::default()
        };
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
            BoardFsmInput::AddDevice {
                component,
                device,
                footprint,
            } => {
                self.abort(cx);
                self.add_device(cx, *component, *device, *footprint)
            }
            BoardFsmInput::Rotate(angle) => self.rotate(cx, *angle),
            BoardFsmInput::Flip(o) => self.mirror(cx, *o),
            BoardFsmInput::PointerMoved(e) => {
                if self.placing.is_none() {
                    return false;
                }
                self.translate_to(e.pos.mapped_to_grid(cx.grid()));
                self.preview(cx);
                true
            }
            BoardFsmInput::LeftPressed(e) | BoardFsmInput::LeftDoubleClicked(e) => {
                if self.placing.is_none() {
                    return false;
                }
                self.translate_to(e.pos.mapped_to_grid(cx.grid()));
                self.preview(cx);
                let component = self.placing.take().map(|p| p.device.component());
                match cx.commit() {
                    Ok(_) => {
                        if let Some(c) = component {
                            cx.out
                                .requests
                                .push(BoardRequest::DevicesChanged([c].into_iter().collect()));
                        }
                        // Placing finished, leave the tool.
                        cx.leave_requested = true;
                    }
                    Err(e) => {
                        cx.error(e);
                        self.abort(cx);
                    }
                }
                true
            }
            BoardFsmInput::RightReleased(_) => {
                self.rotate(cx, Angle::DEG90);
                self.placing.is_some()
            }
            _ => false,
        }
    }

    fn tool_data(&self, _cx: &Cx<'_, '_>) -> BoardToolData {
        BoardToolData::default()
    }
}
