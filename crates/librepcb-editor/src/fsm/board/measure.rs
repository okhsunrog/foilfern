//! The measure tool: port of
//! libs/librepcb/editor/project/board/fsm/boardeditorstate_measure.{h,cpp}
//! and of `MeasureTool::setBoard()` (the snap candidates of a board: device
//! and pad origins, pad outlines, junctions, vias, outlines of planes,
//! polygons and holes, text origins); the tool itself is the shared
//! [`MeasureTool`].

use std::collections::BTreeSet;

use librepcb_core::library::pkg::Footprint;
use librepcb_core::project::Project;
use librepcb_core::project::board::Board;
use librepcb_core::types::Point;
use librepcb_core::utils::transform::Transform;

use super::context::Cx;
use super::output::BoardToolData;
use super::{BoardFsmInput, State};
use crate::fsm::measure::{MeasureTool, snap_candidates_from_circle, snap_candidates_from_path};
use crate::fsm::{CursorShape, Key, KeyEvent};

/// Upstream `MeasureTool::snapCandidatesFromFootprint()`.
fn footprint_candidates(footprint: &Footprint, t: &Transform, out: &mut BTreeSet<Point>) {
    for pad in footprint.pads().iter() {
        let p = pad.pad();
        out.insert(t.map(&p.position()));
        if let Ok(outlines) = p.geometry().to_outlines() {
            for outline in outlines {
                let path = outline
                    .rotated(p.rotation(), Point::ORIGIN)
                    .translated(p.position());
                out.extend(snap_candidates_from_path(&t.map(&path)));
            }
        }
        let pad_t = Transform {
            position: p.position(),
            rotation: p.rotation(),
            mirrored: false,
        };
        for h in p.holes().iter() {
            for v in pad_t.map(h.path()).get().vertices() {
                out.extend(snap_candidates_from_circle(t.map(&v.pos), h.diameter()));
            }
        }
    }
    for p in footprint.polygons().iter() {
        out.extend(snap_candidates_from_path(&t.map(p.path())));
    }
    for c in footprint.circles().iter() {
        out.extend(snap_candidates_from_circle(
            t.map(&c.center()),
            c.diameter(),
        ));
    }
    for s in footprint.stroke_texts().iter() {
        out.insert(t.map(&s.position()));
    }
    for h in footprint.holes().iter() {
        for v in h.path().get().vertices() {
            out.extend(snap_candidates_from_circle(t.map(&v.pos), h.diameter()));
        }
    }
}

/// Upstream `MeasureTool::setBoard()`.
pub(crate) fn board_snap_candidates(project: &Project, board: &Board) -> BTreeSet<Point> {
    let mut out = BTreeSet::new();
    let lib = project.library();
    for d in board.devices().values() {
        out.insert(d.position());
        if let Some(footprint) = lib
            .device(&d.lib_device())
            .and_then(|dev| lib.package(&dev.package_uuid()))
            .and_then(|pkg| pkg.footprints().by_uuid(&d.lib_footprint()))
        {
            footprint_candidates(footprint, &d.transform(), &mut out);
        }
    }
    for seg in board.net_segments().values() {
        for j in seg.junctions().values() {
            out.insert(j.position());
        }
        for v in seg.vias().values() {
            out.insert(v.position());
            let props = board.via_properties(v, seg.net(), project.circuit());
            out.extend(snap_candidates_from_circle(v.position(), props.size));
            out.extend(snap_candidates_from_circle(
                v.position(),
                props.drill_diameter,
            ));
        }
    }
    for plane in board.planes().values() {
        out.extend(snap_candidates_from_path(plane.outline()));
        for fragment in board.derived().fragments_of(plane.id()) {
            out.extend(snap_candidates_from_path(fragment));
        }
    }
    for p in board.polygons().values() {
        out.extend(snap_candidates_from_path(p.path()));
    }
    for t in board.stroke_texts().values() {
        out.insert(t.position());
    }
    for h in board.holes().values() {
        for v in h.path().get().vertices() {
            out.extend(snap_candidates_from_circle(v.pos, h.diameter()));
        }
    }
    out
}

/// The measure tool (upstream `BoardEditorState_Measure`).
#[derive(Debug, Default)]
pub(super) struct MeasureState {
    tool: MeasureTool,
}

impl State for MeasureState {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        let Ok(board) = cx.board() else {
            return false;
        };
        self.tool
            .set_snap_candidates(board_snap_candidates(cx.project(), board));
        let (grid, unit, pos) = (cx.grid(), cx.unit(), cx.cursor_pos);
        self.tool.enter(&mut cx.out.view, grid, unit, pos);
        cx.selection.clear();
        cx.set_cursor(Some(CursorShape::Cross));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.tool.leave(&mut cx.out.view);
        cx.set_cursor(None);
        true
    }

    fn process(&mut self, cx: &mut Cx<'_, '_>, input: &BoardFsmInput) -> bool {
        let view = &mut cx.out.view;
        match input {
            BoardFsmInput::KeyPressed(KeyEvent {
                key: Key::Shift,
                modifiers,
            })
            | BoardFsmInput::KeyReleased(KeyEvent {
                key: Key::Shift,
                modifiers,
            }) => {
                self.tool.modifiers_changed(view, *modifiers);
                true
            }
            BoardFsmInput::PointerMoved(e) => {
                self.tool.pointer_moved(view, e.pos, e.modifiers);
                true
            }
            BoardFsmInput::LeftPressed(_) => {
                self.tool.left_pressed(view);
                true
            }
            BoardFsmInput::Copy => match self.tool.copy(view) {
                Some(text) => {
                    cx.ctx.clipboard.set("text/plain", text.into_bytes());
                    true
                }
                None => false,
            },
            BoardFsmInput::Remove => self.tool.remove(view),
            BoardFsmInput::Abort => self.tool.abort(view),
            _ => false,
        }
    }

    fn tool_data(&self, _cx: &Cx<'_, '_>) -> BoardToolData {
        BoardToolData::default()
    }
}
