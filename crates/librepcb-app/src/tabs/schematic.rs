//! The schematic tab.
//!
//! Port of libs/librepcb/editor/project/schematic/schematictab.{h,cpp}: the
//! schematic page rendered by `librepcb-scene`'s [`SchematicScene`],
//! navigation, grid, pin number visibility, and editing through the
//! schematic editor state machine ([`SchematicEditorFsm`]): pointer and key
//! events go to the FSM, tab actions (tools, clipboard, rotate, mirror,
//! undo, ...) call it, and after every call the scene is synced with the
//! project's change journal, the selection is highlighted, the overlays
//! (selection rectangle, ruler, scene cursor, gray-out) are updated and the
//! tool bar data (`tool-*` fields of `SchematicTabData`) is refreshed.
//!
//! Buses (tool, bus chooser of the tool button, bus member menu), images
//! (file chooser requested from the application) and the "find" field
//! (suggestions, find next/previous, zoom to the found objects) are wired
//! to the FSM like upstream.
//!
//! Differences to upstream: the tool bar's attribute value and unit of the
//! add component tool are not available yet; grid interval and unit
//! changes are undoable modifications of the schematic (upstream: without
//! undo).

use std::collections::{BTreeSet, HashSet};
use std::rc::Rc;
use std::str::FromStr;

use librepcb_app_ui as ui;
use librepcb_canvas::kurbo::{Point, Vec2};
use librepcb_canvas::peniko::Color;
use librepcb_canvas::{Grid, Modifiers, PointerAction, PointerButton, PointerKind};
use librepcb_core::project::{BusId, ComponentInstanceId, Mutation, SchematicId};
use librepcb_core::types::{
    Angle, GridStyle, Layer, Length, LengthUnit, Orientation, PositiveLength, UnsignedLength, Uuid,
};
use librepcb_editor::commands::{ApplyMutations, WireMode};
use librepcb_editor::fsm::find::FindResult;
use librepcb_editor::fsm::schematic::{
    BusMemberChoice, ComponentChoice, ImageData, SchematicContext, SchematicEditorFsm,
    SchematicEditorSettings, SchematicItem, SchematicRequest, SchematicTool, SchematicToolData,
};
use librepcb_editor::fsm::{PointerEvent, ViewState};
use librepcb_i18n::tr;
use librepcb_scene::{ColorScheme, SceneSync, SchematicObject, SchematicScene};

use super::editing::{
    DoubleClick, OverlayColors, Overlays, error_notification, fsm_key_event, fsm_modifiers,
    info_box_text, mouse_cursor, point_from_world, point_to_world, to_multi_line, to_single_line,
};
use super::schematic_view::{SchematicSceneView, schematic_item};
use super::{
    ContextMenuEntry, CrossProbe, PropertiesTarget, TabId, TabRequest, TabUpdate, feature,
    project_index,
};
use crate::canvas_view::{CanvasView, DEFAULT_SCHEMATIC_RECT};
use crate::clipboard::{ensure_opened, with_clipboard};
use crate::helpers::{
    color_to_ui, grid_style_from_ui, grid_style_to_ui, length_from_ui, length_to_ui, unit_from_ui,
    unit_to_ui,
};
use crate::length_edit::{LengthEdit, steps};
use crate::notifications::Notification;
use crate::project::AppProject;

/// Grid color of the default schematic color scheme (upstream
/// `schematicLibrePcbLight()`, secondary color of the background role).
const GRID_COLOR: Color = Color::from_rgb8(0xa0, 0xa0, 0xa4);

/// Components of the tool bar buttons (upstream `SchematicTab::trigger()`):
/// component UUID and symbol variant UUIDs for IEC 60617 and IEEE 315.
const RESISTOR: (&str, &str, &str) = (
    "ef80cd5e-2689-47ee-8888-31d04fc99174",
    "a5995314-f535-45d4-8bd8-2d0b8a0dc42a",
    "d16e1f44-16af-4773-a310-de370f744548",
);
const INDUCTOR: (&str, &str, &str) = (
    "506bd124-6062-400e-9078-b38bd7e1aaee",
    "62a7598c-17fe-41cf-8fa1-4ed274c3adc2",
    "4245d515-6f6d-48cb-9958-a4ea23d0187f",
);
const CAPACITOR_BIPOLAR: (&str, &str, &str) = (
    "d167e0e3-6a92-4b76-b013-77b9c230e5f1",
    "8cd7b37f-e5fa-4af5-a8dd-d78830bba3af",
    "6e639ff1-4e81-423b-9d0e-b28b35693a61",
);
const CAPACITOR_UNIPOLAR: (&str, &str, &str) = (
    "c54375c5-7149-4ded-95c5-7462f7301ee7",
    "5412add2-af9c-44b8-876d-a0fb7c201897",
    "20a01a81-506e-4fee-9dc0-8b50e6537cd4",
);
const GND: (&str, &str, &str) = (
    "8076f6be-bfab-4fc1-9772-5d54465dd7e1",
    "f09ad258-595b-4ee9-a1fc-910804a203ae",
    "f09ad258-595b-4ee9-a1fc-910804a203ae",
);
const VCC: (&str, &str, &str) = (
    "58c3c6cd-11eb-4557-aa3f-d3e05874afde",
    "afb86b45-68ec-47b6-8d96-153d73567228",
    "afb86b45-68ec-47b6-8d96-153d73567228",
);

/// An action of a context menu entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuAction {
    Properties,
    Cut,
    Copy,
    Remove,
    RotateCcw,
    RotateCw,
    MirrorHorizontal,
    MirrorVertical,
    SnapToGrid,
    ResetTexts,
    PlaceRemainingGates(ComponentInstanceId),
    /// An entry of the bus member menu (`None`: cancel).
    BusMember(Option<BusMemberChoice>),
    Separator,
}

/// What the tab shows of the FSM state, to detect changes of the UI data.
#[derive(Debug, Clone, PartialEq)]
struct Snapshot {
    tool: SchematicTool,
    tool_data: SchematicToolData,
    view: ViewState,
    panning: bool,
    undo_index: usize,
    group_active: bool,
    clean: bool,
    scene_rev: u64,
    selection_rev: u64,
}

/// A schematic page tab.
pub struct SchematicTab {
    id: TabId,
    project: Rc<AppProject>,
    schematic: SchematicId,
    scheme: ColorScheme,
    scene: Option<SchematicScene>,
    /// Journal cursor of the scene.
    sync: SceneSync,
    canvas: CanvasView,
    title: String,
    grid_style: GridStyle,
    grid_interval: PositiveLength,
    unit: LengthUnit,
    show_pin_numbers: bool,
    frame: i32,
    scene_image_pos: slint::LogicalPosition,
    fsm: SchematicEditorFsm,
    overlays: Overlays,
    double_click: DoubleClick,
    /// Last pointer position (world coordinates).
    cursor: Option<librepcb_core::types::Point>,
    line_width: LengthEdit,
    size: LengthEdit,
    /// Tool whose length edits are configured.
    configured_tool: Option<SchematicTool>,
    /// Cross-probed by another tab.
    probe: CrossProbe,
    /// Cross-probed by the last "find next/previous" (until the next click).
    find_probe: CrossProbe,
    /// Actions of the entries of the last context menu.
    menu: Vec<MenuAction>,
    /// Suggestions of the "find" field (upstream `SearchContext::getModel()`,
    /// updated in place while typing).
    find_suggestions: Rc<slint::VecModel<ui::SimpleListItemData>>,
}

impl SchematicTab {
    /// Creates the tab of a schematic page.
    pub fn new(project: Rc<AppProject>, schematic: SchematicId, grid_style: GridStyle) -> Self {
        let scheme = ColorScheme::SCHEMATIC_LIGHT;
        let (scene, revision, title, grid_interval, unit) = {
            let p = project.shared().lock();
            let proj = p.project();
            let scene = SchematicScene::build(proj, schematic, &scheme)
                .map_err(|e| log::error!("Failed to build the schematic scene: {e}"))
                .ok();
            let props = proj.schematic(schematic).map(|s| s.properties());
            (
                scene,
                proj.revision(),
                props
                    .as_ref()
                    .map(|p| p.name.to_string())
                    .unwrap_or_default(),
                props.as_ref().map_or(
                    PositiveLength::new(Length::new(2_540_000)).expect("positive"),
                    |p| p.grid_interval,
                ),
                props.map_or(LengthUnit::Millimeters, |p| p.grid_unit),
            )
        };
        let background = scene
            .as_ref()
            .map_or(Color::WHITE, SchematicScene::background);
        let content = scene.as_ref().and_then(SchematicScene::content_bounds);
        let settings = SchematicEditorSettings {
            app_version: crate::app::APP_VERSION.to_owned(),
            ..SchematicEditorSettings::default()
        };
        let mut tab = Self {
            id: TabId::new(),
            project,
            schematic,
            scheme,
            scene,
            sync: SceneSync::at(revision),
            canvas: CanvasView::new(background, DEFAULT_SCHEMATIC_RECT, content),
            title,
            grid_style,
            grid_interval,
            unit,
            show_pin_numbers: true,
            frame: 0,
            scene_image_pos: Default::default(),
            fsm: SchematicEditorFsm::new(schematic, settings),
            overlays: Overlays::default(),
            double_click: DoubleClick::default(),
            cursor: None,
            line_width: LengthEdit::new(steps::GENERIC),
            size: LengthEdit::new(steps::TEXT_HEIGHT),
            configured_tool: None,
            probe: CrossProbe::default(),
            find_probe: CrossProbe::default(),
            menu: Vec::new(),
            find_suggestions: Rc::new(slint::VecModel::default()),
        };
        tab.apply_grid();
        // Enter the select tool (its features, e.g. for the menus).
        tab.run(|f, c| f.select_tool(c));
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

    /// The length unit of the grid (for dialogs).
    pub fn length_unit(&self) -> LengthUnit {
        self.unit
    }

    /// The schematic.
    pub fn schematic(&self) -> SchematicId {
        self.schematic
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
    pub fn scene(&self) -> Option<&SchematicScene> {
        self.scene.as_ref()
    }

    /// The editor state machine.
    pub fn fsm(&self) -> &SchematicEditorFsm {
        &self.fsm
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

    fn apply_pin_numbers(&mut self) {
        let visible = self.show_pin_numbers;
        if let Some(scene) = &mut self.scene
            && let Some(layer) = SchematicScene::layer_id("schematic_pin_numbers")
        {
            scene.scene_mut().set_layer_visible(layer, visible);
        }
    }

    // --- FSM calls ---

    /// Calls the FSM with a context on the project editor, the scene and
    /// the clipboard.
    fn run<R>(
        &mut self,
        f: impl FnOnce(&mut SchematicEditorFsm, &mut SchematicContext<'_>) -> R,
    ) -> Option<R> {
        let scene = self.scene.as_ref()?;
        let view = SchematicSceneView {
            scene,
            view: self.canvas.view(),
            cursor: self.cursor,
        };
        let fsm = &mut self.fsm;
        let mut guard = self.project.shared().lock();
        let result = with_clipboard(|clipboard| {
            let mut ctx = SchematicContext::new(&mut guard.editor, &view, clipboard);
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
        self.sync_scene();
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
        update.repaint = after.scene_rev != before.scene_rev
            || after.selection_rev != before.selection_rev
            || update.repaint;
        update.data_changed = after != *before || update.data_changed;
        update
    }

    /// Changes the color scheme (workspace settings) and rebuilds the scene.
    pub fn set_color_scheme(&mut self, scheme: &ColorScheme) -> TabUpdate {
        if self.scheme == *scheme {
            return TabUpdate::default();
        }
        self.scheme = scheme.clone();
        self.scene = None;
        // Outdated: the scene is built again by the next sync.
        self.sync = SceneSync::at(u64::MAX);
        self.sync_scene();
        let background = self
            .scene
            .as_ref()
            .map_or(Color::WHITE, SchematicScene::background);
        self.canvas.set_background(background);
        TabUpdate {
            repaint: true,
            data_changed: true,
            ..TabUpdate::default()
        }
    }

    /// Syncs the scene with the project's change journal (incrementally
    /// where possible).
    fn sync_scene(&mut self) {
        let project = Rc::clone(&self.project);
        let p = project.shared().lock();
        let proj = p.project();
        if let Some(s) = proj.schematic(self.schematic) {
            let props = s.properties();
            self.title = props.name.to_string();
            self.unit = props.grid_unit;
            if props.grid_interval != self.grid_interval {
                self.grid_interval = props.grid_interval;
                self.apply_grid();
            }
        }
        self.fsm.project_changed(proj);
        if !self.sync.is_outdated(proj) {
            return;
        }
        let rebuilt = match &mut self.scene {
            Some(scene) => match self.sync.sync(proj, scene) {
                Ok(rebuilt) => rebuilt,
                Err(e) => {
                    log::error!("Failed to update the schematic scene: {e}");
                    self.sync = SceneSync::new(proj);
                    true
                }
            },
            None => match SchematicScene::build(proj, self.schematic, &self.scheme) {
                Ok(scene) => {
                    self.scene = Some(scene);
                    self.sync = SceneSync::new(proj);
                    true
                }
                Err(e) => {
                    log::error!("Failed to build the schematic scene: {e}");
                    false
                }
            },
        };
        drop(p);
        if rebuilt {
            self.overlays.forget();
            self.apply_pin_numbers();
        }
    }

    /// Configures the length edits of the tool bar when a tool is entered
    /// (upstream `fsmToolEnter()`), and keeps their values up to date.
    fn configure_tool_edits(&mut self) {
        let tool = self.fsm.tool();
        let data = self.fsm.tool_data();
        if self.configured_tool != Some(tool) {
            self.configured_tool = Some(tool);
            self.line_width
                .configure(data.line_width.get(), Length::ZERO, steps::GENERIC);
            self.size
                .configure(data.size.get(), Length::new(1), steps::TEXT_HEIGHT);
        } else {
            self.line_width.set_value(data.line_width.get());
            self.size.set_value(data.size.get());
        }
    }

    /// Highlights the selected items and the items cross-probed by another
    /// tab.
    fn apply_highlight(&mut self) {
        let Some(scene) = &mut self.scene else {
            return;
        };
        let selection = self.fsm.selection();
        let symbols: HashSet<_> = selection
            .iter()
            .filter_map(|i| match i {
                SchematicItem::Symbol(s) => Some(*s),
                _ => None,
            })
            .collect();
        let want: HashSet<_> = {
            let p = self.project.shared().lock();
            let proj = p.project();
            let sch = proj.schematic(self.schematic);
            let net_of = |seg| {
                sch.and_then(|s| s.net_segments().get(&seg))
                    .map(|s| s.net())
            };
            let component_of = |sym| {
                sch.and_then(|s| s.symbols().get(&sym))
                    .map(|s| s.component())
            };
            let bus_of = |seg| {
                sch.and_then(|s| s.bus_segments().get(&seg))
                    .map(|s| s.bus())
            };
            // Pins of probed component signals (only computed when needed).
            let mut probed_pins: HashSet<(librepcb_core::project::SymbolId, Uuid)> = HashSet::new();
            if !self.probe.component_signals.is_empty()
                && let Some(s) = sch
            {
                for (id, symbol) in s.symbols() {
                    for pin in symbol.pins(proj.view()).unwrap_or_default() {
                        if self.probe.component_signals.contains(&pin.signal()) {
                            probed_pins.insert((*id, pin.uuid()));
                        }
                    }
                }
            }
            scene
                .scene()
                .items()
                .filter_map(|(id, _)| {
                    let object = scene.object(id)?;
                    let item = schematic_item(object);
                    let selected = item.is_some_and(|i| selection.contains(&i))
                        || match item {
                            Some(
                                SchematicItem::SymbolPin(sym, _)
                                | SchematicItem::SymbolText(sym, _),
                            ) => symbols.contains(&sym),
                            _ => false,
                        };
                    let probed = !self.probe.is_empty()
                        && match object {
                            SchematicObject::Symbol(sym) => component_of(sym)
                                .is_some_and(|c| self.probe.components.contains(&c)),
                            SchematicObject::SymbolPin(sym, pin) => {
                                probed_pins.contains(&(sym, pin))
                            }
                            SchematicObject::NetLine(seg, _)
                            | SchematicObject::NetJunction(seg, _)
                            | SchematicObject::NetLabel(seg, _) => {
                                net_of(seg).is_some_and(|n| self.probe.nets.contains(&n))
                            }
                            SchematicObject::BusLine(seg, _)
                            | SchematicObject::BusJunction(seg, _)
                            | SchematicObject::BusLabel(seg, _) => {
                                bus_of(seg).is_some_and(|b| self.probe.buses.contains(&b))
                            }
                            _ => false,
                        };
                    (selected || probed).then_some(id)
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
                OverlayColors::SCHEMATIC_LIGHT,
            );
        }
    }

    fn handle_request(&mut self, request: SchematicRequest, update: &mut TabUpdate) {
        match request {
            SchematicRequest::ShowError(msg) => {
                update.requests.push(TabRequest::Notify(Notification {
                    auto_popup: true,
                    ..Notification::new(
                        ui::NotificationType::Critical,
                        tr!("librepcb::editor::SchematicTab", "Error"),
                        msg,
                    )
                }));
            }
            SchematicRequest::AddComponentDialog { search_term } => {
                update
                    .requests
                    .push(TabRequest::AddComponent { search_term });
            }
            SchematicRequest::SymbolProperties(symbol) => {
                update
                    .requests
                    .push(TabRequest::Properties(PropertiesTarget::Symbol(symbol)));
            }
            SchematicRequest::NetLabelProperties(segment, _) => {
                update
                    .requests
                    .push(TabRequest::Properties(PropertiesTarget::NetSegment(
                        self.schematic,
                        segment.0,
                    )));
            }
            SchematicRequest::BusLabelProperties(segment, _) => {
                update
                    .requests
                    .push(TabRequest::Properties(PropertiesTarget::BusSegment(
                        self.schematic,
                        segment.0,
                    )));
            }
            SchematicRequest::SymbolTextProperties(symbol, uuid) => {
                update
                    .requests
                    .push(TabRequest::Properties(PropertiesTarget::SymbolText(
                        symbol, uuid,
                    )));
            }
            SchematicRequest::ChooseImageFile => {
                update.requests.push(TabRequest::ChooseImageFile);
            }
            SchematicRequest::BusMemberMenu { pos, nets } => {
                // Upstream `determineNetForBusMember()`: "Add New Bus
                // Member" (default), the nets of the bus, "Cancel".
                let mut entries = vec![ContextMenuEntry {
                    is_default: true,
                    ..ContextMenuEntry::new(format!(
                        "{} (Ctrl)",
                        tr!(
                            "librepcb::editor::SchematicEditorState_DrawWire",
                            "Add New Bus Member"
                        )
                    ))
                }];
                let mut actions = vec![MenuAction::BusMember(Some(BusMemberChoice::NewMember))];
                for net in &nets {
                    entries.push(ContextMenuEntry {
                        enabled: net.enabled,
                        ..ContextMenuEntry::new(net.name.clone())
                    });
                    actions.push(MenuAction::BusMember(Some(BusMemberChoice::Net(net.net))));
                }
                entries.push(ContextMenuEntry::separator());
                actions.push(MenuAction::Separator);
                entries.push(ContextMenuEntry::new(tr!(
                    "librepcb::editor::SchematicEditorState_DrawWire",
                    "Cancel"
                )));
                actions.push(MenuAction::BusMember(None));
                self.menu = actions;
                let screen = self.canvas.view().world_to_screen(point_to_world(pos));
                update.requests.push(TabRequest::ContextMenu {
                    pos: screen,
                    entries,
                });
            }
            SchematicRequest::PolygonProperties(uuid) => {
                update
                    .requests
                    .push(TabRequest::Properties(PropertiesTarget::SchematicPolygon(
                        self.schematic,
                        uuid,
                    )));
            }
            SchematicRequest::TextProperties(uuid) => {
                update
                    .requests
                    .push(TabRequest::Properties(PropertiesTarget::SchematicText(
                        self.schematic,
                        uuid,
                    )));
            }
            SchematicRequest::ContextMenu { item, pos, .. } => {
                let entries = self.build_context_menu(item);
                if !entries.is_empty() {
                    let screen = self.canvas.view().world_to_screen(point_to_world(pos));
                    update.requests.push(TabRequest::ContextMenu {
                        pos: screen,
                        entries,
                    });
                }
            }
            SchematicRequest::ZoomAll => {
                let content = self.content_bounds();
                self.canvas.zoom_fit(content);
                self.update_overlays();
                update.repaint = true;
            }
        }
    }

    /// The context menu of an item (upstream
    /// `SchematicEditorState_Select::openContextMenuAtPos()`).
    fn build_context_menu(&mut self, item: SchematicItem) -> Vec<ContextMenuEntry> {
        use MenuAction as A;
        let mut actions = Vec::new();
        let (remaining_gates, is_symbol) = {
            let p = self.project.shared().lock();
            let proj = p.project();
            match item {
                SchematicItem::Symbol(sym) | SchematicItem::SymbolPin(sym, _) => {
                    let component = proj
                        .schematic(self.schematic)
                        .and_then(|s| s.symbols().get(&sym))
                        .map(|s| s.component());
                    let remaining = component.filter(|c| has_remaining_gates(proj, *c));
                    (remaining, true)
                }
                _ => (None, false),
            }
        };
        let edit = [A::Cut, A::Copy, A::Remove];
        let transform = [
            A::RotateCcw,
            A::RotateCw,
            A::MirrorHorizontal,
            A::MirrorVertical,
        ];
        match item {
            SchematicItem::Symbol(_) | SchematicItem::SymbolPin(..) if is_symbol => {
                actions.extend([A::Properties, A::Separator]);
                actions.extend(edit);
                actions.push(A::Separator);
                actions.extend(transform);
                actions.extend([A::Separator, A::SnapToGrid, A::ResetTexts]);
                if let Some(c) = remaining_gates {
                    actions.push(A::PlaceRemainingGates(c));
                }
            }
            SchematicItem::NetLabel(..) => {
                actions.extend([A::Properties, A::Separator, A::Remove, A::Separator]);
                actions.extend(transform);
                actions.push(A::SnapToGrid);
            }
            SchematicItem::Polygon(_) | SchematicItem::Text(_) => {
                actions.extend([A::Properties, A::Separator]);
                actions.extend(edit);
                actions.push(A::Separator);
                actions.extend(transform);
                actions.push(A::SnapToGrid);
            }
            _ => {
                actions.extend(edit);
                actions.extend([A::Separator, A::SnapToGrid]);
            }
        }
        let entries = actions
            .iter()
            .map(|a| match a {
                A::Separator => ContextMenuEntry::separator(),
                A::Properties => ContextMenuEntry {
                    is_default: true,
                    ..ContextMenuEntry::new(tr!("EditorCommandSet", "Properties"))
                },
                A::Cut => ContextMenuEntry::new(tr!("EditorCommandSet", "Cut")),
                A::Copy => ContextMenuEntry::new(tr!("EditorCommandSet", "Copy")),
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
                A::SnapToGrid => ContextMenuEntry::new(tr!("EditorCommandSet", "Snap to Grid")),
                A::ResetTexts => ContextMenuEntry::new(tr!("EditorCommandSet", "Reset All Texts")),
                A::PlaceRemainingGates(_) => {
                    ContextMenuEntry::new(tr!("EditorCommandSet", "Place Remaining Gates"))
                }
                // Not part of item context menus.
                A::BusMember(_) => ContextMenuEntry::separator(),
            })
            .collect();
        self.menu = actions;
        entries
    }

    /// An entry of the last context menu was chosen.
    pub fn context_menu_action(&mut self, index: usize) -> TabUpdate {
        let Some(action) = self.menu.get(index).copied() else {
            return TabUpdate::default();
        };
        self.menu.clear();
        let before = self.snapshot();
        let rotate = |a: Angle| {
            move |f: &mut SchematicEditorFsm, c: &mut SchematicContext<'_>| f.rotate(c, a)
        };
        match action {
            MenuAction::Properties => self.run(|f, c| f.edit_properties(c)),
            MenuAction::Cut => self.run(|f, c| f.cut(c)),
            MenuAction::Copy => self.run(|f, c| f.copy(c)),
            MenuAction::Remove => self.run(|f, c| f.remove(c)),
            MenuAction::RotateCcw => self.run(rotate(Angle::DEG90)),
            MenuAction::RotateCw => self.run(rotate(-Angle::DEG90)),
            MenuAction::MirrorHorizontal => self.run(|f, c| f.mirror(c, Orientation::Horizontal)),
            MenuAction::MirrorVertical => self.run(|f, c| f.mirror(c, Orientation::Vertical)),
            MenuAction::SnapToGrid => self.run(|f, c| f.snap_to_grid(c)),
            MenuAction::ResetTexts => self.run(|f, c| f.reset_all_texts(c)),
            MenuAction::PlaceRemainingGates(cmp) => {
                self.run(|f, c| f.add_remaining_gates(c, cmp, None))
            }
            MenuAction::BusMember(choice) => self.run(|f, c| f.choose_bus_member(c, choice)),
            MenuAction::Separator => None,
        };
        self.after_fsm(&before)
    }

    /// The bus member menu was closed without choosing an entry (like
    /// "Cancel").
    pub fn context_menu_closed(&mut self) -> TabUpdate {
        let pending = self
            .menu
            .iter()
            .any(|a| matches!(a, MenuAction::BusMember(_)));
        self.menu.clear();
        if !pending {
            return TabUpdate::default();
        }
        let before = self.snapshot();
        self.run(|f, c| f.choose_bus_member(c, None));
        self.after_fsm(&before)
    }

    /// An image file was chosen for the image tool (the answer of
    /// [`TabRequest::ChooseImageFile`]).
    pub fn add_image(&mut self, data: ImageData) -> TabUpdate {
        let before = self.snapshot();
        self.run(|f, c| f.add_image(c, Some(data)));
        self.after_fsm(&before)
    }

    /// Sets the term of the "find" field (upstream `setUiData()`); returns
    /// whether it changed.
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

    /// A component was chosen in the "add component" dialog.
    pub fn add_component(&mut self, choice: Option<ComponentChoice>) -> TabUpdate {
        let before = self.snapshot();
        if let Some(choice) = choice {
            self.run(|f, c| f.add_component(c, choice));
        }
        self.after_fsm(&before)
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

    /// What this tab cross-probes: what the FSM reports (upstream
    /// `fsmCrossProbe()`: nets, components, component signals of pins,
    /// buses), the components of selected symbols, the nets of selected
    /// wires and labels, and the objects found by "find".
    pub fn cross_probe(&self) -> CrossProbe {
        let p = self.project.shared().lock();
        let Some(sch) = p.project().schematic(self.schematic) else {
            return CrossProbe::default();
        };
        let mut probe = self.fsm.cross_probe().clone();
        let found = &self.find_probe;
        probe.components.extend(found.components.iter().copied());
        probe.nets.extend(found.nets.iter().copied());
        for item in self.fsm.selection() {
            match *item {
                SchematicItem::Symbol(sym) => {
                    if let Some(s) = sch.symbols().get(&sym) {
                        probe.components.insert(s.component());
                    }
                }
                SchematicItem::NetLine(seg, _)
                | SchematicItem::NetPoint(seg, _)
                | SchematicItem::NetLabel(seg, _) => {
                    if let Some(s) = sch.net_segments().get(&seg) {
                        probe.nets.insert(s.net());
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
        let mut update = self.after_fsm(&before);
        update.repaint = true;
        update.data_changed = true;
        // The modification came from elsewhere.
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
            export_graphics: ui::FeatureState::NotSupported,
            select: feature(f.select),
            cut: edit(f.cut),
            copy: feature(f.copy),
            paste: edit(f.paste),
            remove: edit(f.remove),
            rotate: edit(f.rotate),
            mirror: edit(f.mirror),
            flip: ui::FeatureState::NotSupported,
            move_align: ui::FeatureState::NotSupported,
            snap_to_grid: edit(f.snap_to_grid),
            reset_texts: edit(f.reset_texts),
            lock: ui::FeatureState::NotSupported,
            unlock: ui::FeatureState::NotSupported,
            modify_line_width: ui::FeatureState::NotSupported,
            edit_properties: feature(f.properties),
            find: feature(true),
            ..Default::default()
        };
        ui::TabData {
            r#type: ui::TabType::Schematic,
            find_term: self.fsm.search().term().into(),
            find_autocompletions: slint::ModelRc::from(self.find_suggestions.clone()),
            title: self.title.as_str().into(),
            features,
            read_only: !writable,
            unsaved_changes: p.has_unsaved_changes(),
            undo_text: undo.undo_text().unwrap_or_default().into(),
            redo_text: undo.redo_text().unwrap_or_default().into(),
            ..Default::default()
        }
    }

    /// The `SchematicTabData`.
    pub fn derived_ui_data(&self, projects: &[Rc<AppProject>]) -> ui::SchematicTabData {
        let background = self
            .scene
            .as_ref()
            .map_or(Color::WHITE, SchematicScene::background);
        let [r, g, b, _] = background.to_rgba8().to_u8_array();
        let lightness = (f32::from(r.max(g).max(b)) + f32::from(r.min(g).min(b))) / 510.0;
        let foreground = if lightness >= 0.5 {
            slint::Color::from_rgb_u8(0, 0, 0)
        } else {
            slint::Color::from_rgb_u8(255, 255, 255)
        };
        let schematic_index = {
            let p = self.project.shared().lock();
            p.project()
                .schematic_index(self.schematic)
                .map_or(-1, |i| i as i32)
        };
        let vs = self.fsm.view_state();
        let data = self.fsm.tool_data();
        let layers = sorted_layers(&data.available_layers);
        ui::SchematicTabData {
            project_index: project_index(projects, &self.project),
            schematic_index,
            background_color: color_to_ui(background).into(),
            foreground_color: foreground.into(),
            // Upstream: primary and secondary color of the info box role.
            overlay_color: slint::Color::from_argb_u8(0x82, 0xff, 0xff, 0xff).into(),
            overlay_text_color: slint::Color::from_rgb_u8(0, 0, 0).into(),
            grid_style: grid_style_to_ui(self.grid_style),
            grid_interval: length_to_ui(self.grid_interval.get()),
            unit: unit_to_ui(self.unit),
            show_pin_numbers: self.show_pin_numbers,
            ignore_placement_locks: self.fsm.settings().ignore_locks,
            tool: tool_to_ui(self.fsm.tool()),
            tool_cursor: mouse_cursor(vs.cursor, self.canvas.is_panning()),
            tool_overlay_text: info_box_text(&vs.info_box).into(),
            tool_wire_mode: wire_mode_to_ui(data.wire_mode),
            tool_layer: ui::ComboBoxData {
                items: crate::models::vec_model(
                    layers.iter().map(|l| l.name_tr().into()).collect(),
                ),
                current_index: layers
                    .iter()
                    .position(|l| *l == data.layer)
                    .map_or(-1, |i| i as i32),
            },
            tool_line_width: self.line_width.ui_data(),
            tool_size: self.size.ui_data(),
            tool_filled: data.filled,
            tool_value: ui::LineEditData {
                enabled: true,
                text: to_single_line(&data.value).into(),
                placeholder: Default::default(),
                suggestions: crate::models::vec_model(
                    data.value_suggestions
                        .iter()
                        .map(|s| s.as_str().into())
                        .collect(),
                ),
            },
            tool_attribute_value: ui::LineEditData {
                enabled: false,
                ..Default::default()
            },
            tool_attribute_unit: ui::ComboBoxData {
                items: crate::models::vec_model(Vec::new()),
                current_index: -1,
            },
            scene_image_pos: self.scene_image_pos,
            frame: self.frame,
            ..Default::default()
        }
    }

    /// Applies `SchematicTabData` written by the UI (tool bar edits, grid,
    /// pin numbers).
    pub fn set_derived_ui_data(&mut self, data: &ui::SchematicTabData) -> TabUpdate {
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
        if data.show_pin_numbers != self.show_pin_numbers {
            self.show_pin_numbers = data.show_pin_numbers;
            self.apply_pin_numbers();
            update.repaint = true;
        }
        if data.ignore_placement_locks != self.fsm.settings().ignore_locks {
            let settings = SchematicEditorSettings {
                ignore_locks: data.ignore_placement_locks,
                ..self.fsm.settings().clone()
            };
            self.fsm.set_settings(settings);
            update.data_changed = true;
        }

        // Tool bar.
        let current = self.fsm.tool_data().clone();
        let mode = wire_mode_from_ui(data.tool_wire_mode);
        if mode != current.wire_mode {
            self.run(|f, c| f.set_wire_mode(c, mode));
        }
        let layers = sorted_layers(&current.available_layers);
        if let Some(layer) = usize::try_from(data.tool_layer.current_index)
            .ok()
            .and_then(|i| layers.get(i))
            .copied()
            && layer != current.layer
        {
            self.run(|f, c| f.set_layer(c, layer));
        }
        if let Some(width) = self.line_width.set_ui_data(&data.tool_line_width)
            && let Ok(width) = UnsignedLength::new(width)
            && width != current.line_width
        {
            self.run(|f, c| f.set_line_width(c, width));
        }
        if let Some(size) = self.size.set_ui_data(&data.tool_size)
            && let Ok(size) = PositiveLength::new(size)
            && size != current.size
        {
            self.run(|f, c| f.set_size(c, size));
        }
        if !data.new_bus_uuid.is_empty() {
            // Upstream `setDerivedUiData()`: `processDrawBus(uuid)`, the
            // tool with the bus chosen in the menu of the tool button.
            if let Ok(bus) = data.new_bus_uuid.parse::<BusId>() {
                if self.fsm.tool() != SchematicTool::Bus {
                    self.run(|f, c| f.draw_bus(c));
                }
                self.run(|f, c| f.set_bus(c, Some(bus)));
            }
            update.data_changed = true;
        }
        if data.tool_filled != current.filled {
            let filled = data.tool_filled;
            self.run(|f, c| f.set_filled(c, filled));
        }
        let value = to_multi_line(&data.tool_value.text);
        if value != current.value {
            self.run(|f, c| f.set_value(c, value));
        }
        let mut after = self.after_fsm(&before);
        after.repaint |= update.repaint;
        after.data_changed |= update.data_changed;
        // Length edits reset their increase/decrease flags.
        after.data_changed |= data.tool_line_width.increase
            || data.tool_line_width.decrease
            || data.tool_size.increase
            || data.tool_size.decrease;
        after
    }

    /// Changes the grid of the schematic (a modification of the schematic
    /// like upstream, but undoable).
    fn set_grid(&mut self, interval: PositiveLength, unit: LengthUnit) {
        let mut p = self.project.shared().lock();
        let Some(props) = p
            .project()
            .schematic(self.schematic)
            .map(|s| s.properties())
        else {
            return;
        };
        if props.grid_interval == interval && props.grid_unit == unit {
            return;
        }
        let props = librepcb_core::project::schematic::SchematicProperties {
            grid_interval: interval,
            grid_unit: unit,
            ..props
        };
        if let Err(e) = p.editor.execute(ApplyMutations {
            text: Some(tr!(
                "librepcb::editor::SchematicTab",
                "Change Grid Properties"
            )),
            mutations: vec![Mutation::UpdateSchematic(props)],
        }) {
            log::error!("Failed to change the grid: {e}");
        }
    }

    // --- Actions ---

    /// Handles a tab action (upstream `SchematicTab::trigger()`).
    pub fn trigger(&mut self, action: ui::TabAction) -> TabUpdate {
        use ui::TabAction as A;
        let before = self.snapshot();
        let mut extra = TabUpdate::default();
        let grid = self.grid_interval.get();
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
            A::MirrorHorizontally => {
                self.run(|f, c| f.mirror(c, Orientation::Horizontal));
            }
            A::MirrorVertically => {
                self.run(|f, c| f.mirror(c, Orientation::Vertical));
            }
            A::MoveLeft | A::MoveRight | A::MoveUp | A::MoveDown => {
                let (dx, dy) = match action {
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
                    self.canvas
                        .scroll_steps(f64::from(dx as i32), -f64::from(dy as i32));
                    extra.repaint = true;
                }
            }
            A::SnapToGrid => {
                self.run(|f, c| f.snap_to_grid(c));
            }
            A::ResetTexts => {
                self.run(|f, c| f.reset_all_texts(c));
            }
            A::EditProperties => {
                self.run(|f, c| f.edit_properties(c));
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
                self.run(|f, c| f.select_tool(c));
            }
            A::ToolWire => {
                self.run(|f, c| f.draw_wire(c));
            }
            A::ToolLabel => {
                self.run(|f, c| f.add_net_label(c));
            }
            A::ToolPolygon => {
                self.run(|f, c| f.draw_polygon(c));
            }
            A::ToolText => {
                self.run(|f, c| f.add_text(c));
            }
            A::ToolMeasure => {
                self.run(|f, c| f.measure(c));
            }
            A::ToolComponent => {
                self.fsm.open_add_component_dialog("");
            }
            A::ToolComponentFrame => {
                self.fsm.open_add_component_dialog("schematic frame");
            }
            A::ToolComponentResistor
            | A::ToolComponentInductor
            | A::ToolComponentCapacitorBipolar
            | A::ToolComponentCapacitorUnipolar
            | A::ToolComponentGnd
            | A::ToolComponentVcc => {
                let (cmp, iec, ieee) = match action {
                    A::ToolComponentResistor => RESISTOR,
                    A::ToolComponentInductor => INDUCTOR,
                    A::ToolComponentCapacitorBipolar => CAPACITOR_BIPOLAR,
                    A::ToolComponentCapacitorUnipolar => CAPACITOR_UNIPOLAR,
                    A::ToolComponentGnd => GND,
                    _ => VCC,
                };
                let variant = if self.use_ieee315() { ieee } else { iec };
                if let (Ok(component), Ok(variant)) = (Uuid::from_str(cmp), Uuid::from_str(variant))
                {
                    let choice = ComponentChoice {
                        symbol_variant: Some(variant),
                        ..ComponentChoice::new(component)
                    };
                    self.run(|f, c| f.add_component(c, choice));
                }
            }
            A::ToolBus => {
                self.run(|f, c| f.draw_bus(c));
            }
            A::ToolImage => {
                self.run(|f, c| f.add_image(c, None));
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

    /// Whether the project prefers IEEE 315 symbols (upstream
    /// `ProjectEditor::getUseIeee315Symbols()`).
    fn use_ieee315(&self) -> bool {
        let p = self.project.shared().lock();
        p.project()
            .settings()
            .norm_order
            .first()
            .is_some_and(|n| n.eq_ignore_ascii_case("IEEE 315"))
    }

    fn content_bounds(&mut self) -> Option<librepcb_canvas::kurbo::Rect> {
        let scene = self.scene.as_mut()?;
        // The overlays (e.g. the gray-out) do not count.
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
        // The view may have been fitted: overlays depend on the zoom level.
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
        // A click into the scene closes a pending bus member menu (the
        // popup has no "closed" notification): cancel it like upstream.
        if kind == PointerKind::Down
            && self
                .menu
                .iter()
                .any(|a| matches!(a, MenuAction::BusMember(_)))
        {
            let mut update = self.context_menu_closed();
            let next = self.pointer_event(kind, button, pos, modifiers);
            update.repaint |= next.repaint;
            update.data_changed |= next.data_changed;
            update.project_modified |= next.project_modified;
            update.cursor = next.cursor.or(update.cursor);
            update.status = next.status.or(update.status);
            update.requests.extend(next.requests);
            return update;
        }
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
                self.cursor = Some(e.pos);
                self.find_probe = CrossProbe::default();
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

/// Whether a component has gates which are not placed in any schematic.
fn has_remaining_gates(
    project: &librepcb_core::project::Project,
    component: ComponentInstanceId,
) -> bool {
    let Some(instance) = project.circuit().component_instance(component) else {
        return false;
    };
    let placed: BTreeSet<Uuid> = project
        .component_uses(component)
        .map(|u| u.symbols.keys().copied().collect())
        .unwrap_or_default();
    project
        .library()
        .component(&instance.lib_component())
        .and_then(|c| c.symbol_variants().by_uuid(&instance.lib_variant()))
        .is_some_and(|v| v.symbol_items().uuids().iter().any(|g| !placed.contains(g)))
}

/// Layers of the layer combo box, sorted like upstream `Layer::sorted()`.
fn sorted_layers(layers: &[Layer]) -> Vec<Layer> {
    Layer::all()
        .iter()
        .copied()
        .filter(|l| layers.contains(l))
        .collect()
}

fn tool_to_ui(tool: SchematicTool) -> ui::EditorTool {
    match tool {
        SchematicTool::Select => ui::EditorTool::Select,
        SchematicTool::Wire => ui::EditorTool::Wire,
        SchematicTool::Bus => ui::EditorTool::Bus,
        SchematicTool::Image => ui::EditorTool::Image,
        SchematicTool::Label => ui::EditorTool::Label,
        SchematicTool::Component => ui::EditorTool::Component,
        SchematicTool::Polygon => ui::EditorTool::Polygon,
        SchematicTool::Text => ui::EditorTool::Text,
        SchematicTool::Measure => ui::EditorTool::Measure,
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
