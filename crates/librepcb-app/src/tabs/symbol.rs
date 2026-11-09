//! The symbol editor tab.
//!
//! Port of libs/librepcb/editor/library/sym/symboltab.{h,cpp}: the symbol
//! rendered by `librepcb-scene`'s [`SymbolScene`] (rebuilt after each
//! modification, symbols are small), editing through the symbol editor
//! state machine ([`SymbolEditorFsm`]) with the tool bars of upstream's
//! `.slint` files, the metadata page (name, description, keywords, author,
//! version, deprecated, categories), the checks with approvals and
//! automatic fixes, undo/redo and saving (see [`ElementCore`]).
//!
//! Images and DXF files are chosen by the application (image file dialog,
//! DXF import dialog) and passed to [`SymbolTab::add_image()`] and
//! [`SymbolTab::import_dxf()`].
//!
//! Differences to upstream: graphics export is not available in the symbol
//! editor yet; the file system watcher ("files modified" banner) is not
//! ported.

use std::collections::HashSet;

use librepcb_app_ui as ui;
use librepcb_canvas::kurbo::{Point, Rect, Vec2};
use librepcb_canvas::peniko::Color;
use librepcb_canvas::{
    Grid, Modifiers, PointerAction, PointerButton, PointerKind, SelectionMode, View,
};
use librepcb_core::geometry::Image;
use librepcb_core::library::sym::Symbol;
use librepcb_core::types::{
    Angle, GridStyle, HAlign, Layer, Length, LengthUnit, Orientation, PositiveLength,
    UnsignedLength, VAlign,
};
use librepcb_editor::fsm::library::{
    ElementHost, LibraryContext, LibraryEditorSettings, LibraryRequest, LibraryTool, LibraryView,
    SymbolEditorFsm, SymbolHost,
};
use librepcb_editor::fsm::{PointerEvent, ViewState};
use librepcb_editor::library_editor::commands::SymbolItem;
use librepcb_i18n::tr;
use librepcb_scene::{ColorScheme, SymbolScene, SymbolSceneObject};

use super::editing::{
    DoubleClick, OverlayColors, Overlays, error_notification, fsm_key_event, fsm_modifiers,
    info_box_text, mouse_cursor, point_from_world, point_to_world, to_multi_line, to_single_line,
};
use super::element_core::{ElementCore, OpenMode};
use super::schematic_view::HIT_TOLERANCE_PX;
use super::{ContextMenuEntry, DxfImportKind, TabId, TabRequest, TabUpdate, feature};
use crate::canvas_view::{CanvasView, DEFAULT_SCHEMATIC_RECT};
use crate::clipboard::{ensure_opened, with_clipboard};
use crate::helpers::{
    color_to_ui, grid_style_from_ui, grid_style_to_ui, length_from_ui, length_to_ui, unit_from_ui,
    unit_to_ui,
};
use crate::length_edit::{LengthEdit, steps};

/// Grid color of the schematic color scheme.
const GRID_COLOR: Color = Color::from_rgb8(0xa0, 0xa0, 0xa4);

/// The actions of the entries of a scene context menu (upstream
/// `SymbolEditorState_Select::openContextMenuAtPos()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryMenuAction {
    /// Properties.
    Properties,
    /// Remove the polygon vertices at the position.
    RemoveVertices,
    /// Add a vertex to the polygon segment.
    AddVertex,
    /// Cut.
    Cut,
    /// Copy.
    Copy,
    /// Apply the geometry of the clipboard object to the selected objects
    /// (package editor).
    PasteGeometry,
    /// Remove.
    Remove,
    /// Rotate counterclockwise.
    RotateCcw,
    /// Rotate clockwise.
    RotateCw,
    /// Mirror horizontally.
    MirrorHorizontal,
    /// Mirror vertically.
    MirrorVertical,
    /// Flip horizontally (footprints).
    FlipHorizontal,
    /// Snap to grid.
    SnapToGrid,
    /// Separator.
    Separator,
}

/// The entries of the context menu of an item (`paste_geometry`: the
/// clipboard object's geometry can be applied to the selection, upstream
/// `canPasteGeometry()`).
pub fn context_menu_entries(
    has_properties: bool,
    remove_vertex: bool,
    add_vertex: bool,
    footprint: bool,
    paste_geometry: bool,
) -> (Vec<LibraryMenuAction>, Vec<ContextMenuEntry>) {
    use LibraryMenuAction as A;
    let mut actions = Vec::new();
    if has_properties {
        actions.extend([A::Properties, A::Separator]);
    }
    if remove_vertex {
        actions.push(A::RemoveVertices);
    }
    if add_vertex {
        actions.push(A::AddVertex);
    }
    if remove_vertex || add_vertex {
        actions.push(A::Separator);
    }
    actions.extend([A::Cut, A::Copy]);
    if paste_geometry {
        actions.push(A::PasteGeometry);
    }
    actions.extend([A::Remove, A::Separator, A::RotateCcw, A::RotateCw]);
    if footprint {
        actions.push(A::FlipHorizontal);
    } else {
        actions.extend([A::MirrorHorizontal, A::MirrorVertical]);
    }
    actions.extend([A::Separator, A::SnapToGrid]);
    let entries = actions
        .iter()
        .map(|a| match a {
            A::Separator => ContextMenuEntry::separator(),
            A::Properties => ContextMenuEntry {
                is_default: true,
                ..ContextMenuEntry::new(tr!("EditorCommandSet", "Properties"))
            },
            A::RemoveVertices => ContextMenuEntry::new(tr!("EditorCommandSet", "Remove Vertex")),
            A::AddVertex => ContextMenuEntry::new(tr!("EditorCommandSet", "Add Vertex")),
            A::Cut => ContextMenuEntry::new(tr!("EditorCommandSet", "Cut")),
            A::Copy => ContextMenuEntry::new(tr!("EditorCommandSet", "Copy")),
            A::PasteGeometry => ContextMenuEntry::new(tr!(
                "librepcb::editor::PackageEditorState_Select",
                "Paste Geometry"
            )),
            A::Remove => ContextMenuEntry::new(tr!("EditorCommandSet", "Remove")),
            A::RotateCcw => {
                ContextMenuEntry::new(tr!("EditorCommandSet", "Rotate Counterclockwise"))
            }
            A::RotateCw => ContextMenuEntry::new(tr!("EditorCommandSet", "Rotate Clockwise")),
            A::MirrorHorizontal => {
                ContextMenuEntry::new(tr!("EditorCommandSet", "Mirror Horizontally"))
            }
            A::MirrorVertical => {
                ContextMenuEntry::new(tr!("EditorCommandSet", "Mirror Vertically"))
            }
            A::FlipHorizontal => {
                ContextMenuEntry::new(tr!("EditorCommandSet", "Flip Horizontally"))
            }
            A::SnapToGrid => ContextMenuEntry::new(tr!("EditorCommandSet", "Snap to Grid")),
        })
        .collect();
    (actions, entries)
}

/// Upstream `l2s(HAlign)`.
pub fn halign_to_ui(a: HAlign) -> slint::private_unstable_api::re_exports::TextHorizontalAlignment {
    use slint::private_unstable_api::re_exports::TextHorizontalAlignment as T;
    match a {
        HAlign::Left => T::Left,
        HAlign::Center => T::Center,
        HAlign::Right => T::Right,
    }
}

/// Upstream `s2l(TextHorizontalAlignment)`.
pub fn halign_from_ui(
    a: slint::private_unstable_api::re_exports::TextHorizontalAlignment,
) -> HAlign {
    use slint::private_unstable_api::re_exports::TextHorizontalAlignment as T;
    match a {
        T::Center => HAlign::Center,
        T::Right | T::End => HAlign::Right,
        _ => HAlign::Left,
    }
}

/// Upstream `l2s(VAlign)`.
pub fn valign_to_ui(a: VAlign) -> slint::private_unstable_api::re_exports::TextVerticalAlignment {
    use slint::private_unstable_api::re_exports::TextVerticalAlignment as T;
    match a {
        VAlign::Top => T::Top,
        VAlign::Center => T::Center,
        VAlign::Bottom => T::Bottom,
    }
}

/// Upstream `s2l(TextVerticalAlignment)`.
pub fn valign_from_ui(a: slint::private_unstable_api::re_exports::TextVerticalAlignment) -> VAlign {
    use slint::private_unstable_api::re_exports::TextVerticalAlignment as T;
    match a {
        T::Top => VAlign::Top,
        T::Center => VAlign::Center,
        _ => VAlign::Bottom,
    }
}

/// The editor tool of the tool bar.
pub fn tool_to_ui(tool: LibraryTool) -> ui::EditorTool {
    match tool {
        LibraryTool::Select => ui::EditorTool::Select,
        LibraryTool::AddPins => ui::EditorTool::Pin,
        LibraryTool::AddThtPads => ui::EditorTool::PadTht,
        LibraryTool::AddSmtPads(_) => ui::EditorTool::PadSmt,
        LibraryTool::AddNames => ui::EditorTool::Name,
        LibraryTool::AddValues => ui::EditorTool::Value,
        LibraryTool::DrawLine => ui::EditorTool::Line,
        LibraryTool::DrawRect => ui::EditorTool::Rect,
        LibraryTool::DrawPolygon => ui::EditorTool::Polygon,
        LibraryTool::DrawCircle => ui::EditorTool::Circle,
        LibraryTool::DrawArc => ui::EditorTool::Arc,
        LibraryTool::DrawText => ui::EditorTool::Text,
        LibraryTool::DrawZone => ui::EditorTool::Zone,
        LibraryTool::AddHoles => ui::EditorTool::Hole,
        LibraryTool::Measure => ui::EditorTool::Measure,
        LibraryTool::RenumberPads => ui::EditorTool::RenumberPads,
    }
}

/// Layers sorted like upstream `Layer::sorted()`.
pub fn sorted_layers(layers: &[Layer]) -> Vec<Layer> {
    Layer::all()
        .iter()
        .copied()
        .filter(|l| layers.contains(l))
        .collect()
}

fn symbol_item(o: SymbolSceneObject) -> SymbolItem {
    match o {
        SymbolSceneObject::Pin(u) => SymbolItem::Pin(u),
        SymbolSceneObject::Polygon(u) => SymbolItem::Polygon(u),
        SymbolSceneObject::Circle(u) => SymbolItem::Circle(u),
        SymbolSceneObject::Text(u) => SymbolItem::Text(u),
        SymbolSceneObject::Image(u) => SymbolItem::Image(u),
    }
}

fn scene_object(item: SymbolItem) -> SymbolSceneObject {
    match item {
        SymbolItem::Pin(u) => SymbolSceneObject::Pin(u),
        SymbolItem::Polygon(u) => SymbolSceneObject::Polygon(u),
        SymbolItem::Circle(u) => SymbolSceneObject::Circle(u),
        SymbolItem::Text(u) => SymbolSceneObject::Text(u),
        SymbolItem::Image(u) => SymbolSceneObject::Image(u),
    }
}

/// The view of the symbol tab as the FSM sees it.
struct SymbolSceneView<'a> {
    scene: Option<&'a SymbolScene>,
    view: &'a View,
    cursor: Option<librepcb_core::types::Point>,
    /// The images of the symbol: the scene draws only their borders, so
    /// they are hit tested here (upstream `ImageGraphicsItem::shape()`: the
    /// image rectangle).
    images: &'a [Image],
}

/// The corners of an image (world coordinates, mm).
fn image_corners(image: &Image) -> [Point; 4] {
    let xf = librepcb_canvas::kurbo::Affine::translate(point_to_world(image.position()).to_vec2())
        * librepcb_canvas::kurbo::Affine::rotate(librepcb_canvas::convert::angle(image.rotation()));
    let (w, h) = (image.width().to_mm(), image.height().to_mm());
    [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)].map(|(x, y)| xf * Point::new(x, y))
}

impl LibraryView<SymbolItem> for SymbolSceneView<'_> {
    fn items_at(&self, pos: librepcb_core::types::Point, tolerance: Length) -> Vec<SymbolItem> {
        let mut items: Vec<SymbolItem> = self
            .scene
            .map(|s| s.objects_at(pos, tolerance))
            .unwrap_or_default()
            .into_iter()
            .map(symbol_item)
            .collect();
        // Images are below all other items.
        let p = point_to_world(pos);
        for image in self.images.iter().rev() {
            let item = SymbolItem::Image(image.uuid());
            let path = librepcb_canvas::kurbo::BezPath::from_iter(
                image_corners(image)
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        if i == 0 {
                            librepcb_canvas::kurbo::PathEl::MoveTo(*c)
                        } else {
                            librepcb_canvas::kurbo::PathEl::LineTo(*c)
                        }
                    })
                    .chain([librepcb_canvas::kurbo::PathEl::ClosePath]),
            );
            use librepcb_canvas::kurbo::Shape;
            if !items.contains(&item) && path.contains(p) {
                items.push(item);
            }
        }
        items
    }

    fn items_in_rect(
        &self,
        p1: librepcb_core::types::Point,
        p2: librepcb_core::types::Point,
    ) -> Vec<SymbolItem> {
        let mut items: Vec<SymbolItem> = self
            .scene
            .map(|s| s.objects_in_rect(p1, p2))
            .unwrap_or_default()
            .into_iter()
            .map(symbol_item)
            .collect();
        let rect = Rect::from_points(point_to_world(p1), point_to_world(p2));
        for image in self.images {
            let item = SymbolItem::Image(image.uuid());
            if !items.contains(&item) && image_corners(image).iter().any(|c| rect.contains(*c)) {
                items.push(item);
            }
        }
        items
    }

    fn tolerance(&self) -> Length {
        Length::from_mm(self.view.pixels_to_world(HIT_TOLERANCE_PX)).unwrap_or(Length::ZERO)
    }

    fn cursor_pos(&self) -> Option<librepcb_core::types::Point> {
        self.cursor
    }
}

/// The symbol editor tab (upstream `SymbolTab`).
pub struct SymbolTab {
    id: TabId,
    core: ElementCore<Symbol>,
    fsm: SymbolEditorFsm,
    scheme: ColorScheme,
    scene: Option<SymbolScene>,
    /// Editor state the scene was built for.
    scene_state: Option<u64>,
    canvas: CanvasView,
    overlays: Overlays,
    double_click: DoubleClick,
    cursor: Option<librepcb_core::types::Point>,
    grid_style: GridStyle,
    unit: LengthUnit,
    frame: i32,
    scene_image_pos: slint::LogicalPosition,
    compact_layout: bool,
    line_width: LengthEdit,
    size: LengthEdit,
    configured_tool: Option<LibraryTool>,
    menu: Vec<LibraryMenuAction>,
    menu_pos: (librepcb_core::types::Point, Vec<usize>, Option<usize>),
    menu_item: Option<SymbolItem>,
    import_pins_dismissed: bool,
    library_index: i32,
}

impl SymbolTab {
    /// Creates the tab for an element core.
    pub fn new(core: ElementCore<Symbol>, grid_style: GridStyle) -> Self {
        let scheme = ColorScheme::SCHEMATIC_LIGHT;
        let settings = LibraryEditorSettings {
            app_version: crate::app::APP_VERSION.to_owned(),
            ..LibraryEditorSettings::default()
        };
        let mut tab = Self {
            id: TabId::new(),
            core,
            fsm: SymbolEditorFsm::new(settings),
            scheme,
            scene: None,
            scene_state: None,
            canvas: CanvasView::new(Color::WHITE, DEFAULT_SCHEMATIC_RECT, None),
            overlays: Overlays::default(),
            double_click: DoubleClick::default(),
            cursor: None,
            grid_style,
            unit: LengthUnit::Millimeters,
            frame: 0,
            scene_image_pos: Default::default(),
            compact_layout: false,
            line_width: LengthEdit::new(steps::GENERIC),
            size: LengthEdit::new(steps::TEXT_HEIGHT),
            configured_tool: None,
            menu: Vec::new(),
            menu_pos: (Default::default(), Vec::new(), None),
            menu_item: None,
            import_pins_dismissed: false,
            library_index: -1,
        };
        tab.sync_scene();
        let bg = tab
            .scene
            .as_ref()
            .map_or(Color::WHITE, SymbolScene::background);
        tab.canvas = CanvasView::new(
            bg,
            DEFAULT_SCHEMATIC_RECT,
            tab.scene.as_ref().and_then(SymbolScene::content_bounds),
        );
        tab.apply_grid();
        tab.run(|f, c| f.select_tool(c));
        tab.after_fsm();
        tab
    }

    /// Sets the tab identifier (to connect models before the tab exists).
    pub fn with_id(mut self, id: TabId) -> Self {
        self.id = id;
        self
    }

    /// The unique tab identifier.
    pub fn id(&self) -> TabId {
        self.id
    }

    /// The shared element state.
    pub fn core(&self) -> &ElementCore<Symbol> {
        &self.core
    }

    /// The shared element state, mutably.
    pub fn core_mut(&mut self) -> &mut ElementCore<Symbol> {
        &mut self.core
    }

    /// The FSM.
    pub fn fsm(&self) -> &SymbolEditorFsm {
        &self.fsm
    }

    /// The scene.
    pub fn scene(&self) -> Option<&SymbolScene> {
        self.scene.as_ref()
    }

    /// The canvas view.
    pub fn canvas(&self) -> &CanvasView {
        &self.canvas
    }

    /// The length unit (for dialogs).
    pub fn length_unit(&self) -> LengthUnit {
        self.unit
    }

    /// Sets the grid style (workspace setting).
    pub fn set_grid_style(&mut self, style: GridStyle) -> TabUpdate {
        if style == self.grid_style {
            return TabUpdate::default();
        }
        self.grid_style = style;
        self.apply_grid();
        TabUpdate::repaint()
    }

    fn apply_grid(&mut self) {
        let interval = self.core.editor.element().grid_interval().get().to_mm();
        let grid = match self.grid_style {
            GridStyle::None => None,
            GridStyle::Dots => Some(librepcb_canvas::GridStyle::Dots),
            GridStyle::Lines => Some(librepcb_canvas::GridStyle::Lines),
        }
        .map(|style| Grid {
            interval,
            style,
            color: GRID_COLOR,
        });
        self.canvas.set_grid(grid);
    }

    /// Calls the FSM with a context on the element editor, the scene and
    /// the clipboard.
    fn run<R>(
        &mut self,
        f: impl FnOnce(&mut SymbolEditorFsm, &mut LibraryContext<'_, SymbolHost>) -> R,
    ) -> R {
        let images: Vec<Image> = self
            .core
            .editor
            .element()
            .images()
            .iter()
            .cloned()
            .collect();
        let view = SymbolSceneView {
            scene: self.scene.as_ref(),
            view: self.canvas.view(),
            cursor: self.cursor,
            images: &images,
        };
        let fsm = &mut self.fsm;
        let editor = &mut self.core.editor;
        with_clipboard(|clipboard| {
            let mut ctx = LibraryContext::new(editor, &view, clipboard);
            f(fsm, &mut ctx)
        })
    }

    /// Rebuilds the scene if the element changed.
    fn sync_scene(&mut self) -> bool {
        let state = self.core.editor.state_id();
        if self.scene_state == Some(state) && self.scene.is_some() {
            return false;
        }
        self.scene_state = Some(state);
        self.scene = Some(SymbolScene::build(
            self.core.editor.element(),
            None,
            &self.scheme,
        ));
        self.overlays.forget();
        self.fsm.element_changed(self.core.editor.element());
        true
    }

    /// Updates everything after an FSM call (scene, selection, overlays,
    /// tool bar, requests).
    fn after_fsm(&mut self) -> TabUpdate {
        let mut update = TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        };
        let state_before = self.scene_state;
        update.repaint |= self.sync_scene();
        if state_before != self.scene_state {
            self.core.refresh();
            self.core.run_checks_if_modified();
        }
        self.configure_tool_edits();
        update.repaint |= self.apply_highlight();
        self.update_overlays();
        update.repaint = true;
        for request in self.fsm.take_requests() {
            self.handle_request(request, &mut update);
        }
        if let Some(msg) = self.fsm.take_status_message() {
            update.status = Some(msg.text);
        }
        update
    }

    fn apply_highlight(&mut self) -> bool {
        let Some(scene) = &mut self.scene else {
            return false;
        };
        let want: HashSet<_> = self
            .fsm
            .selection()
            .iter()
            .flat_map(|i| scene.items_of(scene_object(*i)))
            .collect();
        let canvas = scene.scene_mut();
        let current: HashSet<_> = canvas.selection().collect();
        if current == want {
            return false;
        }
        canvas.clear_selection();
        for id in want {
            canvas.set_selected(id, true);
        }
        true
    }

    fn update_overlays(&mut self) {
        if let Some(scene) = &mut self.scene {
            self.overlays.update(
                scene.scene_mut(),
                self.fsm.view_state(),
                self.canvas.view(),
                OverlayColors::SCHEMATIC_LIGHT,
            );
        }
    }

    fn configure_tool_edits(&mut self) {
        let tool = self.fsm.tool();
        let data = self.fsm.tool_data();
        let (line, size, size_steps, size_min) = match tool {
            LibraryTool::AddPins => (
                Length::ZERO,
                data.pin_length.get(),
                steps::PIN_LENGTH,
                Length::ZERO,
            ),
            LibraryTool::AddNames | LibraryTool::AddValues | LibraryTool::DrawText => (
                Length::ZERO,
                data.height.get(),
                steps::TEXT_HEIGHT,
                Length::new(1),
            ),
            _ => (
                data.line_width.get(),
                Length::ZERO,
                steps::GENERIC,
                Length::ZERO,
            ),
        };
        if self.configured_tool != Some(tool) {
            self.configured_tool = Some(tool);
            self.line_width
                .configure(line, Length::ZERO, steps::GENERIC);
            self.size.configure(size, size_min, size_steps);
        } else {
            self.line_width.set_value(line);
            self.size.set_value(size);
        }
    }

    fn handle_request(&mut self, request: LibraryRequest<SymbolItem>, update: &mut TabUpdate) {
        match request {
            LibraryRequest::ShowError(msg) => update.requests.push(error_notification(msg)),
            LibraryRequest::ShowInfo { title, text } => {
                update
                    .requests
                    .push(TabRequest::Notify(crate::notifications::Notification::new(
                        ui::NotificationType::Info,
                        title,
                        text,
                    )));
            }
            LibraryRequest::Properties(item) => {
                update
                    .requests
                    .push(TabRequest::LibraryItemProperties(LibraryItemRef::Symbol(
                        item,
                    )));
            }
            LibraryRequest::ContextMenu {
                item,
                pos,
                vertices,
                segment,
            } => {
                let has_properties = !matches!(item, SymbolItem::Image(_));
                // Always false for symbols (like upstream), asked anyway.
                with_clipboard(ensure_opened);
                let paste_geometry = self.run(|f, c| f.can_paste_geometry(c));
                let (actions, entries) = context_menu_entries(
                    has_properties,
                    !vertices.is_empty(),
                    segment.is_some(),
                    false,
                    paste_geometry,
                );
                self.menu = actions;
                self.menu_item = Some(item);
                self.menu_pos = (pos, vertices, segment);
                update.requests.push(TabRequest::ContextMenu {
                    pos: self.canvas.view().world_to_screen(point_to_world(pos)),
                    entries,
                });
            }
            LibraryRequest::ImportPinsDialog => {
                update.requests.push(TabRequest::ImportPinsDialog);
            }
            LibraryRequest::CourtyardOffsetDialog => {}
        }
    }

    /// An entry of the last context menu was chosen.
    pub fn context_menu_action(&mut self, index: usize) -> TabUpdate {
        use LibraryMenuAction as A;
        let Some(action) = self.menu.get(index).copied() else {
            return TabUpdate::default();
        };
        self.menu.clear();
        let item = self.menu_item.take();
        let (pos, vertices, segment) = std::mem::take(&mut self.menu_pos);
        match action {
            A::Properties => {
                self.run(|f, c| f.edit_properties(c));
            }
            A::RemoveVertices => {
                if let Some(item) = item {
                    let vertices: std::collections::BTreeSet<usize> =
                        vertices.into_iter().collect();
                    self.run(|f, c| f.remove_vertices(c, item, &vertices));
                }
            }
            A::AddVertex => {
                if let (Some(item), Some(segment)) = (item, segment) {
                    self.run(|f, c| f.add_vertex(c, item, segment, pos));
                }
            }
            A::Cut => {
                self.run(|f, c| f.cut(c));
            }
            A::Copy => {
                self.run(|f, c| f.copy(c));
            }
            A::PasteGeometry => {
                with_clipboard(ensure_opened);
                self.run(|f, c| f.paste_geometry(c));
            }
            A::Remove => {
                self.run(|f, c| f.remove(c));
            }
            A::RotateCcw => {
                self.run(|f, c| f.rotate(c, Angle::DEG90));
            }
            A::RotateCw => {
                self.run(|f, c| f.rotate(c, -Angle::DEG90));
            }
            A::MirrorHorizontal | A::FlipHorizontal => {
                self.run(|f, c| f.mirror(c, Orientation::Horizontal));
            }
            A::MirrorVertical => {
                self.run(|f, c| f.mirror(c, Orientation::Vertical));
            }
            A::SnapToGrid => {
                self.run(|f, c| f.snap_to_grid(c));
            }
            A::Separator => {}
        }
        self.after_fsm()
    }

    /// Imports pins with the names of the "import pins" dialog.
    pub fn import_pins(
        &mut self,
        names: Vec<librepcb_core::types::CircuitIdentifier>,
    ) -> TabUpdate {
        self.run(|f, c| f.import_pins(c, &names));
        self.after_fsm()
    }

    /// Adds an image chosen with the image file dialog (the answer of
    /// [`TabRequest::ChooseImageFile`], upstream
    /// `SymbolEditorFsm::processStartAddingImage()`).
    pub fn add_image(&mut self, data: librepcb_editor::fsm::schematic::ImageData) -> TabUpdate {
        self.run(|f, c| f.add_image(c, data));
        self.after_fsm()
    }

    /// Imports a DXF file with the choices of the import dialog (the
    /// answer of [`TabRequest::ImportDxf`], upstream
    /// `SymbolEditorFsm::processStartDxfImport()`: in the select tool).
    pub fn import_dxf(
        &mut self,
        settings: librepcb_editor::fsm::board::DxfImportSettings,
    ) -> TabUpdate {
        self.run(|f, c| f.select_tool(c) && f.import_dxf(c, &settings));
        self.after_fsm()
    }

    /// Applies an object modified in a properties dialog.
    pub fn update_object(
        &mut self,
        object: librepcb_editor::library_editor::commands::SymbolObject,
    ) -> TabUpdate {
        let result = self
            .core
            .editor
            .execute(librepcb_editor::library_editor::commands::UpdateSymbolObject(object));
        let mut update = self.after_fsm();
        if let Err(e) = result {
            update.requests.push(error_notification(e.to_string()));
        }
        update
    }

    /// Aborts the current tool (e.g. before closing).
    pub fn abort_tool(&mut self) -> TabUpdate {
        for _ in 0..3 {
            if !self.core.editor.is_group_active() {
                break;
            }
            self.run(|f, c| f.abort(c));
        }
        self.after_fsm()
    }

    /// Applies a modification of the element done outside the FSM (e.g. a
    /// properties dialog).
    pub fn element_modified(&mut self) -> TabUpdate {
        self.after_fsm()
    }

    // --- UI data ---

    /// The `TabData`.
    pub fn ui_data(&self) -> ui::TabData {
        let mut data = self.core.tab_data();
        let writable = self.core.is_writable();
        let f = self.fsm.view_state().features;
        if !self.core.wizard_mode && (!self.compact_layout || self.core.page_index == 1) {
            let edit = |e: bool| feature(writable && e);
            data.features.grid = feature(writable);
            data.features.zoom = feature(true);
            data.features.select = feature(f.select);
            data.features.cut = edit(f.cut);
            data.features.copy = feature(f.copy);
            data.features.paste = edit(f.paste);
            data.features.remove = edit(f.remove);
            data.features.rotate = edit(f.rotate);
            data.features.mirror = edit(f.mirror);
            data.features.snap_to_grid = edit(f.snap_to_grid);
            data.features.edit_properties = feature(f.properties);
            data.features.import_graphics = edit(f.import_graphics);
        }
        data
    }

    /// The `SymbolTabData`.
    pub fn derived_ui_data(&self) -> ui::SymbolTabData {
        let m = &self.core.meta;
        let vs = self.fsm.view_state();
        let d = self.fsm.tool_data();
        let layers = sorted_layers(&d.available_layers);
        let tool = self.fsm.tool();
        let value = match tool {
            LibraryTool::AddPins => d.pin_name.clone(),
            _ => d.text.clone(),
        };
        let background = self
            .scene
            .as_ref()
            .map_or(Color::WHITE, SymbolScene::background);
        let is_empty = {
            let s = self.core.editor.element();
            s.pins().is_empty() && s.polygons().is_empty() && s.circles().is_empty()
        };
        ui::SymbolTabData {
            library_index: self.core_library_index(),
            path: self.core.directory_path().as_str().into(),
            wizard_mode: self.core.wizard_mode,
            page_index: self.core.page_index,
            name: m.name.as_str().into(),
            name_error: m.name_error.as_str().into(),
            description: m.description.as_str().into(),
            keywords: m.keywords.as_str().into(),
            author: m.author.as_str().into(),
            age_days: m.age_days,
            version: m.version.as_str().into(),
            version_error: m.version_error.as_str().into(),
            deprecated: m.deprecated,
            categories: m.categories_model(),
            categories_tree: m.categories_tree(),
            choose_category: m.choose_category,
            checks: self.core.checks_data(),
            background_color: color_to_ui(background).into(),
            foreground_color: slint::Color::from_rgb_u8(0, 0, 0).into(),
            overlay_color: slint::Color::from_argb_u8(0x82, 0xff, 0xff, 0xff).into(),
            overlay_text_color: slint::Color::from_rgb_u8(0, 0, 0).into(),
            grid_style: grid_style_to_ui(self.grid_style),
            grid_interval: length_to_ui(self.core.editor.element().grid_interval().get()),
            unit: unit_to_ui(self.unit),
            files_modified: false,
            interface_broken_msg: self.core.is_interface_broken(),
            element_duplicated_msg: self.core.element_duplicated && !m.deprecated,
            import_pins_msg: ui::DismissableMessageData {
                visible: is_empty && !self.import_pins_dismissed && self.core.is_writable(),
                supports_dont_show_again: false,
                action: ui::DismissableMessageAction::None,
            },
            tool: tool_to_ui(tool),
            tool_cursor: mouse_cursor(vs.cursor, self.canvas.is_panning()),
            tool_overlay_text: info_box_text(&vs.info_box).into(),
            tool_layer: ui::ComboBoxData {
                items: crate::models::vec_model(
                    layers.iter().map(|l| l.name_tr().into()).collect(),
                ),
                current_index: layers
                    .iter()
                    .position(|l| *l == d.layer)
                    .map_or(-1, |i| i as i32),
            },
            tool_line_width: self.line_width.ui_data(),
            tool_size: self.size.ui_data(),
            tool_angle: ui::AngleEditData {
                value: d.angle.to_micro_deg(),
                increase: false,
                decrease: false,
            },
            tool_filled: d.filled,
            tool_grab_area: d.grab_area,
            tool_value: ui::LineEditData {
                enabled: true,
                text: to_single_line(&value).into(),
                placeholder: Default::default(),
                suggestions: crate::models::vec_model(
                    d.text_suggestions
                        .iter()
                        .map(|s| s.as_str().into())
                        .collect(),
                ),
            },
            tool_halign: halign_to_ui(d.h_align),
            tool_valign: valign_to_ui(d.v_align),
            compact_layout: self.compact_layout,
            scene_image_pos: self.scene_image_pos,
            frame: self.frame,
            new_category: Default::default(),
        }
    }

    fn core_library_index(&self) -> i32 {
        self.library_index
    }

    /// Sets the index of the library in `Data.libraries`.
    pub fn set_library_index(&mut self, index: i32) {
        self.library_index = index;
    }

    /// Applies data written by the UI (upstream `setDerivedUiData()`).
    pub fn set_derived_ui_data(&mut self, data: &ui::SymbolTabData) -> TabUpdate {
        let mut update = TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        };
        self.core.page_index = data.page_index;
        self.compact_layout = data.compact_layout;
        self.scene_image_pos = data.scene_image_pos;
        let category_added = self.core.meta.set_from_ui(
            &data.name,
            &data.description,
            &data.keywords,
            &data.author,
            &data.version,
            data.deprecated,
            &data.new_category,
            data.choose_category,
        );
        if category_added {
            update.requests.extend(self.core.commit_metadata());
        }
        // View.
        let style = grid_style_from_ui(data.grid_style);
        if style != self.grid_style {
            update.repaint |= self.set_grid_style(style).repaint;
        }
        if let Ok(interval) = PositiveLength::new(length_from_ui(data.grid_interval.clone()))
            && interval != self.core.editor.element().grid_interval()
        {
            self.core.editor.set_grid_interval(interval);
            self.apply_grid();
            update.repaint = true;
        }
        self.unit = unit_from_ui(data.unit);
        if data.import_pins_msg.action != ui::DismissableMessageAction::None {
            self.import_pins_dismissed = true;
        }
        if data.deprecated || !data.element_duplicated_msg {
            self.core.element_duplicated = false;
        }

        // Tool bar.
        let current = self.fsm.tool_data().clone();
        let tool = self.fsm.tool();
        let layers = sorted_layers(&current.available_layers);
        let layer = usize::try_from(data.tool_layer.current_index)
            .ok()
            .and_then(|i| layers.get(i))
            .copied();
        let angle = if data.tool_angle.increase {
            current.angle + Angle::DEG45
        } else if data.tool_angle.decrease {
            current.angle + -Angle::DEG45
        } else {
            Angle::new(data.tool_angle.value)
        };
        let line_width = self.line_width.set_ui_data(&data.tool_line_width);
        let size = self.size.set_ui_data(&data.tool_size);
        let value = to_multi_line(&data.tool_value.text);
        let h_align = halign_from_ui(data.tool_halign);
        let v_align = valign_from_ui(data.tool_valign);
        let filled = data.tool_filled;
        let grab_area = data.tool_grab_area;
        let changed = {
            let mut d = current.clone();
            if let Some(layer) = layer {
                d.layer = layer;
            }
            d.angle = angle;
            if let Some(w) = line_width.and_then(|w| UnsignedLength::new(w).ok()) {
                d.line_width = w;
            }
            match tool {
                LibraryTool::AddPins => {
                    if let Some(l) = size.and_then(|s| UnsignedLength::new(s).ok()) {
                        d.pin_length = l;
                    }
                    d.pin_name = value;
                }
                _ => {
                    if let Some(h) = size.and_then(|s| PositiveLength::new(s).ok()) {
                        d.height = h;
                    }
                    d.text = value;
                }
            }
            d.h_align = h_align;
            d.v_align = v_align;
            d.filled = filled;
            d.grab_area = grab_area;
            d
        };
        if changed != current && self.core.is_writable() {
            self.run(|f, c| f.update_tool_data(c, |d| *d = changed));
        }
        let after = self.after_fsm();
        update.repaint |= after.repaint;
        update.requests.extend(after.requests);
        update
    }

    /// A category row was written by the UI (removal).
    pub fn category_row_written(
        &mut self,
        row: usize,
        data: &ui::LibraryElementCategoryData,
    ) -> TabUpdate {
        let mut update = TabUpdate::default();
        if self.core.meta.category_row_written(row, data) {
            update.requests.extend(self.core.commit_metadata());
            update.data_changed = true;
        }
        update
    }

    /// A check message row was written by the UI.
    pub fn check_row_written(&mut self, row: usize, data: &ui::RuleCheckMessageData) -> TabUpdate {
        let (request, mut update) = self.core.check_row_written(row, data);
        match request {
            Some(librepcb_editor::library_editor::checks::FixRequest::StartTool {
                tool, ..
            }) => {
                self.core.page_index = 1;
                self.run(|f, c| f.set_tool(c, tool));
            }
            Some(librepcb_editor::library_editor::checks::FixRequest::ChooseCategories) => {
                self.core.page_index = 0;
                self.core.meta.choose_category = true;
            }
            _ => {}
        }
        let after = self.after_fsm();
        update.repaint |= after.repaint;
        update.requests.extend(after.requests);
        update
    }

    // --- Actions ---

    /// Handles a tab action (upstream `SymbolTab::trigger()`).
    pub fn trigger(&mut self, action: ui::TabAction) -> TabUpdate {
        use ui::TabAction as A;
        if matches!(
            action,
            A::Apply | A::Save | A::Undo | A::Redo | A::Next | A::Unlock
        ) {
            // Abort a tool which keeps an undo group open.
            for _ in 0..3 {
                if !self.core.editor.is_group_active() {
                    break;
                }
                self.run(|f, c| f.abort(c));
            }
        }
        if let Some(mut update) = self.core.trigger_common(action) {
            self.scene_state = None;
            let after = self.after_fsm();
            update.repaint = true;
            update.requests.extend(after.requests);
            return update;
        }
        let grid = self.core.editor.element().grid_interval().get();
        let mut extra = TabUpdate::default();
        match action {
            A::ZoomFit => {
                let bounds = self.content_bounds();
                self.canvas.zoom_fit(bounds);
            }
            A::ZoomIn => self.canvas.zoom_in(),
            A::ZoomOut => self.canvas.zoom_out(),
            A::GridIntervalIncrease => {
                if let Ok(i) = PositiveLength::new(Length::new(grid.to_nm().saturating_mul(2))) {
                    self.core.editor.set_grid_interval(i);
                    self.apply_grid();
                }
            }
            A::GridIntervalDecrease => {
                if grid.to_nm() % 2 == 0
                    && let Ok(i) = PositiveLength::new(Length::new(grid.to_nm() / 2))
                {
                    self.core.editor.set_grid_interval(i);
                    self.apply_grid();
                }
            }
            A::SelectAll => {
                self.run(|f, c| f.select_all(c));
            }
            A::Abort => {
                self.run(|f, c| f.abort(c));
            }
            A::Cut => {
                self.run(|f, c| f.cut(c));
            }
            A::Copy => {
                self.run(|f, c| f.copy(c));
            }
            A::Paste => {
                with_clipboard(ensure_opened);
                self.run(|f, c| f.paste(c));
            }
            A::Delete => {
                self.run(|f, c| f.remove(c));
            }
            A::RotateCcw => {
                self.run(|f, c| f.rotate(c, Angle::DEG90));
            }
            A::RotateCw => {
                self.run(|f, c| f.rotate(c, -Angle::DEG90));
            }
            A::MirrorHorizontally => {
                self.run(|f, c| f.mirror(c, Orientation::Horizontal));
            }
            A::MirrorVertically => {
                self.run(|f, c| f.mirror(c, Orientation::Vertical));
            }
            A::MoveLeft | A::MoveRight | A::MoveUp | A::MoveDown => {
                let (dx, dy): (i64, i64) = match action {
                    A::MoveLeft => (-1, 0),
                    A::MoveRight => (1, 0),
                    A::MoveUp => (0, 1),
                    _ => (0, -1),
                };
                let delta = librepcb_core::types::Point::new(
                    Length::new(grid.to_nm() * dx),
                    Length::new(grid.to_nm() * dy),
                );
                if !self.run(|f, c| f.move_by(c, delta)) {
                    self.canvas.scroll_steps(dx as f64, -dy as f64);
                }
            }
            A::SnapToGrid => {
                self.run(|f, c| f.snap_to_grid(c));
            }
            A::EditProperties => {
                self.run(|f, c| f.edit_properties(c));
            }
            A::SymbolImportPins => {
                self.run(|f, c| f.start_adding_pins(c, true));
            }
            A::ToolSelect => {
                self.run(|f, c| f.select_tool(c));
            }
            A::ToolPin => {
                self.run(|f, c| f.start_adding_pins(c, false));
            }
            A::ToolLine
            | A::ToolRect
            | A::ToolPolygon
            | A::ToolCircle
            | A::ToolArc
            | A::ToolName
            | A::ToolValue
            | A::ToolText
            | A::ToolMeasure => {
                let tool = match action {
                    A::ToolLine => LibraryTool::DrawLine,
                    A::ToolRect => LibraryTool::DrawRect,
                    A::ToolPolygon => LibraryTool::DrawPolygon,
                    A::ToolCircle => LibraryTool::DrawCircle,
                    A::ToolArc => LibraryTool::DrawArc,
                    A::ToolName => LibraryTool::AddNames,
                    A::ToolValue => LibraryTool::AddValues,
                    A::ToolText => LibraryTool::DrawText,
                    _ => LibraryTool::Measure,
                };
                self.run(|f, c| f.set_tool(c, tool));
            }
            A::SaveAs => {
                extra.requests.extend(self.core.commit_metadata());
                extra.requests.push(TabRequest::DuplicateLibraryElement);
            }
            A::ToolImage => {
                // Upstream `processStartAddingImage()` opens the image
                // chooser; here the application does and passes the file
                // to `add_image()`.
                if self.core.is_writable() {
                    extra.requests.push(TabRequest::ChooseImageFile);
                }
            }
            A::ImportDxf => {
                // Upstream `processStartDxfImport()` opens the file chooser
                // and the import dialog; here the application does and
                // passes the choice to `import_dxf()`.
                if self.core.is_writable() {
                    extra.requests.push(TabRequest::ImportDxf {
                        layers: <SymbolHost as ElementHost>::polygon_layers(),
                        kind: DxfImportKind::Symbol,
                    });
                }
            }
            A::ExportPdf | A::ExportImage | A::Print => {
                extra.status = Some(tr!(
                    "librepcb::editor::MainWindow",
                    "Not available yet in this version: {0}",
                    format!("{action:?}")
                ));
            }
            _ => return TabUpdate::default(),
        }
        let mut update = self.after_fsm();
        update.status = extra.status.or(update.status);
        update.requests.extend(extra.requests);
        update
    }

    fn content_bounds(&mut self) -> Option<Rect> {
        let scene = self.scene.as_mut()?;
        self.overlays.clear(scene.scene_mut());
        let bounds = scene.content_bounds();
        self.update_overlays();
        bounds
    }

    // --- Rendering and events ---

    /// Renders the scene.
    pub fn render_scene(&mut self, width: f32, height: f32, scale_factor: f32) -> slint::Image {
        let Some(scene) = &self.scene else {
            return slint::Image::default();
        };
        let image = self.canvas.render(
            scene.scene(),
            scene.content_bounds(),
            width,
            height,
            scale_factor,
        );
        self.update_overlays();
        image
    }

    /// Handles a pointer event.
    pub fn pointer_event(
        &mut self,
        kind: PointerKind,
        button: PointerButton,
        pos: Point,
        modifiers: Modifiers,
    ) -> TabUpdate {
        let panning = self.canvas.is_panning();
        let action = self.canvas.pointer_event(kind, button, pos);
        let m = fsm_modifiers(modifiers);
        let event = |world: Point| PointerEvent::with_modifiers(point_from_world(world), m);
        let mut cursor = None;
        match action {
            PointerAction::ViewChanged => {
                self.update_overlays();
                return TabUpdate {
                    repaint: true,
                    data_changed: self.canvas.is_panning() != panning,
                    ..TabUpdate::default()
                };
            }
            PointerAction::LeftPressed(world) => {
                let e = event(world);
                self.cursor = Some(e.pos);
                if self.double_click.press(pos) {
                    self.run(|f, c| f.left_double_clicked(c, e));
                } else {
                    self.run(|f, c| f.left_pressed(c, e));
                }
            }
            PointerAction::LeftReleased(world) => {
                let e = event(world);
                self.cursor = Some(e.pos);
                self.run(|f, c| f.left_released(c, e));
            }
            PointerAction::Moved(world) => {
                let e = event(world);
                self.cursor = Some(e.pos);
                cursor = Some((world, self.unit));
                self.run(|f, c| f.pointer_moved(c, e));
            }
            PointerAction::ContextMenu(world) => {
                let e = event(world);
                self.cursor = Some(e.pos);
                self.run(|f, c| f.right_released(c, e));
            }
            PointerAction::None => return TabUpdate::default(),
        }
        let mut update = self.after_fsm();
        update.cursor = cursor;
        update
    }

    /// Handles a scroll event.
    pub fn scrolled(&mut self, pos: Point, delta: Vec2, modifiers: Modifiers) -> bool {
        let changed = self.canvas.scroll_event(pos, delta, modifiers);
        if changed {
            self.update_overlays();
        }
        changed
    }

    /// Handles a key event of the scene.
    pub fn key_event(
        &mut self,
        event: &slint::language::KeyEvent,
        pressed: bool,
    ) -> (bool, TabUpdate) {
        let Some(e) = fsm_key_event(event) else {
            return (false, TabUpdate::default());
        };
        let handled = self.run(|f, c| {
            if pressed {
                f.key_pressed(c, e)
            } else {
                f.key_released(c, e)
            }
        });
        (handled, self.after_fsm())
    }

    /// Bumps the frame counter.
    pub fn bump_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    /// The view state of the FSM (for tests).
    pub fn view_state(&self) -> &ViewState {
        self.fsm.view_state()
    }

    /// The items of the scene touching a screen rectangle (tests).
    pub fn scene_items_in(&self, rect: Rect) -> usize {
        self.scene.as_ref().map_or(0, |s| {
            s.scene()
                .items_in_rect(rect, SelectionMode::Intersects)
                .len()
        })
    }
}

/// A library element item of a properties request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryItemRef {
    /// A symbol item.
    Symbol(SymbolItem),
    /// A footprint item of a package (footprint UUID, item).
    Footprint(
        Option<librepcb_core::types::Uuid>,
        librepcb_editor::library_editor::commands::FootprintItem,
    ),
}

/// The mode of opening (re-export for the application).
pub type SymbolOpenMode = OpenMode;
