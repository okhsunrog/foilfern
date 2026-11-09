//! The "find" feature of the board editor: port of the find handling of
//! libs/librepcb/editor/project/board/board2dtab.{h,cpp}
//! (`FindRefreshSuggestions`, `FindNext`, `FindPrevious`,
//! `goToObjects()`), see [`crate::fsm::find`].

use librepcb_core::types::Point;

use super::view::BoardItemRef;
use super::{BoardContext, BoardEditorFsm, BoardTool};
use crate::fsm::find::{
    FindCandidate, FindResult, SearchContext, net_candidates, resolve_candidates, zoom_rect,
};

impl BoardEditorFsm {
    /// The search state (term and suggestions) of the "find" field.
    pub fn search(&self) -> &SearchContext {
        &self.search
    }

    /// Sets the search term of the "find" field.
    pub fn set_find_term(&mut self, term: &str) {
        self.search.set_term(term);
    }

    /// Refreshes the suggestions: the components placed on the board and
    /// the named nets (upstream `FindRefreshSuggestions`).
    pub fn refresh_find_suggestions(&mut self, ctx: &BoardContext<'_>) {
        let p = ctx.editor.project();
        let mut candidates = Vec::new();
        if let Some(board) = p.board(self.board) {
            for id in board.devices().keys() {
                if let Some(c) = p.circuit().component_instance(*id) {
                    candidates.push(FindCandidate::component(c.name().to_string()));
                }
            }
        }
        candidates.extend(net_candidates(p));
        self.search.set_candidates(candidates);
    }

    /// Goes to the next match (upstream `FindNext`): selects it and returns
    /// the rectangle to zoom to.
    pub fn find_next(&mut self, ctx: &mut BoardContext<'_>) -> FindResult {
        let objects = self.search.find_next();
        self.go_to_objects(ctx, &objects)
    }

    /// Goes to the previous match (upstream `FindPrevious`).
    pub fn find_previous(&mut self, ctx: &mut BoardContext<'_>) -> FindResult {
        let objects = self.search.find_previous();
        self.go_to_objects(ctx, &objects)
    }

    /// Selects the devices of the found components and the pads, vias,
    /// traces and planes of the found nets, highlights the nets and
    /// returns the zoom rectangle (upstream `Board2dTab::goToObjects()`).
    /// Only has an effect in the select tool.
    pub fn go_to_objects(
        &mut self,
        ctx: &mut BoardContext<'_>,
        objects: &[FindCandidate],
    ) -> FindResult {
        let p = ctx.editor.project();
        let (components, nets) = resolve_candidates(p, objects);
        let mut items = Vec::new();
        let mut points: Vec<Point> = Vec::new();
        if let Some(board) = p.board(self.board) {
            for (id, device) in board.devices() {
                if components.contains(id) {
                    items.push(BoardItemRef::Device(*id));
                    points.push(device.position());
                }
                // Footprint pads of the found nets.
                for pad in device.pads(p.library(), p.circuit()).unwrap_or_default() {
                    if pad.net().is_some_and(|n| nets.contains(&n)) {
                        items.push(BoardItemRef::FootprintPad(*id, pad.uuid()));
                        points.push(pad.position());
                    } else if components.contains(id) {
                        points.push(pad.position());
                    }
                }
            }
            for (seg_id, seg) in board.net_segments() {
                if !seg.net().is_some_and(|n| nets.contains(&n)) {
                    continue;
                }
                for pad in seg.pads().values() {
                    items.push(BoardItemRef::Pad(*seg_id, pad.uuid()));
                    points.push(pad.pad().position());
                }
                for via in seg.vias().values() {
                    items.push(BoardItemRef::Via(*seg_id, via.uuid()));
                    points.push(via.position());
                }
                for trace in seg.traces().values() {
                    items.push(BoardItemRef::Trace(*seg_id, trace.uuid()));
                    for anchor in [trace.p1(), trace.p2()] {
                        if let Some(pos) =
                            board.anchor_position(seg, anchor, p.library(), p.circuit())
                        {
                            points.push(pos);
                        }
                    }
                }
            }
            for (id, plane) in board.planes() {
                if plane.net().is_some_and(|n| nets.contains(&n)) {
                    items.push(BoardItemRef::Plane(*id));
                    points.extend(plane.outline().vertices().iter().map(|v| v.pos));
                }
            }
        }
        let result = FindResult {
            objects: objects.to_vec(),
            components: components.iter().copied().collect(),
            nets: nets.iter().copied().collect(),
            zoom_rect: if items.is_empty() {
                None
            } else {
                zoom_rect(points)
            },
        };
        if self.tool() == BoardTool::Select {
            self.out.highlighted_nets = nets;
            self.set_selection(ctx, items);
        }
        result
    }
}
