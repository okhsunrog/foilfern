//! The board 2D tab.
//!
//! Port of libs/librepcb/editor/project/board/board2dtab.{h,cpp}: the board
//! rendered by `librepcb-scene`'s [`BoardScene`], the layers panel
//! (visibility of the board layers), top/bottom view, navigation, grid, and
//! editing through the board editor state machine ([`BoardEditorFsm`]):
//! pointer and key events go to the FSM, tab actions (tools, clipboard,
//! rotate, flip, lock, undo, ...) call it, and after every call the scene
//! is synced with the project's change journal, the selection and the
//! highlighted nets are shown, the overlays are updated and the tool bar
//! data (`tool-*` fields of `Board2dTabData`) is refreshed.
//!
//! The standalone pad tools (with their tool bar: net, board side, shape,
//! size, drill, corner radius, press-fit), the DXF import (file chooser and
//! import dialog of the application), the "find" field, cross-probing of
//! components, nets and component signals (pads) and the plane visibility
//! of the context menu are wired to the FSM like upstream.
//!
//! Not available yet: the unplaced components panel. Grid changes are
//! undoable modifications of the board settings (upstream: without undo);
//! the plane visibility is kept in the model without an undo step (like
//! upstream, it is not saved).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use librepcb_app_ui as ui;
use librepcb_canvas::kurbo::{Point, Vec2};
use librepcb_canvas::peniko::Color;
use librepcb_canvas::{Grid, Modifiers, PointerAction, PointerButton, PointerKind};
use librepcb_core::geometry::ComponentSide;
use librepcb_core::geometry::ZoneRules;
use librepcb_core::project::{BoardId, BoardMutation, ComponentInstanceId, Mutation, NetSignalId};
use librepcb_core::types::{
    Angle, GridStyle, Layer, Length, LengthUnit, Orientation, PositiveLength, Ratio,
    UnsignedLength, UnsignedLimitedRatio, Uuid,
};
use librepcb_editor::commands::ApplyMutations;
use librepcb_editor::fsm::board::{
    BoardContext, BoardEditorFsm, BoardEditorSettings, BoardItemRef, BoardRequest, BoardTool,
    BoardToolData, ContextAction, DxfImportSettings, ToolNet, ToolPadShape, ToolSetting, WireMode,
};
use librepcb_editor::fsm::find::FindResult;
use librepcb_editor::fsm::{PointerEvent, ViewState};
use librepcb_i18n::tr;
use librepcb_scene::{BoardObject, BoardScene, BoardSceneLayer, BoardSide, ColorScheme, SceneSync};

use super::board_view::{BoardSceneView, board_item};
use super::editing::{
    DoubleClick, OverlayColors, Overlays, error_notification, fsm_key_event, fsm_modifiers,
    info_box_text, mouse_cursor, point_from_world, point_to_world, to_multi_line, to_single_line,
};
use super::{
    ContextMenuEntry, CrossProbe, PropertiesTarget, TabId, TabRequest, TabUpdate, feature,
    project_index,
};
use crate::canvas_view::{CanvasView, DEFAULT_BOARD_RECT};
use crate::clipboard::{ensure_opened, with_clipboard};
use crate::helpers::{
    color_to_ui, grid_style_from_ui, grid_style_to_ui, length_from_ui, length_to_ui, unit_from_ui,
    unit_to_ui,
};
use crate::length_edit::{LengthEdit, steps};
use crate::models::{UiModel, model_rc, vec_model};
use crate::notifications::Notification;
use crate::project::AppProject;

/// Grid color of the default board color scheme (upstream
/// `boardLibrePcbDark()`, secondary color of the background role).
const GRID_COLOR: Color = Color::from_rgb8(0xa0, 0xa0, 0xa4);

/// What the tab shows of the FSM state, to detect changes of the UI data.
#[derive(Debug, Clone, PartialEq)]
struct Snapshot {
    tool: BoardTool,
    tool_data: BoardToolData,
    view: ViewState,
    panning: bool,
    undo_index: usize,
    group_active: bool,
    clean: bool,
    scene_rev: u64,
    selection_rev: u64,
}

/// A board tab (2D view).
pub struct Board2dTab {
    id: TabId,
    project: Rc<AppProject>,
    board: BoardId,
    scheme: ColorScheme,
    side: BoardSide,
    scene: Option<BoardScene>,
    /// Journal cursor of the scene.
    sync: SceneSync,
    canvas: CanvasView,
    title: String,
    grid_style: GridStyle,
    grid_interval: PositiveLength,
    unit: LengthUnit,
    /// Board layers listed in the layers panel (same order as the model).
    layers: Vec<Layer>,
    /// Visibility per listed layer.
    visible: Vec<bool>,
    layers_model: Rc<UiModel<ui::GraphicsLayerData>>,
    frame: i32,
    scene_image_pos: slint::LogicalPosition,
    fsm: BoardEditorFsm,
    overlays: Overlays,
    double_click: DoubleClick,
    /// Last pointer position (world coordinates).
    cursor: Option<librepcb_core::types::Point>,
    line_width: LengthEdit,
    size: LengthEdit,
    drill: LengthEdit,
    /// Tool whose length edits are configured.
    configured_tool: Option<BoardTool>,
    /// Cross-probed by another tab.
    probe: CrossProbe,
    /// Actions of the entries of the last context menu (`None`:
    /// separator).
    menu: Vec<Option<ContextAction>>,
    /// The item of the last context menu.
    menu_item: Option<BoardItemRef>,
    /// Cross-probed by the last "find next/previous" (until the next click).
    find_probe: CrossProbe,
    /// Suggestions of the "find" field (updated in place while typing).
    find_suggestions: Rc<slint::VecModel<ui::SimpleListItemData>>,
}

impl Board2dTab {
    /// Creates the tab of a board.
    pub fn new(project: Rc<AppProject>, board: BoardId, grid_style: GridStyle) -> Self {
        let scheme = ColorScheme::BOARD_DARK;
        let side = BoardSide::Top;
        let (scene, revision, title, grid_interval, unit) = {
            let mut p = project.shared().lock();
            // The scene shows the air wires of the board.
            if !p.editor.undo_stack().is_group_active()
                && let Err(e) = p.editor.rebuild_air_wires(board)
            {
                log::warn!("Failed to build the air wires: {e}");
            }
            let proj = p.project();
            let scene = BoardScene::build(proj, board, side, &scheme)
                .map_err(|e| log::error!("Failed to build the board scene: {e}"))
                .ok();
            let b = proj.board(board);
            (
                scene,
                proj.revision(),
                b.map(|b| b.properties().name.to_string())
                    .unwrap_or_default(),
                b.map_or(
                    PositiveLength::new(Length::new(635_000)).expect("positive"),
                    |b| b.settings().grid_interval,
                ),
                b.map_or(LengthUnit::Millimeters, |b| b.settings().grid_unit),
            )
        };
        let background = scene.as_ref().map_or(Color::BLACK, BoardScene::background);
        let content = scene.as_ref().and_then(BoardScene::content_bounds);
        let settings = BoardEditorSettings {
            app_version: crate::app::APP_VERSION.to_owned(),
            ..BoardEditorSettings::default()
        };
        let mut tab = Self {
            id: TabId::new(),
            project,
            board,
            scheme,
            side,
            scene,
            sync: SceneSync::at(revision),
            canvas: CanvasView::new(background, DEFAULT_BOARD_RECT, content),
            title,
            grid_style,
            grid_interval,
            unit,
            layers: Vec::new(),
            visible: Vec::new(),
            layers_model: UiModel::shared(Vec::new()),
            frame: 0,
            scene_image_pos: Default::default(),
            fsm: BoardEditorFsm::new(board, settings),
            overlays: Overlays::default(),
            double_click: DoubleClick::default(),
            cursor: None,
            line_width: LengthEdit::new(steps::GENERIC),
            size: LengthEdit::new(steps::GENERIC),
            drill: LengthEdit::new(steps::DRILL_DIAMETER),
            configured_tool: None,
            probe: CrossProbe::default(),
            menu: Vec::new(),
            menu_item: None,
            find_probe: CrossProbe::default(),
            find_suggestions: Rc::new(slint::VecModel::default()),
        };
        tab.init_layers();
        tab.apply_grid();
        // Enter the select tool (its features, e.g. for the menus).
        tab.run(|f, c| f.set_tool(c, BoardTool::Select));
        tab
    }

    /// The unique tab identifier.
    pub fn id(&self) -> TabId {
        self.id
    }

    /// The project.
    pub fn project(&self) -> &Rc<AppProject> {
        &self.project
    }

    /// The board shown.
    pub fn board(&self) -> BoardId {
        self.board
    }

    /// The graphics view.
    pub fn canvas(&self) -> &CanvasView {
        &self.canvas
    }

    /// The graphics view.
    pub fn canvas_mut(&mut self) -> &mut CanvasView {
        &mut self.canvas
    }

    /// The scene (if it could be built).
    pub fn scene(&self) -> Option<&BoardScene> {
        self.scene.as_ref()
    }

    /// The editor state machine.
    pub fn fsm(&self) -> &BoardEditorFsm {
        &self.fsm
    }

    /// The layers panel model (`TabData.layers`).
    pub fn layers_model(&self) -> &Rc<UiModel<ui::GraphicsLayerData>> {
        &self.layers_model
    }

    // --- Layers ---

    /// Lists the board layers of the scene (upstream
    /// `GraphicsLayerList::boardLayers()`, only layers used by the board).
    fn init_layers(&mut self) {
        let Some(scene) = &self.scene else { return };
        let used: Vec<Layer> = scene
            .layers()
            .iter()
            .filter_map(|l| match l {
                BoardSceneLayer::Board(layer) => Some(*layer),
                _ => None,
            })
            .collect();
        self.layers = Layer::all()
            .iter()
            .copied()
            .filter(|l| used.contains(l))
            .collect();
        self.visible = self
            .layers
            .iter()
            .map(|l| {
                scene
                    .scene()
                    .is_layer_visible(BoardSceneLayer::Board(*l).id())
            })
            .collect();
        self.update_layers_model();
    }

    /// The layers hidden in the layers panel (their planes are not rebuilt
    /// automatically, upstream `getVisibleCopperLayers()`).
    pub fn hidden_layers(&self) -> impl Iterator<Item = Layer> + '_ {
        self.layers
            .iter()
            .zip(&self.visible)
            .filter(|(_, v)| !**v)
            .map(|(l, _)| *l)
    }

    /// Re-lists the layers after a rebuild (the board's layers may have
    /// changed), keeping the visibility of known layers.
    fn init_layers_keep_visibility(&mut self) {
        let old: Vec<(Layer, bool)> = self
            .layers
            .iter()
            .copied()
            .zip(self.visible.iter().copied())
            .collect();
        self.init_layers();
        for (i, layer) in self.layers.iter().enumerate() {
            if let Some((_, v)) = old.iter().find(|(l, _)| l == layer) {
                self.visible[i] = *v;
            }
        }
        self.apply_layers_visibility();
        self.update_layers_model();
    }

    fn apply_layers_visibility(&mut self) {
        if let Some(scene) = &mut self.scene {
            for (layer, visible) in self.layers.iter().zip(&self.visible) {
                scene.set_layer_visible(*layer, *visible);
            }
        }
    }

    fn update_layers_model(&self) {
        let Some(scene) = &self.scene else { return };
        let rows = self
            .layers
            .iter()
            .zip(&self.visible)
            .map(|(layer, visible)| {
                let canvas_layer = scene.scene().layer(BoardSceneLayer::Board(*layer).id());
                let (color, highlight) = canvas_layer
                    .map(|l| (l.color, l.highlight_color))
                    .unwrap_or((Color::WHITE, Color::WHITE));
                ui::GraphicsLayerData {
                    name: layer.name_tr().into(),
                    color: color_to_ui(color),
                    color_highlighted: color_to_ui(highlight),
                    visible: *visible,
                }
            })
            .collect();
        self.layers_model.replace_all(rows);
    }

    /// Shows or hides the layer of a layers panel row.
    pub fn set_layer_visible(&mut self, row: usize, visible: bool) -> TabUpdate {
        let Some(layer) = self.layers.get(row).copied() else {
            return TabUpdate::default();
        };
        if self.visible[row] == visible {
            return TabUpdate::default();
        }
        self.visible[row] = visible;
        if let Some(scene) = &mut self.scene {
            scene.set_layer_visible(layer, visible);
        }
        TabUpdate::repaint()
    }

    /// Applies a layer visibility preset (upstream `TabAction::Layers*`).
    fn apply_layer_preset(&mut self, action: ui::TabAction) {
        for (i, layer) in self.layers.iter().enumerate() {
            let inner = layer.is_copper() && !layer.is_top() && !layer.is_bottom();
            self.visible[i] = match action {
                ui::TabAction::LayersNone => false,
                ui::TabAction::LayersTop => !layer.is_bottom() && !inner,
                ui::TabAction::LayersBottom => !layer.is_top() && !inner,
                ui::TabAction::LayersTopBottom => !inner,
                _ => true,
            };
        }
        self.apply_layers_visibility();
        self.update_layers_model();
    }

    // --- Grid ---

    fn apply_grid(&mut self) {
        let interval = self.grid_interval.get().to_mm();
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

    /// Sets the grid style (a workspace setting shared by all tabs).
    pub fn set_grid_style(&mut self, style: GridStyle) -> TabUpdate {
        if style == self.grid_style {
            return TabUpdate::default();
        }
        self.grid_style = style;
        self.apply_grid();
        TabUpdate::repaint()
    }

    /// Changes the grid of the board (the board settings like upstream, but
    /// undoable).
    fn set_grid(&mut self, interval: PositiveLength, unit: LengthUnit) {
        let mut p = self.project.shared().lock();
        let Some(settings) = p.project().board(self.board).map(|b| b.settings().clone()) else {
            return;
        };
        if settings.grid_interval == interval && settings.grid_unit == unit {
            return;
        }
        let mut settings = settings;
        settings.grid_interval = interval;
        settings.grid_unit = unit;
        if let Err(e) = p.editor.execute(ApplyMutations {
            text: Some(tr!(
                "librepcb::editor::Board2dTab",
                "Change Grid Properties"
            )),
            mutations: vec![Mutation::Board(BoardMutation::SetSettings {
                board: self.board,
                settings: Box::new(settings),
            })],
        }) {
            log::error!("Failed to change the grid: {e}");
        }
    }

    /// Changes the color scheme (workspace settings) and rebuilds the scene.
    pub fn set_color_scheme(&mut self, scheme: &ColorScheme) -> TabUpdate {
        if self.scheme == *scheme {
            return TabUpdate::default();
        }
        self.scheme = scheme.clone();
        self.rebuild();
        let background = self
            .scene
            .as_ref()
            .map_or(Color::BLACK, BoardScene::background);
        self.canvas.set_background(background);
        TabUpdate {
            repaint: true,
            data_changed: true,
            ..TabUpdate::default()
        }
    }

    /// Rebuilds the scene from scratch (e.g. after the side was flipped),
    /// keeping the layer visibility.
    fn rebuild(&mut self) {
        let p = self.project.shared().lock();
        let proj = p.project();
        match BoardScene::build(proj, self.board, self.side, &self.scheme) {
            Ok(scene) => {
                self.scene = Some(scene);
                self.sync = SceneSync::new(proj);
            }
            Err(e) => log::error!("Failed to rebuild the board scene: {e}"),
        }
        drop(p);
        self.overlays.forget();
        self.apply_layers_visibility();
    }

    // --- FSM calls ---

    /// Calls the FSM with a context on the project editor, the scene and
    /// the clipboard.
    fn run<R>(
        &mut self,
        f: impl FnOnce(&mut BoardEditorFsm, &mut BoardContext<'_>) -> R,
    ) -> Option<R> {
        let scene = self.scene.as_ref()?;
        let view = BoardSceneView {
            scene,
            view: self.canvas.view(),
            cursor: self.cursor,
        };
        let fsm = &mut self.fsm;
        let mut guard = self.project.shared().lock();
        let result = with_clipboard(|clipboard| {
            let mut ctx = BoardContext::new(&mut guard.editor, &view, clipboard);
            f(fsm, &mut ctx)
        });
        Some(result)
    }

    fn snapshot(&self) -> Snapshot {
        let (undo_index, group_active, clean) = {
            let p = self.project.shared().lock();
            let undo = p.editor.undo_stack();
            (undo.index(), undo.is_group_active(), undo.is_clean())
        };
        Snapshot {
            tool: self.fsm.tool(),
            tool_data: self.fsm.tool_data().clone(),
            view: ViewState {
                status_message: None,
                ..self.fsm.view_state().clone()
            },
            panning: self.canvas.is_panning(),
            undo_index,
            group_active,
            clean,
            scene_rev: self.scene.as_ref().map_or(0, |s| s.scene().rev()),
            selection_rev: self.scene.as_ref().map_or(0, |s| s.scene().selection_rev()),
        }
    }

    /// Updates everything after an FSM call or a project modification and
    /// reports what changed compared to `before`.
    fn after_fsm(&mut self, before: &Snapshot) -> TabUpdate {
        let mut update = TabUpdate::default();
        let revision_before = self.sync.cursor();
        if self.sync_scene() {
            update.data_changed = true;
        }
        update.project_modified = self.sync.cursor() != revision_before;
        self.configure_tool_edits();
        self.apply_highlight();
        self.update_overlays();
        for request in self.fsm.take_requests() {
            self.handle_request(request, &mut update);
        }
        if let Some(msg) = self.fsm.take_status_message() {
            update.status = Some(msg.text);
        }
        let after = self.snapshot();
        update.repaint |=
            after.scene_rev != before.scene_rev || after.selection_rev != before.selection_rev;
        update.data_changed |= after != *before;
        update
    }

    /// Syncs the scene with the project's change journal; returns whether
    /// the layers changed.
    fn sync_scene(&mut self) -> bool {
        let project = Rc::clone(&self.project);
        let p = project.shared().lock();
        let proj = p.project();
        if let Some(b) = proj.board(self.board) {
            self.title = b.properties().name.to_string();
            self.unit = b.settings().grid_unit;
            if b.settings().grid_interval != self.grid_interval {
                self.grid_interval = b.settings().grid_interval;
                self.apply_grid();
            }
        }
        self.fsm.project_changed(proj);
        if !self.sync.is_outdated(proj) {
            return false;
        }
        let rebuilt = match &mut self.scene {
            Some(scene) => match self.sync.sync(proj, scene) {
                Ok(rebuilt) => rebuilt,
                Err(e) => {
                    log::warn!("Incremental board scene update failed, rebuilding: {e}");
                    true
                }
            },
            None => true,
        };
        drop(p);
        if rebuilt {
            self.rebuild();
            self.init_layers_keep_visibility();
        }
        rebuilt
    }

    /// Configures the length edits of the tool bar when a tool is entered
    /// (upstream `fsmToolEnter()`), and keeps their values up to date.
    fn configure_tool_edits(&mut self) {
        let tool = self.fsm.tool();
        let data = self.fsm.tool_data();
        let line_width = data.line_width.map_or(Length::ZERO, |w| w.get());
        // Fiducials: the size field is the copper clearance.
        let size = data
            .size
            .map(|s| s.get())
            .or(data.copper_clearance.map(|c| c.get()))
            .unwrap_or(Length::ZERO);
        let drill = data.drill.map_or(Length::ZERO, |d| d.get());
        if self.configured_tool != Some(tool) {
            self.configured_tool = Some(tool);
            let pad = matches!(tool, BoardTool::AddThtPad | BoardTool::AddSmtPad(_));
            let (line_min, size_steps) = match tool {
                BoardTool::DrawTrace => (Length::new(1), steps::GENERIC),
                BoardTool::AddStrokeText => (Length::ZERO, steps::TEXT_HEIGHT),
                _ if pad => (Length::new(1), steps::GENERIC),
                _ => (Length::ZERO, steps::GENERIC),
            };
            let size_min = if data.fiducial {
                Length::ZERO
            } else {
                Length::new(1)
            };
            self.line_width
                .configure(line_width, line_min, steps::GENERIC);
            self.size.configure(size, size_min, size_steps);
            self.drill
                .configure(drill, Length::new(1), steps::DRILL_DIAMETER);
        } else {
            self.line_width.set_value(line_width);
            self.size.set_value(size);
            self.drill.set_value(drill);
        }
    }

    /// Highlights the selection, the nets highlighted by the FSM and the
    /// items cross-probed by another tab.
    fn apply_highlight(&mut self) {
        let Some(scene) = &mut self.scene else {
            return;
        };
        let selection = self.fsm.selection().items();
        let devices: HashSet<ComponentInstanceId> = selection
            .iter()
            .filter_map(|i| match i {
                BoardItemRef::Device(c) => Some(*c),
                _ => None,
            })
            .chain(self.probe.components.iter().copied())
            .collect();
        let device_texts: HashSet<Uuid> = selection
            .iter()
            .filter_map(|i| match i {
                BoardItemRef::DeviceStrokeText(_, u) => Some(*u),
                _ => None,
            })
            .collect();
        let nets: BTreeSet<NetSignalId> = self
            .fsm
            .highlighted_nets()
            .iter()
            .chain(self.probe.nets.iter())
            .copied()
            .collect();
        let signals = &self.probe.component_signals;
        let want: HashSet<_> = {
            let p = self.project.shared().lock();
            let proj = p.project();
            let board = proj.board(self.board);
            let segment_net = |seg| board.and_then(|b| b.net_segment(seg)).and_then(|s| s.net());
            // Nets of footprint pads and the pads of cross-probed component
            // signals (only computed when needed).
            let mut pad_nets: HashMap<(ComponentInstanceId, Uuid), NetSignalId> = HashMap::new();
            let mut probed_pads: HashSet<(ComponentInstanceId, Uuid)> = HashSet::new();
            if (!nets.is_empty() || !signals.is_empty())
                && let Some(b) = board
            {
                for device in b.devices().values() {
                    if let Ok(pads) = device.pads(proj.library(), proj.circuit()) {
                        for pad in pads {
                            if let Some(net) = pad.net() {
                                pad_nets.insert((device.component(), pad.uuid()), net);
                            }
                            if pad.component_signal().is_some_and(|s| signals.contains(&s)) {
                                probed_pads.insert((device.component(), pad.uuid()));
                            }
                        }
                    }
                }
            }
            scene
                .scene()
                .items()
                .filter_map(|(id, _)| {
                    let object = scene.object(id)?;
                    let selected = board_item(object).is_some_and(|i| selection.contains(&i))
                        || match object {
                            BoardObject::Device(c) | BoardObject::FootprintPad(c, _) => {
                                devices.contains(&c)
                            }
                            BoardObject::StrokeText(u) => device_texts.contains(&u),
                            _ => false,
                        };
                    let probed_pad = match object {
                        BoardObject::FootprintPad(c, pad) => probed_pads.contains(&(c, pad)),
                        _ => false,
                    };
                    let net = !nets.is_empty()
                        && match object {
                            BoardObject::Trace(seg, _)
                            | BoardObject::Via(seg, _)
                            | BoardObject::Pad(seg, _) => {
                                segment_net(seg).is_some_and(|n| nets.contains(&n))
                            }
                            BoardObject::FootprintPad(c, pad) => {
                                pad_nets.get(&(c, pad)).is_some_and(|n| nets.contains(n))
                            }
                            _ => false,
                        };
                    (selected || net || probed_pad).then_some(id)
                })
                .collect()
        };
        let canvas = scene.scene_mut();
        let current: HashSet<_> = canvas.selection().collect();
        if current != want {
            canvas.clear_selection();
            for id in want {
                canvas.set_selected(id, true);
            }
        }
    }

    fn update_overlays(&mut self) {
        if let Some(scene) = &mut self.scene {
            self.overlays.update(
                scene.scene_mut(),
                self.fsm.view_state(),
                self.canvas.view(),
                OverlayColors::BOARD_DARK,
            );
        }
    }

    fn handle_request(&mut self, request: BoardRequest, update: &mut TabUpdate) {
        match request {
            BoardRequest::ShowError(msg) => update.requests.push(error_notification(msg)),
            BoardRequest::Message {
                title,
                text,
                warning,
            } => {
                let kind = if warning {
                    ui::NotificationType::Warning
                } else {
                    ui::NotificationType::Info
                };
                update.requests.push(TabRequest::Notify(Notification {
                    auto_popup: true,
                    ..Notification::new(kind, title, text)
                }));
            }
            BoardRequest::Properties(item) => {
                update
                    .requests
                    .push(TabRequest::Properties(PropertiesTarget::Board(
                        self.board, item,
                    )));
            }
            BoardRequest::LineWidthDialog { current } => {
                update.requests.push(TabRequest::LineWidth { current });
            }
            BoardRequest::ContextMenu { pos, items, item } => {
                self.menu_item = Some(item);
                let entries: Vec<ContextMenuEntry> = items
                    .iter()
                    .map(|i| match i.action {
                        None => ContextMenuEntry::separator(),
                        Some(_) => ContextMenuEntry {
                            text: i.text.clone(),
                            enabled: i.enabled,
                            checked: i.checked,
                            is_default: i.default,
                        },
                    })
                    .collect();
                self.menu = items.iter().map(|i| i.action).collect();
                if !entries.is_empty() {
                    update.requests.push(TabRequest::ContextMenu {
                        pos: self.canvas.view().world_to_screen(point_to_world(pos)),
                        entries,
                    });
                }
            }
            BoardRequest::DevicesChanged(_) => {}
        }
    }

    /// The answer of the "Set Width" dialog.
    pub fn set_line_width(&mut self, width: UnsignedLength) -> TabUpdate {
        let before = self.snapshot();
        self.run(|f, c| f.set_line_width(c, width));
        self.after_fsm(&before)
    }

    /// The length unit of the grid (for dialogs).
    pub fn length_unit(&self) -> LengthUnit {
        self.unit
    }

    /// An entry of the last context menu was chosen.
    pub fn context_menu_action(&mut self, index: usize) -> TabUpdate {
        let Some(Some(action)) = self.menu.get(index).copied() else {
            return TabUpdate::default();
        };
        self.menu.clear();
        let before = self.snapshot();
        if let ContextAction::PlaneVisible(visible) = action {
            // A view setting (upstream `BI_Plane::setVisible()`, not
            // saved and not undoable).
            let item = self.menu_item.take();
            if let Some(BoardItemRef::Plane(plane)) = item {
                self.set_plane_visible(plane, visible);
            }
            let mut update = self.after_fsm(&before);
            update.repaint = true;
            return update;
        }
        self.run(|f, c| f.context_menu_action(c, action));
        self.after_fsm(&before)
    }

    /// Shows or hides the fragments of a plane (context menu "Visible").
    pub fn set_plane_visible(&mut self, plane: librepcb_core::project::PlaneId, visible: bool) {
        let result = self
            .project
            .shared()
            .lock()
            .editor
            .set_plane_visible(self.board, plane, visible);
        if let Err(e) = result {
            log::error!("Failed to change the plane visibility: {e}");
        }
    }

    /// Imports a DXF file with the choices of the import dialog (the
    /// answer of [`TabRequest::ImportDxf`]).
    pub fn import_dxf(&mut self, settings: DxfImportSettings) -> TabUpdate {
        let before = self.snapshot();
        self.run(|f, c| f.import_dxf(c, settings));
        self.after_fsm(&before)
    }

    /// Sets the term of the "find" field; returns whether it changed.
    pub fn set_find_term(&mut self, term: &str) -> bool {
        if self.fsm.search().term() == term.trim() {
            return false;
        }
        self.fsm.set_find_term(term);
        self.update_find_suggestions();
        true
    }

    fn update_find_suggestions(&self) {
        use slint::Model;
        let items: Vec<ui::SimpleListItemData> = self
            .fsm
            .search()
            .suggestions()
            .iter()
            .map(|c| ui::SimpleListItemData {
                icon: Default::default(),
                text: c.name.as_str().into(),
            })
            .collect();
        let model = &self.find_suggestions;
        if model
            .iter()
            .map(|i| i.text)
            .eq(items.iter().map(|i| i.text.clone()))
        {
            return;
        }
        model.set_vec(items);
    }

    /// Zooms to the objects found by "find next/previous" (upstream
    /// `goToObjects()`) and cross-probes them.
    fn go_to_found(&mut self, result: Option<FindResult>, extra: &mut TabUpdate) {
        let Some(result) = result else { return };
        if result.zoom_rect.is_some() {
            // Zoom to the bounding box of the found items (upstream uses
            // the graphics items, the FSM only their positions).
            self.apply_highlight();
            if let Some(rect) = self
                .scene
                .as_ref()
                .and_then(|s| super::editing::selection_zoom_rect(s.scene()))
            {
                self.canvas.zoom_to(rect);
                self.update_overlays();
                extra.repaint = true;
            }
        }
        self.find_probe = CrossProbe {
            components: result.components.into_iter().collect(),
            nets: result.nets.into_iter().collect(),
            ..CrossProbe::default()
        };
    }

    /// Aborts the current tool if the project has an open undo group.
    pub fn abort_blocking_tool(&mut self) -> TabUpdate {
        let before = self.snapshot();
        for _ in 0..3 {
            if !self.group_active() {
                break;
            }
            self.run(|f, c| f.abort(c));
        }
        self.after_fsm(&before)
    }

    fn group_active(&self) -> bool {
        self.project
            .shared()
            .lock()
            .editor
            .undo_stack()
            .is_group_active()
    }

    /// What this tab cross-probes: what the FSM reports
    /// ([`BoardEditorFsm::cross_probe()`]: nets, components of devices,
    /// component signals of pads), the components of selected devices, the
    /// nets of selected traces, vias and pads, the nets the current tool
    /// highlights and the objects found by "find".
    pub fn cross_probe(&self) -> CrossProbe {
        let p = self.project.shared().lock();
        let board = p.project().board(self.board);
        let mut probe = self.fsm.cross_probe().clone();
        probe
            .nets
            .extend(self.fsm.highlighted_nets().iter().copied());
        probe
            .components
            .extend(self.find_probe.components.iter().copied());
        probe.nets.extend(self.find_probe.nets.iter().copied());
        for item in self.fsm.selection().items() {
            match *item {
                BoardItemRef::Device(c) => {
                    probe.components.insert(c);
                }
                BoardItemRef::Trace(seg, _)
                | BoardItemRef::Via(seg, _)
                | BoardItemRef::Pad(seg, _)
                | BoardItemRef::Junction(seg, _) => {
                    if let Some(net) = board.and_then(|b| b.net_segment(seg)).and_then(|s| s.net())
                    {
                        probe.nets.insert(net);
                    }
                }
                _ => {}
            }
        }
        probe
    }

    /// Highlights what another tab cross-probes.
    pub fn set_cross_probe(&mut self, probe: &CrossProbe) -> TabUpdate {
        if *probe == self.probe {
            return TabUpdate::default();
        }
        self.probe = probe.clone();
        let rev = self.scene.as_ref().map(|s| s.scene().selection_rev());
        self.apply_highlight();
        TabUpdate {
            repaint: self.scene.as_ref().map(|s| s.scene().selection_rev()) != rev,
            ..TabUpdate::default()
        }
    }

    /// Updates the scene from the project's change journal if the project
    /// changed (e.g. through the MCP server or another tab).
    pub fn rebuild_if_modified(&mut self) -> TabUpdate {
        let outdated = {
            let p = self.project.shared().lock();
            self.sync.is_outdated(p.project())
        };
        if !outdated {
            return TabUpdate::default();
        }
        let before = self.snapshot();
        {
            // Air wires are derived data (e.g. not restored by undo).
            let mut p = self.project.shared().lock();
            if !p.editor.undo_stack().is_group_active()
                && let Err(e) = p.editor.rebuild_air_wires(self.board)
            {
                log::warn!("Failed to rebuild the air wires: {e}");
            }
        }
        let mut update = self.after_fsm(&before);
        update.repaint = true;
        update.data_changed = true;
        update.project_modified = false;
        update
    }

    // --- UI data ---

    /// The `TabData`.
    pub fn ui_data(&self) -> ui::TabData {
        let p = self.project.shared().lock();
        let undo = p.editor.undo_stack();
        let writable = self.project.is_writable();
        let f = self.fsm.view_state().features;
        let edit = |enabled: bool| feature(writable && enabled);
        let features = ui::TabFeatures {
            save: feature(writable),
            undo: feature(undo.can_undo()),
            redo: feature(undo.can_redo()),
            grid: feature(writable),
            zoom: feature(true),
            background_image: ui::FeatureState::NotSupported,
            import_graphics: edit(f.import_graphics),
            export_graphics: ui::FeatureState::NotSupported,
            select: feature(f.select),
            cut: edit(f.cut),
            copy: feature(f.copy),
            paste: edit(f.paste),
            remove: edit(f.remove),
            rotate: edit(f.rotate),
            mirror: ui::FeatureState::NotSupported,
            flip: edit(f.flip),
            move_align: ui::FeatureState::NotSupported,
            snap_to_grid: edit(f.snap_to_grid),
            reset_texts: edit(f.reset_texts),
            lock: edit(f.lock),
            unlock: edit(f.unlock),
            modify_line_width: edit(f.modify_line_width),
            edit_properties: feature(f.properties),
            find: feature(true),
        };
        ui::TabData {
            r#type: ui::TabType::Board2d,
            find_term: self.fsm.search().term().into(),
            find_autocompletions: slint::ModelRc::from(self.find_suggestions.clone()),
            title: self.title.as_str().into(),
            features,
            read_only: !writable,
            unsaved_changes: p.has_unsaved_changes(),
            undo_text: undo.undo_text().unwrap_or_default().into(),
            redo_text: undo.redo_text().unwrap_or_default().into(),
            layers: model_rc(&self.layers_model),
        }
    }

    /// The items of the net combo box of the current tool (upstream
    /// `fsmToolEnter()` of the via and plane tools).
    fn tool_nets(&self) -> Vec<(ToolNet, String)> {
        let data = self.fsm.tool_data();
        let mut nets = Vec::new();
        match self.fsm.tool() {
            BoardTool::AddVia => {
                nets.push((
                    ToolNet {
                        auto: true,
                        net: None,
                    },
                    format!("[{}]", tr!("librepcb::editor::Board2dTab", "Auto")),
                ));
            }
            BoardTool::DrawPlane | BoardTool::AddThtPad => {}
            BoardTool::AddSmtPad(_) if !data.fiducial => {}
            _ => return nets,
        }
        nets.push((
            ToolNet::default(),
            format!("[{}]", tr!("librepcb::editor::Board2dTab", "None")),
        ));
        for (id, name) in &data.nets {
            nets.push((
                ToolNet {
                    auto: false,
                    net: Some(*id),
                },
                name.clone(),
            ));
        }
        nets
    }

    /// The `Board2dTabData`.
    pub fn derived_ui_data(&self, projects: &[Rc<AppProject>]) -> ui::Board2dTabData {
        let background = self
            .scene
            .as_ref()
            .map_or(Color::BLACK, BoardScene::background);
        let board_index = {
            let p = self.project.shared().lock();
            p.project().board_index(self.board).map_or(-1, |i| i as i32)
        };
        let vs = self.fsm.view_state();
        let data = self.fsm.tool_data();
        let nets = self.tool_nets();
        let layers = sorted_layers(&data.layers);
        let text_tool = self.fsm.tool() == BoardTool::AddStrokeText;
        ui::Board2dTabData {
            project_index: project_index(projects, &self.project),
            board_index,
            background_color: color_to_ui(background).into(),
            foreground_color: slint::Color::from_rgb_u8(255, 255, 255).into(),
            overlay_color: slint::Color::from_argb_u8(0x82, 0, 0, 0).into(),
            overlay_text_color: slint::Color::from_rgb_u8(0xff, 0xff, 0).into(),
            grid_style: grid_style_to_ui(self.grid_style),
            grid_interval: length_to_ui(self.grid_interval.get()),
            unit: unit_to_ui(self.unit),
            flip_view: self.side == BoardSide::Bottom,
            background_image_alpha: 1.0,
            ignore_placement_locks: self.fsm.settings().ignore_locks,
            unplaced_components: vec_model(Vec::new()),
            unplaced_components_index: -1,
            unplaced_components_devices: vec_model(Vec::new()),
            unplaced_components_devices_index: -1,
            unplaced_components_footprints: vec_model(Vec::new()),
            unplaced_components_footprints_index: -1,
            tool: tool_to_ui(self.fsm.tool()),
            tool_cursor: mouse_cursor(vs.cursor, self.canvas.is_panning()),
            tool_overlay_text: info_box_text(&vs.info_box).into(),
            tool_wire_mode: wire_mode_to_ui(data.wire_mode),
            tool_net: ui::ComboBoxData {
                items: vec_model(nets.iter().map(|(_, n)| n.as_str().into()).collect()),
                current_index: nets
                    .iter()
                    .position(|(n, _)| *n == data.net)
                    .map_or(-1, |i| i as i32),
            },
            tool_netclass_name: data.net_class_name.as_str().into(),
            tool_layer: ui::ComboBoxData {
                items: vec_model(layers.iter().map(|l| l.name_tr().into()).collect()),
                current_index: data
                    .layer
                    .and_then(|layer| layers.iter().position(|l| *l == layer))
                    .map_or(-1, |i| i as i32),
            },
            tool_line_width: self.line_width.ui_data(),
            tool_size: self.size.ui_data(),
            tool_drill: self.drill.ui_data(),
            tool_angle: ui::AngleEditData {
                value: data.angle.to_micro_deg(),
                increase: false,
                decrease: false,
            },
            tool_filled: if self.fsm.tool() == BoardTool::DrawTrace {
                data.auto_width
            } else {
                data.filled
            },
            tool_mirrored: if text_tool {
                data.mirrored
            } else {
                data.auto_size
            },
            tool_value: ui::LineEditData {
                enabled: true,
                text: to_single_line(&data.value).into(),
                placeholder: Default::default(),
                suggestions: vec_model(
                    data.value_suggestions
                        .iter()
                        .map(|s| s.as_str().into())
                        .collect(),
                ),
            },
            tool_pressfit: match data.press_fit {
                Some(press_fit) => press_fit,
                None => data.auto_drill,
            },
            tool_bottom: data.component_side == Some(ComponentSide::Bottom),
            tool_shape: match data.pad_shape {
                Some(ToolPadShape::RoundedRect) => ui::PadShape::RoundedRect,
                Some(ToolPadShape::Rect) => ui::PadShape::Rect,
                Some(ToolPadShape::Octagon) => ui::PadShape::Octagon,
                Some(ToolPadShape::Round) | None => ui::PadShape::Round,
            },
            tool_ratio: {
                let ratio = data.pad_radius.map_or(0, |r| r.get().to_ppm());
                ui::RatioEditData {
                    value: ratio,
                    minimum: 0,
                    maximum: 1_000_000,
                    can_increase: ratio < 1_000_000,
                    can_decrease: ratio > 0,
                    increase: false,
                    decrease: false,
                }
            },
            tool_fiducial: data.fiducial,
            tool_no_copper: data.zone_rules.contains(ZoneRules::NO_COPPER),
            tool_no_planes: data.zone_rules.contains(ZoneRules::NO_PLANES),
            tool_no_exposures: data.zone_rules.contains(ZoneRules::NO_EXPOSURE),
            tool_no_devices: data.zone_rules.contains(ZoneRules::NO_DEVICES),
            scene_image_pos: self.scene_image_pos,
            frame: self.frame,
            ..Default::default()
        }
    }

    /// Applies `Board2dTabData` written by the UI (tool bar edits, grid,
    /// flip view).
    pub fn set_derived_ui_data(&mut self, data: &ui::Board2dTabData) -> TabUpdate {
        let before = self.snapshot();
        let mut update = TabUpdate::default();
        self.scene_image_pos = data.scene_image_pos;
        let style = grid_style_from_ui(data.grid_style);
        if style != self.grid_style {
            update = self.set_grid_style(style);
        }
        let interval = PositiveLength::new(length_from_ui(data.grid_interval.clone()))
            .unwrap_or(self.grid_interval);
        let unit = unit_from_ui(data.unit);
        if interval != self.grid_interval || unit != self.unit {
            self.set_grid(interval, unit);
        }
        let side = if data.flip_view {
            BoardSide::Bottom
        } else {
            BoardSide::Top
        };
        if side != self.side {
            self.side = side;
            self.canvas.set_mirrored(side == BoardSide::Bottom);
            self.rebuild();
            update.repaint = true;
            update.data_changed = true;
        }
        if data.ignore_placement_locks != self.fsm.settings().ignore_locks {
            let settings = BoardEditorSettings {
                ignore_locks: data.ignore_placement_locks,
                ..self.fsm.settings().clone()
            };
            self.fsm.set_settings(settings);
            update.data_changed = true;
        }
        for setting in self.tool_settings(data) {
            self.run(|f, c| f.tool_setting(c, setting));
        }
        let mut after = self.after_fsm(&before);
        after.repaint |= update.repaint;
        after.data_changed |= update.data_changed
            || data.tool_line_width.increase
            || data.tool_line_width.decrease
            || data.tool_size.increase
            || data.tool_size.decrease
            || data.tool_drill.increase
            || data.tool_drill.decrease
            || data.tool_angle.increase
            || data.tool_angle.decrease
            || data.tool_ratio.increase
            || data.tool_ratio.decrease;
        after
    }

    /// The tool bar values changed by the UI (upstream
    /// `Board2dTab::setDerivedUiData()`, only values which differ from the
    /// FSM's).
    fn tool_settings(&mut self, data: &ui::Board2dTabData) -> Vec<ToolSetting> {
        let tool = self.fsm.tool();
        let current = self.fsm.tool_data().clone();
        let mut settings = Vec::new();
        let nets = self.tool_nets();
        if let Some((net, _)) = usize::try_from(data.tool_net.current_index)
            .ok()
            .and_then(|i| nets.get(i))
            && *net != current.net
        {
            settings.push(ToolSetting::Net(*net));
        }
        let layers = sorted_layers(&current.layers);
        if let Some(layer) = usize::try_from(data.tool_layer.current_index)
            .ok()
            .and_then(|i| layers.get(i))
            && Some(*layer) != current.layer
        {
            settings.push(ToolSetting::Layer(*layer));
        }
        let mode = wire_mode_from_ui(data.tool_wire_mode);
        if mode != current.wire_mode && tool == BoardTool::DrawTrace {
            settings.push(ToolSetting::WireMode(mode));
        }
        if let Some(width) = self.line_width.set_ui_data(&data.tool_line_width) {
            match tool {
                BoardTool::DrawTrace => {
                    if let Ok(w) = PositiveLength::new(width) {
                        settings.push(ToolSetting::TraceWidth(w));
                    }
                }
                _ => {
                    if let Ok(w) = UnsignedLength::new(width) {
                        settings.push(ToolSetting::LineWidth(w));
                    }
                }
            }
        }
        match tool {
            BoardTool::DrawTrace | BoardTool::AddVia => {
                if tool == BoardTool::DrawTrace && data.tool_filled != current.auto_width {
                    settings.push(ToolSetting::AutoWidth(data.tool_filled));
                }
                let size_changed = self.size.set_ui_data(&data.tool_size).is_some();
                if size_changed || data.tool_mirrored != current.auto_size {
                    let size = PositiveLength::new(self.size.value()).ok();
                    settings.push(ToolSetting::ViaSize(if data.tool_mirrored {
                        None
                    } else {
                        size
                    }));
                }
                let drill_changed = self.drill.set_ui_data(&data.tool_drill).is_some();
                if drill_changed || data.tool_pressfit != current.auto_drill {
                    let drill = PositiveLength::new(self.drill.value()).ok();
                    settings.push(ToolSetting::ViaDrill(if data.tool_pressfit {
                        None
                    } else {
                        drill
                    }));
                }
            }
            BoardTool::AddStrokeText => {
                if self.size.set_ui_data(&data.tool_size).is_some()
                    && let Ok(h) = PositiveLength::new(self.size.value())
                {
                    settings.push(ToolSetting::TextHeight(h));
                }
                if data.tool_mirrored != current.mirrored {
                    settings.push(ToolSetting::Mirrored(data.tool_mirrored));
                }
                let text = to_multi_line(&data.tool_value.text);
                if text != current.value {
                    settings.push(ToolSetting::Text(text));
                }
            }
            BoardTool::AddThtPad | BoardTool::AddSmtPad(_) => {
                settings.extend(self.pad_settings(data, &current));
            }
            BoardTool::AddHole => {
                if self.drill.set_ui_data(&data.tool_drill).is_some()
                    && let Ok(d) = PositiveLength::new(self.drill.value())
                {
                    settings.push(ToolSetting::HoleDiameter(d));
                }
            }
            BoardTool::DrawPolygon | BoardTool::DrawZone => {
                if tool == BoardTool::DrawPolygon && data.tool_filled != current.filled {
                    settings.push(ToolSetting::Filled(data.tool_filled));
                }
                let angle = if data.tool_angle.increase {
                    current.angle + Angle::DEG45
                } else if data.tool_angle.decrease {
                    current.angle + -Angle::DEG45
                } else {
                    Angle::new(data.tool_angle.value)
                };
                if angle != current.angle {
                    settings.push(ToolSetting::Angle(angle));
                }
                if tool == BoardTool::DrawZone {
                    let mut rules = ZoneRules::empty();
                    rules.set(ZoneRules::NO_COPPER, data.tool_no_copper);
                    rules.set(ZoneRules::NO_PLANES, data.tool_no_planes);
                    rules.set(ZoneRules::NO_EXPOSURE, data.tool_no_exposures);
                    rules.set(ZoneRules::NO_DEVICES, data.tool_no_devices);
                    if rules != current.zone_rules {
                        settings.push(ToolSetting::ZoneRules(rules));
                    }
                }
            }
            _ => {}
        }
        settings
    }

    /// The tool bar values of the pad tools changed by the UI (upstream
    /// `Board2dTab::setDerivedUiData()`: the ratio before the shape).
    fn pad_settings(
        &mut self,
        data: &ui::Board2dTabData,
        current: &BoardToolData,
    ) -> Vec<ToolSetting> {
        let mut settings = Vec::new();
        if let Some(side) = current.component_side {
            let bottom = if data.tool_bottom {
                ComponentSide::Bottom
            } else {
                ComponentSide::Top
            };
            if bottom != side {
                settings.push(ToolSetting::ComponentSide(bottom));
            }
        }
        let radius = current.pad_radius.map_or(0, |r| r.get().to_ppm());
        let step = Ratio::from_percent(1).to_ppm();
        let new_radius = if data.tool_ratio.increase {
            (radius + step).min(1_000_000)
        } else if data.tool_ratio.decrease {
            (radius - step).max(0)
        } else {
            data.tool_ratio.value
        };
        if new_radius != radius
            && let Ok(r) = UnsignedLimitedRatio::new(Ratio::new(new_radius))
        {
            settings.push(ToolSetting::PadRadius(r));
        }
        let shape = match data.tool_shape {
            ui::PadShape::Round => ToolPadShape::Round,
            ui::PadShape::RoundedRect => ToolPadShape::RoundedRect,
            ui::PadShape::Rect => ToolPadShape::Rect,
            ui::PadShape::Octagon => ToolPadShape::Octagon,
        };
        if Some(shape) != current.pad_shape {
            settings.push(ToolSetting::PadShape(shape));
        }
        if let Some(width) = self.line_width.set_ui_data(&data.tool_line_width)
            && let Ok(w) = PositiveLength::new(width)
        {
            settings.push(ToolSetting::PadWidth(w));
        }
        if self.size.set_ui_data(&data.tool_size).is_some() {
            if current.fiducial {
                if let Ok(c) = UnsignedLength::new(self.size.value()) {
                    settings.push(ToolSetting::FiducialClearance(c));
                }
            } else if let Ok(h) = PositiveLength::new(self.size.value()) {
                settings.push(ToolSetting::PadHeight(h));
            }
        }
        if current.drill.is_some()
            && self.drill.set_ui_data(&data.tool_drill).is_some()
            && let Ok(d) = PositiveLength::new(self.drill.value())
        {
            settings.push(ToolSetting::HoleDiameter(d));
        }
        if let Some(press_fit) = current.press_fit
            && data.tool_pressfit != press_fit
        {
            settings.push(ToolSetting::PressFit(data.tool_pressfit));
        }
        settings
    }

    // --- Actions ---

    /// Handles a tab action (upstream `Board2dTab::trigger()`).
    pub fn trigger(&mut self, action: ui::TabAction) -> TabUpdate {
        use ui::TabAction as A;
        let before = self.snapshot();
        let mut extra = TabUpdate::default();
        let grid = self.grid_interval.get();
        let tool =
            |t: BoardTool| move |f: &mut BoardEditorFsm, c: &mut BoardContext<'_>| f.set_tool(c, t);
        let setting = |s: ToolSetting| {
            move |f: &mut BoardEditorFsm, c: &mut BoardContext<'_>| f.tool_setting(c, s)
        };
        match action {
            A::ZoomFit => {
                let content = self.content_bounds();
                self.canvas.zoom_fit(content);
                extra.repaint = true;
            }
            A::ZoomIn => {
                self.canvas.zoom_in();
                extra.repaint = true;
            }
            A::ZoomOut => {
                self.canvas.zoom_out();
                extra.repaint = true;
            }
            A::PlanesRebuild => {
                extra.requests.push(TabRequest::RebuildPlanes(self.board));
            }
            A::LayersNone | A::LayersTop | A::LayersBottom | A::LayersTopBottom | A::LayersAll => {
                self.apply_layer_preset(action);
                extra.repaint = true;
            }
            A::GridIntervalIncrease => {
                if let Ok(i) = PositiveLength::new(Length::new(grid.to_nm().saturating_mul(2))) {
                    self.set_grid(i, self.unit);
                }
            }
            A::GridIntervalDecrease => {
                if grid.to_nm() % 2 == 0
                    && let Ok(i) = PositiveLength::new(Length::new(grid.to_nm() / 2))
                {
                    self.set_grid(i, self.unit);
                }
            }
            A::Abort => {
                self.run(|f, c| f.abort(c));
            }
            A::SelectAll => {
                self.run(|f, c| f.select_all(c));
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
            A::FlipHorizontally => {
                self.run(|f, c| f.flip(c, Orientation::Horizontal));
            }
            A::FlipVertically => {
                self.run(|f, c| f.flip(c, Orientation::Vertical));
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
                if !self.run(|f, c| f.move_by(c, delta)).unwrap_or(false) {
                    self.canvas.scroll_steps(dx as f64, -dy as f64);
                    extra.repaint = true;
                }
            }
            A::SnapToGrid => {
                self.run(|f, c| f.snap_to_grid(c));
            }
            A::Lock | A::Unlock => {
                let locked = action == A::Lock;
                self.run(|f, c| f.set_locked(c, locked));
            }
            A::LineWidthIncrease => {
                self.run(|f, c| f.change_line_width(c, 1));
            }
            A::LineWidthDecrease => {
                self.run(|f, c| f.change_line_width(c, -1));
            }
            A::LineWidthSet => {
                self.run(|f, c| f.change_line_width(c, 0));
            }
            A::ResetTexts => {
                self.run(|f, c| f.reset_all_texts(c));
            }
            A::EditProperties => {
                self.run(|f, c| f.edit_properties(c));
            }
            A::BoardAddPlane => {
                self.run(|f, c| f.auto_add_plane(c));
            }
            A::Undo | A::Redo => {
                self.abort_blocking_tool();
                let result = {
                    let mut p = self.project.shared().lock();
                    if action == A::Undo {
                        p.editor.undo()
                    } else {
                        p.editor.redo()
                    }
                };
                if let Err(e) = result {
                    extra.requests.push(error_notification(e.to_string()));
                }
                // Air wires are derived data (not undone).
                let mut p = self.project.shared().lock();
                if let Err(e) = p.editor.rebuild_air_wires(self.board) {
                    log::warn!("Failed to rebuild the air wires: {e}");
                }
            }
            A::Save => {
                self.abort_blocking_tool();
                let result = self.project.save();
                match result {
                    Ok(()) => {
                        extra.status =
                            Some(tr!("librepcb::editor::ProjectEditor", "Project saved!"))
                    }
                    Err(e) => extra.requests.push(error_notification(e.to_string())),
                }
                extra.project_modified = true;
                extra.data_changed = true;
            }
            A::ToolSelect => {
                self.run(tool(BoardTool::Select));
            }
            A::ToolWire => {
                self.run(tool(BoardTool::DrawTrace));
            }
            A::ToolVia => {
                self.run(tool(BoardTool::AddVia));
            }
            A::ToolPolygon => {
                self.run(tool(BoardTool::DrawPolygon));
            }
            A::ToolText => {
                self.run(tool(BoardTool::AddStrokeText));
            }
            A::ToolPlane => {
                self.run(tool(BoardTool::DrawPlane));
            }
            A::ToolZone => {
                self.run(tool(BoardTool::DrawZone));
            }
            A::ToolHole => {
                self.run(tool(BoardTool::AddHole));
            }
            A::ToolMeasure => {
                self.run(tool(BoardTool::Measure));
            }
            A::ToolbarTraceWidthSaveInBoard => {
                self.run(setting(ToolSetting::SaveTraceWidthInBoard));
            }
            A::ToolbarTraceWidthSaveInNetclass => {
                self.run(setting(ToolSetting::SaveTraceWidthInNetClass));
            }
            A::ToolbarViaDrillSaveInBoard => {
                self.run(setting(ToolSetting::SaveViaDrillInBoard));
            }
            A::ToolbarViaDrillSaveInNetclass => {
                self.run(setting(ToolSetting::SaveViaDrillInNetClass));
            }
            A::ToolPadTht => {
                self.run(tool(BoardTool::AddThtPad));
            }
            A::ToolPadSmt
            | A::ToolPadThermal
            | A::ToolPadBga
            | A::ToolPadEdgeConnector
            | A::ToolPadTestPoint
            | A::ToolPadLocalFiducial
            | A::ToolPadGlobalFiducial => {
                use librepcb_core::geometry::PadFunction as F;
                let function = match action {
                    A::ToolPadThermal => F::ThermalPad,
                    A::ToolPadBga => F::BgaPad,
                    A::ToolPadEdgeConnector => F::EdgeConnectorPad,
                    A::ToolPadTestPoint => F::TestPad,
                    A::ToolPadLocalFiducial => F::LocalFiducial,
                    A::ToolPadGlobalFiducial => F::GlobalFiducial,
                    _ => F::StandardPad,
                };
                self.run(tool(BoardTool::AddSmtPad(function)));
            }
            A::ImportDxf => {
                // Upstream `processImportDxf()` opens the file chooser and
                // the import dialog from the select state; here the
                // application does and passes the choice to `import_dxf()`.
                if self.fsm.view_state().features.import_graphics {
                    extra.requests.push(TabRequest::ImportDxf {
                        layers: self.geometry_layers(),
                        kind: super::DxfImportKind::Board,
                    });
                }
            }
            A::FindRefreshSuggestions => {
                self.run(|f, c| f.refresh_find_suggestions(c));
                self.update_find_suggestions();
            }
            A::FindNext => {
                let result = self.run(|f, c| f.find_next(c));
                self.go_to_found(result, &mut extra);
            }
            A::FindPrevious => {
                let result = self.run(|f, c| f.find_previous(c));
                self.go_to_found(result, &mut extra);
            }
            _ => return TabUpdate::default(),
        }
        let mut update = self.after_fsm(&before);
        update.repaint |= extra.repaint;
        update.data_changed |= extra.data_changed;
        update.project_modified |= extra.project_modified;
        if extra.status.is_some() {
            update.status = extra.status;
        }
        update.requests.extend(extra.requests);
        update
    }

    /// The layers of imported DXF polygons (upstream
    /// `getAllowedGeometryLayers()`), sorted.
    fn geometry_layers(&self) -> Vec<Layer> {
        let p = self.project.shared().lock();
        let copper = p
            .project()
            .board(self.board)
            .map(|b| b.copper_layers())
            .unwrap_or_default();
        crate::dialogs::board::board_geometry_layers(&copper)
    }

    fn content_bounds(&mut self) -> Option<librepcb_canvas::kurbo::Rect> {
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

    /// Handles a pointer event (upstream `SlintGraphicsView::pointerEvent()`
    /// and the `graphicsScene*()` handlers).
    pub fn pointer_event(
        &mut self,
        kind: PointerKind,
        button: PointerButton,
        pos: Point,
        modifiers: Modifiers,
    ) -> TabUpdate {
        let before = self.snapshot();
        let action = self.canvas.pointer_event(kind, button, pos);
        let m = fsm_modifiers(modifiers);
        let event = |world: Point| PointerEvent::with_modifiers(point_from_world(world), m);
        let mut cursor = None;
        match action {
            PointerAction::ViewChanged => {
                self.update_overlays();
                return TabUpdate {
                    repaint: true,
                    data_changed: self.canvas.is_panning() != before.panning,
                    ..TabUpdate::default()
                };
            }
            PointerAction::LeftPressed(world) => {
                let e = event(world);
                self.find_probe = CrossProbe::default();
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
            PointerAction::None => {}
        }
        let mut update = self.after_fsm(&before);
        update.cursor = cursor;
        update
    }

    /// Handles a scroll event (zoom and scroll, like upstream).
    pub fn scrolled(&mut self, pos: Point, delta: Vec2, modifiers: Modifiers) -> bool {
        let changed = self.canvas.scroll_event(pos, delta, modifiers);
        if changed {
            self.update_overlays();
        }
        changed
    }

    /// Handles a key press or release in the scene; returns whether the FSM
    /// handled it.
    pub fn key_event(
        &mut self,
        event: &slint::language::KeyEvent,
        pressed: bool,
    ) -> (bool, TabUpdate) {
        let Some(e) = fsm_key_event(event) else {
            return (false, TabUpdate::default());
        };
        let before = self.snapshot();
        let handled = self
            .run(|f, c| {
                if pressed {
                    f.key_pressed(c, e)
                } else {
                    f.key_released(c, e)
                }
            })
            .unwrap_or(false);
        (handled, self.after_fsm(&before))
    }

    /// Bumps the frame counter (the UI then requests a new image).
    pub fn bump_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }
}

/// Layers of the layer combo box, sorted like upstream `Layer::sorted()`.
fn sorted_layers(layers: &[Layer]) -> Vec<Layer> {
    Layer::all()
        .iter()
        .copied()
        .filter(|l| layers.contains(l))
        .collect()
}

fn tool_to_ui(tool: BoardTool) -> ui::EditorTool {
    match tool {
        BoardTool::Select | BoardTool::AddDevice => ui::EditorTool::Select,
        BoardTool::DrawTrace => ui::EditorTool::Wire,
        BoardTool::AddVia => ui::EditorTool::Via,
        BoardTool::DrawPolygon => ui::EditorTool::Polygon,
        BoardTool::DrawPlane => ui::EditorTool::Plane,
        BoardTool::DrawZone => ui::EditorTool::Zone,
        BoardTool::AddHole => ui::EditorTool::Hole,
        BoardTool::AddThtPad => ui::EditorTool::PadTht,
        BoardTool::AddSmtPad(_) => ui::EditorTool::PadSmt,
        BoardTool::AddStrokeText => ui::EditorTool::Text,
        BoardTool::Measure => ui::EditorTool::Measure,
    }
}

fn wire_mode_to_ui(mode: WireMode) -> ui::WireMode {
    match mode {
        WireMode::HV => ui::WireMode::HV,
        WireMode::VH => ui::WireMode::VH,
        WireMode::Deg9045 => ui::WireMode::Deg9045,
        WireMode::Deg4590 => ui::WireMode::Deg4590,
        WireMode::Straight => ui::WireMode::Straight,
    }
}

fn wire_mode_from_ui(mode: ui::WireMode) -> WireMode {
    match mode {
        ui::WireMode::HV => WireMode::HV,
        ui::WireMode::VH => WireMode::VH,
        ui::WireMode::Deg9045 => WireMode::Deg9045,
        ui::WireMode::Deg4590 => WireMode::Deg4590,
        ui::WireMode::Straight => WireMode::Straight,
    }
}
