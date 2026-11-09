//! The select tool: port of
//! libs/librepcb/editor/project/board/fsm/boardeditorstate_select.{h,cpp}
//! (selection, rubber band, dragging, rotating, flipping, removing, locking,
//! line widths, texts reset, vertex editing, clipboard, context menus,
//! properties requests, info box and available features).

use std::collections::BTreeSet;

use librepcb_core::geometry::{NonEmptyPath, Path, TraceAnchor, Vertex};
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::project::board::{BoardHoleData, BoardPolygonData};
use librepcb_core::project::board::{BoardItem, BoardNetSegment};
use librepcb_core::project::{BoardMutation, Mutation, NetSegmentId, NetSignalId};
use librepcb_core::types::MaskConfig;
use librepcb_core::types::{Angle, Length, Orientation, Point, UnsignedLength, Uuid};
use librepcb_core::utils::toolbox;
use librepcb_i18n::{tr, trn};

use super::clipboard::{BoardClipboardData, PasteBoardItems, board_clipboard_mime_type};
use super::context::Cx;
use super::output::{BoardRequest, BoardToolData, ContextAction, ContextMenuItem};
use super::selection::{SelectionQuery, all_items, segment_items};
use super::transform::{DragItems, flip_mutations};
use super::view::{BoardItemRef, FindFilter, FindFlags};
use super::{BoardFsmInput, DxfImportSettings, State};
use crate::commands::{BoardSelection as RemoveSelection, RemoveBoardItems};
use crate::fsm::{CrossProbe, Features};
use crate::library_editor::commands::{FootprintClipboardData, footprint_clipboard_mime_type};

/// A running drag operation (upstream `mSelectedItemsDragCommand`).
#[derive(Debug)]
struct Drag {
    items: DragItems,
    /// Group length when the preview started (`None`: not started yet).
    mark: Option<usize>,
    /// Whether the drag started the group (not pasting).
    own_group: bool,
    /// Whether the devices of dragged pads were selected already.
    devices_selected: bool,
}

/// Which outline's vertices are edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutlineItem {
    Polygon(Uuid),
    Plane(librepcb_core::project::PlaneId),
    Zone(Uuid),
}

/// A running vertex edit (upstream `mCmdPolygonEdit` etc.).
#[derive(Debug)]
struct VertexEdit {
    item: OutlineItem,
    vertices: Vec<usize>,
    path: Path,
    mark: usize,
}

/// The select tool (upstream `BoardEditorState_Select`).
#[derive(Debug, Default)]
pub(super) struct SelectState {
    /// A paste group is active (upstream `mIsUndoCmdActive`).
    pasting: bool,
    drag: Option<Drag>,
    vertex_edit: Option<VertexEdit>,
    /// Where the left button was pressed (rubber band).
    down_pos: Option<Point>,
    /// The item and position of the last context menu.
    context: Option<(BoardItemRef, Point)>,
    /// Whether a line width was requested with a dialog.
    width_requested: bool,
}

fn drag_text() -> String {
    tr!(
        "librepcb::editor::CmdDragSelectedBoardItems",
        "Drag Board Elements"
    )
}

impl SelectState {
    fn busy(&self) -> bool {
        self.pasting || self.drag.is_some() || self.vertex_edit.is_some()
    }

    fn query<'a>(cx: &'a Cx<'_, '_>, include_locked: bool) -> Option<SelectionQuery<'a>> {
        let board = cx.board().ok()?;
        Some(SelectionQuery::new(
            cx.project(),
            board,
            cx.selection,
            include_locked,
        ))
    }

    /// Applies the current drag preview.
    fn update_drag(&mut self, cx: &mut Cx<'_, '_>) {
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        if !drag.items.has_changes() && drag.mark.is_none() {
            return;
        }
        if drag.mark.is_none() {
            if !cx.is_group_active() {
                if let Err(e) = cx.begin(drag_text()) {
                    cx.error(e);
                    self.drag = None;
                    return;
                }
                drag.own_group = true;
            }
            drag.mark = Some(cx.group_len());
        }
        let result = drag
            .items
            .mutations(cx.project())
            .and_then(|m| cx.preview(drag.mark.unwrap_or(0), m));
        if let Err(e) = result {
            cx.error(e);
            self.abort_command(cx);
        }
    }

    /// Finishes the drag (upstream: execute the drag command).
    fn finish_drag(&mut self, cx: &mut Cx<'_, '_>) {
        self.update_drag(cx);
        if let Some(drag) = self.drag.take()
            && drag.own_group
            && let Err(e) = cx.commit()
        {
            cx.error(e);
        }
    }

    /// Executes a one-shot modification of the selection (keyboard move,
    /// rotate, snap, lock, width, texts reset).
    fn modify_selection(
        &mut self,
        cx: &mut Cx<'_, '_>,
        include_locked: bool,
        include_traces: bool,
        f: impl FnOnce(&mut DragItems),
    ) -> bool {
        let Some(mut query) = Self::query(cx, include_locked) else {
            return false;
        };
        let mut items = DragItems::new(&mut query, include_traces, Point::ORIGIN);
        f(&mut items);
        let result = items
            .mutations(cx.project())
            .and_then(|m| cx.apply(drag_text(), m));
        match result {
            Ok(()) => true,
            Err(e) => {
                cx.error(e);
                false
            }
        }
    }

    fn rotate(&mut self, cx: &mut Cx<'_, '_>, angle: Angle) -> bool {
        if let Some(drag) = self.drag.as_mut() {
            drag.items.rotate(angle, true);
            self.update_drag(cx);
            true
        } else {
            let ignore = cx.ignore_locks();
            self.modify_selection(cx, ignore, false, |i| i.rotate(angle, false))
        }
    }

    fn flip(&mut self, cx: &mut Cx<'_, '_>, orientation: Orientation) -> bool {
        let ignore = cx.ignore_locks();
        let Some(mut query) = Self::query(cx, ignore) else {
            return false;
        };
        let result = flip_mutations(&mut query, orientation).and_then(|m| {
            cx.apply(
                tr!(
                    "librepcb::editor::CmdFlipSelectedBoardItems",
                    "Flip Board Elements"
                ),
                m,
            )
        });
        match result {
            Ok(()) => true,
            Err(e) => {
                cx.error(e);
                false
            }
        }
    }

    fn remove(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        let ignore = cx.ignore_locks();
        let Some(mut query) = Self::query(cx, ignore) else {
            return false;
        };
        query
            .add_devices()
            .add_board_pads()
            .add_vias()
            .add_traces()
            .add_junctions_of_traces(true)
            .add_planes()
            .add_zones()
            .add_polygons()
            .add_board_texts()
            .add_device_texts()
            .add_holes();
        let selection = RemoveSelection {
            devices: query.devices.iter().copied().collect(),
            pads: query.pads.iter().copied().collect(),
            vias: query.vias.iter().copied().collect(),
            junctions: Vec::new(),
            traces: query.traces.iter().copied().collect(),
            planes: query.planes.iter().copied().collect(),
            zones: query.zones.iter().copied().collect(),
            polygons: query.polygons.iter().copied().collect(),
            stroke_texts: query.stroke_texts.iter().copied().collect(),
            device_texts: query.device_texts.iter().copied().collect(),
            holes: query.holes.iter().copied().collect(),
        };
        let board = cx.board_id();
        cx.selection.clear();
        let before: std::collections::BTreeMap<NetSegmentId, BoardNetSegment> = cx
            .board()
            .map(|b| {
                b.net_segments()
                    .iter()
                    .map(|(id, s)| (*id, s.clone()))
                    .collect()
            })
            .unwrap_or_default();
        match cx.exec(RemoveBoardItems {
            board: Some(board),
            selection,
        }) {
            Ok(_) => {
                // Upstream: `CmdSimplifyBoardNetSegments` of the modified
                // segments as a separate undo step.
                let modified: Vec<NetSegmentId> = cx
                    .board()
                    .map(|b| {
                        b.net_segments()
                            .iter()
                            .filter(|(id, s)| before.get(id) != Some(*s))
                            .map(|(id, _)| *id)
                            .collect()
                    })
                    .unwrap_or_default();
                super::draw_trace::simplify_segments(cx, modified);
                true
            }
            Err(e) => {
                cx.error(e);
                false
            }
        }
    }

    fn copy(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        let cursor = cx.cursor_pos;
        let Some(mut query) = Self::query(cx, true) else {
            return false;
        };
        match BoardClipboardData::from_selection(&mut query, cursor).and_then(|data| data.to_zip())
        {
            Ok(zip) => {
                let mime = board_clipboard_mime_type(&cx.settings.app_version);
                cx.ctx.clipboard.set(&mime, zip);
                cx.status(
                    tr!(
                        "librepcb::editor::BoardEditorState_Select",
                        "Copied to clipboard!"
                    ),
                    Some(2000),
                );
            }
            Err(e) => cx.error(e),
        }
        true
    }

    fn paste(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        let version = &cx.settings.app_version;
        let result = if let Some(zip) = cx.ctx.clipboard.get(&board_clipboard_mime_type(version)) {
            BoardClipboardData::from_zip(&zip)
        } else if let Some(zip) = cx
            .ctx
            .clipboard
            .get(&footprint_clipboard_mime_type(version))
        {
            // Graphical elements from the package editor.
            FootprintClipboardData::from_bytes(&zip)
                .and_then(|data| BoardClipboardData::from_footprint_data(&data))
        } else {
            return false;
        };
        let data = match result {
            Ok(data) => data,
            Err(e) => {
                cx.error(e);
                return false;
            }
        };
        self.start_paste(cx, data, None)
    }

    /// Upstream `startPaste()`: pastes the items and lets them follow the
    /// cursor, or places them at `fixed_offset` and finishes.
    fn start_paste(
        &mut self,
        cx: &mut Cx<'_, '_>,
        data: BoardClipboardData,
        fixed_offset: Option<Point>,
    ) -> bool {
        cx.selection.clear();
        if let Err(e) = cx.begin(tr!(
            "librepcb::editor::BoardEditorState_Select",
            "Paste board elements"
        )) {
            cx.error(e);
            return false;
        }
        self.pasting = true;
        let start = cx.cursor_pos;
        let offset =
            fixed_offset.unwrap_or_else(|| (start - data.cursor_pos).mapped_to_grid(cx.grid()));
        let board = cx.board_id();
        match cx.exec(PasteBoardItems {
            board,
            data,
            offset,
        }) {
            Ok(items) if !items.is_empty() && fixed_offset.is_some() => {
                self.pasting = false;
                if let Err(e) = cx.commit() {
                    cx.error(e);
                }
                cx.selection.replace(items);
                true
            }
            Ok(items) if !items.is_empty() => {
                cx.selection.replace(items);
                let Some(mut query) = Self::query(cx, true) else {
                    return false;
                };
                let items = DragItems::new(&mut query, false, start);
                self.drag = Some(Drag {
                    items,
                    mark: None,
                    own_group: false,
                    devices_selected: true,
                });
                true
            }
            Ok(_) => {
                cx.abort();
                self.pasting = false;
                false
            }
            Err(e) => {
                cx.error(e);
                self.abort_command(cx);
                false
            }
        }
    }

    /// Upstream `processImportDxf()` (the dialog's choices are the
    /// settings): reads the DXF file and pastes its polygons and holes.
    fn import_dxf(&mut self, cx: &mut Cx<'_, '_>, settings: &DxfImportSettings) -> bool {
        if self.busy() {
            return false;
        }
        let result = (|| -> crate::error::Result<BoardClipboardData> {
            let (paths, circles) = crate::fsm::read_dxf_import(settings)?;
            let board_uuid = cx.board()?.uuid();
            let mut data = BoardClipboardData::new(board_uuid, Point::ORIGIN)?;
            for path in paths {
                data.polygons.push(BoardPolygonData::new(
                    Uuid::new_random(),
                    settings.layer,
                    settings.line_width,
                    path,
                    false,
                    false,
                    false,
                ));
            }
            for circle in &circles {
                if settings.circles_as_drills {
                    data.holes.push(BoardHoleData::new(
                        Uuid::new_random(),
                        circle.diameter,
                        NonEmptyPath::from_point(circle.position),
                        MaskConfig::Automatic,
                        false,
                    ));
                } else {
                    data.polygons.push(BoardPolygonData::new(
                        Uuid::new_random(),
                        settings.layer,
                        settings.line_width,
                        Path::circle(circle.diameter).translated(circle.position),
                        false,
                        false,
                        false,
                    ));
                }
            }
            Ok(data)
        })();
        match result {
            Ok(data) => self.start_paste(cx, data, settings.placement),
            Err(e) => {
                cx.error(e);
                self.abort_command(cx);
                false
            }
        }
    }

    fn abort_command(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        let own = self.drag.as_ref().is_some_and(|d| d.own_group);
        let edit = self.vertex_edit.is_some();
        self.vertex_edit = None;
        self.drag = None;
        if self.pasting || own || edit {
            cx.abort();
        }
        self.pasting = false;
        true
    }

    /// Finds the vertices of a selected polygon, plane or zone at `pos`
    /// (upstream `find*VerticesAtPosition()`).
    fn find_vertices(
        &self,
        cx: &Cx<'_, '_>,
        pos: Point,
    ) -> Option<(OutlineItem, Path, Vec<usize>)> {
        let board = cx.board().ok()?;
        let tolerance = cx.ctx.view.tolerance();
        let unlocked = |locked: bool| !locked || cx.ignore_locks();
        let at = |path: &Path| -> Vec<usize> {
            path.vertices()
                .iter()
                .enumerate()
                .filter(|(_, v)| *(v.pos - pos).length() <= tolerance)
                .map(|(i, _)| i)
                .collect()
        };
        for (u, p) in board.polygons() {
            if cx.selection.contains(BoardItemRef::Polygon(*u)) && unlocked(p.locked()) {
                let v = at(p.path());
                if !v.is_empty() {
                    return Some((OutlineItem::Polygon(*u), p.path().clone(), v));
                }
            }
        }
        for (id, p) in board.planes() {
            if cx.selection.contains(BoardItemRef::Plane(*id)) && unlocked(p.locked()) {
                let v = at(p.outline());
                if !v.is_empty() {
                    return Some((OutlineItem::Plane(*id), p.outline().clone(), v));
                }
            }
        }
        for (u, z) in board.zones() {
            if cx.selection.contains(BoardItemRef::Zone(*u)) && unlocked(z.locked()) {
                let v = at(z.outline());
                if !v.is_empty() {
                    return Some((OutlineItem::Zone(*u), z.outline().clone(), v));
                }
            }
        }
        None
    }

    /// The mutation setting the outline of an item.
    fn outline_mutation(cx: &Cx<'_, '_>, item: OutlineItem, path: Path) -> Option<Mutation> {
        let board = cx.board().ok()?;
        let b = cx.board_id();
        Some(Mutation::Board(match item {
            OutlineItem::Polygon(u) => {
                let mut p = board.polygons().get(&u)?.clone();
                p.set_path(path);
                BoardMutation::UpdateItem {
                    board: b,
                    item: BoardItem::Polygon(p),
                }
            }
            OutlineItem::Plane(id) => {
                let mut p = board.plane(id)?.clone();
                p.set_outline(path);
                BoardMutation::UpdatePlane { board: b, plane: p }
            }
            OutlineItem::Zone(u) => {
                let mut z = board.zones().get(&u)?.clone();
                z.set_outline(path);
                BoardMutation::UpdateItem {
                    board: b,
                    item: BoardItem::Zone(z),
                }
            }
        }))
    }

    fn outline_edit_text(item: OutlineItem) -> String {
        match item {
            OutlineItem::Polygon(_) => tr!("librepcb::editor::CmdBoardPolygonEdit", "Edit polygon"),
            OutlineItem::Plane(_) => tr!("librepcb::editor::CmdBoardPlaneEdit", "Edit plane"),
            OutlineItem::Zone(_) => tr!("librepcb::editor::CmdBoardZoneEdit", "Edit zone"),
        }
    }

    fn start_vertex_edit(
        &mut self,
        cx: &mut Cx<'_, '_>,
        item: OutlineItem,
        path: Path,
        vertices: Vec<usize>,
    ) {
        if let Err(e) = cx.begin(Self::outline_edit_text(item)) {
            cx.error(e);
            return;
        }
        let mark = cx.group_len();
        self.vertex_edit = Some(VertexEdit {
            item,
            vertices,
            path,
            mark,
        });
    }

    fn move_vertices(&mut self, cx: &mut Cx<'_, '_>, pos: Point) {
        let grid = cx.grid();
        let Some(edit) = self.vertex_edit.as_mut() else {
            return;
        };
        let mut vertices: Vec<Vertex> = edit.path.vertices().to_vec();
        for i in &edit.vertices {
            if let Some(v) = vertices.get_mut(*i) {
                v.pos = pos.mapped_to_grid(grid);
            }
        }
        let (item, mark) = (edit.item, edit.mark);
        if let Some(m) = Self::outline_mutation(cx, item, Path::new(vertices))
            && let Err(e) = cx.preview(mark, vec![m])
        {
            cx.error(e);
        }
    }

    /// Upstream `startAdding*Vertex()`: inserts a vertex after the line at
    /// `pos` and starts moving it.
    fn start_adding_vertex(&mut self, cx: &mut Cx<'_, '_>, item: BoardItemRef, pos: Point) {
        let Some((outline, path)) = self.outline_of(cx, item) else {
            return;
        };
        let Some(index) = line_index_at(&path, pos, cx.ctx.view.tolerance()) else {
            return;
        };
        let mut vertices = path.vertices().to_vec();
        let angle = vertices[index - 1].angle;
        vertices.insert(index, Vertex::new(pos.mapped_to_grid(cx.grid()), angle));
        let new_path = Path::new(vertices);
        self.start_vertex_edit(cx, outline, new_path.clone(), vec![index]);
        if let Some(edit) = self.vertex_edit.as_mut() {
            edit.path = new_path.clone();
        }
        if let Some(edit) = &self.vertex_edit
            && let Some(m) = Self::outline_mutation(cx, edit.item, new_path)
        {
            let mark = edit.mark;
            if let Err(e) = cx.preview(mark, vec![m]) {
                cx.error(e);
            }
        }
    }

    fn outline_of(&self, cx: &Cx<'_, '_>, item: BoardItemRef) -> Option<(OutlineItem, Path)> {
        let board = cx.board().ok()?;
        match item {
            BoardItemRef::Polygon(u) => Some((
                OutlineItem::Polygon(u),
                board.polygons().get(&u)?.path().clone(),
            )),
            BoardItemRef::Plane(id) => {
                Some((OutlineItem::Plane(id), board.plane(id)?.outline().clone()))
            }
            BoardItemRef::Zone(u) => Some((
                OutlineItem::Zone(u),
                board.zones().get(&u)?.outline().clone(),
            )),
            _ => None,
        }
    }

    /// Upstream `remove*Vertices()`.
    fn remove_vertices(&mut self, cx: &mut Cx<'_, '_>, item: BoardItemRef, pos: Point) {
        let Some((outline, path)) = self.outline_of(cx, item) else {
            return;
        };
        let tolerance = cx.ctx.view.tolerance();
        let remove: Vec<usize> = path
            .vertices()
            .iter()
            .enumerate()
            .filter(|(_, v)| *(v.pos - pos).length() <= tolerance)
            .map(|(i, _)| i)
            .collect();
        let mut new_path = Path::new(
            path.vertices()
                .iter()
                .enumerate()
                .filter(|(i, _)| !remove.contains(i))
                .map(|(_, v)| *v)
                .collect(),
        );
        if matches!(outline, OutlineItem::Zone(_)) {
            new_path.open();
        } else {
            if path.is_closed() && new_path.vertices().len() > 2 {
                new_path.close();
            }
            if new_path.is_closed() && new_path.vertices().len() == 3 {
                new_path.vertices_mut().pop(); // Avoid overlapping lines.
            }
        }
        if new_path.vertices().len() < 2 {
            return; // Do not allow to create invalid outlines!
        }
        if let Some(m) = Self::outline_mutation(cx, outline, new_path)
            && let Err(e) = cx.apply(Self::outline_edit_text(outline), vec![m])
        {
            cx.error(e);
        }
    }

    fn change_line_width(&mut self, cx: &mut Cx<'_, '_>, step: i32) -> bool {
        let Some(mut query) = Self::query(cx, true) else {
            return false;
        };
        let items = DragItems::new(&mut query, true, Point::ORIGIN);
        if items.is_empty() {
            return false;
        }
        let current = items.median_line_width();
        let mut width = None;
        if step != 0 {
            let board = match cx.board() {
                Ok(b) => b,
                Err(_) => return false,
            };
            let mut widths: BTreeSet<UnsignedLength> = BTreeSet::new();
            let mut add = |w: UnsignedLength| {
                if w != current && ((w > current) == (step > 0)) {
                    widths.insert(w);
                }
            };
            if items.has_traces() {
                for seg in board.net_segments().values() {
                    for t in seg.traces().values() {
                        add(t.width().into());
                    }
                }
            }
            if items.has_polygons() {
                for p in board.polygons().values() {
                    add(p.line_width());
                }
            }
            if items.has_texts() {
                for t in board.stroke_texts().values() {
                    add(t.stroke_width());
                }
                for d in board.devices().values() {
                    for t in d.stroke_texts().values() {
                        add(t.stroke_width());
                    }
                }
            }
            width = if step > 0 {
                widths.first().copied()
            } else {
                widths.last().copied()
            };
        }
        match width {
            Some(w) => self.set_line_width(cx, w),
            None => {
                // Ask for a custom value.
                self.width_requested = true;
                cx.out
                    .requests
                    .push(BoardRequest::LineWidthDialog { current });
                true
            }
        }
    }

    fn set_line_width(&mut self, cx: &mut Cx<'_, '_>, width: UnsignedLength) -> bool {
        let ok = self.modify_selection(cx, true, true, |i| i.set_line_width(width));
        if ok {
            let unit = cx
                .board()
                .map(|b| b.settings().grid_unit)
                .unwrap_or_default();
            cx.status(format_length(unit, *width), Some(5000));
        }
        ok
    }

    /// Upstream `measureSelectedItems()`.
    fn measure_selected_traces(&mut self, cx: &mut Cx<'_, '_>, seg: NetSegmentId, trace: Uuid) {
        let Ok(board) = cx.board() else {
            return;
        };
        let p = cx.project();
        let Some(segment) = board.net_segment(seg) else {
            return;
        };
        let length = |t: &librepcb_core::geometry::Trace| -> Length {
            match (
                board.anchor_position(segment, t.p1(), p.library(), p.circuit()),
                board.anchor_position(segment, t.p2(), p.library(), p.circuit()),
            ) {
                (Some(a), Some(b)) => *(b - a).length(),
                _ => Length::ZERO,
            }
        };
        let Some(start) = segment.traces().get(&trace) else {
            return;
        };
        let mut visited: BTreeSet<Uuid> = BTreeSet::from([trace]);
        let mut total = length(start);
        let mut error = None;
        for backwards in [false, true] {
            let mut anchor = if backwards { start.p1() } else { start.p2() };
            loop {
                let mut next: Option<&librepcb_core::geometry::Trace> = None;
                for t in segment.traces_at(anchor) {
                    if visited.contains(&t.uuid()) {
                        continue;
                    }
                    if cx.selection.contains(BoardItemRef::Trace(seg, t.uuid())) {
                        if next.is_some() {
                            error = Some(tr!(
                                "librepcb::editor::BoardEditorState_Select",
                                "Selected trace segments may not branch!"
                            ));
                            break;
                        }
                        total += length(t);
                        next = Some(t);
                        visited.insert(t.uuid());
                    }
                }
                if error.is_some() {
                    break;
                }
                match next {
                    Some(t) => anchor = if t.p1() == anchor { t.p2() } else { t.p1() },
                    None => break,
                }
            }
        }
        if let Some(e) = error {
            cx.error(e);
            return;
        }
        let selected = cx
            .selection
            .items()
            .iter()
            .filter(|i| matches!(i, BoardItemRef::Trace(..)))
            .count();
        let title = tr!(
            "librepcb::editor::BoardEditorState_Select",
            "Measurement Result"
        );
        let mut text = trn!(
            "librepcb::editor::BoardEditorState_Select",
            "Total length of {n} trace segment(s): {0} mm / {1} in",
            "Total length of {n} trace segment(s): {0} mm / {1} in",
            visited.len(),
            float_to_string(total.to_mm(), 6),
            float_to_string(total.to_inch(), 6)
        );
        let warning = selected != visited.len();
        if warning {
            text += "\n\n";
            text += &tr!(
                "librepcb::editor::BoardEditorState_Select",
                "WARNING: There are {0} trace segments selected, but not all of them are connected!",
                selected
            );
        }
        cx.out.requests.push(BoardRequest::Message {
            title,
            text,
            warning,
        });
    }

    fn open_properties(&self, cx: &mut Cx<'_, '_>, item: BoardItemRef) -> bool {
        let item = match item {
            BoardItemRef::FootprintPad(c, _) => BoardItemRef::Device(c),
            BoardItemRef::Junction(..) | BoardItemRef::Trace(..) => return false,
            other => other,
        };
        cx.out.requests.push(BoardRequest::Properties(item));
        true
    }

    fn left_pressed(
        &mut self,
        cx: &mut Cx<'_, '_>,
        pos: Point,
        control: bool,
        shift: bool,
    ) -> bool {
        if self.pasting {
            // Place the pasted items.
            if let Some(drag) = self.drag.as_mut() {
                drag.items.set_current_position(pos, true);
            }
            self.update_drag(cx);
            self.drag = None;
            if let Err(e) = cx.commit() {
                cx.error(e);
            }
            self.pasting = false;
            return true;
        }
        if self.busy() {
            return false;
        }
        if let Some((item, path, vertices)) = self.find_vertices(cx, pos) {
            self.start_vertex_edit(cx, item, path, vertices);
            return true;
        }
        let mut flags = FindFlags::ALL | FindFlags::ACCEPT_NEAR_MATCH;
        if control {
            // For multi-selection, individual pads make no sense.
            flags |= FindFlags::DEVICES_OF_PADS;
        }
        let items = cx.find_items(pos, flags, &FindFilter::default());
        if items.is_empty() {
            // No items under the mouse: start the rubber band.
            cx.selection.clear();
            self.down_pos = Some(pos);
            return true;
        }
        let selected = items.iter().copied().find(|i| cx.selection.contains(*i));
        if control {
            // Toggle the selection.
            let item = selected.unwrap_or(items[0]);
            let sel = cx.selection.contains(item);
            cx.selection.set(item, !sel);
        } else if shift {
            // Cycle the selection.
            let next = items
                .iter()
                .position(|i| cx.selection.contains(*i))
                .map_or(0, |i| (i + 1) % items.len());
            cx.selection.clear();
            cx.selection.set(items[next], true);
        } else if selected.is_none() {
            cx.selection.clear();
            cx.selection.set(items[0], true);
        }
        // Start moving the selected items.
        let ignore = cx.ignore_locks();
        if let Some(mut query) = Self::query(cx, ignore) {
            let items = DragItems::new(&mut query, false, pos);
            self.drag = Some(Drag {
                items,
                mark: None,
                own_group: false,
                devices_selected: false,
            });
        }
        true
    }

    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_>, pos: Point, left: bool) -> bool {
        if let Some(drag) = self.drag.as_mut() {
            // Select the devices of dragged pads.
            if !drag.devices_selected {
                drag.devices_selected = true;
                let devices: Vec<_> = drag.items.auto_selected_devices().iter().copied().collect();
                for c in devices {
                    cx.selection.set(BoardItemRef::Device(c), true);
                }
            }
            drag.items.set_current_position(pos, true);
            self.update_drag(cx);
            return true;
        }
        if self.vertex_edit.is_some() {
            self.move_vertices(cx, pos);
            return true;
        }
        if left && let Some(down) = self.down_pos {
            // Rubber band selection.
            cx.out.view.rubber_band = Some((down, pos));
            let items = rect_items(cx, down, pos);
            cx.selection.replace(items);
            return true;
        }
        // Hover (not upstream: the item under the cursor).
        let hover = cx.find_item(pos, FindFlags::ALL, &FindFilter::default());
        cx.out.hovered = hover;
        false
    }

    fn left_released(&mut self, cx: &mut Cx<'_, '_>, pos: Point) -> bool {
        if !self.pasting
            && let Some(drag) = self.drag.as_mut()
        {
            drag.items.set_current_position(pos, true);
            self.finish_drag(cx);
            return true;
        }
        if let Some(edit) = self.vertex_edit.take() {
            if let Err(e) = cx.commit() {
                cx.error(e);
            }
            let _ = edit;
            return false;
        }
        cx.out.view.rubber_band = None;
        self.down_pos = None;
        true
    }

    fn double_clicked(&mut self, cx: &mut Cx<'_, '_>, pos: Point) -> bool {
        if self.drag.is_some() || self.vertex_edit.is_some() {
            return false;
        }
        let items = cx.find_items(
            pos,
            FindFlags::ALL | FindFlags::DEVICES_OF_PADS | FindFlags::ACCEPT_NEAR_MATCH,
            &FindFilter::default(),
        );
        for item in items {
            if cx.selection.contains(item) && self.open_properties(cx, item) {
                return true;
            } else if let BoardItemRef::Device(_) = item {
                // Select the device by double-clicking one of its pads.
                cx.selection.set(item, true);
                return true;
            }
        }
        false
    }

    fn right_released(&mut self, cx: &mut Cx<'_, '_>, pos: Point) -> bool {
        if self.drag.is_some() {
            return self.rotate(cx, Angle::DEG90);
        }
        if self.vertex_edit.is_some() {
            return true;
        }
        let items = cx.find_items(
            pos,
            FindFlags::ALL | FindFlags::DEVICES_OF_PADS | FindFlags::ACCEPT_NEAR_MATCH,
            &FindFilter::default(),
        );
        if items.is_empty() {
            return false;
        }
        let item = match items.iter().copied().find(|i| cx.selection.contains(*i)) {
            Some(i) => i,
            None => {
                cx.selection.clear();
                cx.selection.set(items[0], true);
                items[0]
            }
        };
        let item = match item {
            BoardItemRef::FootprintPad(c, _) => {
                cx.selection.set(BoardItemRef::Device(c), true);
                BoardItemRef::Device(c)
            }
            other => other,
        };
        let menu = context_menu(cx, item, pos);
        if menu.is_empty() {
            return true;
        }
        self.context = Some((item, pos));
        cx.out.requests.push(BoardRequest::ContextMenu {
            pos,
            item,
            items: menu,
        });
        true
    }

    fn context_action(&mut self, cx: &mut Cx<'_, '_>, action: ContextAction) -> bool {
        let Some((item, pos)) = self.context else {
            return false;
        };
        match action {
            ContextAction::Properties => self.open_properties(cx, item),
            ContextAction::RotateCcw => self.rotate(cx, Angle::DEG90),
            ContextAction::RotateCw => self.rotate(cx, -Angle::DEG90),
            ContextAction::FlipHorizontal => self.flip(cx, Orientation::Horizontal),
            ContextAction::FlipVertical => self.flip(cx, Orientation::Vertical),
            ContextAction::Remove => self.remove(cx),
            ContextAction::Cut => {
                self.copy(cx);
                self.remove(cx)
            }
            ContextAction::Copy => self.copy(cx),
            ContextAction::SnapToGrid => {
                let ignore = cx.ignore_locks();
                self.modify_selection(cx, ignore, false, |i| i.snap_to_grid())
            }
            ContextAction::Lock(locked) => {
                self.modify_selection(cx, true, false, |i| i.set_locked(locked))
            }
            ContextAction::ResetTexts => {
                self.modify_selection(cx, true, false, |i| i.reset_all_texts())
            }
            ContextAction::ChangeFootprint(footprint) => {
                let BoardItemRef::Device(c) = item else {
                    return false;
                };
                let Some(device) = cx
                    .board()
                    .ok()
                    .and_then(|b| b.device(c))
                    .map(|d| d.lib_device())
                else {
                    return false;
                };
                let board = cx.board_id();
                if let Err(e) = cx.exec(crate::commands::ReplaceDevice {
                    component: c.into(),
                    board: Some(board),
                    device,
                    footprint: Some(footprint),
                }) {
                    cx.error(e);
                }
                true
            }
            ContextAction::ChangeDevice(device) => {
                let BoardItemRef::Device(c) = item else {
                    return false;
                };
                let board = cx.board_id();
                if let Err(e) = cx.exec(crate::commands::ReplaceDevice {
                    component: c.into(),
                    board: Some(board),
                    device,
                    footprint: None,
                }) {
                    cx.error(e);
                }
                true
            }
            ContextAction::ChangeModel(model) => {
                let BoardItemRef::Device(c) = item else {
                    return false;
                };
                let Some(mut device) = cx.board().ok().and_then(|b| b.device(c)).cloned() else {
                    return false;
                };
                device.set_lib_model(model);
                let board = cx.board_id();
                if let Err(e) = cx.apply(
                    tr!("librepcb::editor::CmdDeviceInstanceEdit", "Edit Device"),
                    vec![Mutation::Board(BoardMutation::UpdateDevice {
                        board,
                        device,
                    })],
                ) {
                    cx.error(e);
                }
                true
            }
            ContextAction::SetLineWidth => self.change_line_width(cx, 0),
            ContextAction::RemoveWholeTrace => {
                if let Some(seg) = segment_of(cx, item) {
                    let items = cx
                        .board()
                        .ok()
                        .and_then(|b| b.net_segment(seg).map(|s| segment_items(seg, s, false)));
                    for i in items.unwrap_or_default() {
                        cx.selection.set(i, true);
                    }
                }
                self.remove(cx)
            }
            ContextAction::SelectWholeTrace => {
                if let Some(seg) = segment_of(cx, item) {
                    let items = cx
                        .board()
                        .ok()
                        .and_then(|b| b.net_segment(seg).map(|s| segment_items(seg, s, false)));
                    for i in items.unwrap_or_default() {
                        cx.selection.set(i, true);
                    }
                }
                true
            }
            ContextAction::MeasureLength => {
                let trace = match item {
                    BoardItemRef::Trace(s, u) => Some((s, u)),
                    BoardItemRef::Junction(s, j) => cx.board().ok().and_then(|b| {
                        b.net_segment(s)?
                            .traces_at(TraceAnchor::Junction(j))
                            .next()
                            .map(|t| (s, t.uuid()))
                    }),
                    _ => None,
                };
                if let Some((s, u)) = trace {
                    cx.selection.set(BoardItemRef::Trace(s, u), true);
                    self.measure_selected_traces(cx, s, u);
                }
                true
            }
            ContextAction::RemoveVertices => {
                self.remove_vertices(cx, item, pos);
                true
            }
            ContextAction::AddVertex => {
                self.start_adding_vertex(cx, item, pos);
                true
            }
            // Plane visibility is a view setting of the application.
            ContextAction::PlaneVisible(_) => false,
        }
    }

    /// Upstream `updateAvailableFeatures()` + `processSelection()`.
    pub(super) fn update_features(&self, cx: &mut Cx<'_, '_>, cross_probe: bool) {
        if self.vertex_edit.is_some() {
            return;
        }
        let mut features = Features::default();
        if self.drag.is_none() {
            features.select = true;
            features.import_graphics = true;
        }
        let version = &cx.settings.app_version;
        features.paste = [
            board_clipboard_mime_type(version),
            footprint_clipboard_mime_type(version),
        ]
        .iter()
        .any(|mime| cx.ctx.clipboard.get(mime).is_some());
        let grid = cx.grid();
        let Some(mut query) = Self::query(cx, true) else {
            return;
        };
        query.add_all();
        let mut info = String::new();
        if !query.is_empty() {
            features.cut = true;
            features.copy = true;
            features.remove = true;
            features.rotate = true;
            features.flip = true;
        }
        if !query.devices.is_empty() {
            features.reset_texts = true;
        }
        if !query.traces.is_empty() {
            features.modify_line_width = true;
        }
        let board = query.board();
        let mut lockable = |on_grid: bool, locked: bool| {
            if !on_grid {
                features.snap_to_grid = true;
            }
            if locked {
                features.unlock = true;
            } else {
                features.lock = true;
            }
        };
        for c in &query.devices {
            if let Some(d) = board.device(*c) {
                lockable(d.position().is_on_grid(grid), d.locked());
            }
        }
        for (s, u) in &query.pads {
            if let Some(p) = board.net_segment(*s).and_then(|seg| seg.pads().get(u)) {
                lockable(p.pad().position().is_on_grid(grid), p.locked());
            }
        }
        for id in &query.planes {
            if let Some(p) = board.plane(*id) {
                lockable(p.outline().is_on_grid(grid), p.locked());
            }
        }
        for u in &query.zones {
            if let Some(z) = board.zones().get(u) {
                lockable(z.outline().is_on_grid(grid), z.locked());
            }
        }
        for u in &query.polygons {
            if let Some(p) = board.polygons().get(u) {
                lockable(p.path().is_on_grid(grid), p.locked());
            }
        }
        for u in &query.stroke_texts {
            if let Some(t) = board.stroke_texts().get(u) {
                lockable(t.position().is_on_grid(grid), t.locked());
            }
        }
        for u in &query.holes {
            if let Some(h) = board.holes().get(u) {
                lockable(h.path().first().pos.is_on_grid(grid), h.locked());
            }
        }
        for (s, u) in &query.vias {
            if let Some(v) = board.net_segment(*s).and_then(|seg| seg.vias().get(u))
                && !v.position().is_on_grid(grid)
            {
                features.snap_to_grid = true;
            }
        }
        for (s, u) in &query.junctions {
            if let Some(j) = board.net_segment(*s).and_then(|seg| seg.junctions().get(u))
                && !j.position().is_on_grid(grid)
            {
                features.snap_to_grid = true;
            }
        }
        query.add_footprint_pads();
        if !query.devices.is_empty()
            || !query.pads.is_empty()
            || !query.footprint_pads.is_empty()
            || !query.vias.is_empty()
            || !query.planes.is_empty()
            || !query.zones.is_empty()
            || !query.polygons.is_empty()
            || !query.stroke_texts.is_empty()
            || !query.holes.is_empty()
        {
            features.properties = true;
        }
        let probe = info_box(&query, &mut info);
        cx.out.view.features = features;
        cx.out.view.info_box = info;
        if cross_probe {
            cx.highlight_nets(probe.nets.iter().copied());
            cx.out.cross_probe = probe;
        }
    }
}

impl State for SelectState {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        cx.set_cursor(None);
        self.update_features(cx, true);
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.abort_command(cx);
        // Avoid propagating the selection to other tools.
        cx.selection.clear();
        cx.out.view.info_box.clear();
        cx.out.view.features = Features::default();
        cx.out.view.rubber_band = None;
        cx.out.hovered = None;
        cx.highlight_nets([]);
        cx.out.cross_probe = CrossProbe::default();
        true
    }

    fn process(&mut self, cx: &mut Cx<'_, '_>, input: &BoardFsmInput) -> bool {
        let selection_rev = cx.selection.revision();
        let handled = match input {
            BoardFsmInput::SelectAll => {
                if self.busy() {
                    false
                } else {
                    let items = cx.board().map(all_items).unwrap_or_default();
                    cx.selection.replace(items);
                    true
                }
            }
            BoardFsmInput::Cut => !self.busy() && self.copy(cx) && self.remove(cx),
            BoardFsmInput::Copy => !self.busy() && self.copy(cx),
            BoardFsmInput::Paste => !self.busy() && self.paste(cx),
            BoardFsmInput::Move(delta) => {
                if self.busy() {
                    false
                } else {
                    let ignore = cx.ignore_locks();
                    let delta = *delta;
                    self.modify_selection(cx, ignore, false, |i| {
                        i.set_current_position(delta, false)
                    })
                }
            }
            BoardFsmInput::Rotate(angle) => self.vertex_edit.is_none() && self.rotate(cx, *angle),
            BoardFsmInput::Flip(o) => !self.busy() && self.flip(cx, *o),
            BoardFsmInput::SnapToGrid => {
                if self.busy() {
                    false
                } else {
                    let ignore = cx.ignore_locks();
                    self.modify_selection(cx, ignore, false, |i| i.snap_to_grid())
                }
            }
            BoardFsmInput::SetLocked(locked) => {
                let locked = *locked;
                !self.busy() && self.modify_selection(cx, true, false, |i| i.set_locked(locked))
            }
            BoardFsmInput::ChangeLineWidth(step) => {
                !self.busy() && self.change_line_width(cx, *step)
            }
            BoardFsmInput::SetLineWidth(width) => {
                if std::mem::take(&mut self.width_requested) && !self.busy() {
                    self.set_line_width(cx, *width)
                } else {
                    false
                }
            }
            BoardFsmInput::ResetAllTexts => {
                !self.busy() && self.modify_selection(cx, true, false, |i| i.reset_all_texts())
            }
            BoardFsmInput::Remove => !self.busy() && self.remove(cx),
            BoardFsmInput::EditProperties => {
                if self.busy() {
                    false
                } else {
                    let Some(mut query) = Self::query(cx, true) else {
                        return false;
                    };
                    query.add_devices();
                    query.add_devices_of_selected_pads();
                    query
                        .add_board_pads()
                        .add_vias()
                        .add_planes()
                        .add_zones()
                        .add_polygons()
                        .add_board_texts()
                        .add_device_texts()
                        .add_holes();
                    let item = query
                        .devices
                        .iter()
                        .map(|c| BoardItemRef::Device(*c))
                        .chain(query.pads.iter().map(|(s, u)| BoardItemRef::Pad(*s, *u)))
                        .chain(query.vias.iter().map(|(s, u)| BoardItemRef::Via(*s, *u)))
                        .chain(query.planes.iter().map(|p| BoardItemRef::Plane(*p)))
                        .chain(query.zones.iter().map(|u| BoardItemRef::Zone(*u)))
                        .chain(query.polygons.iter().map(|u| BoardItemRef::Polygon(*u)))
                        .chain(
                            query
                                .stroke_texts
                                .iter()
                                .map(|u| BoardItemRef::StrokeText(*u)),
                        )
                        .chain(
                            query
                                .device_texts
                                .iter()
                                .map(|(c, u)| BoardItemRef::DeviceStrokeText(*c, *u)),
                        )
                        .chain(query.holes.iter().map(|u| BoardItemRef::Hole(*u)))
                        .next();
                    match item {
                        Some(item) => self.open_properties(cx, item),
                        None => false,
                    }
                }
            }
            BoardFsmInput::Abort => {
                self.abort_command(cx);
                cx.selection.clear();
                cx.out.view.rubber_band = None;
                self.down_pos = None;
                true
            }
            BoardFsmInput::PointerMoved(e) => {
                let left = cx.left_button;
                self.pointer_moved(cx, e.pos, left)
            }
            BoardFsmInput::LeftPressed(e) => {
                self.left_pressed(cx, e.pos, e.modifiers.control, e.modifiers.shift)
            }
            BoardFsmInput::LeftReleased(e) => self.left_released(cx, e.pos),
            BoardFsmInput::LeftDoubleClicked(e) => {
                if e.modifiers.shift || e.modifiers.control {
                    self.left_pressed(cx, e.pos, e.modifiers.control, e.modifiers.shift)
                } else {
                    self.double_clicked(cx, e.pos)
                }
            }
            BoardFsmInput::RightReleased(e) => self.right_released(cx, e.pos),
            BoardFsmInput::ContextMenu(action) => self.context_action(cx, *action),
            BoardFsmInput::ImportDxf(settings) => self.import_dxf(cx, settings),
            _ => false,
        };
        // Upstream updates the features on selection and undo stack changes
        // (delayed); pointer moves only change the selection.
        if !matches!(input, BoardFsmInput::PointerMoved(_))
            || cx.selection.revision() != selection_rev
        {
            self.update_features(cx, true);
        }
        handled
    }

    fn tool_data(&self, _cx: &Cx<'_, '_>) -> BoardToolData {
        BoardToolData::default()
    }
}

/// The net segment of a net segment element.
fn segment_of(cx: &Cx<'_, '_>, item: BoardItemRef) -> Option<NetSegmentId> {
    let board = cx.board().ok()?;
    item.resolved(board)?.segment()
}

/// The index of the vertex after the line of `path` at `pos` (upstream
/// `getLineIndexAtPosition()`, returning the index of the vertex to
/// insert).
fn line_index_at(path: &Path, pos: Point, tolerance: Length) -> Option<usize> {
    path.vertices().windows(2).enumerate().find_map(|(i, w)| {
        let (d, _) = toolbox::shortest_distance_between_point_and_line(pos, w[0].pos, w[1].pos);
        (*d <= tolerance).then_some(i + 1)
    })
}

/// The items of a rubber band selection (upstream
/// `BoardGraphicsScene::selectItemsInRect()`): devices are selected if one
/// of their pads is in the rectangle.
fn rect_items(cx: &Cx<'_, '_>, p1: Point, p2: Point) -> Vec<BoardItemRef> {
    let mut items: Vec<BoardItemRef> = Vec::new();
    let board = cx.board().ok();
    for item in cx.ctx.view.items_in_rect(p1, p2) {
        let item = match (item, board) {
            (BoardItemRef::FootprintPad(c, _), _) => BoardItemRef::Device(c),
            (BoardItemRef::StrokeText(_), Some(b)) => item.resolved(b).unwrap_or(item),
            _ => item,
        };
        if !items.contains(&item) {
            items.push(item);
        }
    }
    // Junctions (not in the scene).
    if let Some(b) = board {
        let (x0, x1) = (p1.x.min(p2.x), p1.x.max(p2.x));
        let (y0, y1) = (p1.y.min(p2.y), p1.y.max(p2.y));
        for seg in b.net_segments().values() {
            for j in seg.junctions().values() {
                let Some(layer) = seg.junction_layer(&j.uuid()) else {
                    continue;
                };
                let p = j.position();
                if cx.ctx.view.is_layer_visible(layer)
                    && p.x >= x0
                    && p.x <= x1
                    && p.y >= y0
                    && p.y <= y1
                {
                    items.push(BoardItemRef::Junction(seg.id(), j.uuid()));
                }
            }
        }
    }
    items
}

fn format_length(unit: librepcb_core::types::LengthUnit, l: Length) -> String {
    format!(
        "{} {}",
        float_to_string(
            unit.convert_to_unit(l),
            unit.reasonable_number_of_decimals() + 1
        ),
        unit.to_short_str()
    )
}

/// Builds the context menu of an item (upstream
/// `processGraphicsSceneRightMouseButtonReleased()`).
fn context_menu(cx: &Cx<'_, '_>, item: BoardItemRef, pos: Point) -> Vec<ContextMenuItem> {
    let entry = |action: ContextAction, text: String| ContextMenuItem {
        action: Some(action),
        text,
        enabled: true,
        checked: None,
        default: false,
    };
    let separator = || ContextMenuItem {
        action: None,
        text: String::new(),
        enabled: false,
        checked: None,
        default: false,
    };
    let properties = || ContextMenuItem {
        default: true,
        ..entry(
            ContextAction::Properties,
            tr!("EditorCommandSet", "Properties"),
        )
    };
    let rotate_flip = || {
        vec![
            entry(
                ContextAction::RotateCcw,
                tr!("EditorCommandSet", "Rotate Counterclockwise"),
            ),
            entry(
                ContextAction::RotateCw,
                tr!("EditorCommandSet", "Rotate Clockwise"),
            ),
            entry(
                ContextAction::FlipHorizontal,
                tr!("EditorCommandSet", "Flip Horizontally"),
            ),
            entry(
                ContextAction::FlipVertical,
                tr!("EditorCommandSet", "Flip Vertically"),
            ),
        ]
    };
    let cut_copy_remove = || {
        vec![
            entry(ContextAction::Cut, tr!("EditorCommandSet", "Cut")),
            entry(ContextAction::Copy, tr!("EditorCommandSet", "Copy")),
            entry(ContextAction::Remove, tr!("EditorCommandSet", "Remove")),
        ]
    };
    let grid = cx.grid();
    let snap = |p: Point| ContextMenuItem {
        enabled: !p.is_on_grid(grid),
        ..entry(
            ContextAction::SnapToGrid,
            tr!("EditorCommandSet", "Snap to Grid"),
        )
    };
    let lock = |locked: bool| ContextMenuItem {
        checked: Some(locked),
        ..entry(
            ContextAction::Lock(!locked),
            tr!("EditorCommandSet", "Lock Placement"),
        )
    };
    let tolerance = cx.ctx.view.tolerance();
    let vertex_entries = |path: &Path| {
        let mut out = Vec::new();
        let vertices = path
            .vertices()
            .iter()
            .filter(|v| *(v.pos - pos).length() <= tolerance)
            .count();
        if vertices > 0 {
            out.push(ContextMenuItem {
                enabled: path.vertices().len() - vertices >= 2,
                ..entry(
                    ContextAction::RemoveVertices,
                    tr!("EditorCommandSet", "Remove Vertex"),
                )
            });
        }
        if line_index_at(path, pos, tolerance).is_some() {
            out.push(entry(
                ContextAction::AddVertex,
                tr!("EditorCommandSet", "Add Vertex"),
            ));
        }
        if !out.is_empty() {
            out.push(separator());
        }
        out
    };
    let Ok(board) = cx.board() else {
        return Vec::new();
    };
    let mut m = Vec::new();
    match item {
        BoardItemRef::Device(c) => {
            let Some(d) = board.device(c) else {
                return m;
            };
            m.push(properties());
            m.push(separator());
            m.extend(rotate_flip());
            m.push(entry(
                ContextAction::Remove,
                tr!("EditorCommandSet", "Remove"),
            ));
            m.push(separator());
            m.push(snap(d.position()));
            m.push(lock(d.locked()));
            m.push(separator());
            m.push(entry(
                ContextAction::ResetTexts,
                tr!("EditorCommandSet", "Reset All Texts"),
            ));
            // Devices of the component (upstream "Change Device" menu).
            let devices = device_menu_items(cx, c);
            if !devices.is_empty() {
                m.push(separator());
                for (uuid, name) in devices {
                    let current = uuid == d.lib_device();
                    m.push(ContextMenuItem {
                        enabled: !current,
                        checked: current.then_some(true),
                        ..entry(ContextAction::ChangeDevice(uuid), name)
                    });
                }
            }
            // Footprints and 3D models of the package.
            let lib = cx.project().library();
            if let Some(pkg) = lib
                .device(&d.lib_device())
                .and_then(|dev| lib.package(&dev.package_uuid()))
            {
                m.push(separator());
                for fpt in pkg.footprints().iter() {
                    m.push(ContextMenuItem {
                        enabled: fpt.uuid() != d.lib_footprint(),
                        checked: (fpt.uuid() == d.lib_footprint()).then_some(true),
                        ..entry(
                            ContextAction::ChangeFootprint(fpt.uuid()),
                            fpt.names().default_value().to_string(),
                        )
                    });
                }
                m.push(separator());
                m.push(ContextMenuItem {
                    enabled: d.lib_model().is_some(),
                    checked: d.lib_model().is_none().then_some(true),
                    ..entry(
                        ContextAction::ChangeModel(None),
                        tr!("librepcb::editor::BoardEditorState_Select", "None"),
                    )
                });
                for model in pkg.models().iter() {
                    let used = pkg
                        .footprints()
                        .by_uuid(&d.lib_footprint())
                        .is_some_and(|f| f.models().contains(&model.uuid()));
                    if !used {
                        continue;
                    }
                    let current = d.lib_model() == Some(model.uuid());
                    m.push(ContextMenuItem {
                        enabled: !current,
                        checked: current.then_some(true),
                        ..entry(
                            ContextAction::ChangeModel(Some(model.uuid())),
                            model.name().to_string(),
                        )
                    });
                }
            }
        }
        BoardItemRef::Trace(..) => {
            m.push(entry(
                ContextAction::SetLineWidth,
                tr!("EditorCommandSet", "Set Line Width"),
            ));
            m.push(entry(
                ContextAction::Remove,
                tr!("EditorCommandSet", "Remove"),
            ));
            m.push(entry(
                ContextAction::RemoveWholeTrace,
                tr!("EditorCommandSet", "Remove Whole Trace"),
            ));
            m.push(separator());
            m.push(entry(
                ContextAction::SelectWholeTrace,
                tr!("EditorCommandSet", "Select Whole Trace"),
            ));
            m.push(separator());
            m.push(entry(
                ContextAction::MeasureLength,
                tr!("EditorCommandSet", "Measure Selected Segments Length"),
            ));
        }
        BoardItemRef::Junction(s, u) => {
            let Some(j) = board.net_segment(s).and_then(|seg| seg.junctions().get(&u)) else {
                return m;
            };
            m.push(entry(
                ContextAction::RemoveWholeTrace,
                tr!("EditorCommandSet", "Remove Whole Trace"),
            ));
            m.push(separator());
            m.push(entry(
                ContextAction::SelectWholeTrace,
                tr!("EditorCommandSet", "Select Whole Trace"),
            ));
            m.push(separator());
            m.push(snap(j.position()));
            m.push(separator());
            m.push(entry(
                ContextAction::MeasureLength,
                tr!("EditorCommandSet", "Measure Selected Segments Length"),
            ));
        }
        BoardItemRef::Via(s, u) => {
            let Some(v) = board.net_segment(s).and_then(|seg| seg.vias().get(&u)) else {
                return m;
            };
            m.push(properties());
            m.push(separator());
            m.extend(cut_copy_remove());
            m.push(entry(
                ContextAction::RemoveWholeTrace,
                tr!("EditorCommandSet", "Remove Whole Trace"),
            ));
            m.push(separator());
            m.push(entry(
                ContextAction::SelectWholeTrace,
                tr!("EditorCommandSet", "Select Whole Trace"),
            ));
            m.push(separator());
            m.push(snap(v.position()));
        }
        BoardItemRef::Plane(id) => {
            let Some(p) = board.plane(id) else {
                return m;
            };
            m.push(properties());
            m.push(separator());
            m.extend(vertex_entries(p.outline()));
            m.extend(cut_copy_remove());
            m.push(separator());
            m.extend(rotate_flip());
            m.push(separator());
            m.push(lock(p.locked()));
        }
        BoardItemRef::Zone(u) => {
            let Some(z) = board.zones().get(&u) else {
                return m;
            };
            m.push(properties());
            m.push(separator());
            m.extend(vertex_entries(z.outline()));
            m.extend(cut_copy_remove());
            m.push(separator());
            m.extend(rotate_flip());
            m.push(separator());
            m.push(lock(z.locked()));
        }
        BoardItemRef::Polygon(u) => {
            let Some(p) = board.polygons().get(&u) else {
                return m;
            };
            m.push(properties());
            m.push(separator());
            m.extend(vertex_entries(p.path()));
            m.extend(cut_copy_remove());
            m.push(separator());
            m.extend(rotate_flip());
            m.push(separator());
            m.push(lock(p.locked()));
        }
        BoardItemRef::StrokeText(_) | BoardItemRef::DeviceStrokeText(..) => {
            let text = match item {
                BoardItemRef::StrokeText(u) => board.stroke_texts().get(&u),
                BoardItemRef::DeviceStrokeText(c, u) => {
                    board.device(c).and_then(|d| d.stroke_texts().get(&u))
                }
                _ => None,
            };
            let Some(t) = text else {
                return m;
            };
            m.push(properties());
            m.push(separator());
            m.extend(cut_copy_remove());
            m.push(separator());
            m.extend(rotate_flip());
            m.push(separator());
            m.push(snap(t.position()));
            m.push(lock(t.locked()));
        }
        BoardItemRef::Hole(u) => {
            let Some(h) = board.holes().get(&u) else {
                return m;
            };
            m.push(properties());
            m.push(separator());
            m.extend(cut_copy_remove());
            m.push(separator());
            m.push(snap(h.path().first().pos));
            m.push(lock(h.locked()));
        }
        BoardItemRef::Pad(..) | BoardItemRef::FootprintPad(..) => {}
    }
    m
}

/// Collects values for the info box (upstream `InfoBoxValue`).
#[derive(Debug)]
struct InfoValue<T> {
    value: Option<T>,
    multiple: bool,
}

impl<T: PartialEq> Default for InfoValue<T> {
    fn default() -> Self {
        Self {
            value: None,
            multiple: false,
        }
    }
}

impl<T: PartialEq + Clone> InfoValue<T> {
    fn add(&mut self, v: T) {
        if self.value.as_ref().is_some_and(|x| *x != v) || self.multiple {
            self.value = None;
            self.multiple = true;
        } else {
            self.value = Some(v);
        }
    }
    fn set_multiple(&mut self) {
        self.multiple = true;
    }
    fn get(&self) -> Option<T> {
        if self.multiple {
            None
        } else {
            self.value.clone()
        }
    }
}

/// Builds the info box text of the selection and returns the nets to
/// highlight (upstream `processSelection()`).
fn info_box(query: &SelectionQuery<'_>, text: &mut String) -> CrossProbe {
    let p = query.project();
    let board = query.board();
    let total = query.devices.len()
        + query.pads.len()
        + query.footprint_pads.len()
        + query.vias.len()
        + query.junctions.len()
        + query.traces.len()
        + query.planes.len()
        + query.zones.len()
        + query.polygons.len()
        + query.stroke_texts.len()
        + query.device_texts.len()
        + query.holes.len();
    let mut probe = CrossProbe::default();
    if total == 0 {
        return probe;
    }
    let unit = board.settings().grid_unit;
    let fmt = |l: Length| {
        float_to_string(
            unit.convert_to_unit(l),
            unit.reasonable_number_of_decimals() + 1,
        )
    };
    let net_name = |n: Option<NetSignalId>| -> String {
        n.and_then(|n| p.circuit().net_signal(n))
            .map_or_else(|| "✖".to_owned(), |n| n.name().to_string())
    };
    let mut kv: Vec<(String, String)> = Vec::new();
    let mut net: InfoValue<Option<NetSignalId>> = InfoValue::default();
    let mut layer: InfoValue<librepcb_core::types::Layer> = InfoValue::default();
    let mut height: InfoValue<Length> = InfoValue::default();
    let mut width: InfoValue<Length> = InfoValue::default();
    let mut drill: InfoValue<Length> = InfoValue::default();
    let mut size: InfoValue<Length> = InfoValue::default();
    let mut position = None;
    let texts = query.stroke_texts.len() + query.device_texts.len();
    let pads = query.pads.len() + query.footprint_pads.len();
    probe.components.extend(query.devices.iter().copied());
    // Component signals of the selected footprint pads (upstream: only of
    // a single selected pad or of pads selected with traces or vias).
    let pad_signals = || {
        query.footprint_pads.iter().filter_map(|(c, u)| {
            board
                .device(*c)?
                .pad(u, p.library(), p.circuit())
                .ok()
                .flatten()?
                .component_signal()
        })
    };
    if !query.devices.is_empty() {
        // Devices have priority, other objects are ignored.
        if query.devices.len() == 1
            && let Some(c) = query.devices.iter().next()
            && let Some(d) = board.device(*c)
            && let Some(cmp) = p.circuit().component_instance(*c)
        {
            kv.push((
                tr!("librepcb::editor::BoardEditorState_Select", "Name"),
                cmp.name().to_string(),
            ));
            kv.push((
                tr!("librepcb::editor::BoardEditorState_Select", "Value"),
                cmp.value().to_string(),
            ));
            let lib = p.library();
            if let Some(pkg) = lib
                .device(&d.lib_device())
                .and_then(|dev| lib.package(&dev.package_uuid()))
            {
                kv.push((
                    tr!("librepcb::editor::BoardEditorState_Select", "Package"),
                    pkg.metadata().names().default_value().to_string(),
                ));
            }
            position = Some(d.position());
        }
    } else if pads == total {
        if pads == 1 {
            probe.component_signals.extend(pad_signals());
            if let Some((c, u)) = query.footprint_pads.iter().next()
                && let Some(d) = board.device(*c)
                && let Ok(Some(pad)) = d.pad(u, p.library(), p.circuit())
            {
                if let Some(pp) = pad.package_pad() {
                    kv.push((
                        tr!("librepcb::editor::BoardEditorState_Select", "Pad"),
                        pp.name().to_string(),
                    ));
                }
                if let Some(s) = pad.component_signal_name() {
                    kv.push((
                        tr!("librepcb::editor::BoardEditorState_Select", "Signal"),
                        s.to_string(),
                    ));
                }
                kv.push((
                    tr!("librepcb::editor::BoardEditorState_Select", "Net"),
                    net_name(pad.net()),
                ));
                position = Some(pad.position());
            } else if let Some((s, u)) = query.pads.iter().next()
                && let Some(seg) = board.net_segment(*s)
                && let Some(pad) = seg.pads().get(u)
            {
                kv.push((
                    tr!("librepcb::editor::BoardEditorState_Select", "Net"),
                    net_name(seg.net()),
                ));
                position = Some(pad.pad().position());
            }
        }
    } else if query.holes.len() == total {
        for u in &query.holes {
            if let Some(h) = board.holes().get(u) {
                drill.add(*h.diameter());
            }
        }
        if query.holes.len() == 1
            && let Some(h) = query.holes.iter().next().and_then(|u| board.holes().get(u))
            && h.path().get().vertices().len() == 1
        {
            position = Some(h.path().first().pos);
        }
    } else if texts == total {
        let all = query
            .stroke_texts
            .iter()
            .filter_map(|u| board.stroke_texts().get(u))
            .chain(
                query
                    .device_texts
                    .iter()
                    .filter_map(|(c, u)| board.device(*c)?.stroke_texts().get(u)),
            );
        for t in all {
            layer.add(t.layer());
            height.add(*t.height());
            width.add(*t.stroke_width());
        }
    } else if query.polygons.len() == total {
        for poly in query
            .polygons
            .iter()
            .filter_map(|u| board.polygons().get(u))
        {
            layer.add(poly.layer());
            width.add(*poly.line_width());
        }
    } else if query.vias.len() == total {
        for (s, u) in &query.vias {
            let Some(seg) = board.net_segment(*s) else {
                continue;
            };
            let Some(v) = seg.vias().get(u) else {
                continue;
            };
            net.add(seg.net());
            probe.nets.extend(seg.net());
            let rules = board.design_rules();
            let d = v
                .drill_diameter()
                .map(|d| *d)
                .unwrap_or_else(|| *rules.default_via_drill_diameter());
            drill.add(d);
            if let Some(s) = v.size() {
                size.add(*s);
            } else {
                size.set_multiple();
            }
        }
    } else if query.traces.len()
        + query.junctions.len()
        + pads
        + query.planes.len()
        + query.vias.len()
        == total
    {
        if !query.vias.is_empty() {
            layer.set_multiple();
        }
        probe.component_signals.extend(pad_signals());
        for (s, _) in &query.vias {
            let n = board.net_segment(*s).and_then(|seg| seg.net());
            net.add(n);
            probe.nets.extend(n);
        }
        for id in &query.planes {
            if let Some(plane) = board.plane(*id) {
                net.add(plane.net());
                layer.add(plane.layer());
                probe.nets.extend(plane.net());
            }
        }
        for (s, u) in &query.traces {
            let Some(seg) = board.net_segment(*s) else {
                continue;
            };
            if let Some(t) = seg.traces().get(u) {
                net.add(seg.net());
                layer.add(t.layer());
                width.add(*t.width());
                probe.nets.extend(seg.net());
            }
        }
    }
    if let Some(n) = net.get() {
        kv.push((
            tr!("librepcb::editor::BoardEditorState_Select", "Net"),
            net_name(n),
        ));
        if let Some(ns) = n.and_then(|n| p.circuit().net_signal(n))
            && p.circuit().net_classes().len() > 1
            && let Some(nc) = p.circuit().net_class(ns.net_class())
        {
            kv.push((
                tr!("librepcb::editor::BoardEditorState_Select", "Class"),
                nc.name().to_string(),
            ));
        }
    }
    if let Some(l) = layer.get() {
        kv.push((
            tr!("librepcb::editor::BoardEditorState_Select", "Layer"),
            l.name_tr(),
        ));
    }
    let short = unit.to_short_str();
    if let Some(v) = height.get() {
        kv.push((
            tr!("librepcb::editor::BoardEditorState_Select", "Height"),
            format!("{} {short}", fmt(v)),
        ));
    }
    if let Some(v) = width.get() {
        kv.push((
            tr!("librepcb::editor::BoardEditorState_Select", "Width"),
            format!("{} {short}", fmt(v)),
        ));
    }
    if let Some(v) = drill.get() {
        kv.push((
            tr!("librepcb::editor::BoardEditorState_Select", "Drill"),
            format!("{} {short}", fmt(v)),
        ));
    }
    if let Some(v) = size.get() {
        kv.push((
            tr!("librepcb::editor::BoardEditorState_Select", "Size"),
            format!("{} {short}", fmt(v)),
        ));
    }
    if let Some(pos) = position {
        kv.push((
            tr!("librepcb::editor::BoardEditorState_Select", "Position"),
            format!("{}, {} {short}", fmt(pos.x), fmt(pos.y)),
        ));
    }
    kv.retain(|(_, v)| !v.is_empty());
    let max = kv.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0);
    let lines: Vec<String> = kv
        .into_iter()
        .map(|(k, v)| {
            let pad = " ".repeat(max - k.chars().count());
            format!("{k}: {pad}{v}")
        })
        .collect();
    *text = lines.join("\n");
    probe
}

/// Formats a number with at most `decimals` decimals, without trailing
/// zeros (upstream `Toolbox::floatToString()` in the C locale).
fn float_to_string(value: f64, decimals: usize) -> String {
    let s = format!("{value:.decimals$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    };
    if s == "-0" { "0".to_owned() } else { s }
}

/// The devices of the component of a board device from the library
/// element source, as (UUID, "device name [package name]") sorted by name
/// (upstream `getDeviceMenuItems()`); devices of the component's assembly
/// options are marked with a check mark.
fn device_menu_items(
    cx: &Cx<'_, '_>,
    component: librepcb_core::project::ComponentInstanceId,
) -> Vec<(Uuid, String)> {
    use crate::commands::library::open_element;
    use crate::undo_stack::LibraryElement;
    use librepcb_core::project::LibraryElementKind;
    let p = cx.project();
    let Some(instance) = p.circuit().component_instance(component) else {
        return Vec::new();
    };
    let compatible = instance.compatible_devices();
    let source = cx.ctx.editor.source();
    let mut items: Vec<(Uuid, String)> = source
        .component_devices(&instance.lib_component())
        .into_iter()
        .filter_map(|uuid| {
            let dir = source.element_directory(LibraryElementKind::Device, &uuid)?;
            let LibraryElement::Device(device) =
                open_element(LibraryElementKind::Device, &dir).ok()?
            else {
                return None;
            };
            let pkg_name = source
                .element_directory(LibraryElementKind::Package, &device.package_uuid())
                .and_then(|dir| open_element(LibraryElementKind::Package, &dir).ok())
                .and_then(|e| match e {
                    LibraryElement::Package(pkg) => {
                        Some(pkg.metadata().names().default_value().to_string())
                    }
                    _ => None,
                })
                .unwrap_or_default();
            let mut name = format!("{} [{pkg_name}]", device.metadata().names().default_value());
            if compatible.contains(&uuid) {
                name += " \u{2714}";
            }
            Some((uuid, name))
        })
        .collect();
    items.sort_by(|a, b| toolbox::compare_numeric(&a.1, &b.1));
    items
}
