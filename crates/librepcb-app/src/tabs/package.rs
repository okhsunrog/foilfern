//! The package editor tab.
//!
//! Port of libs/librepcb/editor/library/pkg/packagetab.{h,cpp} with the
//! list models of the tab (libs/librepcb/editor/library/pkg/
//! {packagepadlistmodel,footprintlistmodel,packagemodellistmodel}.{h,cpp}):
//! the current footprint rendered by `librepcb-scene`'s [`FootprintScene`]
//! (rebuilt after each modification), editing through the package editor
//! state machine ([`PackageEditorFsm`]) with the tool bars of upstream's
//! `.slint` files, the package pads, footprints (tags, 3D model transform)
//! and 3D models lists, the assembly type and minimum copper clearance,
//! the metadata page, the checks with approvals and automatic fixes,
//! undo/redo and saving (see [`ElementCore`]).
//!
//! Differences to upstream: there is no 3D view (the 3D model list, the
//! footprint's model assignment and transform are editable, the STEP files
//! are not rendered); the background image and graphics export are not
//! available in the package editor yet; the file system watcher ("files
//! modified" banner) is not ported. DXF files are chosen by the
//! application (DXF import dialog) and passed to
//! [`PackageTab::import_dxf()`].

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::rc::Rc;

use librepcb_app_ui as ui;
use librepcb_canvas::kurbo::{Point, Rect, Vec2};
use librepcb_canvas::peniko::Color;
use librepcb_canvas::{Grid, Modifiers, PointerAction, PointerButton, PointerKind, View};
use librepcb_core::font::StrokeFont;
use librepcb_core::geometry::{ComponentSide, Pad, PadFunction, PadShape, ZoneLayers, ZoneRules};
use librepcb_core::library::pkg::{AssemblyType, Package};
use librepcb_core::types::{
    Angle, CircuitIdentifier, ElementName, GridStyle, Length, LengthUnit, MaskConfig, Orientation,
    PositiveLength, Ratio, Tag, UnsignedLength, UnsignedLimitedRatio, Uuid,
};
use librepcb_core::utils::toolbox;
use librepcb_editor::fsm::library::{
    ElementHost, LibraryContext, LibraryEditorSettings, LibraryRequest, LibraryTool, LibraryView,
    PackageEditorFsm, PackageHost,
};
use librepcb_editor::fsm::{PointerEvent, ViewState};
use librepcb_editor::library_editor::commands::{
    AddFootprint, AddPackageModel, AddPackagePads, DuplicateFootprint, EditFootprint, EditPackage,
    EditPackageModel, FootprintItem, FootprintObject, MoveFootprintUp, MovePackageModelUp,
    RemoveFootprint, RemovePackageModel, RemovePackagePad, RenamePackagePad, UpdateFootprintObject,
};
use librepcb_i18n::tr;
use librepcb_scene::{ColorScheme, FootprintScene, FootprintSceneObject};

use super::editing::{
    DoubleClick, OverlayColors, Overlays, error_notification, fsm_key_event, fsm_modifiers,
    info_box_text, mouse_cursor, point_from_world, point_to_world, to_multi_line, to_single_line,
};
use super::element_core::ElementCore;
use super::schematic_view::HIT_TOLERANCE_PX;
use super::symbol::{
    LibraryItemRef, LibraryMenuAction, context_menu_entries, halign_from_ui, halign_to_ui,
    sorted_layers, tool_to_ui, valign_from_ui, valign_to_ui,
};
use super::{DxfImportKind, TabId, TabRequest, TabUpdate, feature};
use crate::canvas_view::{CanvasView, DEFAULT_BOARD_RECT};
use crate::clipboard::{ensure_opened, with_clipboard};
use crate::helpers::{
    color_to_ui, grid_style_from_ui, grid_style_to_ui, length_from_ui, length_to_ui, unit_from_ui,
    unit_to_ui,
};
use crate::length_edit::{LengthEdit, steps};
use crate::models::{UiModel, model_rc, vec_model};
use crate::validation;

/// Grid color of the board color scheme.
const GRID_COLOR: Color = Color::from_rgb8(0xa0, 0xa0, 0xa4);

/// The page of the graphical editor.
const EDITOR_PAGE: i32 = 2;

/// A row of one of the tab's list models written by the UI.
#[derive(Debug, Clone)]
pub enum PackageRowEvent {
    /// A package pad row (name edited, deleted).
    Pad(usize, ui::PackagePadData),
    /// A footprint row (name, tags, 3D transform, actions).
    Footprint(usize, ui::FootprintData),
    /// The 3D model check box of a footprint.
    FootprintModel {
        /// Footprint row.
        footprint: usize,
        /// Model row.
        model: usize,
        /// Checked.
        checked: bool,
    },
    /// A 3D model row (name, actions).
    Model(usize, ui::PackageModelData),
}

/// Receives the rows written by the UI (deferred to the application).
pub type PackageRowSink = Rc<dyn Fn(PackageRowEvent)>;

/// Upstream `l2s(Package::AssemblyType)`.
fn assembly_type_to_ui(t: AssemblyType) -> i32 {
    match t {
        AssemblyType::Tht => 0,
        AssemblyType::Smt => 1,
        AssemblyType::Mixed => 2,
        AssemblyType::Other => 3,
        AssemblyType::None => 4,
        AssemblyType::Auto => 5,
    }
}

/// Upstream `s2assemblyType()`.
fn assembly_type_from_ui(i: i32) -> Option<AssemblyType> {
    Some(match i {
        0 => AssemblyType::Tht,
        1 => AssemblyType::Smt,
        2 => AssemblyType::Mixed,
        3 => AssemblyType::Other,
        4 => AssemblyType::None,
        5 => AssemblyType::Auto,
        _ => return None,
    })
}

/// The pad shape of the tool bar (upstream `getCurrentShape()` of
/// `PackageTab::fsmToolEnter(PackageEditorState_AddPads&)`).
fn pad_shape_to_ui(shape: PadShape, radius: UnsignedLimitedRatio) -> ui::PadShape {
    match shape {
        PadShape::RoundedRect if radius.get() == Ratio::from_percent(0) => ui::PadShape::Rect,
        PadShape::RoundedRect if radius.get() == Ratio::from_percent(100) => ui::PadShape::Round,
        PadShape::RoundedRect => ui::PadShape::RoundedRect,
        _ => ui::PadShape::Octagon,
    }
}

fn percent(p: i32) -> UnsignedLimitedRatio {
    // Only called with 0 and 100.
    UnsignedLimitedRatio::new(Ratio::from_percent(p)).expect("0..100% is a valid ratio")
}

fn scene_object(item: FootprintItem) -> Option<FootprintSceneObject> {
    Some(match item {
        FootprintItem::Pad(u) => FootprintSceneObject::Pad(u),
        FootprintItem::Polygon(u) => FootprintSceneObject::Polygon(u),
        FootprintItem::Circle(u) => FootprintSceneObject::Circle(u),
        FootprintItem::StrokeText(u) => FootprintSceneObject::StrokeText(u),
        FootprintItem::Hole(u) => FootprintSceneObject::Hole(u),
        FootprintItem::Zone(u) => FootprintSceneObject::Zone(u),
    })
}

fn footprint_item(o: FootprintSceneObject) -> FootprintItem {
    match o {
        FootprintSceneObject::Pad(u) => FootprintItem::Pad(u),
        FootprintSceneObject::Polygon(u) => FootprintItem::Polygon(u),
        FootprintSceneObject::Circle(u) => FootprintItem::Circle(u),
        FootprintSceneObject::StrokeText(u) => FootprintItem::StrokeText(u),
        FootprintSceneObject::Hole(u) => FootprintItem::Hole(u),
        FootprintSceneObject::Zone(u) => FootprintItem::Zone(u),
    }
}

/// The view of the package tab as the FSM sees it.
struct FootprintSceneView<'a> {
    scene: Option<&'a FootprintScene>,
    view: &'a View,
    cursor: Option<librepcb_core::types::Point>,
}

impl LibraryView<FootprintItem> for FootprintSceneView<'_> {
    fn items_at(&self, pos: librepcb_core::types::Point, tolerance: Length) -> Vec<FootprintItem> {
        self.scene
            .map(|s| s.objects_at(pos, tolerance))
            .unwrap_or_default()
            .into_iter()
            .map(footprint_item)
            .collect()
    }

    fn items_in_rect(
        &self,
        p1: librepcb_core::types::Point,
        p2: librepcb_core::types::Point,
    ) -> Vec<FootprintItem> {
        self.scene
            .map(|s| s.objects_in_rect(p1, p2))
            .unwrap_or_default()
            .into_iter()
            .map(footprint_item)
            .collect()
    }

    fn tolerance(&self) -> Length {
        Length::from_mm(self.view.pixels_to_world(HIT_TOLERANCE_PX)).unwrap_or(Length::ZERO)
    }

    fn cursor_pos(&self) -> Option<librepcb_core::types::Point> {
        self.cursor
    }
}

/// What the footprint rows show (to rebuild them only on changes).
type FootprintRowKey = (
    Uuid,
    String,
    BTreeSet<Tag>,
    librepcb_core::types::Point3D,
    librepcb_core::types::Angle3D,
    BTreeSet<Uuid>,
);

/// The package editor tab (upstream `PackageTab`).
pub struct PackageTab {
    id: TabId,
    core: ElementCore<Package>,
    fsm: PackageEditorFsm,
    scheme: ColorScheme,
    font: Option<StrokeFont>,
    scene: Option<FootprintScene>,
    /// Editor state and footprint the scene was built for.
    scene_state: Option<(u64, Option<Uuid>)>,
    canvas: CanvasView,
    overlays: Overlays,
    double_click: DoubleClick,
    cursor: Option<librepcb_core::types::Point>,
    grid_style: GridStyle,
    unit: LengthUnit,
    frame: i32,
    scene_image_pos: slint::LogicalPosition,
    line_width: LengthEdit,
    size: LengthEdit,
    drill: LengthEdit,
    configured_tool: Option<LibraryTool>,
    menu: Vec<LibraryMenuAction>,
    menu_pos: (librepcb_core::types::Point, Vec<usize>, Option<usize>),
    menu_item: Option<FootprintItem>,
    library_index: i32,
    // Package properties (committed with "apply").
    assembly_type: AssemblyType,
    min_copper_clearance: LengthEdit,
    // Lists.
    sink: PackageRowSink,
    pads: Rc<UiModel<ui::PackagePadData>>,
    pad_rows: Vec<Uuid>,
    pads_key: Vec<(Uuid, String)>,
    new_pad_name: String,
    new_pad_name_error: String,
    footprints: Rc<UiModel<ui::FootprintData>>,
    footprints_key: Option<(Vec<FootprintRowKey>, Vec<Uuid>)>,
    models: Rc<UiModel<ui::PackageModelData>>,
    models_key: Vec<(Uuid, String)>,
    current_model: Option<Uuid>,
    /// Errors of commands executed since the last update.
    pending_errors: Vec<String>,
    /// Requests collected since the last update.
    pending_requests: Vec<TabRequest>,
}

impl PackageTab {
    /// Creates the tab for an element core; `sink` receives the rows of
    /// the list models written by the UI.
    pub fn new(core: ElementCore<Package>, grid_style: GridStyle, sink: PackageRowSink) -> Self {
        let scheme = ColorScheme::BOARD_DARK;
        let settings = LibraryEditorSettings {
            app_version: crate::app::APP_VERSION.to_owned(),
            ..LibraryEditorSettings::default()
        };
        let pads = UiModel::shared(Vec::new());
        let s = Rc::clone(&sink);
        pads.set_handler(move |row, data| s(PackageRowEvent::Pad(row, data)));
        let footprints = UiModel::shared(Vec::new());
        let s = Rc::clone(&sink);
        footprints.set_handler(move |row, data| s(PackageRowEvent::Footprint(row, data)));
        let models = UiModel::shared(Vec::new());
        let s = Rc::clone(&sink);
        models.set_handler(move |row, data| s(PackageRowEvent::Model(row, data)));
        let background = scheme.color_or_transparent("board_background");
        let mut tab = Self {
            id: TabId::new(),
            core,
            fsm: PackageEditorFsm::new(settings),
            scheme,
            font: librepcb_scene::default_stroke_font(),
            scene: None,
            scene_state: None,
            canvas: CanvasView::new(background, DEFAULT_BOARD_RECT, None),
            overlays: Overlays::default(),
            double_click: DoubleClick::default(),
            cursor: None,
            grid_style,
            unit: LengthUnit::Millimeters,
            frame: 0,
            scene_image_pos: Default::default(),
            line_width: LengthEdit::new(steps::GENERIC),
            size: LengthEdit::new(steps::TEXT_HEIGHT),
            drill: LengthEdit::new(steps::DRILL_DIAMETER),
            configured_tool: None,
            menu: Vec::new(),
            menu_pos: (Default::default(), Vec::new(), None),
            menu_item: None,
            library_index: -1,
            assembly_type: AssemblyType::Auto,
            min_copper_clearance: LengthEdit::new(steps::GENERIC),
            sink,
            pads,
            pad_rows: Vec::new(),
            pads_key: Vec::new(),
            new_pad_name: String::new(),
            new_pad_name_error: String::new(),
            footprints,
            footprints_key: None,
            models,
            models_key: Vec::new(),
            current_model: None,
            pending_errors: Vec::new(),
            pending_requests: Vec::new(),
        };
        tab.refresh_package_data();
        let first = tab.first_footprint();
        tab.run(|f, c| f.set_footprint(c, first));
        tab.sync_scene();
        tab.canvas = CanvasView::new(
            background,
            DEFAULT_BOARD_RECT,
            tab.scene.as_ref().and_then(FootprintScene::content_bounds),
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
    pub fn core(&self) -> &ElementCore<Package> {
        &self.core
    }

    /// The shared element state, mutably.
    pub fn core_mut(&mut self) -> &mut ElementCore<Package> {
        &mut self.core
    }

    /// The FSM.
    pub fn fsm(&self) -> &PackageEditorFsm {
        &self.fsm
    }

    /// The scene.
    pub fn scene(&self) -> Option<&FootprintScene> {
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

    /// The pads of the package (UUID, name) for the pad properties dialog.
    pub fn package_pads(&self) -> Vec<(Uuid, String)> {
        self.core
            .editor
            .element()
            .pads()
            .iter()
            .map(|p| (p.uuid(), p.name().to_string()))
            .collect()
    }

    /// The footprint being edited.
    pub fn current_footprint(&self) -> Option<Uuid> {
        self.fsm.footprint()
    }

    fn first_footprint(&self) -> Option<Uuid> {
        self.core
            .editor
            .element()
            .footprints()
            .iter()
            .next()
            .map(|f| f.uuid())
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
        f: impl FnOnce(&mut PackageEditorFsm, &mut LibraryContext<'_, PackageHost>) -> R,
    ) -> R {
        let view = FootprintSceneView {
            scene: self.scene.as_ref(),
            view: self.canvas.view(),
            cursor: self.cursor,
        };
        let fsm = &mut self.fsm;
        let editor = &mut self.core.editor;
        with_clipboard(|clipboard| {
            let mut ctx = LibraryContext::new(editor, &view, clipboard);
            f(fsm, &mut ctx)
        })
    }

    /// Rebuilds the scene if the element or the footprint changed.
    fn sync_scene(&mut self) -> bool {
        let state = (self.core.editor.state_id(), self.fsm.footprint());
        if self.scene_state == Some(state) {
            return false;
        }
        self.scene_state = Some(state);
        let package = self.core.editor.element();
        self.scene = state
            .1
            .and_then(|uuid| package.footprints().by_uuid(&uuid))
            .map(|fpt| FootprintScene::build_for_editor(fpt, self.font.as_ref(), &self.scheme));
        self.overlays.forget();
        self.fsm.element_changed(self.core.editor.element());
        true
    }

    /// Updates everything after an FSM call (scene, lists, selection,
    /// overlays, tool bar, requests).
    fn after_fsm(&mut self) -> TabUpdate {
        let mut update = TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        };
        let state_before = self.scene_state.map(|s| s.0);
        let current = self.fsm.footprint();
        // Upstream `refreshUiData()`: switch to another footprint if the
        // current one was removed (or load the first one added).
        let package = self.core.editor.element();
        if !self.core.editor.is_group_active()
            && current.is_none_or(|u| package.footprints().by_uuid(&u).is_none())
        {
            let first = self.first_footprint();
            if first != current {
                self.run(|f, c| f.set_footprint(c, first));
            }
        }
        update.repaint |= self.sync_scene();
        if state_before != self.scene_state.map(|s| s.0) {
            self.core.refresh();
            self.core.run_checks_if_modified();
            self.refresh_package_data();
        }
        self.auto_select_model();
        self.configure_tool_edits();
        self.apply_highlight();
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

    /// Updates the package properties and the lists from the element
    /// (upstream `refreshUiData()` and the list models' `listEdited()`).
    fn refresh_package_data(&mut self) {
        let package = self.core.editor.element();
        self.assembly_type = package.assembly_type();
        self.min_copper_clearance.configure(
            *package.min_copper_clearance(),
            Length::ZERO,
            steps::GENERIC,
        );
        // Pads, sorted by name (upstream `PackagePadListModel` sorted by
        // the sort index, numbers naturally).
        let pads_key: Vec<(Uuid, String)> = package
            .pads()
            .iter()
            .map(|p| (p.uuid(), p.name().to_string()))
            .collect();
        if pads_key != self.pads_key || self.pads.len() != pads_key.len() {
            let mut sorted = pads_key.clone();
            sorted.sort_by(|a, b| crate::dialogs::natural_cmp(&a.1, &b.1));
            self.pad_rows = sorted.iter().map(|(u, _)| *u).collect();
            self.pads.replace_all(
                sorted
                    .iter()
                    .enumerate()
                    .map(|(i, (uuid, name))| ui::PackagePadData {
                        id: uuid.to_string()[..8].into(),
                        name: name.as_str().into(),
                        name_error: Default::default(),
                        delete: false,
                        sort_index: i as i32,
                    })
                    .collect(),
            );
            self.pads_key = pads_key;
        }
        // 3D models.
        let models_key: Vec<(Uuid, String)> = package
            .models()
            .iter()
            .map(|m| (m.uuid(), m.name().to_string()))
            .collect();
        if models_key != self.models_key || self.models.len() != models_key.len() {
            self.models.replace_all(
                models_key
                    .iter()
                    .map(|(uuid, name)| ui::PackageModelData {
                        id: uuid.to_string()[..8].into(),
                        name: name.as_str().into(),
                        action: ui::PackageModelAction::None,
                    })
                    .collect(),
            );
            self.models_key = models_key;
        }
        // Footprints.
        let model_uuids: Vec<Uuid> = package.models().iter().map(|m| m.uuid()).collect();
        let rows: Vec<FootprintRowKey> = package
            .footprints()
            .iter()
            .map(|f| {
                (
                    f.uuid(),
                    f.names().default_value().to_string(),
                    f.tags().clone(),
                    f.model_position(),
                    f.model_rotation(),
                    f.models().clone(),
                )
            })
            .collect();
        let key = Some((rows, model_uuids));
        if key != self.footprints_key || self.footprints.len() != package.footprints().len() {
            let (rows, model_uuids) = key.clone().unwrap_or_default();
            let data = rows
                .iter()
                .enumerate()
                .map(|(row, (uuid, name, tags, pos, rot, models))| {
                    let checked =
                        UiModel::shared(model_uuids.iter().map(|m| models.contains(m)).collect());
                    let sink = Rc::clone(&self.sink);
                    checked.set_handler(move |model, checked| {
                        sink(PackageRowEvent::FootprintModel {
                            footprint: row,
                            model,
                            checked,
                        })
                    });
                    ui::FootprintData {
                        id: uuid.to_string()[..8].into(),
                        name: name.as_str().into(),
                        tags: vec_model(tags.iter().map(|t| t.to_string().into()).collect()),
                        model_x: length_to_ui(pos.0),
                        model_y: length_to_ui(pos.1),
                        model_z: length_to_ui(pos.2),
                        model_rx: rot.0.to_micro_deg(),
                        model_ry: rot.1.to_micro_deg(),
                        model_rz: rot.2.to_micro_deg(),
                        models: model_rc(&checked),
                        new_tag: Default::default(),
                        remove_tag: Default::default(),
                        action: ui::FootprintAction::None,
                    }
                })
                .collect();
            self.footprints.replace_all(data);
            self.footprints_key = key;
        }
    }

    /// Upstream `autoSelectCurrentModelIndex()`: the current 3D model must
    /// be one of the current footprint.
    fn auto_select_model(&mut self) {
        let package = self.core.editor.element();
        let Some(fpt) = self
            .fsm
            .footprint()
            .and_then(|u| package.footprints().by_uuid(&u))
        else {
            return;
        };
        if self
            .current_model
            .is_none_or(|m| !fpt.models().contains(&m))
        {
            self.current_model = package
                .models()
                .iter()
                .map(|m| m.uuid())
                .find(|m| fpt.models().contains(m));
        }
    }

    fn apply_highlight(&mut self) -> bool {
        let Some(scene) = &mut self.scene else {
            return false;
        };
        let want: HashSet<_> = self
            .fsm
            .selection()
            .iter()
            .filter_map(|i| scene_object(*i))
            .flat_map(|o| scene.items_of(o))
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
                OverlayColors::BOARD_DARK,
            );
        }
    }

    fn is_fiducial_tool(&self) -> bool {
        matches!(self.fsm.tool(), LibraryTool::AddSmtPads(f) if f.is_fiducial())
    }

    fn configure_tool_edits(&mut self) {
        let tool = self.fsm.tool();
        let d = self.fsm.tool_data().clone();
        let fiducial = self.is_fiducial_tool();
        // (line width, its minimum, size, its minimum, size steps, drill)
        let (line, line_min, size, size_min, size_steps, drill) = match tool {
            LibraryTool::AddThtPads | LibraryTool::AddSmtPads(_) => (
                *d.pad_width,
                Length::new(1),
                if fiducial {
                    *d.copper_clearance
                } else {
                    *d.pad_height
                },
                if fiducial {
                    Length::ZERO
                } else {
                    Length::new(1)
                },
                steps::GENERIC,
                *d.drill,
            ),
            LibraryTool::AddHoles => (
                Length::ZERO,
                Length::ZERO,
                Length::ZERO,
                Length::ZERO,
                steps::GENERIC,
                *d.drill,
            ),
            LibraryTool::AddNames | LibraryTool::AddValues | LibraryTool::DrawText => (
                *d.stroke_width,
                Length::ZERO,
                *d.height,
                Length::new(1),
                steps::TEXT_HEIGHT,
                Length::ZERO,
            ),
            _ => (
                *d.line_width,
                Length::ZERO,
                Length::ZERO,
                Length::ZERO,
                steps::GENERIC,
                Length::ZERO,
            ),
        };
        let fiducial_changed = self.configured_tool.is_some_and(|t| {
            matches!(t, LibraryTool::AddSmtPads(f) if f.is_fiducial()) != fiducial
        });
        if self.configured_tool != Some(tool) || fiducial_changed {
            self.configured_tool = Some(tool);
            self.line_width.configure(line, line_min, steps::GENERIC);
            self.size.configure(size, size_min, size_steps);
            self.drill
                .configure(drill, Length::new(1), steps::DRILL_DIAMETER);
        } else {
            self.line_width.set_value(line);
            self.size.set_value(size);
            self.drill.set_value(drill);
        }
    }

    fn handle_request(&mut self, request: LibraryRequest<FootprintItem>, update: &mut TabUpdate) {
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
                update.requests.push(TabRequest::LibraryItemProperties(
                    LibraryItemRef::Footprint(self.fsm.footprint(), item),
                ));
            }
            LibraryRequest::ContextMenu {
                item,
                pos,
                vertices,
                segment,
            } => {
                with_clipboard(ensure_opened);
                let paste_geometry = self.run(|f, c| f.can_paste_geometry(c));
                let (actions, entries) = context_menu_entries(
                    true,
                    !vertices.is_empty(),
                    segment.is_some(),
                    true,
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
            LibraryRequest::CourtyardOffsetDialog => {
                update.requests.push(TabRequest::CourtyardOffsetDialog);
            }
            LibraryRequest::ImportPinsDialog => {}
        }
    }

    /// Imports a DXF file into the current footprint with the choices of
    /// the import dialog (the answer of [`TabRequest::ImportDxf`], upstream
    /// `PackageEditorFsm::processStartDxfImport()`: in the select tool).
    pub fn import_dxf(
        &mut self,
        settings: librepcb_editor::fsm::board::DxfImportSettings,
    ) -> TabUpdate {
        self.run(|f, c| f.select_tool(c) && f.import_dxf(c, &settings));
        self.after_fsm()
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
                    let vertices: BTreeSet<usize> = vertices.into_iter().collect();
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
            A::FlipHorizontal => {
                self.run(|f, c| f.flip(c, Orientation::Horizontal));
            }
            A::MirrorHorizontal => {
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

    /// Executes a package command, reporting errors.
    fn execute<C>(&mut self, cmd: C) -> Option<C::Output>
    where
        C: librepcb_editor::library_editor::ElementCommand<Package>,
    {
        match self.core.editor.execute(cmd) {
            Ok(output) => Some(output),
            Err(e) => {
                self.pending_errors.push(e.to_string());
                None
            }
        }
    }

    /// Applies an object modified in a properties dialog.
    pub fn update_object(&mut self, footprint: Option<Uuid>, object: FootprintObject) -> TabUpdate {
        if let Some(footprint) = footprint.or(self.fsm.footprint()) {
            self.execute(UpdateFootprintObject { footprint, object });
        }
        self.finish()
    }

    /// Generates the courtyard with the offset of the "courtyard excess"
    /// dialog.
    pub fn generate_courtyard(&mut self, offset: PositiveLength) -> TabUpdate {
        self.run(|f, c| f.generate_courtyard(c, Some(offset)));
        self.after_fsm()
    }

    /// Moves the selected items to the positions of the "move/align"
    /// dialog.
    pub fn move_align(&mut self, positions: &[librepcb_core::types::Point]) -> TabUpdate {
        self.run(|f, c| f.move_align(c, positions));
        self.after_fsm()
    }

    /// A STEP file was chosen for a new 3D model (`model == None`, upstream
    /// `PackageModelListModel::add()`) or to replace a model's file
    /// ("browse").
    pub fn step_file_chosen(
        &mut self,
        model: Option<Uuid>,
        file_stem: &str,
        content: Vec<u8>,
    ) -> TabUpdate {
        match model {
            Some(model) => {
                self.execute(EditPackageModel {
                    model,
                    name: None,
                    step: Some(content),
                });
            }
            None => {
                // A unique name from the file name.
                let base = match ElementName::clean(file_stem) {
                    s if s.is_empty() => "Model".to_owned(),
                    s => s,
                };
                let package = self.core.editor.element();
                let mut number = 1;
                let name = loop {
                    let name = if number > 1 {
                        format!("{base} ({number})")
                    } else {
                        base.clone()
                    };
                    if !package.models().iter().any(|m| **m.name() == name) {
                        break name;
                    }
                    number += 1;
                };
                match ElementName::new(name) {
                    Ok(name) => {
                        if let Some(uuid) = self.execute(AddPackageModel {
                            name,
                            step: content,
                            add_to_footprints: true,
                        }) {
                            self.current_model = Some(uuid);
                        }
                    }
                    Err(e) => self.pending_errors.push(e.to_string()),
                }
            }
        }
        self.finish()
    }

    /// Aborts the current tool (e.g. before closing).
    pub fn abort_tool(&mut self) -> TabUpdate {
        self.abort_group();
        self.after_fsm()
    }

    fn abort_group(&mut self) {
        for _ in 0..3 {
            if !self.core.editor.is_group_active() {
                break;
            }
            self.run(|f, c| f.abort(c));
        }
    }

    /// `after_fsm()` with the errors of executed commands.
    fn finish(&mut self) -> TabUpdate {
        let mut update = self.after_fsm();
        for e in std::mem::take(&mut self.pending_errors) {
            update.requests.push(error_notification(e));
        }
        update
            .requests
            .extend(std::mem::take(&mut self.pending_requests));
        update
    }

    // --- Lists ---

    /// A row of a list model was written by the UI.
    pub fn row_written(&mut self, event: PackageRowEvent) -> TabUpdate {
        if !self.core.is_writable() {
            self.pads_key.clear();
            self.models_key.clear();
            self.footprints_key = None;
            self.refresh_package_data();
            return TabUpdate {
                data_changed: true,
                ..TabUpdate::default()
            };
        }
        match event {
            PackageRowEvent::Pad(row, data) => self.pad_row_written(row, data),
            PackageRowEvent::Footprint(row, data) => self.footprint_row_written(row, data),
            PackageRowEvent::FootprintModel {
                footprint,
                model,
                checked,
            } => {
                let package = self.core.editor.element();
                let (Some(fpt), Some(model)) = (
                    package.footprints().iter().nth(footprint),
                    package.models().iter().nth(model),
                ) else {
                    return TabUpdate::default();
                };
                let mut models = fpt.models().clone();
                if checked {
                    models.insert(model.uuid());
                } else {
                    models.remove(&model.uuid());
                }
                if models != *fpt.models() {
                    let footprint = fpt.uuid();
                    self.execute(EditFootprint {
                        footprint,
                        name: None,
                        tags: None,
                        model_position: None,
                        model_rotation: None,
                        models: Some(models),
                    });
                }
            }
            PackageRowEvent::Model(row, data) => self.model_row_written(row, data),
        }
        self.finish()
    }

    fn pad_row_written(&mut self, row: usize, data: ui::PackagePadData) {
        let Some(uuid) = self.pad_rows.get(row).copied() else {
            return;
        };
        if data.delete {
            self.abort_group();
            self.execute(RemovePackagePad { pad: uuid });
            return;
        }
        // Keep the edited name until "apply" (upstream
        // `PackagePadListModel::set_row_data()`).
        let package = self.core.editor.element();
        let Some(pad) = package.pads().by_uuid(&uuid) else {
            return;
        };
        let name = data.name.to_string();
        let duplicate = name != **pad.name()
            && package
                .pads()
                .contains_name(&CircuitIdentifier::clean(&name));
        let mut error = String::new();
        validation::circuit_identifier(&name, &mut error, duplicate);
        self.pads.set(
            row,
            ui::PackagePadData {
                name_error: error.into(),
                delete: false,
                ..data
            },
        );
    }

    fn footprint_row_written(&mut self, row: usize, data: ui::FootprintData) {
        let package = self.core.editor.element();
        let Some(fpt) = package.footprints().iter().nth(row) else {
            return;
        };
        let footprint = fpt.uuid();
        match data.action {
            ui::FootprintAction::MoveUp => {
                self.execute(MoveFootprintUp { footprint });
                return;
            }
            ui::FootprintAction::Duplicate => {
                self.abort_group();
                if let Some(copy) = self.execute(DuplicateFootprint { footprint }) {
                    self.run(|f, c| f.set_footprint(c, Some(copy)));
                }
                return;
            }
            ui::FootprintAction::Delete => {
                self.abort_group();
                self.execute(RemoveFootprint { footprint });
                return;
            }
            ui::FootprintAction::None => {}
        }
        let mut cmd = EditFootprint {
            footprint,
            name: None,
            tags: None,
            model_position: None,
            model_rotation: None,
            models: None,
        };
        let name = data.name.to_string();
        if name != **fpt.names().default_value() {
            let cleaned = ElementName::clean(&name);
            if package
                .footprints()
                .iter()
                .any(|f| **f.names().default_value() == cleaned)
            {
                self.pending_errors.push(tr!(
                    "librepcb::editor::FootprintListModel",
                    "There is already a footprint with the name \"{0}\".",
                    cleaned
                ));
            } else {
                match ElementName::new(cleaned) {
                    Ok(n) => cmd.name = Some(n),
                    Err(e) => self.pending_errors.push(e.to_string()),
                }
            }
        }
        let pos = (
            length_from_ui(data.model_x.clone()),
            length_from_ui(data.model_y.clone()),
            length_from_ui(data.model_z.clone()),
        );
        if pos != fpt.model_position() {
            cmd.model_position = Some(pos);
        }
        let rot = (
            Angle::new(data.model_rx),
            Angle::new(data.model_ry),
            Angle::new(data.model_rz),
        );
        if rot != fpt.model_rotation() {
            cmd.model_rotation = Some(rot);
        }
        let mut tags = fpt.tags().clone();
        if !data.new_tag.is_empty() {
            // Suggested tags have a description in parentheses.
            let tag = data.new_tag.split('(').next().unwrap_or_default();
            if let Ok(tag) = Tag::new(Tag::clean(tag)) {
                tags.insert(tag);
            }
        }
        if !data.remove_tag.is_empty()
            && let Ok(tag) = Tag::new(data.remove_tag.to_string())
        {
            tags.remove(&tag);
        }
        if tags != *fpt.tags() {
            cmd.tags = Some(tags);
        }
        if cmd.name.is_some()
            || cmd.model_position.is_some()
            || cmd.model_rotation.is_some()
            || cmd.tags.is_some()
        {
            self.execute(cmd);
        } else {
            // Reset the actions/tags of the row.
            self.footprints_key = None;
        }
    }

    fn model_row_written(&mut self, row: usize, data: ui::PackageModelData) {
        let package = self.core.editor.element();
        let Some(model) = package.models().iter().nth(row) else {
            return;
        };
        let uuid = model.uuid();
        match data.action {
            ui::PackageModelAction::MoveUp => {
                self.execute(MovePackageModelUp { model: uuid });
            }
            ui::PackageModelAction::Browse => {
                self.models_key.clear();
                self.pending_requests
                    .push(TabRequest::ChooseStepFile { model: Some(uuid) });
            }
            ui::PackageModelAction::Delete => {
                self.execute(RemovePackageModel { model: uuid });
            }
            ui::PackageModelAction::None => {
                let name = ElementName::clean(&data.name);
                if name != **model.name() {
                    if package.models().iter().any(|m| **m.name() == name) {
                        self.pending_errors.push(tr!(
                            "librepcb::editor::PackageModelListModel",
                            "There is already a 3D model with the name \"{0}\".",
                            name
                        ));
                    } else {
                        match ElementName::new(name) {
                            Ok(name) => {
                                self.execute(EditPackageModel {
                                    model: uuid,
                                    name: Some(name),
                                    step: None,
                                });
                            }
                            Err(e) => self.pending_errors.push(e.to_string()),
                        }
                    }
                }
            }
        }
    }

    /// Applies the package properties and the edited pad names (upstream
    /// `commitUiData()` after the metadata).
    fn commit_package(&mut self) -> Vec<TabRequest> {
        if self.core.editor.is_group_active() {
            return Vec::new();
        }
        let mut requests: Vec<TabRequest> = self.core.commit_metadata().into_iter().collect();
        let package = self.core.editor.element();
        let mut cmd = EditPackage::default();
        if self.assembly_type != package.assembly_type() {
            cmd.assembly_type = Some(self.assembly_type);
        }
        if let Ok(c) = UnsignedLength::new(self.min_copper_clearance.value())
            && c != package.min_copper_clearance()
        {
            cmd.min_copper_clearance = Some(c);
        }
        let mut renames = Vec::new();
        for (row, uuid) in self.pad_rows.iter().enumerate() {
            let (Some(data), Some(pad)) = (self.pads.get(row), package.pads().by_uuid(uuid)) else {
                continue;
            };
            if data.name.as_str() != &**pad.name()
                && data.name_error.is_empty()
                && let Ok(name) = CircuitIdentifier::new(CircuitIdentifier::clean(&data.name))
            {
                renames.push(RenamePackagePad { pad: *uuid, name });
            }
        }
        if cmd != EditPackage::default() {
            self.execute(cmd);
        }
        for rename in renames {
            self.execute(rename);
        }
        // Reset edited pad names (upstream `apply()`).
        self.pads_key.clear();
        self.core.refresh();
        self.core.run_checks_if_modified();
        self.refresh_package_data();
        requests.extend(
            std::mem::take(&mut self.pending_errors)
                .into_iter()
                .map(error_notification),
        );
        requests
    }

    // --- UI data ---

    /// The `TabData`.
    pub fn ui_data(&self) -> ui::TabData {
        let mut data = self.core.tab_data();
        let writable = self.core.is_writable();
        let f = self.fsm.view_state().features;
        if !self.core.wizard_mode
            && self.core.page_index == EDITOR_PAGE
            && self.fsm.footprint().is_some()
        {
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
            data.features.flip = edit(f.flip);
            data.features.move_align = edit(f.rotate && !self.fsm.selection().is_empty());
            data.features.snap_to_grid = edit(f.snap_to_grid);
            data.features.edit_properties = feature(f.properties);
            data.features.import_graphics = edit(f.import_graphics);
        }
        data
    }

    /// The `PackageTabData`.
    pub fn derived_ui_data(&self) -> ui::PackageTabData {
        let m = &self.core.meta;
        let vs = self.fsm.view_state();
        let d = self.fsm.tool_data();
        let layers = sorted_layers(&d.available_layers);
        let tool = self.fsm.tool();
        let package = self.core.editor.element();
        let background = self.scheme.color_or_transparent("board_background");
        let info = self.scheme.color_or_transparent("board_info_box");
        let fiducial = self.is_fiducial_tool();
        let (pad_items, pad_uuids): (Vec<slint::SharedString>, Vec<Option<Uuid>>) = {
            let mut items = vec![(
                tr!("librepcb::editor::PackageTab", "(unconnected)").into(),
                None,
            )];
            if !fiducial {
                items.extend(
                    package
                        .pads()
                        .iter()
                        .map(|p| (p.name().to_string().into(), Some(p.uuid()))),
                );
            }
            items.into_iter().unzip()
        };
        let ratio = d.pad_radius.get().to_ppm();
        let footprint_index = self
            .fsm
            .footprint()
            .and_then(|u| package.footprints().index_of_uuid(&u))
            .map_or(-1, |i| i as i32);
        let model_index = self
            .current_model
            .and_then(|u| package.models().index_of_uuid(&u))
            .map_or(-1, |i| i as i32);
        ui::PackageTabData {
            library_index: self.library_index,
            path: self.core.directory_path().as_str().into(),
            wizard_mode: self.core.wizard_mode,
            page_index: self.core.page_index,
            view_3d: false,
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
            assembly_type: assembly_type_to_ui(self.assembly_type),
            min_copper_clearance: self.min_copper_clearance.ui_data(),
            pads: model_rc(&self.pads),
            new_pad_name: self.new_pad_name.as_str().into(),
            new_pad_name_error: self.new_pad_name_error.as_str().into(),
            footprints: model_rc(&self.footprints),
            footprint_index,
            models: model_rc(&self.models),
            model_index,
            checks: self.core.checks_data(),
            background_color: color_to_ui(background).into(),
            foreground_color: slint::Color::from_rgb_u8(0xff, 0xff, 0xff).into(),
            overlay_color: color_to_ui(info).into(),
            overlay_text_color: slint::Color::from_rgb_u8(0xff, 0xff, 0xff).into(),
            grid_style: grid_style_to_ui(self.grid_style),
            grid_interval: length_to_ui(*package.grid_interval()),
            unit: unit_to_ui(self.unit),
            background_image_set: false,
            background_image_alpha: 0.0,
            solderresist_alpha: 1.0,
            silkscreen_alpha: 1.0,
            solderpaste_alpha: 1.0,
            devices_alpha: 1.0,
            refreshing: false,
            opengl_error: Default::default(),
            files_modified: false,
            interface_broken_msg: self.core.is_interface_broken(),
            element_duplicated_msg: self.core.element_duplicated && !m.deprecated,
            tool: tool_to_ui(tool),
            tool_cursor: mouse_cursor(vs.cursor, self.canvas.is_panning()),
            tool_overlay_text: info_box_text(&vs.info_box).into(),
            tool_layer: ui::ComboBoxData {
                items: vec_model(layers.iter().map(|l| l.name_tr().into()).collect()),
                current_index: layers
                    .iter()
                    .position(|l| *l == d.layer)
                    .map_or(-1, |i| i as i32),
            },
            tool_line_width: self.line_width.ui_data(),
            tool_size: self.size.ui_data(),
            tool_drill: self.drill.ui_data(),
            tool_angle: ui::AngleEditData {
                value: d.angle.to_micro_deg(),
                increase: false,
                decrease: false,
            },
            tool_ratio: ui::RatioEditData {
                value: ratio,
                minimum: 0,
                maximum: 1_000_000,
                can_increase: ratio < 1_000_000,
                can_decrease: ratio > 0,
                increase: false,
                decrease: false,
            },
            tool_filled: d.filled,
            tool_grab_area: d.grab_area,
            tool_value: ui::LineEditData {
                enabled: true,
                text: to_single_line(&d.text).into(),
                placeholder: Default::default(),
                suggestions: vec_model(
                    d.text_suggestions
                        .iter()
                        .map(|s| s.as_str().into())
                        .collect(),
                ),
            },
            tool_halign: halign_to_ui(d.h_align),
            tool_valign: valign_to_ui(d.v_align),
            tool_package_pad: ui::ComboBoxData {
                items: vec_model(pad_items),
                current_index: pad_uuids
                    .iter()
                    .position(|u| *u == d.package_pad)
                    .map_or(-1, |i| i as i32),
            },
            tool_bottom: d.component_side == ComponentSide::Bottom,
            tool_shape: pad_shape_to_ui(d.pad_shape, d.pad_radius),
            tool_fiducial: fiducial,
            tool_pressfit: d.pad_function == PadFunction::PressFitPad,
            tool_layer_top: d.zone_layers.contains(ZoneLayers::TOP),
            tool_layer_inner: d.zone_layers.contains(ZoneLayers::INNER),
            tool_layer_bottom: d.zone_layers.contains(ZoneLayers::BOTTOM),
            tool_no_copper: d.zone_rules.contains(ZoneRules::NO_COPPER),
            tool_no_planes: d.zone_rules.contains(ZoneRules::NO_PLANES),
            tool_no_exposures: d.zone_rules.contains(ZoneRules::NO_EXPOSURE),
            tool_no_devices: d.zone_rules.contains(ZoneRules::NO_DEVICES),
            scene_image_pos: self.scene_image_pos,
            frame: self.frame,
            new_category: Default::default(),
            new_footprint: Default::default(),
        }
    }

    /// Sets the index of the library in `Data.libraries`.
    pub fn set_library_index(&mut self, index: i32) {
        self.library_index = index;
    }

    /// Applies data written by the UI (upstream `setDerivedUiData()`).
    pub fn set_derived_ui_data(&mut self, data: &ui::PackageTabData) -> TabUpdate {
        let mut update = TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        };
        let package = self.core.editor.element();
        // Current model and footprint.
        if let Some(model) = usize::try_from(data.model_index)
            .ok()
            .and_then(|i| package.models().iter().nth(i))
        {
            self.current_model = Some(model.uuid());
        }
        let footprint = usize::try_from(data.footprint_index)
            .ok()
            .and_then(|i| package.footprints().iter().nth(i))
            .map(|f| f.uuid());
        if footprint.is_some() && footprint != self.fsm.footprint() {
            self.run(|f, c| f.set_footprint(c, footprint));
            update.repaint = true;
        }
        self.core.page_index = data.page_index;
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
        if let Some(t) = assembly_type_from_ui(data.assembly_type) {
            self.assembly_type = t;
        }
        self.min_copper_clearance
            .set_ui_data(&data.min_copper_clearance);

        // New pad name (upstream: validated while typing).
        if data.new_pad_name.as_str() != self.new_pad_name {
            self.new_pad_name = data.new_pad_name.to_string();
            let names = toolbox::expand_ranges_in_string(&self.new_pad_name);
            let package = self.core.editor.element();
            let duplicate = names
                .iter()
                .any(|n| package.pads().contains_name(&CircuitIdentifier::clean(n)));
            if self.new_pad_name.trim().is_empty() {
                self.new_pad_name_error.clear();
            } else {
                validation::circuit_identifier(
                    names.first().map_or("", String::as_str),
                    &mut self.new_pad_name_error,
                    duplicate,
                );
            }
        }
        // New footprint.
        if !data.new_footprint.is_empty() {
            self.add_footprint(&data.new_footprint);
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
        if data.deprecated || !data.element_duplicated_msg {
            self.core.element_duplicated = false;
        }

        // Tool bar.
        if self.core.is_writable() {
            self.set_tool_data_from_ui(data);
        }
        let after = self.finish();
        update.repaint |= after.repaint;
        update.requests.extend(after.requests);
        update
    }

    /// Upstream `FootprintListModel::add()`.
    fn add_footprint(&mut self, name: &str) {
        let name = ElementName::clean(name);
        let package = self.core.editor.element();
        if package
            .footprints()
            .iter()
            .any(|f| **f.names().default_value() == name)
        {
            self.pending_errors.push(tr!(
                "librepcb::editor::FootprintListModel",
                "There is already a footprint with the name \"{0}\".",
                name
            ));
            return;
        }
        match ElementName::new(name) {
            Ok(name) => {
                self.abort_group();
                if let Some(uuid) = self.execute(AddFootprint { name }) {
                    self.run(|f, c| f.set_footprint(c, Some(uuid)));
                }
            }
            Err(e) => self.pending_errors.push(e.to_string()),
        }
    }

    fn set_tool_data_from_ui(&mut self, data: &ui::PackageTabData) {
        let current = self.fsm.tool_data().clone();
        let tool = self.fsm.tool();
        let fiducial = self.is_fiducial_tool();
        let layers = sorted_layers(&current.available_layers);
        let mut d = current.clone();
        if let Some(layer) = usize::try_from(data.tool_layer.current_index)
            .ok()
            .and_then(|i| layers.get(i))
        {
            d.layer = *layer;
        }
        d.angle = if data.tool_angle.increase {
            current.angle + Angle::DEG45
        } else if data.tool_angle.decrease {
            current.angle + -Angle::DEG45
        } else {
            Angle::new(data.tool_angle.value)
        };
        d.filled = data.tool_filled;
        d.grab_area = data.tool_grab_area;
        d.text = to_multi_line(&data.tool_value.text);
        d.h_align = halign_from_ui(data.tool_halign);
        d.v_align = valign_from_ui(data.tool_valign);
        // Note: the drill is set before width/height (upstream order).
        let drill = self.drill.set_ui_data(&data.tool_drill);
        let line_width = self.line_width.set_ui_data(&data.tool_line_width);
        let size = self.size.set_ui_data(&data.tool_size);
        match tool {
            LibraryTool::AddThtPads | LibraryTool::AddSmtPads(_) => {
                if let Some(v) = drill.and_then(|v| PositiveLength::new(v).ok()) {
                    d.drill = v;
                }
                if let Some(w) = line_width.and_then(|v| PositiveLength::new(v).ok()) {
                    d.pad_width = w;
                    if fiducial {
                        d.pad_height = w;
                    }
                }
                if let Some(s) = size {
                    if fiducial {
                        if let Ok(c) = UnsignedLength::new(s) {
                            d.copper_clearance = c;
                            d.stop_mask = MaskConfig::Manual(s);
                        }
                    } else if let Ok(h) = PositiveLength::new(s) {
                        d.pad_height = h;
                    }
                }
                let mut pads: Vec<Option<Uuid>> = vec![None];
                if !fiducial {
                    pads.extend(
                        self.core
                            .editor
                            .element()
                            .pads()
                            .iter()
                            .map(|p| Some(p.uuid())),
                    );
                }
                if let Some(pad) = usize::try_from(data.tool_package_pad.current_index)
                    .ok()
                    .and_then(|i| pads.get(i))
                {
                    d.package_pad = *pad;
                }
                if matches!(tool, LibraryTool::AddSmtPads(_)) {
                    d.component_side = if data.tool_bottom {
                        ComponentSide::Bottom
                    } else {
                        ComponentSide::Top
                    };
                }
                if data.tool_shape != pad_shape_to_ui(current.pad_shape, current.pad_radius) {
                    let (shape, radius) = match data.tool_shape {
                        ui::PadShape::Round => (PadShape::RoundedRect, percent(100)),
                        ui::PadShape::RoundedRect => (
                            PadShape::RoundedRect,
                            Pad::recommended_radius(d.pad_width, d.pad_height),
                        ),
                        ui::PadShape::Rect => (PadShape::RoundedRect, percent(0)),
                        ui::PadShape::Octagon => (PadShape::RoundedOctagon, percent(0)),
                    };
                    d.pad_shape = shape;
                    d.pad_radius = radius;
                } else {
                    let radius = current.pad_radius.get().to_ppm();
                    let step = Ratio::from_percent(1).to_ppm();
                    let new_radius = if data.tool_ratio.increase {
                        (radius + step).min(1_000_000)
                    } else if data.tool_ratio.decrease {
                        (radius - step).max(0)
                    } else {
                        data.tool_ratio.value
                    };
                    if let Ok(r) = UnsignedLimitedRatio::new(Ratio::new(new_radius)) {
                        d.pad_radius = r;
                    }
                }
                if tool == LibraryTool::AddThtPads {
                    d.pad_function = if data.tool_pressfit {
                        PadFunction::PressFitPad
                    } else {
                        PadFunction::StandardPad
                    };
                }
            }
            LibraryTool::AddHoles => {
                if let Some(v) = drill.and_then(|v| PositiveLength::new(v).ok()) {
                    d.drill = v;
                }
            }
            LibraryTool::DrawZone => {
                let mut layers = ZoneLayers::empty();
                layers.set(ZoneLayers::TOP, data.tool_layer_top);
                layers.set(ZoneLayers::INNER, data.tool_layer_inner);
                layers.set(ZoneLayers::BOTTOM, data.tool_layer_bottom);
                d.zone_layers = layers;
                let mut rules = ZoneRules::empty();
                rules.set(ZoneRules::NO_COPPER, data.tool_no_copper);
                rules.set(ZoneRules::NO_PLANES, data.tool_no_planes);
                rules.set(ZoneRules::NO_EXPOSURE, data.tool_no_exposures);
                rules.set(ZoneRules::NO_DEVICES, data.tool_no_devices);
                d.zone_rules = rules;
            }
            LibraryTool::AddNames | LibraryTool::AddValues | LibraryTool::DrawText => {
                if let Some(h) = size.and_then(|v| PositiveLength::new(v).ok()) {
                    d.height = h;
                }
                if let Some(w) = line_width.and_then(|v| UnsignedLength::new(v).ok()) {
                    d.stroke_width = w;
                }
            }
            _ => {
                if let Some(w) = line_width.and_then(|v| UnsignedLength::new(v).ok()) {
                    d.line_width = w;
                }
            }
        }
        if d != current {
            self.run(|f, c| f.update_tool_data(c, |t| *t = d));
        }
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
        use librepcb_editor::library_editor::checks::FixRequest;
        let (request, mut update) = self.core.check_row_written(row, data);
        match request {
            Some(FixRequest::StartTool { footprint, tool }) => {
                self.core.page_index = EDITOR_PAGE;
                if footprint.is_some() {
                    self.run(|f, c| f.set_footprint(c, footprint));
                }
                self.run(|f, c| f.set_tool(c, tool));
            }
            Some(FixRequest::ChooseCategories) => {
                self.core.page_index = 0;
                self.core.meta.choose_category = true;
            }
            Some(FixRequest::ShowModels { footprint }) => {
                self.run(|f, c| f.set_footprint(c, Some(footprint)));
                self.core.page_index = EDITOR_PAGE;
            }
            _ => {}
        }
        let after = self.after_fsm();
        update.repaint |= after.repaint;
        update.requests.extend(after.requests);
        update
    }

    // --- Actions ---

    /// Handles a tab action (upstream `PackageTab::trigger()`).
    pub fn trigger(&mut self, action: ui::TabAction) -> TabUpdate {
        use ui::TabAction as A;
        let mut extra = TabUpdate::default();
        if matches!(
            action,
            A::Apply | A::Save | A::Undo | A::Redo | A::Next | A::Unlock | A::SaveAs
        ) {
            // Abort a tool which keeps an undo group open.
            self.abort_group();
            if !matches!(action, A::Unlock) {
                extra.requests.extend(self.commit_package());
            }
        }
        if matches!(action, A::Back) && self.core.wizard_mode && self.core.page_index > 0 {
            self.core.page_index -= 1;
            return TabUpdate {
                data_changed: true,
                ..TabUpdate::default()
            };
        }
        if let Some(mut update) = self.core.trigger_common(action) {
            self.scene_state = None;
            self.pads_key.clear();
            let after = self.after_fsm();
            update.repaint = true;
            extra.requests.extend(update.requests);
            update.requests = extra.requests;
            update.requests.extend(after.requests);
            return update;
        }
        let grid = self.core.editor.element().grid_interval().get();
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
            A::Accept => {
                self.run(|f, c| f.accept(c));
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
            A::FlipHorizontally => {
                self.run(|f, c| f.flip(c, Orientation::Horizontal));
            }
            A::FlipVertically => {
                self.run(|f, c| f.flip(c, Orientation::Vertical));
            }
            A::MoveAlign => {
                if let Some(positions) = self.run(|f, c| f.move_align_positions(c)) {
                    extra.requests.push(TabRequest::MoveAlign { positions });
                }
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
            A::PackageAddPads => {
                let names = self.new_pad_name.clone();
                if self.execute(AddPackagePads { names }).is_some() {
                    self.new_pad_name.clear();
                    self.new_pad_name_error.clear();
                }
            }
            A::PackageAddModel => {
                extra
                    .requests
                    .push(TabRequest::ChooseStepFile { model: None });
            }
            A::PackageGenerateOutline => {
                self.run(|f, c| f.generate_outline(c));
            }
            A::PackageGenerateCourtyard => {
                self.run(|f, c| f.generate_courtyard(c, None));
            }
            A::ToolSelect => {
                self.run(|f, c| f.select_tool(c));
            }
            A::ToolLine
            | A::ToolRect
            | A::ToolPolygon
            | A::ToolCircle
            | A::ToolArc
            | A::ToolName
            | A::ToolValue
            | A::ToolText
            | A::ToolPadTht
            | A::ToolPadSmt
            | A::ToolPadThermal
            | A::ToolPadBga
            | A::ToolPadEdgeConnector
            | A::ToolPadTestPoint
            | A::ToolPadLocalFiducial
            | A::ToolPadGlobalFiducial
            | A::ToolZone
            | A::ToolHole
            | A::ToolRenumberPads
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
                    A::ToolPadTht => LibraryTool::AddThtPads,
                    A::ToolPadSmt => LibraryTool::AddSmtPads(PadFunction::StandardPad),
                    A::ToolPadThermal => LibraryTool::AddSmtPads(PadFunction::ThermalPad),
                    A::ToolPadBga => LibraryTool::AddSmtPads(PadFunction::BgaPad),
                    A::ToolPadEdgeConnector => {
                        LibraryTool::AddSmtPads(PadFunction::EdgeConnectorPad)
                    }
                    A::ToolPadTestPoint => LibraryTool::AddSmtPads(PadFunction::TestPad),
                    A::ToolPadLocalFiducial => LibraryTool::AddSmtPads(PadFunction::LocalFiducial),
                    A::ToolPadGlobalFiducial => {
                        LibraryTool::AddSmtPads(PadFunction::GlobalFiducial)
                    }
                    A::ToolZone => LibraryTool::DrawZone,
                    A::ToolHole => LibraryTool::AddHoles,
                    A::ToolRenumberPads => LibraryTool::RenumberPads,
                    _ => LibraryTool::Measure,
                };
                self.run(|f, c| f.set_tool(c, tool));
            }
            A::SaveAs => {
                extra.requests.push(TabRequest::DuplicateLibraryElement);
            }
            A::ImportDxf => {
                // Upstream `processStartDxfImport()` opens the file chooser
                // and the import dialog; here the application does and
                // passes the choice to `import_dxf()`.
                if self.core.is_writable() && self.fsm.footprint().is_some() {
                    extra.requests.push(TabRequest::ImportDxf {
                        layers: <PackageHost as ElementHost>::polygon_layers(),
                        kind: DxfImportKind::Package,
                    });
                }
            }
            A::ExportPdf
            | A::ExportImage
            | A::Print
            | A::ToggleBackgroundImage
            | A::ReloadFromDisk => {
                extra.status = Some(tr!(
                    "librepcb::editor::MainWindow",
                    "Not available yet in this version: {0}",
                    format!("{action:?}")
                ));
            }
            _ => return extra,
        }
        let mut update = self.finish();
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

    /// The pad rows (tests): UUIDs in the order of the `pads` model.
    pub fn pad_rows(&self) -> &[Uuid] {
        &self.pad_rows
    }

    /// Selects the footprint (tests and the footprint list).
    pub fn select_footprint(&mut self, footprint: Option<Uuid>) -> TabUpdate {
        self.run(|f, c| f.set_footprint(c, footprint));
        self.after_fsm()
    }

    /// The package pads connected by footprint pads (tests).
    pub fn connected_pads(&self) -> BTreeMap<Uuid, Option<Uuid>> {
        let package = self.core.editor.element();
        self.fsm
            .footprint()
            .and_then(|u| package.footprints().by_uuid(&u))
            .map(|f| {
                f.pads()
                    .iter()
                    .map(|p| (p.uuid(), p.package_pad_uuid()))
                    .collect()
            })
            .unwrap_or_default()
    }
}
