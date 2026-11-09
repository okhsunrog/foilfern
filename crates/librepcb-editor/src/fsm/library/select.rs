//! Port of libs/librepcb/editor/library/sym/fsm/symboleditorstate_select.{h,cpp}
//! and libs/librepcb/editor/library/pkg/fsm/packageeditorstate_select.{h,cpp}.
//!
//! Differences to upstream: "move align" and pasting geometry into pads
//! are not ported; the generation of outline and courtyard are FSM methods
//! of the package editor. DXF import and adding images are FSM methods
//! (`import_dxf()`, `add_image()`) which get the file and options from the
//! application.

use std::collections::BTreeSet;

use librepcb_core::types::{Angle, Length, Orientation, Point, PositiveLength};
use librepcb_i18n::tr;

use super::hit_test::{segment_index_at, vertex_indices_at};
use super::{Cx, ElementHost, LibraryRequest, LibraryTool, State};
use crate::fsm::{Features, PointerEvent};
use crate::library_editor::commands::DragSelectedItems;

/// Sub states of the select state (upstream `SubState`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum SubState {
    #[default]
    Idle,
    Selecting,
    Moving,
    MovingVertex,
    Pasting,
    /// Resizing a selected image with its handle (upstream
    /// `RESIZING_IMAGE`).
    ResizingImage,
    /// Setting the size of an image being added, after it was placed
    /// (upstream `SymbolEditorState_AddImage` in `State::Resizing`).
    AddingImage,
}

/// The image being resized, with its aspect ratio.
#[derive(Debug, Clone)]
struct ImageResize {
    image: librepcb_core::geometry::Image,
    aspect_ratio: f64,
}

/// The vertex handle radius of images: 20 pixels (upstream
/// `ImageGraphicsItem`), i.e. four times the hit tolerance.
const IMAGE_HANDLE_TOLERANCE_FACTOR: i64 = 4;

/// The select state.
#[derive(Debug)]
pub(crate) struct SelectState<H: ElementHost> {
    sub: SubState,
    start_pos: Point,
    /// The active transformation (moving or pasting; an undo group is open
    /// while it exists).
    drag: Option<DragSelectedItems<H::Item>>,
    /// The polygon/zone and its vertices being moved.
    vertices: Option<(H::Item, Vec<usize>)>,
    /// The image being resized.
    image_resize: Option<ImageResize>,
    /// Whether the items being pasted are an image being added (its size is
    /// set after placing it).
    adding_image: bool,
}

impl<H: ElementHost> Default for SelectState<H> {
    fn default() -> Self {
        Self {
            sub: SubState::Idle,
            start_pos: Point::ORIGIN,
            drag: None,
            vertices: None,
            image_resize: None,
            adding_image: false,
        }
    }
}

fn drag_text<H: ElementHost>() -> String {
    if H::IS_FOOTPRINT {
        tr!(
            "librepcb::editor::CmdDragSelectedFootprintItems",
            "Drag Footprint Elements"
        )
    } else {
        tr!(
            "librepcb::editor::CmdDragSelectedSymbolItems",
            "Drag Symbol Elements"
        )
    }
}

fn remove_text<H: ElementHost>() -> String {
    if H::IS_FOOTPRINT {
        tr!(
            "librepcb::editor::CmdRemoveSelectedFootprintItems",
            "Remove Footprint Elements"
        )
    } else {
        tr!(
            "librepcb::editor::CmdRemoveSelectedSymbolItems",
            "Remove Symbol Elements"
        )
    }
}

impl<H: ElementHost> SelectState<H> {
    /// Finds vertices of a selected polygon/zone at `pos` (upstream
    /// `findPolygonVerticesAtPosition()` / `findZoneVerticesAtPosition()`).
    fn find_vertices_at(&self, cx: &Cx<'_, '_, H>, pos: Point) -> Option<(H::Item, Vec<usize>)> {
        let tol = cx.ctx.view.tolerance();
        cx.out.selection.iter().find_map(|item| {
            let path = H::item_path(cx.element(), cx.fpt(), *item)?;
            let v = vertex_indices_at(&path, pos, tol);
            (!v.is_empty()).then_some((*item, v))
        })
    }

    /// Finds the resize handle of a selected image at `pos` (upstream
    /// `findImageHandleAtPosition()`).
    fn find_image_handle(&self, cx: &Cx<'_, '_, H>, pos: Point) -> Option<ImageResize> {
        let tolerance = cx.ctx.view.tolerance() * IMAGE_HANDLE_TOLERANCE_FACTOR;
        cx.out.selection.iter().find_map(|item| {
            let image = H::image(cx.element(), cx.fpt(), *item)?;
            let rel = pos.rotated(-image.rotation(), image.position()) - image.position();
            let corner = Point::new(image.width().get(), image.height().get());
            ((corner - rel).length().get() <= tolerance).then(|| ImageResize {
                aspect_ratio: image.width().to_mm() / image.height().to_mm(),
                image,
            })
        })
    }

    /// Resizes the image being resized or added so that its corner is at
    /// `pos` (upstream `updateSize()`; the aspect ratio is kept).
    fn resize_image(&mut self, cx: &mut Cx<'_, '_, H>, pos: Point) {
        let Some(resize) = &self.image_resize else {
            return;
        };
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
        cx.modify(|e, fpt| H::set_image(e, fpt, image));
    }

    /// The pointer position mapped to the grid unless Shift is pressed.
    fn image_pos(cx: &Cx<'_, '_, H>, e: PointerEvent) -> Point {
        if e.modifiers.shift {
            e.pos
        } else {
            e.pos.mapped_to_grid(cx.grid())
        }
    }

    /// Applies `f` to a new or the active transformation and the
    /// container; creates an own undo group if no drag is active (upstream
    /// `execCmd(new CmdDragSelected*Items)`).
    fn transform(
        &mut self,
        cx: &mut Cx<'_, '_, H>,
        f: impl FnOnce(
            &mut DragSelectedItems<H::Item>,
            &mut dyn crate::library_editor::commands::ItemContainer<H::Item>,
        ),
    ) -> bool {
        let grid = cx.grid();
        if let Some(drag) = self.drag.as_mut() {
            cx.modify(|e, fpt| {
                if let Some(c) = H::container_mut(e, fpt) {
                    f(drag, c);
                }
            });
            return true;
        }
        let items = cx.out.selection.clone();
        if items.is_empty() {
            return false;
        }
        cx.exec_group(drag_text::<H>(), |e, fpt| {
            if let Some(c) = H::container_mut(e, fpt) {
                let mut drag = DragSelectedItems::new(&*c, items, grid);
                f(&mut drag, c);
            }
            Ok(())
        });
        true
    }

    /// Upstream `rotateSelectedItems()`.
    fn rotate_selected(&mut self, cx: &mut Cx<'_, '_, H>, angle: Angle) -> bool {
        self.transform(cx, |d, c| d.rotate(c, angle))
    }

    /// Upstream `mirrorSelectedItems()`.
    fn mirror_selected(
        &mut self,
        cx: &mut Cx<'_, '_, H>,
        orientation: Orientation,
        flip: bool,
    ) -> bool {
        self.transform(cx, |d, c| {
            d.mirror_geometry(c, orientation);
            if flip {
                d.mirror_layer(c);
            }
        })
    }

    /// Upstream `removeSelectedItems()`.
    fn remove_selected(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        let items = cx.out.selection.clone();
        if items.is_empty() {
            return false;
        }
        cx.exec_group(remove_text::<H>(), |e, fpt| {
            H::remove_items(e, fpt, &items);
            Ok(())
        });
        cx.out.selection.clear();
        true
    }

    /// Upstream `copySelectedItemsToClipboard()`.
    fn copy_selected(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        let pos = cx.cursor_pos();
        match H::copy_items(
            cx.element(),
            cx.fpt(),
            &cx.out.selection,
            pos,
            &cx.settings.app_version,
        ) {
            Ok(Some((mime, data))) => {
                cx.ctx.clipboard.set(&mime, data);
                let ctx = if H::IS_FOOTPRINT {
                    "librepcb::editor::PackageEditorState_Select"
                } else {
                    "librepcb::editor::SymbolEditorState_Select"
                };
                cx.out
                    .view
                    .set_status(tr!(ctx, "Copied to clipboard!"), Some(2000));
            }
            Ok(None) => {}
            Err(e) => cx.error(e),
        }
        true
    }

    /// Upstream `processPaste()` with data from the clipboard.
    fn start_paste(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        match H::clipboard_data(&*cx.ctx.clipboard, &cx.settings.app_version) {
            Ok(Some(d))
                if H::can_paste_geometry(cx.element(), cx.fpt(), &d.0, &cx.out.selection) =>
            {
                // Only the geometry is applied to the selected objects.
                self.paste_geometry_data(cx, &d.0)
            }
            Ok(Some(d)) => self.start_paste_data(cx, d, None),
            Ok(None) => false,
            Err(e) => {
                cx.error(e);
                false
            }
        }
    }

    /// Whether the clipboard content can be pasted as geometry onto the
    /// selected objects (upstream `canPasteGeometry()`, for the "Paste
    /// Geometry" context menu entry).
    pub(crate) fn can_paste_geometry(&self, cx: &Cx<'_, '_, H>) -> bool {
        self.sub == SubState::Idle
            && matches!(
                H::clipboard_data(&*cx.ctx.clipboard, &cx.settings.app_version),
                Ok(Some(d)) if H::can_paste_geometry(cx.element(), cx.fpt(), &d.0, &cx.out.selection)
            )
    }

    /// Pastes the geometry of the clipboard object onto the selected
    /// objects (upstream `pasteGeometryFromClipboard()`).
    pub(crate) fn paste_geometry(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        match H::clipboard_data(&*cx.ctx.clipboard, &cx.settings.app_version) {
            Ok(Some(d))
                if self.sub == SubState::Idle
                    && H::can_paste_geometry(cx.element(), cx.fpt(), &d.0, &cx.out.selection) =>
            {
                self.paste_geometry_data(cx, &d.0)
            }
            Ok(_) => false,
            Err(e) => {
                cx.error(e);
                false
            }
        }
    }

    fn paste_geometry_data(&mut self, cx: &mut Cx<'_, '_, H>, data: &H::ClipboardData) -> bool {
        let selection = cx.out.selection.clone();
        // Only the package editor supports it (see `can_paste_geometry()`).
        let text = tr!(
            "librepcb::editor::PackageEditorState_Select",
            "Paste Geometry"
        );
        cx.exec_group(text, |e, fpt| {
            H::paste_geometry(e, fpt, data, &selection);
            Ok(())
        })
        .is_some()
    }

    /// Upstream `startPaste()`: pastes `data` (with its cursor position)
    /// and lets the items follow the cursor, or pastes them with the fixed
    /// offset `fixed` and finishes.
    pub(crate) fn start_paste_data(
        &mut self,
        cx: &mut Cx<'_, '_, H>,
        data: (H::ClipboardData, Point),
        fixed: Option<Point>,
    ) -> bool {
        self.start_paste_with_text(cx, H::paste_text(), data, fixed)
    }

    /// Starts adding an image (upstream `SymbolEditorState_AddImage`):
    /// `data` holds the image, which follows the cursor until a click
    /// places it; then its size follows the cursor (keeping the aspect
    /// ratio) until the next click. One undo group with `text`.
    pub(crate) fn start_adding_image(
        &mut self,
        cx: &mut Cx<'_, '_, H>,
        text: String,
        data: (H::ClipboardData, Point),
    ) -> bool {
        let ok = self.start_paste_with_text(cx, text, data, None);
        self.adding_image = ok;
        ok
    }

    fn start_paste_with_text(
        &mut self,
        cx: &mut Cx<'_, '_, H>,
        text: String,
        data: (H::ClipboardData, Point),
        fixed: Option<Point>,
    ) -> bool {
        if self.sub != SubState::Idle {
            return false;
        }
        cx.out.selection.clear();
        if !cx.begin(text) {
            return false;
        }
        self.start_pos = cx.cursor_pos();
        let offset = fixed.unwrap_or_else(|| (self.start_pos - data.1).mapped_to_grid(cx.grid()));
        let pasted = cx.modify(|e, fpt| H::paste(e, fpt, data.0, offset));
        match pasted {
            Some(Ok(items)) if !items.is_empty() && fixed.is_some() => {
                // Fixed position (no interactive placement): finish.
                cx.commit()
            }
            Some(Ok(items)) if !items.is_empty() => {
                let grid = cx.grid();
                self.drag = H::container(cx.element(), cx.fpt())
                    .map(|c| DragSelectedItems::new(c, items.clone(), grid));
                cx.out.selection = items;
                self.sub = SubState::Pasting;
                true
            }
            Some(Err(e)) => {
                cx.error(e);
                cx.abort_group();
                false
            }
            _ => {
                cx.abort_group();
                false
            }
        }
    }

    /// Removes vertices of a polygon/zone (upstream
    /// `removePolygonVertices()`).
    pub(crate) fn remove_vertices(
        &mut self,
        cx: &mut Cx<'_, '_, H>,
        item: H::Item,
        vertices: &BTreeSet<usize>,
    ) -> bool {
        if self.sub != SubState::Idle {
            return false;
        }
        let Some(path) = H::item_path(cx.element(), cx.fpt(), item) else {
            return false;
        };
        let Some(new) =
            crate::library_editor::commands::symbol::path_without_vertices(&path, vertices)
        else {
            return false;
        };
        cx.exec_group(
            tr!("librepcb::editor::CmdPolygonEdit", "Edit polygon"),
            |e, fpt| {
                H::set_item_path(e, fpt, item, new);
                Ok(())
            },
        );
        true
    }

    /// Inserts a vertex before `index` and starts moving it (upstream
    /// `startAddingPolygonVertex()`).
    pub(crate) fn start_adding_vertex(
        &mut self,
        cx: &mut Cx<'_, '_, H>,
        item: H::Item,
        index: usize,
        pos: Point,
    ) -> bool {
        if self.sub != SubState::Idle || index == 0 {
            return false;
        }
        let Some(path) = H::item_path(cx.element(), cx.fpt(), item) else {
            return false;
        };
        if index > path.vertices().len() {
            return false;
        }
        let mut new = path.clone();
        let angle = path.vertices()[index - 1].angle;
        new.insert_vertex(index, pos.mapped_to_grid(cx.grid()), angle);
        if !cx.begin(tr!("librepcb::editor::CmdPolygonEdit", "Edit polygon")) {
            return false;
        }
        cx.modify(|e, fpt| H::set_item_path(e, fpt, item, new));
        self.vertices = Some((item, vec![index]));
        self.start_pos = pos;
        self.sub = SubState::MovingVertex;
        true
    }

    /// The positions of the selected items for "move/align" (upstream
    /// `CmdDragSelectedFootprintItems::getPositions()`).
    pub(crate) fn move_align_positions(&self, cx: &Cx<'_, '_, H>) -> Option<Vec<Point>> {
        if self.sub != SubState::Idle || cx.out.selection.is_empty() {
            return None;
        }
        H::container(cx.element(), cx.fpt()).map(|c| c.positions(&cx.out.selection))
    }

    /// Moves the selected items to new positions (upstream
    /// `processMoveAlign()` with the result of the "move/align" dialog).
    pub(crate) fn move_align(&mut self, cx: &mut Cx<'_, '_, H>, positions: &[Point]) -> bool {
        if self.sub != SubState::Idle || cx.out.selection.is_empty() {
            return false;
        }
        let items = cx.out.selection.clone();
        let grid = cx.grid();
        let positions = positions.to_vec();
        cx.exec_group(drag_text::<H>(), |e, fpt| {
            let c = H::container_mut(e, fpt)
                .ok_or_else(|| crate::Error::InvalidArgument("No footprint.".to_owned()))?;
            let mut drag = DragSelectedItems::new(&*c, items, grid);
            drag.set_new_positions(c, &positions)
        })
        .is_some()
    }

    /// Upstream `openContextMenuAtPos()`.
    fn open_context_menu(&mut self, cx: &mut Cx<'_, '_, H>, pos: Point) -> bool {
        let items = cx.ctx.view.items_at(pos, cx.ctx.view.tolerance());
        let Some(first) = items.first().copied() else {
            return false;
        };
        let selected = items
            .iter()
            .rev()
            .find(|i| cx.out.selection.contains(i))
            .copied();
        let item = match selected {
            Some(i) => i,
            None => {
                cx.out.selection = [first].into_iter().collect();
                first
            }
        };
        let tol = cx.ctx.view.tolerance();
        let (vertices, segment) = match H::item_path(cx.element(), cx.fpt(), item) {
            Some(path) => (
                vertex_indices_at(&path, pos, tol),
                segment_index_at(&path, pos, tol),
            ),
            None => (Vec::new(), None),
        };
        cx.out.requests.push(LibraryRequest::ContextMenu {
            item,
            pos,
            vertices,
            segment,
        });
        true
    }

    /// Upstream `openPropertiesDialogOfItemAtPos()`.
    fn open_properties_at(&mut self, cx: &mut Cx<'_, '_, H>, pos: Point) -> bool {
        let items = cx.ctx.view.items_at(pos, cx.ctx.view.tolerance());
        match items.into_iter().find(|i| cx.out.selection.contains(i)) {
            Some(item) => {
                cx.out.requests.push(LibraryRequest::Properties(item));
                true
            }
            None => false,
        }
    }

    fn update_features(&self, cx: &mut Cx<'_, '_, H>) {
        let writable = cx.writable();
        let has_selection = !cx.out.selection.is_empty();
        let idle = self.sub == SubState::Idle;
        let placing = matches!(self.sub, SubState::Moving | SubState::Pasting);
        cx.out.view.features = Features {
            select: idle,
            cut: idle && has_selection && writable,
            copy: idle && has_selection,
            paste: idle && writable,
            remove: idle && has_selection && writable,
            rotate: (idle || placing) && has_selection && writable,
            mirror: (idle || placing) && has_selection && writable,
            flip: H::IS_FOOTPRINT && (idle || placing) && has_selection && writable,
            snap_to_grid: (idle || placing) && has_selection && writable,
            properties: idle && has_selection,
            // Upstream `ImportGraphics` (DXF import).
            import_graphics: idle && writable,
            ..Features::default()
        };
    }
}

impl<H: ElementHost> State<H> for SelectState<H> {
    fn entry(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        cx.out.tool = LibraryTool::Select;
        self.update_features(cx);
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        State::<H>::abort(self, cx);
        // Avoid propagating the selection to other tools.
        cx.out.selection.clear();
        cx.out.view.rubber_band = None;
        cx.out.view.features = Features::default();
        true
    }

    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_, H>, e: PointerEvent) -> bool {
        let pos = e.pos;
        match self.sub {
            SubState::Selecting => {
                cx.out.view.rubber_band = Some((self.start_pos, pos));
                cx.out.selection = cx
                    .ctx
                    .view
                    .items_in_rect(self.start_pos, pos)
                    .into_iter()
                    .collect();
                true
            }
            SubState::Moving | SubState::Pasting => {
                let grid = cx.grid();
                if self.drag.is_none() {
                    // Moving: start the undo group on the first move.
                    if !cx.begin(drag_text::<H>()) {
                        self.sub = SubState::Idle;
                        return false;
                    }
                    let items = cx.out.selection.clone();
                    self.drag = H::container(cx.element(), cx.fpt())
                        .map(|c| DragSelectedItems::new(c, items, grid));
                }
                let delta = (pos - self.start_pos).mapped_to_grid(grid);
                if let Some(drag) = self.drag.as_mut() {
                    cx.modify(|e, fpt| {
                        if let Some(c) = H::container_mut(e, fpt) {
                            drag.set_delta_to_start_pos(c, delta);
                        }
                    });
                }
                true
            }
            SubState::MovingVertex => {
                let Some((item, indices)) = self.vertices.clone() else {
                    return false;
                };
                if !cx.ctx.editor.is_group_active()
                    && !cx.begin(tr!("librepcb::editor::CmdPolygonEdit", "Edit polygon"))
                {
                    return false;
                }
                let Some(path) = H::item_path(cx.element(), cx.fpt(), item) else {
                    return false;
                };
                let snapped = pos.mapped_to_grid(cx.grid());
                let mut v = path.into_vertices();
                for i in indices {
                    if let Some(vertex) = v.get_mut(i) {
                        vertex.pos = snapped;
                    }
                }
                let path = librepcb_core::geometry::Path::new(v);
                cx.modify(|e, fpt| H::set_item_path(e, fpt, item, path));
                true
            }
            SubState::ResizingImage => {
                // Start the undo group on the first move.
                if !cx.ctx.editor.is_group_active()
                    && !cx.begin(tr!("librepcb::editor::CmdImageEdit", "Edit Image"))
                {
                    return false;
                }
                let pos = Self::image_pos(cx, e);
                self.resize_image(cx, pos);
                true
            }
            SubState::AddingImage => {
                let pos = Self::image_pos(cx, e);
                if self
                    .image_resize
                    .as_ref()
                    .is_some_and(|r| r.image.position() != pos)
                {
                    self.resize_image(cx, pos);
                }
                true
            }
            SubState::Idle => {
                cx.out.hovered = cx
                    .ctx
                    .view
                    .items_at(pos, cx.ctx.view.tolerance())
                    .first()
                    .copied();
                false
            }
        }
    }

    fn left_pressed(&mut self, cx: &mut Cx<'_, '_, H>, e: PointerEvent) -> bool {
        match self.sub {
            SubState::Idle => {
                self.start_pos = e.pos;
                if cx.writable()
                    && let Some(v) = self.find_vertices_at(cx, e.pos)
                {
                    self.vertices = Some(v);
                    self.sub = SubState::MovingVertex;
                    return true;
                }
                if cx.writable()
                    && let Some(resize) = self.find_image_handle(cx, e.pos)
                {
                    self.image_resize = Some(resize);
                    self.sub = SubState::ResizingImage;
                    return true;
                }
                let items = cx.ctx.view.items_at(e.pos, cx.ctx.view.tolerance());
                if items.is_empty() {
                    // Start selecting.
                    cx.out.selection.clear();
                    self.sub = SubState::Selecting;
                    return true;
                }
                let selected = items.iter().find(|i| cx.out.selection.contains(i)).copied();
                if e.modifiers.control {
                    // Toggle selection.
                    let item = selected.unwrap_or(items[0]);
                    if !cx.out.selection.remove(&item) {
                        cx.out.selection.insert(item);
                    }
                } else if e.modifiers.shift {
                    // Cycle selection.
                    let next = items
                        .iter()
                        .position(|i| cx.out.selection.contains(i))
                        .map_or(0, |i| (i + 1) % items.len());
                    cx.out.selection = [items[next]].into_iter().collect();
                } else if selected.is_none() {
                    cx.out.selection = [items[0]].into_iter().collect();
                }
                if cx.writable() {
                    self.drag = None;
                    self.sub = SubState::Moving;
                }
                true
            }
            SubState::Pasting if self.adding_image => {
                // The image is placed, now its size follows the cursor.
                self.drag = None;
                let image = cx
                    .out
                    .selection
                    .iter()
                    .find_map(|item| H::image(cx.element(), cx.fpt(), *item));
                match image {
                    Some(image) => {
                        self.image_resize = Some(ImageResize {
                            aspect_ratio: image.width().to_mm() / image.height().to_mm(),
                            image,
                        });
                        self.sub = SubState::AddingImage;
                    }
                    None => {
                        cx.commit();
                        self.adding_image = false;
                        self.sub = SubState::Idle;
                    }
                }
                true
            }
            SubState::Pasting => {
                self.drag = None;
                cx.commit();
                self.sub = SubState::Idle;
                cx.out.selection.clear();
                true
            }
            SubState::AddingImage => {
                // Upstream `finish()`: a click at the image position aborts.
                let pos = Self::image_pos(cx, e);
                if self
                    .image_resize
                    .as_ref()
                    .is_some_and(|r| r.image.position() == pos)
                {
                    cx.abort_group();
                } else {
                    self.resize_image(cx, pos);
                    cx.commit();
                }
                self.image_resize = None;
                self.adding_image = false;
                self.sub = SubState::Idle;
                cx.out.selection.clear();
                true
            }
            _ => false,
        }
    }

    fn left_released(&mut self, cx: &mut Cx<'_, '_, H>, _e: PointerEvent) -> bool {
        match self.sub {
            SubState::Selecting => {
                cx.out.view.rubber_band = None;
                self.sub = SubState::Idle;
                true
            }
            SubState::Moving => {
                if self.drag.take().is_some() {
                    cx.commit();
                }
                self.sub = SubState::Idle;
                true
            }
            SubState::MovingVertex => {
                if cx.ctx.editor.is_group_active() {
                    cx.commit();
                }
                self.vertices = None;
                self.sub = SubState::Idle;
                true
            }
            SubState::ResizingImage => {
                if cx.ctx.editor.is_group_active() {
                    cx.commit();
                }
                self.image_resize = None;
                self.sub = SubState::Idle;
                true
            }
            _ => false,
        }
    }

    fn left_double_clicked(&mut self, cx: &mut Cx<'_, '_, H>, e: PointerEvent) -> bool {
        // With Shift or Ctrl, the user is modifying the selection.
        if e.modifiers.shift || e.modifiers.control {
            return State::<H>::left_pressed(self, cx, e);
        }
        if self.sub == SubState::Idle {
            self.open_properties_at(cx, e.pos)
        } else {
            false
        }
    }

    fn right_released(&mut self, cx: &mut Cx<'_, '_, H>, e: PointerEvent) -> bool {
        match self.sub {
            SubState::Idle => self.open_context_menu(cx, e.pos),
            SubState::Moving | SubState::Pasting => self.rotate_selected(cx, Angle::DEG90),
            _ => false,
        }
    }

    fn select_all(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        if self.sub != SubState::Idle {
            return false;
        }
        cx.out.selection = H::all_items(cx.element(), cx.fpt());
        true
    }

    fn cut(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        self.sub == SubState::Idle && self.copy_selected(cx) && self.remove_selected(cx)
    }

    fn copy(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        self.sub == SubState::Idle && self.copy_selected(cx)
    }

    fn paste(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        self.sub == SubState::Idle && self.start_paste(cx)
    }

    fn move_by(&mut self, cx: &mut Cx<'_, '_, H>, delta: Point) -> bool {
        if self.sub != SubState::Idle {
            return false;
        }
        self.transform(cx, |d, c| d.translate(c, delta))
    }

    fn rotate(&mut self, cx: &mut Cx<'_, '_, H>, angle: Angle) -> bool {
        match self.sub {
            SubState::Idle | SubState::Moving | SubState::Pasting => {
                self.rotate_selected(cx, angle)
            }
            _ => false,
        }
    }

    fn mirror(&mut self, cx: &mut Cx<'_, '_, H>, orientation: Orientation) -> bool {
        match self.sub {
            SubState::Idle | SubState::Moving | SubState::Pasting => {
                self.mirror_selected(cx, orientation, false)
            }
            _ => false,
        }
    }

    fn flip(&mut self, cx: &mut Cx<'_, '_, H>, orientation: Orientation) -> bool {
        match self.sub {
            SubState::Idle | SubState::Moving | SubState::Pasting if H::IS_FOOTPRINT => {
                self.mirror_selected(cx, orientation, true)
            }
            _ => false,
        }
    }

    fn snap_to_grid(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        match self.sub {
            SubState::Idle | SubState::Moving | SubState::Pasting => {
                let grid = cx.grid();
                self.transform(cx, |d, c| d.snap_to_grid(c, grid))
            }
            _ => false,
        }
    }

    fn remove(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        self.sub == SubState::Idle && self.remove_selected(cx)
    }

    fn edit_properties(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        if self.sub != SubState::Idle {
            return false;
        }
        match cx.out.selection.iter().next().copied() {
            Some(item) => {
                cx.out.requests.push(LibraryRequest::Properties(item));
                true
            }
            None => false,
        }
    }

    fn abort(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        match self.sub {
            SubState::Moving | SubState::Pasting => {
                if self.drag.take().is_some() || self.sub == SubState::Pasting {
                    cx.abort_group();
                }
                if self.sub == SubState::Pasting {
                    cx.out.selection.clear();
                }
                self.adding_image = false;
                self.sub = SubState::Idle;
            }
            SubState::ResizingImage | SubState::AddingImage => {
                if cx.ctx.editor.is_group_active() {
                    cx.abort_group();
                }
                if self.sub == SubState::AddingImage {
                    cx.out.selection.clear();
                }
                self.image_resize = None;
                self.adding_image = false;
                self.sub = SubState::Idle;
            }
            SubState::MovingVertex => {
                cx.abort_group();
                self.vertices = None;
                self.sub = SubState::Idle;
            }
            SubState::Selecting => {
                cx.out.view.rubber_band = None;
                self.sub = SubState::Idle;
            }
            SubState::Idle => {
                cx.out.selection.clear();
            }
        }
        true
    }

    fn update(&mut self, cx: &mut Cx<'_, '_, H>) {
        self.update_features(cx);
    }
}
