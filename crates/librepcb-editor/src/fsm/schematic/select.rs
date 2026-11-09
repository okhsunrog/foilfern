//! Port of libs/librepcb/editor/project/schematic/fsm/schematiceditorstate_select.{h,cpp}.
//!
//! Not ported: the context menu itself (requested from the application),
//! cross-probing.

use librepcb_core::geometry::{Image, Path, Polygon, Vertex};
use librepcb_core::project::{BusId, Mutation, NetSignalId, SchematicMutation, SymbolId};
use librepcb_core::types::{Angle, Length, Orientation, Point, PositiveLength, Uuid};
use librepcb_core::utils::toolbox;
use librepcb_i18n::tr;

use super::clipboard::{
    PasteSchematicItems, SchematicClipboardData, schematic_clipboard_mime_type,
};
use super::drag::DragSelection;
use super::hit_test::{FindFlags, find_items_at};
use super::selection::{SelectionQuery, all_items, item_exists};
use super::simplify::SimplifySchematicSegments;
use super::{CrossProbe, Cx, SchematicItem, SchematicRequest, State};
use crate::commands::{RemoveSchematicItems, SchematicSelection};
use crate::fsm::{CursorShape, Features, PointerEvent};
use crate::library_editor::commands::{SymbolClipboardData, symbol_clipboard_mime_type};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum SubState {
    #[default]
    Idle,
    Selecting,
    Moving,
    Pasting,
    MovingPolygonVertices,
    ResizingImage,
}

/// Moving vertices of a polygon (upstream `mCmdPolygonEdit`).
#[derive(Debug, Clone)]
struct VertexEdit {
    polygon: Polygon,
    vertices: Vec<usize>,
}

/// Resizing an image (upstream `mCmdImageEdit`).
#[derive(Debug, Clone)]
struct ImageResize {
    image: Image,
    aspect_ratio: f64,
}

/// The select state.
#[derive(Debug, Default)]
pub(crate) struct SelectState {
    sub: SubState,
    start_pos: Point,
    drag: Option<DragSelection>,
    /// Length of the undo group before the drag preview (pasting: after
    /// the pasted items).
    base_len: usize,
    vertex_edit: Option<VertexEdit>,
    image_resize: Option<ImageResize>,
}

/// The vertex handle radius of images: 20 pixels (upstream
/// `ImageGraphicsItem`), i.e. four times the hit tolerance.
const IMAGE_HANDLE_TOLERANCE_FACTOR: i64 = 4;

/// Indices of the vertices of `path` nearest to `pos` within `tolerance`
/// (upstream `PolygonGraphicsItem::getVertexIndicesAtPosition()`).
pub(crate) fn vertices_at(path: &Path, pos: Point, tolerance: Length) -> Vec<usize> {
    let distances: Vec<(usize, Length)> = path
        .vertices()
        .iter()
        .enumerate()
        .map(|(i, v)| (i, (v.pos - pos).length().get()))
        .filter(|(_, d)| *d <= tolerance)
        .collect();
    let Some(min) = distances.iter().map(|(_, d)| *d).min() else {
        return Vec::new();
    };
    distances
        .into_iter()
        .filter(|(_, d)| *d == min)
        .map(|(i, _)| i)
        .collect()
}

/// The index of the vertex after the line of `path` at `pos` (upstream
/// `getLineIndexAtPosition()`).
pub(crate) fn line_index_at(path: &Path, pos: Point, tolerance: Length) -> Option<usize> {
    path.vertices().windows(2).enumerate().find_map(|(i, w)| {
        let (d, _) = toolbox::shortest_distance_between_point_and_line(pos, w[0].pos, w[1].pos);
        (*d <= tolerance).then_some(i + 1)
    })
}

impl SelectState {
    fn query(cx: &Cx<'_, '_>) -> SelectionQuery {
        match cx.sch() {
            Some(s) => SelectionQuery::new(&cx.out.selection, s),
            None => SelectionQuery::default(),
        }
    }

    /// Finds vertices of a selected polygon at `pos` (upstream
    /// `findPolygonVerticesAtPosition()`).
    fn find_polygon_vertices(cx: &Cx<'_, '_>, pos: Point) -> Option<VertexEdit> {
        let s = cx.sch()?;
        let tolerance = cx.ctx.view.tolerance();
        s.polygons()
            .iter()
            .filter(|(id, _)| cx.out.selection.contains(&SchematicItem::Polygon(**id)))
            .find_map(|(_, polygon)| {
                let vertices = vertices_at(polygon.path(), pos, tolerance);
                (!vertices.is_empty()).then(|| VertexEdit {
                    polygon: polygon.clone(),
                    vertices,
                })
            })
    }

    /// Finds the resize handle of a selected image at `pos` (upstream
    /// `findImageHandleAtPosition()`).
    fn find_image_handle(cx: &Cx<'_, '_>, pos: Point) -> Option<ImageResize> {
        let s = cx.sch()?;
        let tolerance = cx.ctx.view.tolerance() * IMAGE_HANDLE_TOLERANCE_FACTOR;
        s.images()
            .iter()
            .filter(|(id, _)| cx.out.selection.contains(&SchematicItem::Image(**id)))
            .find_map(|(_, image)| {
                let rel = pos.rotated(-image.rotation(), image.position()) - image.position();
                let corner = Point::new(image.width().get(), image.height().get());
                ((corner - rel).length().get() <= tolerance).then(|| ImageResize {
                    image: image.clone(),
                    aspect_ratio: image.width().to_mm() / image.height().to_mm(),
                })
            })
    }

    /// Moves the vertices being edited to `pos` (upstream: mouse move in
    /// `MOVING_POLYGON_VERTICES`).
    fn move_vertices(&mut self, cx: &mut Cx<'_, '_>, pos: Point) {
        let Some(edit) = &self.vertex_edit else {
            return;
        };
        let mut vertices: Vec<Vertex> = edit.polygon.path().vertices().to_vec();
        for i in &edit.vertices {
            if let Some(v) = vertices.get_mut(*i) {
                v.pos = pos.mapped_to_grid(cx.grid());
            }
        }
        let mut polygon = edit.polygon.clone();
        polygon.set_path(Path::new(vertices));
        cx.ctx.editor.rollback_group_to(0);
        if let Err(e) = cx.ctx.editor.execute(crate::commands::ApplyMutations {
            text: None,
            mutations: vec![Mutation::Schematic(SchematicMutation::UpdatePolygon {
                schematic: cx.schematic,
                polygon,
            })],
        }) {
            log::warn!("Failed to move the polygon vertices: {e}");
        }
    }

    /// Resizes the image being edited (upstream: mouse move in
    /// `RESIZING_IMAGE`; the aspect ratio is kept).
    fn resize_image(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) {
        let Some(resize) = &self.image_resize else {
            return;
        };
        let mut pos = e.pos;
        if !e.modifiers.shift {
            pos = pos.mapped_to_grid(cx.grid());
        }
        let image = &resize.image;
        let rel = pos.rotated(-image.rotation(), image.position()) - image.position();
        let width = rel.x;
        let Ok(height) = Length::from_mm(width.to_mm() / resize.aspect_ratio) else {
            return;
        };
        let (Ok(width), Ok(height)) = (PositiveLength::new(width), PositiveLength::new(height))
        else {
            return;
        };
        let mut image = image.clone();
        image.set_width(width);
        image.set_height(height);
        cx.ctx.editor.rollback_group_to(0);
        if let Err(e) = cx.ctx.editor.execute(crate::commands::ApplyMutations {
            text: None,
            mutations: vec![Mutation::Schematic(SchematicMutation::UpdateImage {
                schematic: cx.schematic,
                image,
            })],
        }) {
            log::warn!("Failed to resize the image: {e}");
        }
    }

    /// Removes the vertices of a polygon at `pos` (upstream
    /// `removePolygonVertices()`, context menu "Remove Vertex").
    pub fn remove_polygon_vertices(
        &mut self,
        cx: &mut Cx<'_, '_>,
        polygon: Uuid,
        pos: Point,
    ) -> bool {
        if self.sub != SubState::Idle {
            return false;
        }
        let Some(original) = cx.sch().and_then(|s| s.polygons().get(&polygon)).cloned() else {
            return false;
        };
        let remove = vertices_at(original.path(), pos, cx.ctx.view.tolerance());
        if remove.is_empty() {
            return false;
        }
        let path = original.path();
        let mut new_path = Path::new(
            path.vertices()
                .iter()
                .enumerate()
                .filter(|(i, _)| !remove.contains(i))
                .map(|(_, v)| *v)
                .collect(),
        );
        if path.is_closed() && new_path.vertices().len() > 2 {
            new_path.close();
        }
        if new_path.is_closed() && new_path.vertices().len() == 3 {
            new_path.vertices_mut().pop(); // Avoid overlapping lines.
        }
        if new_path.vertices().len() < 2 {
            return false; // Do not allow to create invalid polygons!
        }
        let mut polygon = original;
        polygon.set_path(new_path);
        match cx.ctx.editor.execute(crate::commands::ApplyMutations {
            text: Some(tr!("librepcb::editor::CmdPolygonEdit", "Edit polygon")),
            mutations: vec![Mutation::Schematic(SchematicMutation::UpdatePolygon {
                schematic: cx.schematic,
                polygon,
            })],
        }) {
            Ok(()) => true,
            Err(e) => {
                cx.error(e);
                false
            }
        }
    }

    /// Inserts a vertex into the line of a polygon at `pos` which then
    /// follows the cursor until the left button is released (upstream
    /// `startAddingPolygonVertex()`, context menu "Add Vertex").
    pub fn add_polygon_vertex(&mut self, cx: &mut Cx<'_, '_>, polygon: Uuid, pos: Point) -> bool {
        if self.sub != SubState::Idle {
            return false;
        }
        let Some(original) = cx.sch().and_then(|s| s.polygons().get(&polygon)).cloned() else {
            return false;
        };
        let Some(index) = line_index_at(original.path(), pos, cx.ctx.view.tolerance()) else {
            return false;
        };
        let mut vertices = original.path().vertices().to_vec();
        let angle = vertices[index - 1].angle;
        vertices.insert(index, Vertex::new(pos.mapped_to_grid(cx.grid()), angle));
        let mut polygon = original;
        polygon.set_path(Path::new(vertices));
        if let Err(e) = cx
            .ctx
            .editor
            .begin_group(tr!("librepcb::editor::CmdPolygonEdit", "Edit polygon"))
        {
            cx.error(e);
            return false;
        }
        self.vertex_edit = Some(VertexEdit {
            polygon,
            vertices: vec![index],
        });
        self.sub = SubState::MovingPolygonVertices;
        self.move_vertices(cx, pos);
        true
    }

    /// Applies the current drag state as preview inside the open group.
    fn apply_drag_preview(&mut self, cx: &mut Cx<'_, '_>) {
        let Some(drag) = &self.drag else {
            return;
        };
        cx.ctx.editor.rollback_group_to(self.base_len);
        let mutations = drag.mutations(cx.project());
        if let Err(e) = cx.ctx.editor.execute(crate::commands::ApplyMutations {
            text: None,
            mutations,
        }) {
            log::warn!("Failed to update the drag preview: {e}");
        }
    }

    /// Starts a drag of the selection (upstream
    /// `startMovingSelectedItems()`).
    fn start_moving(&mut self, cx: &mut Cx<'_, '_>, pos: Point) -> bool {
        let query = Self::query(cx);
        let Some(s) = cx.sch() else {
            return false;
        };
        let drag = DragSelection::new(s, &query, pos);
        if let Err(e) = cx.ctx.editor.begin_group(tr!(
            "librepcb::editor::CmdDragSelectedSchematicItems",
            "Drag Schematic Elements"
        )) {
            cx.error(e);
            return false;
        }
        self.base_len = 0;
        self.drag = Some(drag);
        self.sub = SubState::Moving;
        true
    }

    /// Finishes a drag or paste: applies the final state and commits the
    /// group. After a drag (not after pasting), the modified segments are
    /// simplified in a separate undo step like upstream.
    fn finish_drag(&mut self, cx: &mut Cx<'_, '_>, pos: Point, simplify: bool) {
        if let Some(drag) = &mut self.drag {
            drag.set_current_position(pos, None);
        }
        self.apply_drag_preview(cx);
        let drag = self.drag.take();
        match cx.ctx.editor.commit_group() {
            Ok(committed) => {
                if let Some(drag) = drag
                    && simplify
                    && committed
                    && drag.has_changes()
                {
                    let segments = drag.modified_segments().clone();
                    let bus_segments = drag.modified_bus_segments().clone();
                    if !(segments.is_empty() && bus_segments.is_empty())
                        && let Err(e) = cx.ctx.editor.execute(SimplifySchematicSegments {
                            schematic: cx.schematic,
                            segments,
                            bus_segments,
                        })
                    {
                        log::error!("Failed to simplify schematic segments: {e}");
                    }
                }
            }
            Err(e) => cx.error(e),
        }
        self.sub = SubState::Idle;
    }

    /// Runs a one-shot drag operation (move, rotate, mirror, snap, reset
    /// texts) as one undo group (like upstream without a simplification of
    /// the modified segments).
    fn one_shot(&mut self, cx: &mut Cx<'_, '_>, f: impl FnOnce(&mut DragSelection)) -> bool {
        let query = Self::query(cx);
        let Some(s) = cx.sch() else {
            return false;
        };
        let mut drag = DragSelection::new(s, &query, Point::ORIGIN);
        if drag.is_empty() {
            return false;
        }
        f(&mut drag);
        if !drag.has_changes() {
            return true;
        }
        let mutations = drag.mutations(cx.project());
        match cx.ctx.editor.execute(crate::commands::ApplyMutations {
            text: Some(tr!(
                "librepcb::editor::CmdDragSelectedSchematicItems",
                "Drag Schematic Elements"
            )),
            mutations,
        }) {
            Ok(()) => true,
            Err(e) => {
                cx.error(e);
                false
            }
        }
    }

    fn rotate_selected(&mut self, cx: &mut Cx<'_, '_>, angle: Angle) -> bool {
        let grid = cx.grid();
        if let Some(drag) = &mut self.drag {
            drag.rotate(angle, true, grid);
            self.apply_drag_preview(cx);
            return true;
        }
        self.one_shot(cx, |d| d.rotate(angle, false, grid))
    }

    fn mirror_selected(&mut self, cx: &mut Cx<'_, '_>, orientation: Orientation) -> bool {
        let grid = cx.grid();
        if let Some(drag) = &mut self.drag {
            drag.mirror(orientation, true, grid);
            self.apply_drag_preview(cx);
            return true;
        }
        self.one_shot(cx, |d| d.mirror(orientation, false, grid))
    }

    /// Removes the selected items (upstream `removeSelectedItems()`), then
    /// simplifies the modified net and bus segments (separate undo step).
    fn remove_selected(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        let q = Self::query(cx);
        let selection = SchematicSelection {
            symbols: q.symbols.iter().copied().collect(),
            net_lines: q.net_lines.iter().copied().collect(),
            net_labels: q.net_labels.iter().copied().collect(),
            bus_lines: q.bus_lines.iter().copied().collect(),
            bus_labels: q.bus_labels.iter().copied().collect(),
            symbol_texts: q
                .symbol_texts
                .iter()
                .filter(|(sym, _)| !q.symbols.contains(sym))
                .copied()
                .collect(),
            polygons: q.polygons.iter().copied().collect(),
            texts: q.texts.iter().copied().collect(),
            images: q.images.iter().copied().collect(),
        };
        cx.out.selection.clear();
        let before = cx
            .sch()
            .map(|s| (s.net_segments().clone(), s.bus_segments().clone()));
        match cx.ctx.editor.execute(RemoveSchematicItems {
            schematic: cx.schematic,
            selection,
        }) {
            Ok(_) => {
                let modified = match (before, cx.sch()) {
                    (Some((nets, buses)), Some(s)) => {
                        let segments: std::collections::BTreeSet<_> = s
                            .net_segments()
                            .iter()
                            .filter(|(id, seg)| nets.get(id) != Some(*seg))
                            .map(|(id, _)| *id)
                            .collect();
                        let bus_segments: std::collections::BTreeSet<_> = s
                            .bus_segments()
                            .iter()
                            .filter(|(id, seg)| buses.get(id) != Some(*seg))
                            .map(|(id, _)| *id)
                            .collect();
                        Some((segments, bus_segments))
                    }
                    _ => None,
                };
                if let Some((segments, bus_segments)) = modified
                    && !(segments.is_empty() && bus_segments.is_empty())
                    && let Err(e) = cx.ctx.editor.execute(SimplifySchematicSegments {
                        schematic: cx.schematic,
                        segments,
                        bus_segments,
                    })
                {
                    log::error!("Failed to simplify schematic segments: {e}");
                }
                true
            }
            Err(e) => {
                cx.error(e);
                false
            }
        }
    }

    /// Whether an item counts as selected (texts of a selected symbol are
    /// selected with it, like upstream).
    fn is_selected(cx: &Cx<'_, '_>, item: &SchematicItem) -> bool {
        cx.out.selection.contains(item)
            || matches!(item, SchematicItem::SymbolText(sym, _)
                if cx.out.selection.contains(&SchematicItem::Symbol(*sym)))
    }

    /// Copies the selection to the clipboard (upstream
    /// `copySelectedItemsToClipboard()`).
    fn copy_selected(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        let query = Self::query(cx);
        let cursor = cx.cursor_pos();
        let Some(s) = cx.sch() else {
            return false;
        };
        let result = SchematicClipboardData::build(cx.project(), s, &query, cursor)
            .and_then(|data| data.to_zip());
        match result {
            Ok(zip) => {
                let mime = schematic_clipboard_mime_type(&cx.settings.app_version);
                cx.ctx.clipboard.set(&mime, zip);
                cx.out.view.set_status(
                    tr!(
                        "librepcb::editor::SchematicEditorState_Select",
                        "Copied to clipboard!"
                    ),
                    Some(2000),
                );
            }
            Err(e) => cx.error(e),
        }
        true
    }

    /// Pastes from the clipboard (upstream `pasteFromClipboard()`).
    fn paste_from_clipboard(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        let version = &cx.settings.app_version;
        let result = if let Some(zip) = cx
            .ctx
            .clipboard
            .get(&schematic_clipboard_mime_type(version))
        {
            SchematicClipboardData::from_zip(&zip)
        } else if let Some(zip) = cx.ctx.clipboard.get(&symbol_clipboard_mime_type(version)) {
            // Graphical elements from the symbol editor.
            let Some(s) = cx.sch() else {
                return false;
            };
            SymbolClipboardData::from_zip(&zip).and_then(|symbol_data| {
                SchematicClipboardData::from_symbol_data(&symbol_data, cx.project(), s)
            })
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
        if data.is_empty() {
            return false;
        }
        self.start_pos = cx.cursor_pos();
        cx.out.selection.clear();
        if let Err(e) = cx.ctx.editor.begin_group(tr!(
            "librepcb::editor::SchematicEditorState_Select",
            "Paste Schematic Elements"
        )) {
            cx.error(e);
            return false;
        }
        let offset = (self.start_pos - data.cursor_pos).mapped_to_grid(cx.grid());
        match cx.ctx.editor.execute(PasteSchematicItems {
            schematic: cx.schematic,
            data,
            offset,
        }) {
            Ok(items) if !items.is_empty() => {
                cx.out.selection = items.into_iter().collect();
                self.base_len = cx.ctx.editor.active_group_len().unwrap_or(0);
                let query = Self::query(cx);
                if let Some(s) = cx.sch() {
                    self.drag = Some(DragSelection::new(s, &query, self.start_pos));
                }
                self.sub = SubState::Pasting;
                true
            }
            Ok(_) => {
                let _ = cx.ctx.editor.abort_group();
                false
            }
            Err(e) => {
                let _ = cx.ctx.editor.abort_group();
                cx.error(e);
                false
            }
        }
    }

    /// Opens the properties dialog of an item (upstream
    /// `openPropertiesDialog()`).
    fn open_properties(cx: &mut Cx<'_, '_>, item: SchematicItem) -> bool {
        let request = match item {
            SchematicItem::Symbol(id) => SchematicRequest::SymbolProperties(id),
            SchematicItem::NetLabel(seg, l) => SchematicRequest::NetLabelProperties(seg, l),
            SchematicItem::BusLabel(seg, l) => SchematicRequest::BusLabelProperties(seg, l),
            SchematicItem::Polygon(id) => SchematicRequest::PolygonProperties(id),
            SchematicItem::Text(id) => SchematicRequest::TextProperties(id),
            SchematicItem::SymbolText(sym, id) => SchematicRequest::SymbolTextProperties(sym, id),
            _ => return false,
        };
        cx.out.requests.push(request);
        true
    }

    /// Updates the available features and the info box (upstream
    /// `updateAvailableFeatures()`).
    fn update_features(&self, cx: &mut Cx<'_, '_>) {
        let mut f = Features::default();
        match self.sub {
            SubState::Pasting => {
                f.rotate = true;
                f.mirror = true;
            }
            SubState::Idle | SubState::Moving => {
                f.select = true;
                let version = &cx.settings.app_version;
                f.paste = [
                    schematic_clipboard_mime_type(version),
                    symbol_clipboard_mime_type(version),
                ]
                .iter()
                .any(|mime| cx.ctx.clipboard.get(mime).is_some());
                let q = Self::query(cx);
                let grid = cx.grid();
                if q.has_modifiable_items() {
                    f.cut = true;
                    f.copy = true;
                    f.remove = true;
                    f.rotate = true;
                    f.mirror = true;
                }
                if !q.symbols.is_empty() {
                    f.reset_texts = true;
                }
                if let Some(s) = cx.sch() {
                    let off = |p: Point| !p.is_on_grid(grid);
                    f.snap_to_grid = q.symbols.iter().any(|id| off(s.symbols()[id].position()))
                        || q.net_points
                            .iter()
                            .any(|(seg, j)| off(s.net_segments()[seg].junctions()[j].position()))
                        || q.net_labels
                            .iter()
                            .any(|(seg, l)| off(s.net_segments()[seg].labels()[l].position()))
                        || q.polygons
                            .iter()
                            .any(|id| !s.polygons()[id].path().is_on_grid(grid))
                        || q.bus_junctions
                            .iter()
                            .any(|(seg, j)| off(s.bus_segments()[seg].junctions()[j].position()))
                        || q.bus_labels
                            .iter()
                            .any(|(seg, l)| off(s.bus_segments()[seg].labels()[l].position()))
                        || q.texts.iter().any(|id| off(s.texts()[id].position()))
                        || q.symbol_texts
                            .iter()
                            .any(|(sym, t)| off(s.symbols()[sym].texts()[t].position()))
                        || q.images.iter().any(|id| off(s.images()[id].position()));
                }
                if !q.symbols.is_empty()
                    || !q.net_labels.is_empty()
                    || !q.bus_labels.is_empty()
                    || !q.polygons.is_empty()
                    || !q.texts.is_empty()
                    || !q.symbol_texts.is_empty()
                {
                    f.properties = true;
                }
                let (text, probe) = info_text(cx, &q);
                cx.out.view.info_box = text;
                cx.out.cross_probe = probe;
            }
            SubState::Selecting | SubState::MovingPolygonVertices | SubState::ResizingImage => {}
        }
        if !matches!(
            self.sub,
            SubState::Selecting | SubState::MovingPolygonVertices | SubState::ResizingImage
        ) {
            cx.out.view.features = f;
        }
    }
}

/// Builds the info box text of the selection and the objects to cross-probe
/// (upstream `processSelection()`).
fn info_text(cx: &Cx<'_, '_>, q: &SelectionQuery) -> (String, CrossProbe) {
    let mut probe = CrossProbe::default();
    if q.count() == 0 {
        return (String::new(), probe);
    }
    let p = cx.project();
    let Some(s) = cx.sch() else {
        return (String::new(), probe);
    };
    let ctx = p.view();
    let mut key_values: Vec<(String, String)> = Vec::new();
    let mut net: Option<Option<NetSignalId>> = None;
    let mut multiple_nets = false;
    let mut add_net = |n: Option<NetSignalId>| {
        if net.is_some_and(|x| x != n) {
            multiple_nets = true;
        } else {
            net = Some(n);
        }
    };
    let mut bus: Option<BusId> = None;
    let mut multiple_buses = false;
    let mut add_bus = |b: BusId| {
        if bus.is_some_and(|x| x != b) {
            multiple_buses = true;
        } else {
            bus = Some(b);
        }
    };
    for id in &q.symbols {
        if let Some(sym) = s.symbols().get(id) {
            probe.components.insert(sym.component());
        }
    }
    for (seg, _) in q.net_labels.iter().chain(&q.net_lines) {
        let n = s.net_segments()[seg].net();
        add_net(Some(n));
        probe.nets.insert(n);
    }
    let pin_view = |(symbol, pin): &(SymbolId, librepcb_core::types::Uuid)| {
        s.symbols()
            .get(symbol)
            .and_then(|sym| sym.pin(ctx, *pin).ok().flatten())
    };
    for pin in &q.pins {
        if let Some(view) = pin_view(pin) {
            add_net(view.net());
            probe.component_signals.insert(view.signal());
        }
    }
    for (seg, _) in q.bus_labels.iter().chain(&q.bus_lines) {
        let b = s.bus_segments()[seg].bus();
        add_bus(b);
        probe.buses.insert(b);
    }

    let primary_device = |c: &librepcb_core::project::circuit::ComponentInstance| {
        p.boards().iter().find_map(|b| b.device(c.id()))
    };
    if q.symbols.len() == 1
        && let Some(symbol) = q.symbols.iter().next().and_then(|id| s.symbols().get(id))
        && let Ok(resolved) = symbol.resolve(ctx)
        && let Some(device) = primary_device(resolved.component)
        && !resolved.lib_component.schematic_only()
    {
        let cmp = resolved.component;
        let part = device.parts(cmp, None).into_iter().next();
        let lookup = librepcb_core::project::ProjectAttributeLookup::for_symbol(
            p,
            s,
            symbol,
            Some(device),
            part.as_ref(),
            None,
        );
        let value = lookup
            .substitute(cmp.value())
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let mpn = cmp
            .assembly_options()
            .first()
            .and_then(|ao| ao.parts().first())
            .map_or_else(|| "\u{2716}".to_owned(), |part| part.mpn().to_string());
        key_values.push((
            tr!("librepcb::editor::SchematicEditorState_Select", "Name"),
            cmp.name().to_string(),
        ));
        let value_key = tr!("librepcb::editor::SchematicEditorState_Select", "Value");
        let mpn_key = tr!("librepcb::editor::SchematicEditorState_Select", "MPN");
        if value == mpn {
            key_values.push((format!("{value_key}/{mpn_key}"), value));
        } else {
            key_values.push((value_key, value));
            key_values.push((mpn_key, mpn));
        }
    }
    if let (Some(n), false) = (net, multiple_nets) {
        let name = n
            .and_then(|id| p.circuit().net_signal(id))
            .map_or_else(|| "\u{2716}".to_owned(), |x| x.name().to_string());
        key_values.push((
            tr!("librepcb::editor::SchematicEditorState_Select", "Net"),
            name,
        ));
        if let Some(signal) = n.and_then(|id| p.circuit().net_signal(id))
            && p.circuit().net_classes().len() > 1
            && let Some(class) = p.circuit().net_class(signal.net_class())
        {
            key_values.push((
                tr!("librepcb::editor::SchematicEditorState_Select", "Class"),
                class.name().to_string(),
            ));
        }
    }
    if let (Some(b), false) = (bus, multiple_buses)
        && let Some(bus) = p.circuit().bus(b)
    {
        key_values.push((
            tr!("librepcb::editor::SchematicEditorState_Select", "Bus"),
            bus.name().to_string(),
        ));
    }
    if q.pins.len() == 1
        && let Some(view) = q.pins.iter().next().and_then(pin_view)
    {
        let signal = view.signal();
        key_values.push((
            tr!("librepcb::editor::SchematicEditorState_Select", "Signal"),
            view.lib_signal().name().to_string(),
        ));
        key_values.push((
            tr!("librepcb::editor::SchematicEditorState_Select", "Pin"),
            view.name().to_string(),
        ));
        // Pad names of the signal on the primary device.
        let mut pads: Vec<String> = Vec::new();
        if let Some(cmp) = p.circuit().component_instance(signal.component)
            && let Some(device) = primary_device(cmp)
            && let Ok(views) = device.pads(p.library(), p.circuit())
        {
            for pad in views {
                if pad.component_signal() == Some(signal)
                    && let Some(pkg_pad) = pad.package_pad()
                {
                    pads.push(pkg_pad.name().to_string());
                }
            }
        }
        pads.sort_by(|a, b| toolbox::compare_numeric(a, b));
        pads.dedup();
        key_values.push((
            tr!("librepcb::editor::SchematicEditorState_Select", "Pad(s)"),
            pads.join(","),
        ));
        // Forced net name mismatch.
        let forced = p
            .circuit()
            .component_instance(signal.component)
            .and_then(|c| {
                let lib = p.library().component(&c.lib_component())?;
                let sig = lib.signals().by_uuid(&signal.signal)?;
                sig.is_net_signal_name_forced().then(|| {
                    librepcb_core::project::ProjectAttributeLookup::for_component(p, c, None, None)
                        .substitute(sig.forced_net_name())
                })
            })
            .unwrap_or_default();
        if let Some(net) = view.net().and_then(|id| p.circuit().net_signal(id))
            && !forced.is_empty()
            && net.name().as_str() != forced
        {
            key_values.push((
                String::new(),
                tr!(
                    "librepcb::editor::SchematicEditorState_Select",
                    "Wire net '{0}' does not match forced net '{1}'!",
                    net.name(),
                    forced
                ),
            ));
        }
    }
    key_values.retain(|(_, v)| !v.is_empty());
    let max_len = key_values
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
    let text = key_values
        .iter()
        .map(|(k, v)| {
            if k.is_empty() {
                v.clone()
            } else {
                format!("{k}: {}{v}", " ".repeat(max_len - k.chars().count()))
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    (text, probe)
}

impl State for SelectState {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        cx.out.tool = super::SchematicTool::Select;
        self.update_features(cx);
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        if matches!(
            self.sub,
            SubState::Pasting
                | SubState::Moving
                | SubState::MovingPolygonVertices
                | SubState::ResizingImage
        ) && cx.ctx.editor.undo_stack().is_group_active()
            && let Err(e) = cx.ctx.editor.abort_group()
        {
            cx.error(e);
            return false;
        }
        self.drag = None;
        self.vertex_edit = None;
        self.image_resize = None;
        self.sub = SubState::Idle;
        // Avoid propagating the selection to other tools.
        cx.out.selection.clear();
        cx.out.view.rubber_band = None;
        cx.out.view.info_box.clear();
        cx.out.view.features = Features::default();
        true
    }

    fn select_all(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        if self.sub != SubState::Idle {
            return false;
        }
        if let Some(s) = cx.sch() {
            cx.out.selection = all_items(s);
        }
        true
    }

    fn cut(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.sub == SubState::Idle && self.copy_selected(cx) && self.remove_selected(cx)
    }

    fn copy(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.sub == SubState::Idle && self.copy_selected(cx)
    }

    fn paste(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.sub == SubState::Idle && self.paste_from_clipboard(cx)
    }

    fn move_by(&mut self, cx: &mut Cx<'_, '_>, delta: Point) -> bool {
        if self.sub != SubState::Idle || self.drag.is_some() {
            return false;
        }
        self.one_shot(cx, |d| d.set_current_position(delta, None))
    }

    fn rotate(&mut self, cx: &mut Cx<'_, '_>, angle: Angle) -> bool {
        self.rotate_selected(cx, angle)
    }

    fn mirror(&mut self, cx: &mut Cx<'_, '_>, orientation: Orientation) -> bool {
        self.mirror_selected(cx, orientation)
    }

    fn snap_to_grid(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        let grid = cx.grid();
        if let Some(drag) = &mut self.drag {
            drag.snap_to_grid(grid);
            self.apply_drag_preview(cx);
            return true;
        }
        self.one_shot(cx, |d| d.snap_to_grid(grid))
    }

    fn reset_all_texts(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        if self.sub != SubState::Idle {
            return false;
        }
        self.one_shot(cx, |d| d.reset_all_texts())
    }

    fn remove(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        if self.sub != SubState::Idle {
            return false;
        }
        self.remove_selected(cx);
        true
    }

    fn edit_properties(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        if self.sub != SubState::Idle {
            return false;
        }
        let q = Self::query(cx);
        let item = q
            .symbols
            .iter()
            .map(|id| SchematicItem::Symbol(*id))
            .chain(
                q.net_labels
                    .iter()
                    .map(|(s, l)| SchematicItem::NetLabel(*s, *l)),
            )
            .chain(
                q.bus_labels
                    .iter()
                    .map(|(s, l)| SchematicItem::BusLabel(*s, *l)),
            )
            .chain(q.polygons.iter().map(|id| SchematicItem::Polygon(*id)))
            .chain(q.texts.iter().map(|id| SchematicItem::Text(*id)))
            .chain(
                q.symbol_texts
                    .iter()
                    .map(|(s, t)| SchematicItem::SymbolText(*s, *t)),
            )
            .next();
        item.is_some_and(|item| Self::open_properties(cx, item))
    }

    fn abort(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        match self.sub {
            SubState::Idle => {
                cx.out.selection.clear();
                true
            }
            SubState::Pasting => {
                if let Err(e) = cx.ctx.editor.abort_group() {
                    cx.error(e);
                }
                self.drag = None;
                cx.out.selection.clear();
                self.sub = SubState::Idle;
                true
            }
            SubState::MovingPolygonVertices | SubState::ResizingImage => {
                if let Err(e) = cx.ctx.editor.abort_group() {
                    cx.error(e);
                }
                self.vertex_edit = None;
                self.image_resize = None;
                self.sub = SubState::Idle;
                true
            }
            _ => false,
        }
    }

    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        match self.sub {
            SubState::Selecting => {
                cx.out.view.rubber_band = Some((self.start_pos, e.pos));
                let items = cx.ctx.view.items_in_rect(self.start_pos, e.pos);
                let mut selection: std::collections::BTreeSet<SchematicItem> =
                    items.into_iter().collect();
                // Junctions are selected on the model.
                if let Some(s) = cx.sch() {
                    let (x0, x1) = (self.start_pos.x.min(e.pos.x), self.start_pos.x.max(e.pos.x));
                    let (y0, y1) = (self.start_pos.y.min(e.pos.y), self.start_pos.y.max(e.pos.y));
                    let inside = |p: Point| p.x >= x0 && p.x <= x1 && p.y >= y0 && p.y <= y1;
                    for (seg_id, seg) in s.net_segments() {
                        for j in seg.junctions().values() {
                            if inside(j.position()) {
                                selection.insert(SchematicItem::NetPoint(*seg_id, j.uuid()));
                            }
                        }
                    }
                    for (seg_id, seg) in s.bus_segments() {
                        for j in seg.junctions().values() {
                            if inside(j.position()) {
                                selection.insert(SchematicItem::BusJunction(*seg_id, j.uuid()));
                            }
                        }
                    }
                    selection.retain(|i| item_exists(i, s));
                }
                cx.out.selection = selection;
                true
            }
            SubState::Moving | SubState::Pasting => {
                if let Some(drag) = &mut self.drag {
                    let old = drag.delta();
                    drag.set_current_position(e.pos, None);
                    if drag.delta() != old {
                        self.apply_drag_preview(cx);
                    }
                }
                true
            }
            SubState::MovingPolygonVertices => {
                self.move_vertices(cx, e.pos);
                true
            }
            SubState::ResizingImage => {
                self.resize_image(cx, e);
                true
            }
            SubState::Idle => {
                let items = find_items_at(cx, e.pos, FindFlags::ALL.near(), &[]);
                cx.out.hovered = items.first().copied();
                false
            }
        }
    }

    fn left_pressed(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        match self.sub {
            SubState::Idle => {
                if let Some(edit) = Self::find_polygon_vertices(cx, e.pos) {
                    if let Err(err) = cx
                        .ctx
                        .editor
                        .begin_group(tr!("librepcb::editor::CmdPolygonEdit", "Edit polygon"))
                    {
                        cx.error(err);
                        return false;
                    }
                    self.vertex_edit = Some(edit);
                    self.sub = SubState::MovingPolygonVertices;
                    return true;
                }
                if let Some(resize) = Self::find_image_handle(cx, e.pos) {
                    if let Err(err) = cx
                        .ctx
                        .editor
                        .begin_group(tr!("librepcb::editor::CmdImageEdit", "Edit Image"))
                    {
                        cx.error(err);
                        return false;
                    }
                    self.image_resize = Some(resize);
                    self.sub = SubState::ResizingImage;
                    return true;
                }
                let items = find_items_at(cx, e.pos, FindFlags::ALL.near(), &[]);
                if items.is_empty() {
                    // No items under the cursor: start a selection rectangle.
                    cx.out.selection.clear();
                    self.start_pos = e.pos;
                    self.sub = SubState::Selecting;
                    return true;
                }
                let selected = items
                    .iter()
                    .rev()
                    .find(|i| Self::is_selected(cx, i))
                    .copied();
                if e.modifiers.control {
                    // Toggle the selection when CTRL is pressed.
                    let item = selected.unwrap_or(items[0]);
                    if !cx.out.selection.remove(&item) {
                        cx.out.selection.insert(item);
                    }
                } else if e.modifiers.shift {
                    // Cycle the selection when holding shift.
                    let index = selected
                        .and_then(|s| items.iter().position(|i| *i == s))
                        .map_or(0, |i| (i + 1) % items.len());
                    cx.out.selection.clear();
                    cx.out.selection.insert(items[index]);
                } else if selected.is_none() {
                    // Only select the topmost item when clicking an
                    // unselected item without CTRL.
                    cx.out.selection.clear();
                    cx.out.selection.insert(items[0]);
                }
                self.start_moving(cx, e.pos)
            }
            SubState::Pasting => {
                self.finish_drag(cx, e.pos, false);
                false
            }
            _ => false,
        }
    }

    fn left_released(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        match self.sub {
            SubState::Selecting => {
                cx.out.view.rubber_band = None;
                self.sub = SubState::Idle;
                true
            }
            SubState::Moving => {
                self.finish_drag(cx, e.pos, true);
                false
            }
            SubState::MovingPolygonVertices | SubState::ResizingImage => {
                if let Err(err) = cx.ctx.editor.commit_group() {
                    cx.error(err);
                }
                self.vertex_edit = None;
                self.image_resize = None;
                self.sub = SubState::Idle;
                false
            }
            _ => false,
        }
    }

    fn left_double_clicked(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        // With SHIFT or CTRL, the user is modifying the selection.
        if e.modifiers.shift || e.modifiers.control {
            return self.left_pressed(cx, e);
        }
        if self.sub != SubState::Idle {
            return false;
        }
        let items = find_items_at(cx, e.pos, FindFlags::ALL.near(), &[]);
        for item in items {
            if Self::is_selected(cx, &item) && Self::open_properties(cx, item) {
                return true;
            }
        }
        false
    }

    fn right_released(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        if self.drag.is_some() {
            return self.rotate_selected(cx, Angle::DEG90);
        }
        if self.sub != SubState::Idle {
            return false;
        }
        let items = find_items_at(cx, e.pos, FindFlags::ALL.near(), &[]);
        let Some(first) = items.first().copied() else {
            return false;
        };
        let selected = items
            .iter()
            .rev()
            .find(|i| Self::is_selected(cx, i))
            .copied();
        let item = match selected {
            Some(item) => item,
            None => {
                cx.out.selection.clear();
                cx.out.selection.insert(first);
                first
            }
        };
        if matches!(
            item,
            SchematicItem::Symbol(_)
                | SchematicItem::NetLabel(..)
                | SchematicItem::Polygon(_)
                | SchematicItem::Text(_)
                | SchematicItem::SymbolText(..)
        ) {
            let (remove_vertex, add_vertex) = match item {
                SchematicItem::Polygon(id) => {
                    let tolerance = cx.ctx.view.tolerance();
                    match cx.sch().and_then(|s| s.polygons().get(&id)) {
                        Some(polygon) => {
                            let path = polygon.path();
                            let vertices = vertices_at(path, e.pos, tolerance);
                            (
                                (!vertices.is_empty())
                                    .then(|| path.vertices().len() - vertices.len() >= 2),
                                line_index_at(path, e.pos, tolerance).is_some(),
                            )
                        }
                        None => (None, false),
                    }
                }
                _ => (None, false),
            };
            cx.out.requests.push(SchematicRequest::ContextMenu {
                item,
                pos: e.pos,
                remove_vertex,
                add_vertex,
            });
            return true;
        }
        false
    }

    fn update(&mut self, cx: &mut Cx<'_, '_>) {
        cx.out.view.cursor = match self.sub {
            SubState::Moving | SubState::Pasting => Some(CursorShape::ClosedHand),
            SubState::MovingPolygonVertices | SubState::ResizingImage => Some(CursorShape::SizeAll),
            _ => None,
        };
        self.update_features(cx);
    }
}
