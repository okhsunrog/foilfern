//! The schematic editor state machine: port of
//! libs/librepcb/editor/project/schematic/fsm/schematiceditorfsm.{h,cpp},
//! schematiceditorfsmadapter.h and schematiceditorstate.{h,cpp}, with the
//! states in the submodules (`select`, `draw_wire`, `add_label`,
//! `add_component`, `add_text`, `draw_polygon`, `measure`).
//!
//! Usage: the application creates one [`SchematicEditorFsm`] per schematic
//! tab and calls its methods with a [`SchematicContext`] (the project
//! editor, the view used for hit testing and the clipboard). Afterwards it
//! syncs its scene with the project's change journal, reflects
//! [`selection()`](SchematicEditorFsm::selection) in the scene and shows
//! [`view_state()`](SchematicEditorFsm::view_state) and
//! [`tool_data()`](SchematicEditorFsm::tool_data); dialogs are requested
//! through [`take_requests()`](SchematicEditorFsm::take_requests).
//!
//! Differences to upstream:
//!
//! - The FSM owns the selection (a set of [`SchematicItem`]s) instead of
//!   the graphics items; the view only answers hit tests
//!   ([`SchematicView`]). Junctions are hit tested on the model (upstream:
//!   invisible graphics items), so the view does not need to report them.
//! - Interactive operations keep an undo group open and replace their live
//!   preview inside it (`ProjectEditor::rollback_group_to()`) instead of
//!   modifying the model without undo commands (`immediate` edits).
//! - Error message boxes become [`SchematicRequest::ShowError`], the
//!   context menu becomes [`SchematicRequest::ContextMenu`].
//! - Cross-probing is reported through
//!   [`cross_probe()`](SchematicEditorFsm::cross_probe); the application
//!   forwards it to the board editors.

mod add_component;
mod add_image;
mod add_label;
mod add_text;
pub mod clipboard;
mod drag;
mod draw_bus;
mod draw_polygon;
mod draw_wire;
mod find;
mod hit_test;
mod measure;
mod select;
mod selection;
mod simplify;

use std::collections::BTreeSet;

use librepcb_core::project::schematic::Schematic;
use librepcb_core::project::{
    BusId, BusSegmentId, ComponentInstanceId, NetSegmentId, NetSignalId, Project, SchematicId,
    SymbolId,
};
use librepcb_core::types::{
    Angle, Layer, Length, LengthUnit, Orientation, Point, PositiveLength, UnsignedLength, Uuid,
};

use super::{Clipboard, CursorShape, KeyEvent, PointerEvent, ViewState};
use crate::ProjectEditor;
use crate::commands::WireMode;

pub use super::CrossProbe;
pub use clipboard::{SCHEMATIC_CLIPBOARD_MIME_PREFIX, SchematicClipboardData};
pub use simplify::SimplifySchematicSegments;

/// An item of a schematic page as the editor sees it (upstream: the
/// graphics items of `SchematicGraphicsScene`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SchematicItem {
    /// A symbol.
    Symbol(SymbolId),
    /// A pin of a symbol (library pin UUID).
    SymbolPin(SymbolId, Uuid),
    /// A text of a symbol (e.g. its name or value), selectable on its own.
    SymbolText(SymbolId, Uuid),
    /// A junction of a net segment (upstream `SI_NetPoint`).
    NetPoint(NetSegmentId, Uuid),
    /// A net line.
    NetLine(NetSegmentId, Uuid),
    /// A net label.
    NetLabel(NetSegmentId, Uuid),
    /// A junction of a bus segment.
    BusJunction(BusSegmentId, Uuid),
    /// A line of a bus segment.
    BusLine(BusSegmentId, Uuid),
    /// A label of a bus segment.
    BusLabel(BusSegmentId, Uuid),
    /// A polygon.
    Polygon(Uuid),
    /// A text of the page.
    Text(Uuid),
    /// An image.
    Image(Uuid),
}

/// What the FSM needs from the view: hit testing on the displayed scene
/// (upstream `SchematicGraphicsScene` and `fsmCalcPosWithTolerance()`).
///
/// The application implements it on its `librepcb_scene::SchematicScene`
/// and canvas view (mapping `SchematicObject::NetJunction(seg,
/// Junction(j))` to [`SchematicItem::NetPoint`] and `NetJunction(_, Pin
/// {..})` to [`SchematicItem::SymbolPin`]); the order of the returned items
/// does not matter.
pub trait SchematicView {
    /// All items whose grab area is at most `tolerance` away from `pos`
    /// (`tolerance == 0`: exactly under `pos`).
    fn items_at(&self, pos: Point, tolerance: Length) -> Vec<SchematicItem>;

    /// All items touching the rectangle spanned by `p1` and `p2`.
    fn items_in_rect(&self, p1: Point, p2: Point) -> Vec<SchematicItem>;

    /// The hit tolerance: upstream uses 5 screen pixels, so this is the
    /// world length of 5 pixels at the current zoom level (see
    /// [`tolerance_for_pixel_size()`](super::tolerance_for_pixel_size)).
    fn tolerance(&self) -> Length;

    /// The current cursor position in world coordinates, if known (used
    /// when a tool starts without pointer event, e.g. paste or add
    /// component; upstream `QCursor::pos()`).
    fn cursor_pos(&self) -> Option<Point> {
        None
    }
}

/// The context of an FSM call.
pub struct SchematicContext<'a> {
    /// The project editor (commands and undo groups).
    pub editor: &'a mut ProjectEditor,
    /// Hit testing on the view.
    pub view: &'a dyn SchematicView,
    /// The clipboard.
    pub clipboard: &'a mut dyn Clipboard,
}

impl<'a> SchematicContext<'a> {
    /// Creates a context.
    pub fn new(
        editor: &'a mut ProjectEditor,
        view: &'a dyn SchematicView,
        clipboard: &'a mut dyn Clipboard,
    ) -> Self {
        Self {
            editor,
            view,
            clipboard,
        }
    }
}

/// The active tool (upstream `SchematicEditorFsm::State`, Slint
/// `EditorTool`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SchematicTool {
    /// Select, move, rotate, ... (upstream `SchematicEditorState_Select`).
    #[default]
    Select,
    /// Draw wires.
    Wire,
    /// Draw buses.
    Bus,
    /// Add net labels.
    Label,
    /// Add components.
    Component,
    /// Draw polygons.
    Polygon,
    /// Add texts.
    Text,
    /// Add images.
    Image,
    /// Measure distances.
    Measure,
}

/// The tool bar data of the active tool (the `tool-*` fields of upstream's
/// `SchematicTabData`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchematicToolData {
    /// Wire mode (draw wire, draw bus).
    pub wire_mode: WireMode,
    /// Buses to choose from, sorted by name (draw bus).
    pub buses: Vec<(BusId, String)>,
    /// The bus of new bus segments (draw bus; `None`: a new bus).
    pub bus: Option<BusId>,
    /// Layer (draw polygon, add text).
    pub layer: Layer,
    /// Layers to choose from (draw polygon, add text).
    pub available_layers: Vec<Layer>,
    /// Line width (draw polygon).
    pub line_width: UnsignedLength,
    /// Text height (add text).
    pub size: PositiveLength,
    /// Whether polygons are filled (draw polygon).
    pub filled: bool,
    /// Text (add text) or value (add component).
    pub value: String,
    /// Suggestions for [`value`](Self::value).
    pub value_suggestions: Vec<String>,
    /// The first component attribute referenced by the value (add
    /// component; upstream `getValueAttribute*()`), editable in the tool
    /// bar (value and unit).
    pub value_attribute: Option<librepcb_core::attribute::Attribute>,
}

impl Default for SchematicToolData {
    fn default() -> Self {
        Self {
            wire_mode: WireMode::HV,
            buses: Vec::new(),
            bus: None,
            layer: Layer::SCHEMATIC_GUIDE,
            available_layers: Vec::new(),
            line_width: UnsignedLength::new(Length::new(300_000)).unwrap_or_default(),
            size: default_text_height(),
            filled: false,
            value: String::new(),
            value_suggestions: Vec::new(),
            value_attribute: None,
        }
    }
}

fn default_text_height() -> PositiveLength {
    // 2.5 mm is positive.
    PositiveLength::new(Length::new(2_500_000)).unwrap_or_else(|_| unreachable!())
}

/// A request of the FSM to the application (dialogs and menus upstream
/// opens itself).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchematicRequest {
    /// Show an error message (upstream `QMessageBox::critical()`).
    ShowError(String),
    /// Open the "add component" dialog; the application calls
    /// [`SchematicEditorFsm::add_component()`] with the choice.
    AddComponentDialog {
        /// Search term to preselect (e.g. "schematic frame").
        search_term: String,
    },
    /// Open the properties dialog of a symbol (component).
    SymbolProperties(SymbolId),
    /// Open the rename dialog of the net segment of a net label.
    NetLabelProperties(NetSegmentId, Uuid),
    /// Open the rename dialog of the bus segment of a bus label (upstream
    /// `RenameBusSegmentDialog`).
    BusLabelProperties(BusSegmentId, Uuid),
    /// Open the properties dialog of a text of a symbol.
    SymbolTextProperties(SymbolId, Uuid),
    /// Open the properties dialog of a polygon.
    PolygonProperties(Uuid),
    /// Open the properties dialog of a text.
    TextProperties(Uuid),
    /// Show the context menu for the (selected) item at `pos`; its actions
    /// map to the FSM methods (properties, cut, copy, remove, rotate,
    /// mirror, snap to grid, reset texts, place remaining gates).
    ContextMenu {
        /// The item under the cursor.
        item: SchematicItem,
        /// The position (world coordinates).
        pos: Point,
        /// Polygons: whether vertices are at `pos` which can be removed
        /// ("Remove Vertex", [`SchematicEditorFsm::remove_polygon_vertices()`]).
        remove_vertex: Option<bool>,
        /// Polygons: whether a line is at `pos` ("Add Vertex",
        /// [`SchematicEditorFsm::add_polygon_vertex()`]).
        add_vertex: bool,
    },
    /// Zoom to show all items (e.g. after adding a drawing frame).
    ZoomAll,
    /// Open the image chooser dialog; the application calls
    /// [`SchematicEditorFsm::add_image()`] with the chosen file.
    ChooseImageFile,
    /// Show the menu of the bus members when a wire starts or ends at a bus
    /// (upstream `determineNetForBusMember()`); the application calls
    /// [`SchematicEditorFsm::choose_bus_member()`] with the choice ("Add
    /// New Bus Member" is the default entry, "Cancel" is `None`).
    BusMemberMenu {
        /// The position (world coordinates).
        pos: Point,
        /// The nets of the bus.
        nets: Vec<BusMemberNet>,
    },
}

/// An image to add (upstream: the data, format and basename passed to
/// `processAddImage()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageData {
    /// The file content.
    pub data: Vec<u8>,
    /// The format (file extension: `png`, `jpg` or `svg`).
    pub format: String,
    /// The base name for the new file (e.g. of the chosen file; empty:
    /// `image`).
    pub basename: String,
}

/// The MIME types of images the FSM pastes from the clipboard, with their
/// format (upstream `ImageHelpers::getImageFromClipboard()`).
pub const CLIPBOARD_IMAGE_TYPES: [(&str, &str); 3] = [
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/svg+xml", "svg"),
];

/// A net of the bus member menu, see [`SchematicRequest::BusMemberMenu`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusMemberNet {
    /// The net.
    pub net: NetSignalId,
    /// Its name.
    pub name: String,
    /// Whether it can be chosen (anonymous nets cannot).
    pub enabled: bool,
}

/// The choice of the bus member menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusMemberChoice {
    /// "Add New Bus Member": a new net (with an automatic name).
    NewMember,
    /// An existing member of the bus.
    Net(NetSignalId),
}

/// Settings of the schematic editor FSM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchematicEditorSettings {
    /// Ignore placement locks of texts (upstream "ignore placement locks").
    pub ignore_locks: bool,
    /// Application version used in the clipboard MIME type (upstream
    /// `Application::getVersion()`; clipboard data is only exchanged
    /// between identical versions).
    pub app_version: String,
}

impl Default for SchematicEditorSettings {
    fn default() -> Self {
        Self {
            ignore_locks: false,
            app_version: "2.1.2-unstable".to_owned(),
        }
    }
}

/// A component to add (the choice of upstream's "add component" dialog or
/// tool bar buttons).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentChoice {
    /// The library component.
    pub component: Uuid,
    /// The symbol variant (default: by norm order).
    pub symbol_variant: Option<Uuid>,
    /// The library device for an assembly option (default: none).
    pub device: Option<Uuid>,
}

impl ComponentChoice {
    /// A component with default symbol variant and without device.
    pub fn new(component: Uuid) -> Self {
        Self {
            component,
            symbol_variant: None,
            device: None,
        }
    }
}

/// The outputs of the FSM, shared by all states.
#[derive(Debug, Default)]
pub(crate) struct Output {
    pub view: ViewState,
    pub selection: BTreeSet<SchematicItem>,
    pub hovered: Option<SchematicItem>,
    pub tool: SchematicTool,
    pub tool_data: SchematicToolData,
    pub requests: Vec<SchematicRequest>,
    /// A state asked to go back to the select state (upstream
    /// `requestLeavingState()`, processed after the call).
    pub leave_requested: bool,
    /// The last pointer position.
    pub last_pos: Option<Point>,
    /// The objects to cross-probe.
    pub cross_probe: CrossProbe,
}

/// What a state gets in its handlers.
pub(crate) struct Cx<'c, 'a> {
    pub ctx: &'c mut SchematicContext<'a>,
    pub out: &'c mut Output,
    pub schematic: SchematicId,
    pub settings: &'c SchematicEditorSettings,
}

impl Cx<'_, '_> {
    pub fn project(&self) -> &Project {
        self.ctx.editor.project()
    }

    pub fn sch(&self) -> Option<&Schematic> {
        self.ctx.editor.project().schematic(self.schematic)
    }

    /// Grid interval of the page (upstream `getGridInterval()`).
    pub fn grid(&self) -> PositiveLength {
        self.sch()
            .map(|s| s.properties().grid_interval)
            .unwrap_or_else(|| {
                PositiveLength::new(Length::new(2_540_000)).unwrap_or_else(|_| unreachable!())
            })
    }

    /// Length unit of the page (upstream `getLengthUnit()`).
    pub fn unit(&self) -> LengthUnit {
        self.sch()
            .map(|s| s.properties().grid_unit)
            .unwrap_or_default()
    }

    /// The cursor position (from the view, else the last pointer event).
    pub fn cursor_pos(&self) -> Point {
        self.ctx
            .view
            .cursor_pos()
            .or(self.out.last_pos)
            .unwrap_or_default()
    }

    /// Reports an error (upstream: message box).
    pub fn error(&mut self, e: impl std::fmt::Display) {
        let msg = e.to_string();
        log::warn!("Schematic editor: {msg}");
        self.out.requests.push(SchematicRequest::ShowError(msg));
    }

    pub fn set_cursor(&mut self, shape: Option<CursorShape>) {
        self.out.view.cursor = shape;
    }
}

/// The states of the FSM (upstream `SchematicEditorFsm::State`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StateKind {
    Idle,
    Select,
    DrawWire,
    DrawBus,
    AddLabel,
    AddComponent,
    DrawPolygon,
    AddText,
    AddImage,
    Measure,
}

/// The event handlers of a state (upstream `SchematicEditorState`); each
/// returns whether the event was handled.
#[allow(unused_variables)]
pub(crate) trait State {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        true
    }
    fn exit(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        true
    }
    fn abort(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        false
    }
    fn select_all(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        false
    }
    fn cut(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        false
    }
    fn copy(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        false
    }
    fn paste(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        false
    }
    fn move_by(&mut self, cx: &mut Cx<'_, '_>, delta: Point) -> bool {
        false
    }
    fn rotate(&mut self, cx: &mut Cx<'_, '_>, angle: Angle) -> bool {
        false
    }
    fn mirror(&mut self, cx: &mut Cx<'_, '_>, orientation: Orientation) -> bool {
        false
    }
    fn snap_to_grid(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        false
    }
    fn reset_all_texts(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        false
    }
    fn remove(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        false
    }
    fn edit_properties(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        false
    }
    fn key_pressed(&mut self, cx: &mut Cx<'_, '_>, e: KeyEvent) -> bool {
        false
    }
    fn key_released(&mut self, cx: &mut Cx<'_, '_>, e: KeyEvent) -> bool {
        false
    }
    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        false
    }
    fn left_pressed(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        false
    }
    fn left_released(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        false
    }
    fn left_double_clicked(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        false
    }
    fn right_released(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        false
    }
    /// Called after every event while the state is active (feature and
    /// info box updates, upstream: timers and signals).
    fn update(&mut self, cx: &mut Cx<'_, '_>) {}
}

#[derive(Debug, Default)]
struct States {
    select: select::SelectState,
    draw_wire: draw_wire::DrawWireState,
    draw_bus: draw_bus::DrawBusState,
    add_label: add_label::AddLabelState,
    add_component: add_component::AddComponentState,
    draw_polygon: draw_polygon::DrawPolygonState,
    add_text: add_text::AddTextState,
    add_image: add_image::AddImageState,
    measure: measure::MeasureState,
}

impl States {
    fn get(&mut self, kind: StateKind) -> Option<&mut dyn State> {
        Some(match kind {
            StateKind::Idle => return None,
            StateKind::Select => &mut self.select,
            StateKind::DrawWire => &mut self.draw_wire,
            StateKind::DrawBus => &mut self.draw_bus,
            StateKind::AddLabel => &mut self.add_label,
            StateKind::AddComponent => &mut self.add_component,
            StateKind::DrawPolygon => &mut self.draw_polygon,
            StateKind::AddText => &mut self.add_text,
            StateKind::AddImage => &mut self.add_image,
            StateKind::Measure => &mut self.measure,
        })
    }
}

/// The schematic editor state machine of one schematic page (upstream
/// `SchematicEditorFsm`).
#[derive(Debug)]
pub struct SchematicEditorFsm {
    schematic: SchematicId,
    settings: SchematicEditorSettings,
    current: StateKind,
    previous: StateKind,
    entered: bool,
    states: States,
    out: Output,
    search: crate::fsm::find::SearchContext,
}

macro_rules! dispatch {
    ($self:ident, $ctx:ident, |$state:ident, $cx:ident| $body:expr) => {{
        let kind = $self.ensure_entered($ctx);
        let handled = {
            let mut $cx = Cx {
                ctx: $ctx,
                out: &mut $self.out,
                schematic: $self.schematic,
                settings: &$self.settings,
            };
            match $self.states.get(kind) {
                Some($state) => $body,
                None => false,
            }
        };
        $self.after_event($ctx);
        handled
    }};
}

impl SchematicEditorFsm {
    /// Creates the FSM for a schematic page; it starts in the select
    /// state.
    pub fn new(schematic: SchematicId, settings: SchematicEditorSettings) -> Self {
        Self {
            schematic,
            settings,
            current: StateKind::Select,
            previous: StateKind::Idle,
            entered: false,
            states: States::default(),
            out: Output::default(),
            search: crate::fsm::find::SearchContext::new(),
        }
    }

    /// The schematic page.
    pub fn schematic(&self) -> SchematicId {
        self.schematic
    }

    /// The settings.
    pub fn settings(&self) -> &SchematicEditorSettings {
        &self.settings
    }

    /// Changes the settings.
    pub fn set_settings(&mut self, settings: SchematicEditorSettings) {
        self.settings = settings;
    }

    // --- Outputs ---

    /// The active tool.
    pub fn tool(&self) -> SchematicTool {
        self.out.tool
    }

    /// The tool bar data of the active tool.
    pub fn tool_data(&self) -> &SchematicToolData {
        &self.out.tool_data
    }

    /// What the view shows besides the scene.
    pub fn view_state(&self) -> &ViewState {
        &self.out.view
    }

    /// Takes the status bar message set since the last call.
    pub fn take_status_message(&mut self) -> Option<super::StatusMessage> {
        self.out.view.status_message.take()
    }

    /// The selected items.
    pub fn selection(&self) -> &BTreeSet<SchematicItem> {
        &self.out.selection
    }

    /// Replaces the selection (e.g. cross-probing, "find").
    pub fn set_selection(&mut self, items: impl IntoIterator<Item = SchematicItem>) {
        self.out.selection = items.into_iter().collect();
    }

    /// Removes items which no longer exist from the selection; call after
    /// changing the project outside the FSM (undo, redo, MCP). While a tool
    /// keeps an undo group open (dragging, drawing, placing), undo and redo
    /// are not possible: call [`abort()`](Self::abort) first.
    pub fn project_changed(&mut self, project: &Project) {
        if let Some(s) = project.schematic(self.schematic) {
            self.out
                .selection
                .retain(|item| selection::item_exists(item, s));
        }
    }

    /// The objects of the selection to highlight in the other editors
    /// (cross-probing; updated in the select tool).
    pub fn cross_probe(&self) -> &CrossProbe {
        &self.out.cross_probe
    }

    /// The item under the cursor in the select state (for highlighting).
    pub fn hovered(&self) -> Option<SchematicItem> {
        self.out.hovered
    }

    /// Takes the pending requests (dialogs, errors, menus).
    pub fn take_requests(&mut self) -> Vec<SchematicRequest> {
        std::mem::take(&mut self.out.requests)
    }

    // --- Tool switching ---

    /// Switches to the select tool (upstream `processSelect()`).
    pub fn select_tool(&mut self, ctx: &mut SchematicContext<'_>) -> bool {
        self.ensure_entered(ctx);
        let ok = self.set_next_state(ctx, StateKind::Select);
        self.after_event(ctx);
        ok
    }

    /// Switches to the draw wire tool.
    pub fn draw_wire(&mut self, ctx: &mut SchematicContext<'_>) -> bool {
        self.switch_to(ctx, StateKind::DrawWire)
    }

    /// Switches to the draw bus tool.
    pub fn draw_bus(&mut self, ctx: &mut SchematicContext<'_>) -> bool {
        self.switch_to(ctx, StateKind::DrawBus)
    }

    /// Switches to the add net label tool.
    pub fn add_net_label(&mut self, ctx: &mut SchematicContext<'_>) -> bool {
        self.switch_to(ctx, StateKind::AddLabel)
    }

    /// Switches to the draw polygon tool.
    pub fn draw_polygon(&mut self, ctx: &mut SchematicContext<'_>) -> bool {
        self.switch_to(ctx, StateKind::DrawPolygon)
    }

    /// Switches to the add text tool (a text follows the cursor).
    pub fn add_text(&mut self, ctx: &mut SchematicContext<'_>) -> bool {
        self.switch_to(ctx, StateKind::AddText)
    }

    /// Switches to the measure tool.
    pub fn measure(&mut self, ctx: &mut SchematicContext<'_>) -> bool {
        self.switch_to(ctx, StateKind::Measure)
    }

    /// Adds an image (upstream `processAddImage()`): without data, the
    /// image chooser is requested ([`SchematicRequest::ChooseImageFile`]);
    /// with data, the image follows the cursor, the first click places it,
    /// the second one sets its size.
    pub fn add_image(&mut self, ctx: &mut SchematicContext<'_>, data: Option<ImageData>) -> bool {
        let Some(data) = data else {
            self.out.requests.push(SchematicRequest::ChooseImageFile);
            return true;
        };
        let old = self.ensure_entered(ctx);
        if !self.set_next_state(ctx, StateKind::AddImage) {
            self.after_event(ctx);
            return false;
        }
        let ok = {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                schematic: self.schematic,
                settings: &self.settings,
            };
            self.states.add_image.start(&mut cx, data)
        };
        if !ok {
            self.set_next_state(ctx, old);
        }
        self.after_event(ctx);
        ok
    }

    /// Requests the "add component" dialog (upstream
    /// `processAddComponent(searchTerm)`): emits
    /// [`SchematicRequest::AddComponentDialog`]; the application then calls
    /// [`add_component()`](Self::add_component).
    pub fn open_add_component_dialog(&mut self, search_term: impl Into<String>) {
        self.out
            .requests
            .push(SchematicRequest::AddComponentDialog {
                search_term: search_term.into(),
            });
    }

    /// Starts adding a component (upstream `processAddComponent(cmp,
    /// symbVar)`): the first gate follows the cursor; each click places a
    /// gate, then the next component of the same kind is added.
    pub fn add_component(
        &mut self,
        ctx: &mut SchematicContext<'_>,
        choice: ComponentChoice,
    ) -> bool {
        let old = self.ensure_entered(ctx);
        if !self.set_next_state(ctx, StateKind::AddComponent) {
            self.after_event(ctx);
            return false;
        }
        let ok = {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                schematic: self.schematic,
                settings: &self.settings,
            };
            self.states.add_component.start(&mut cx, choice)
        };
        if !ok {
            self.set_next_state(ctx, old);
        }
        self.after_event(ctx);
        ok
    }

    /// The answer of [`SchematicRequest::BusMemberMenu`] (`None`: the menu
    /// was canceled).
    pub fn choose_bus_member(
        &mut self,
        ctx: &mut SchematicContext<'_>,
        choice: Option<BusMemberChoice>,
    ) -> bool {
        let kind = self.ensure_entered(ctx);
        let handled = kind == StateKind::DrawWire && {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                schematic: self.schematic,
                settings: &self.settings,
            };
            self.states.draw_wire.choose_bus_member(&mut cx, choice)
        };
        self.after_event(ctx);
        handled
    }

    /// Removes the vertices of a polygon at `pos` (context menu "Remove
    /// Vertex", upstream `removePolygonVertices()`).
    pub fn remove_polygon_vertices(
        &mut self,
        ctx: &mut SchematicContext<'_>,
        polygon: Uuid,
        pos: Point,
    ) -> bool {
        let kind = self.ensure_entered(ctx);
        let handled = kind == StateKind::Select && {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                schematic: self.schematic,
                settings: &self.settings,
            };
            self.states
                .select
                .remove_polygon_vertices(&mut cx, polygon, pos)
        };
        self.after_event(ctx);
        handled
    }

    /// Adds a vertex to the line of a polygon at `pos` which follows the
    /// cursor until the left button is released (context menu "Add
    /// Vertex", upstream `startAddingPolygonVertex()`).
    pub fn add_polygon_vertex(
        &mut self,
        ctx: &mut SchematicContext<'_>,
        polygon: Uuid,
        pos: Point,
    ) -> bool {
        let kind = self.ensure_entered(ctx);
        let handled = kind == StateKind::Select && {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                schematic: self.schematic,
                settings: &self.settings,
            };
            self.states.select.add_polygon_vertex(&mut cx, polygon, pos)
        };
        self.after_event(ctx);
        handled
    }

    /// Places the remaining gates of a component (or only `gate`), upstream
    /// `processAddRemainingGates()`.
    pub fn add_remaining_gates(
        &mut self,
        ctx: &mut SchematicContext<'_>,
        component: ComponentInstanceId,
        gate: Option<Uuid>,
    ) -> bool {
        let old = self.ensure_entered(ctx);
        let ok = self.add_remaining_gates_impl(ctx, component, gate, old);
        self.after_event(ctx);
        ok
    }

    fn add_remaining_gates_impl(
        &mut self,
        ctx: &mut SchematicContext<'_>,
        component: ComponentInstanceId,
        gate: Option<Uuid>,
        old: StateKind,
    ) -> bool {
        if !self.set_next_state(ctx, StateKind::AddComponent) {
            return false;
        }
        let ok = {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                schematic: self.schematic,
                settings: &self.settings,
            };
            self.states
                .add_component
                .start_remaining_gates(&mut cx, component, gate)
        };
        if !ok {
            self.set_next_state(ctx, old);
        }
        ok
    }

    // --- Actions ---

    /// Aborts the current command, or goes back to the select tool
    /// (upstream `processAbortCommand()`, Escape).
    pub fn abort(&mut self, ctx: &mut SchematicContext<'_>) -> bool {
        let kind = self.ensure_entered(ctx);
        let handled = {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                schematic: self.schematic,
                settings: &self.settings,
            };
            self.states.get(kind).is_some_and(|s| s.abort(&mut cx))
        };
        let result = handled || self.set_next_state(ctx, StateKind::Select);
        self.after_event(ctx);
        result
    }

    /// Selects all items.
    pub fn select_all(&mut self, ctx: &mut SchematicContext<'_>) -> bool {
        dispatch!(self, ctx, |s, cx| s.select_all(&mut cx))
    }

    /// Cuts the selected items to the clipboard.
    pub fn cut(&mut self, ctx: &mut SchematicContext<'_>) -> bool {
        dispatch!(self, ctx, |s, cx| s.cut(&mut cx))
    }

    /// Copies the selected items to the clipboard.
    pub fn copy(&mut self, ctx: &mut SchematicContext<'_>) -> bool {
        dispatch!(self, ctx, |s, cx| s.copy(&mut cx))
    }

    /// Pastes items from the clipboard; they follow the cursor until the
    /// next click. Without schematic data, an image in the clipboard is
    /// added (upstream `SchematicTab`: `ImageHelpers::getImageFromClipboard()`).
    pub fn paste(&mut self, ctx: &mut SchematicContext<'_>) -> bool {
        let version = &self.settings.app_version;
        let has_items = [
            clipboard::schematic_clipboard_mime_type(version),
            crate::library_editor::commands::symbol_clipboard_mime_type(version),
        ]
        .iter()
        .any(|mime| ctx.clipboard.get(mime).is_some());
        if !has_items {
            let image = CLIPBOARD_IMAGE_TYPES.iter().find_map(|(mime, format)| {
                ctx.clipboard.get(mime).map(|data| ImageData {
                    data,
                    format: (*format).to_owned(),
                    basename: String::new(),
                })
            });
            if let Some(image) = image {
                return self.add_image(ctx, Some(image));
            }
        }
        dispatch!(self, ctx, |s, cx| s.paste(&mut cx))
    }

    /// Moves the selected items (arrow keys).
    pub fn move_by(&mut self, ctx: &mut SchematicContext<'_>, delta: Point) -> bool {
        dispatch!(self, ctx, |s, cx| s.move_by(&mut cx, delta))
    }

    /// Rotates the selected items or the item being placed.
    pub fn rotate(&mut self, ctx: &mut SchematicContext<'_>, angle: Angle) -> bool {
        dispatch!(self, ctx, |s, cx| s.rotate(&mut cx, angle))
    }

    /// Mirrors the selected items or the item being placed.
    pub fn mirror(&mut self, ctx: &mut SchematicContext<'_>, orientation: Orientation) -> bool {
        dispatch!(self, ctx, |s, cx| s.mirror(&mut cx, orientation))
    }

    /// Snaps the selected items to the grid.
    pub fn snap_to_grid(&mut self, ctx: &mut SchematicContext<'_>) -> bool {
        dispatch!(self, ctx, |s, cx| s.snap_to_grid(&mut cx))
    }

    /// Resets the texts of the selected symbols.
    pub fn reset_all_texts(&mut self, ctx: &mut SchematicContext<'_>) -> bool {
        dispatch!(self, ctx, |s, cx| s.reset_all_texts(&mut cx))
    }

    /// Removes the selected items.
    pub fn remove(&mut self, ctx: &mut SchematicContext<'_>) -> bool {
        dispatch!(self, ctx, |s, cx| s.remove(&mut cx))
    }

    /// Requests the properties dialog of the selected item.
    pub fn edit_properties(&mut self, ctx: &mut SchematicContext<'_>) -> bool {
        dispatch!(self, ctx, |s, cx| s.edit_properties(&mut cx))
    }

    // --- Input events ---

    /// A key was pressed.
    pub fn key_pressed(&mut self, ctx: &mut SchematicContext<'_>, e: KeyEvent) -> bool {
        dispatch!(self, ctx, |s, cx| s.key_pressed(&mut cx, e))
    }

    /// A key was released.
    pub fn key_released(&mut self, ctx: &mut SchematicContext<'_>, e: KeyEvent) -> bool {
        dispatch!(self, ctx, |s, cx| s.key_released(&mut cx, e))
    }

    /// The pointer moved.
    pub fn pointer_moved(&mut self, ctx: &mut SchematicContext<'_>, e: PointerEvent) -> bool {
        self.out.last_pos = Some(e.pos);
        dispatch!(self, ctx, |s, cx| s.pointer_moved(&mut cx, e))
    }

    /// The left button was pressed.
    pub fn left_pressed(&mut self, ctx: &mut SchematicContext<'_>, e: PointerEvent) -> bool {
        self.out.last_pos = Some(e.pos);
        dispatch!(self, ctx, |s, cx| s.left_pressed(&mut cx, e))
    }

    /// The left button was released.
    pub fn left_released(&mut self, ctx: &mut SchematicContext<'_>, e: PointerEvent) -> bool {
        self.out.last_pos = Some(e.pos);
        dispatch!(self, ctx, |s, cx| s.left_released(&mut cx, e))
    }

    /// The left button was double-clicked.
    pub fn left_double_clicked(&mut self, ctx: &mut SchematicContext<'_>, e: PointerEvent) -> bool {
        self.out.last_pos = Some(e.pos);
        dispatch!(self, ctx, |s, cx| s.left_double_clicked(&mut cx, e))
    }

    /// The right button was released: handled by the state (e.g. rotate
    /// while placing), otherwise aborts the tool, or in the select state
    /// switches back to the previous tool.
    pub fn right_released(&mut self, ctx: &mut SchematicContext<'_>, e: PointerEvent) -> bool {
        self.out.last_pos = Some(e.pos);
        let kind = self.ensure_entered(ctx);
        let handled = {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                schematic: self.schematic,
                settings: &self.settings,
            };
            self.states
                .get(kind)
                .is_some_and(|s| s.right_released(&mut cx, e))
        };
        let result = if handled {
            true
        } else if kind != StateKind::Select {
            self.after_event(ctx);
            return self.abort(ctx);
        } else {
            self.switch_to_previous_state(ctx)
        };
        self.after_event(ctx);
        result
    }

    // --- Tool bar ---

    /// Sets the wire mode (draw wire tool).
    pub fn set_wire_mode(&mut self, ctx: &mut SchematicContext<'_>, mode: WireMode) {
        let kind = self.ensure_entered(ctx);
        self.out.tool_data.wire_mode = mode;
        {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                schematic: self.schematic,
                settings: &self.settings,
            };
            match kind {
                StateKind::DrawWire => self.states.draw_wire.wire_mode_changed(&mut cx),
                StateKind::DrawBus => self.states.draw_bus.wire_mode_changed(&mut cx),
                _ => {}
            }
        }
        self.after_event(ctx);
    }

    /// Chooses the bus of new bus segments (draw bus tool, upstream
    /// `selectBus()`; `None`: a new bus).
    pub fn set_bus(&mut self, ctx: &mut SchematicContext<'_>, bus: Option<BusId>) {
        self.ensure_entered(ctx);
        self.out.tool_data.bus = bus;
        self.after_event(ctx);
    }

    /// Sets the layer (draw polygon, add text).
    pub fn set_layer(&mut self, ctx: &mut SchematicContext<'_>, layer: Layer) {
        let kind = self.ensure_entered(ctx);
        self.out.tool_data.layer = layer;
        self.properties_changed(ctx, kind);
    }

    /// Sets the line width (draw polygon).
    pub fn set_line_width(&mut self, ctx: &mut SchematicContext<'_>, width: UnsignedLength) {
        let kind = self.ensure_entered(ctx);
        self.out.tool_data.line_width = width;
        self.properties_changed(ctx, kind);
    }

    /// Sets whether polygons are filled (draw polygon).
    pub fn set_filled(&mut self, ctx: &mut SchematicContext<'_>, filled: bool) {
        let kind = self.ensure_entered(ctx);
        self.out.tool_data.filled = filled;
        self.properties_changed(ctx, kind);
    }

    /// Sets the text height (add text).
    pub fn set_size(&mut self, ctx: &mut SchematicContext<'_>, size: PositiveLength) {
        let kind = self.ensure_entered(ctx);
        self.out.tool_data.size = size;
        self.properties_changed(ctx, kind);
    }

    /// Sets the text (add text) or the value (add component).
    pub fn set_value(&mut self, ctx: &mut SchematicContext<'_>, value: impl Into<String>) {
        let kind = self.ensure_entered(ctx);
        self.out.tool_data.value = value.into();
        self.properties_changed(ctx, kind);
    }

    /// Sets the value of the value attribute (add component, upstream
    /// `setValueAttributeValue()`).
    pub fn set_value_attribute_value(&mut self, ctx: &mut SchematicContext<'_>, value: &str) {
        let kind = self.ensure_entered(ctx);
        if kind == StateKind::AddComponent {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                schematic: self.schematic,
                settings: &self.settings,
            };
            self.states
                .add_component
                .value_attribute_value_changed(&mut cx, value);
        }
        self.after_event(ctx);
    }

    /// Sets the unit of the value attribute (add component, upstream
    /// `setValueAttributeUnit()`).
    pub fn set_value_attribute_unit(
        &mut self,
        ctx: &mut SchematicContext<'_>,
        unit: Option<&'static librepcb_core::attribute::AttributeUnit>,
    ) {
        let kind = self.ensure_entered(ctx);
        if kind == StateKind::AddComponent {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                schematic: self.schematic,
                settings: &self.settings,
            };
            self.states
                .add_component
                .value_attribute_unit_changed(&mut cx, unit);
        }
        self.after_event(ctx);
    }

    fn properties_changed(&mut self, ctx: &mut SchematicContext<'_>, kind: StateKind) {
        {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                schematic: self.schematic,
                settings: &self.settings,
            };
            match kind {
                StateKind::DrawPolygon => self.states.draw_polygon.properties_changed(&mut cx),
                StateKind::AddText => self.states.add_text.properties_changed(&mut cx),
                StateKind::AddComponent => self.states.add_component.value_changed(&mut cx),
                _ => {}
            }
        }
        self.after_event(ctx);
    }

    // --- State handling ---

    fn switch_to(&mut self, ctx: &mut SchematicContext<'_>, kind: StateKind) -> bool {
        self.ensure_entered(ctx);
        let ok = self.set_next_state(ctx, kind);
        self.after_event(ctx);
        ok
    }

    /// Enters the initial state on first use (it needs a context).
    fn ensure_entered(&mut self, ctx: &mut SchematicContext<'_>) -> StateKind {
        if !self.entered {
            self.entered = true;
            let kind = self.current;
            self.current = StateKind::Idle;
            if !self.enter_next_state(ctx, kind) {
                self.enter_next_state(ctx, StateKind::Select);
            }
        }
        self.current
    }

    fn set_next_state(&mut self, ctx: &mut SchematicContext<'_>, kind: StateKind) -> bool {
        if kind == self.current {
            return true;
        }
        if !self.leave_current_state(ctx) {
            return false;
        }
        if !self.enter_next_state(ctx, kind) {
            self.enter_next_state(ctx, StateKind::Select);
            return false;
        }
        true
    }

    fn leave_current_state(&mut self, ctx: &mut SchematicContext<'_>) -> bool {
        let kind = self.current;
        let ok = {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                schematic: self.schematic,
                settings: &self.settings,
            };
            self.states.get(kind).is_none_or(|s| s.exit(&mut cx))
        };
        if !ok {
            return false;
        }
        // Only memorize states other than select and add component.
        if !matches!(
            kind,
            StateKind::Select | StateKind::AddComponent | StateKind::Idle
        ) {
            self.previous = kind;
        }
        self.current = StateKind::Idle;
        self.out.tool = SchematicTool::Select;
        self.out.view.cursor = None;
        self.out.view.scene_cursor = None;
        self.out.view.rubber_band = None;
        self.out.view.ruler = None;
        self.out.view.gray_out = false;
        self.out.view.info_box.clear();
        self.out.view.features = Default::default();
        self.out.hovered = None;
        self.out.cross_probe = CrossProbe::default();
        true
    }

    fn enter_next_state(&mut self, ctx: &mut SchematicContext<'_>, kind: StateKind) -> bool {
        let ok = {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                schematic: self.schematic,
                settings: &self.settings,
            };
            self.states.get(kind).is_none_or(|s| s.entry(&mut cx))
        };
        if ok {
            self.current = kind;
        }
        ok
    }

    fn switch_to_previous_state(&mut self, ctx: &mut SchematicContext<'_>) -> bool {
        let mut next = self.previous;
        if next == self.current || next == StateKind::Idle {
            next = StateKind::Select;
        }
        self.set_next_state(ctx, next)
    }

    /// Processes queued state requests and lets the state update its
    /// outputs.
    fn after_event(&mut self, ctx: &mut SchematicContext<'_>) {
        if std::mem::take(&mut self.out.leave_requested) {
            self.set_next_state(ctx, StateKind::Select);
        }
        // Forget removed items (e.g. after undo).
        if let Some(s) = ctx.editor.project().schematic(self.schematic) {
            self.out
                .selection
                .retain(|item| selection::item_exists(item, s));
            if self
                .out
                .hovered
                .is_some_and(|item| !selection::item_exists(&item, s))
            {
                self.out.hovered = None;
            }
        }
        let kind = self.current;
        let mut cx = Cx {
            ctx,
            out: &mut self.out,
            schematic: self.schematic,
            settings: &self.settings,
        };
        if let Some(state) = self.states.get(kind) {
            state.update(&mut cx);
        }
    }
}

static_assertions::assert_impl_all!(SchematicEditorFsm: Send, Sync);
