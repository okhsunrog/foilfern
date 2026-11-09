//! Outputs of the board editor FSM: what the view shows (cursor, overlays,
//! info box), the tool bar data (upstream `Board2dTabData::tool-*` and
//! `BoardEditorFsmAdapter::Features`) and events for the application
//! (dialogs, clipboard, messages).

use std::collections::BTreeSet;

use librepcb_core::geometry::{ComponentSide, PadFunction, ZoneRules};
use librepcb_core::project::{ComponentInstanceId, NetSignalId};
use librepcb_core::types::{
    Angle, Layer, Point, PositiveLength, UnsignedLength, UnsignedLimitedRatio, Uuid,
};

use super::view::BoardItemRef;
use crate::fsm::ViewState;

/// The tools (states) of the board editor (upstream
/// `BoardEditorFsm::State`, UI `EditorTool`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BoardTool {
    /// Select, move and modify items.
    #[default]
    Select,
    /// Draw traces.
    DrawTrace,
    /// Add vias.
    AddVia,
    /// Draw polygons.
    DrawPolygon,
    /// Draw planes.
    DrawPlane,
    /// Draw keepout zones.
    DrawZone,
    /// Add non-plated holes.
    AddHole,
    /// Add standalone THT pads.
    AddThtPad,
    /// Add standalone SMT pads of a function (standard, thermal, BGA, edge
    /// connector, test pad, local or global fiducial).
    AddSmtPad(PadFunction),
    /// Add stroke texts.
    AddStrokeText,
    /// Place a device (started with
    /// [`BoardEditorFsm::add_device()`](super::BoardEditorFsm::add_device)).
    AddDevice,
    /// Measure distances.
    Measure,
}

/// The wire mode of the trace tool (upstream
/// `BoardEditorState_DrawTrace::WireMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WireMode {
    /// Horizontal, then vertical.
    #[default]
    HV,
    /// Vertical, then horizontal.
    VH,
    /// 90°, then 45°.
    Deg9045,
    /// 45°, then 90°.
    Deg4590,
    /// Straight.
    Straight,
}

impl WireMode {
    /// The next mode (cycled with the right mouse button).
    pub fn next(self) -> Self {
        match self {
            Self::HV => Self::VH,
            Self::VH => Self::Deg9045,
            Self::Deg9045 => Self::Deg4590,
            Self::Deg4590 => Self::Straight,
            Self::Straight => Self::HV,
        }
    }
}

/// The net selection of a tool (upstream: "Auto", "None" or a net).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ToolNet {
    /// Whether the net is determined automatically (vias only).
    pub auto: bool,
    /// The net (`None`: no net).
    pub net: Option<NetSignalId>,
}

/// Tool bar values of the active tool (upstream `Board2dTabData`
/// `tool-*` fields). Fields which the active tool does not use keep their
/// defaults.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BoardToolData {
    /// The active tool.
    pub tool: BoardTool,
    /// Wire mode (trace tool).
    pub wire_mode: WireMode,
    /// Nets to choose from, sorted by name (via, plane tools).
    pub nets: Vec<(NetSignalId, String)>,
    /// Current net (via, plane, trace tools).
    pub net: ToolNet,
    /// Name of the net class of the current net (read-only; empty if
    /// none).
    pub net_class_name: String,
    /// Layers to choose from, sorted.
    pub layers: Vec<Layer>,
    /// Current layer.
    pub layer: Option<Layer>,
    /// Trace width, polygon line width.
    pub line_width: Option<UnsignedLength>,
    /// Whether the trace width is automatic (UI: `tool-filled`).
    pub auto_width: bool,
    /// Via size or text height (UI: `tool-size`).
    pub size: Option<PositiveLength>,
    /// Whether the via size is automatic (UI: `tool-mirrored`).
    pub auto_size: bool,
    /// Via drill or hole diameter (UI: `tool-drill`).
    pub drill: Option<PositiveLength>,
    /// Whether the via drill is automatic (UI: `tool-pressfit`).
    pub auto_drill: bool,
    /// Arc angle of the next segment (polygon, zone tools).
    pub angle: Angle,
    /// Whether polygons are filled.
    pub filled: bool,
    /// Whether texts are mirrored.
    pub mirrored: bool,
    /// Text of the text tool.
    pub value: String,
    /// Suggestions for the text.
    pub value_suggestions: Vec<String>,
    /// Zone rules (zone tool).
    pub zone_rules: ZoneRules,
    /// Pad shape (pad tools).
    pub pad_shape: Option<ToolPadShape>,
    /// Pad corner radius (pad tools, UI: `tool-ratio`); the pad width is
    /// in [`line_width`](Self::line_width), the height in
    /// [`size`](Self::size) and the drill in [`drill`](Self::drill).
    pub pad_radius: Option<UnsignedLimitedRatio>,
    /// Component side (SMT pad tools).
    pub component_side: Option<ComponentSide>,
    /// Whether the pad is a fiducial (the size field is the clearance).
    pub fiducial: bool,
    /// Whether the THT pad is a press-fit pad (THT pad tool).
    pub press_fit: Option<bool>,
    /// Copper clearance of fiducials (pad tools).
    pub copper_clearance: Option<UnsignedLength>,
}

/// The pad shapes of the pad tools (upstream `ui::PadShape`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolPadShape {
    /// Round (rounded rectangle with 100% radius).
    Round,
    /// Rounded rectangle with the recommended radius.
    RoundedRect,
    /// Rectangle.
    Rect,
    /// Octagon.
    Octagon,
}

/// Changes of tool bar values by the user (upstream `Board2dTab` signals
/// `*Requested`).
#[derive(Debug, Clone, PartialEq)]
pub enum ToolSetting {
    /// Wire mode.
    WireMode(WireMode),
    /// Net.
    Net(ToolNet),
    /// Layer (zone tool: the only layer of new zones).
    Layer(Layer),
    /// Trace width.
    TraceWidth(PositiveLength),
    /// Polygon line width.
    LineWidth(UnsignedLength),
    /// Automatic trace width.
    AutoWidth(bool),
    /// Via size (`None`: automatic).
    ViaSize(Option<PositiveLength>),
    /// Via drill (`None`: automatic).
    ViaDrill(Option<PositiveLength>),
    /// Hole diameter.
    HoleDiameter(PositiveLength),
    /// Text height.
    TextHeight(PositiveLength),
    /// Text.
    Text(String),
    /// Mirrored text.
    Mirrored(bool),
    /// Filled polygon.
    Filled(bool),
    /// Arc angle.
    Angle(Angle),
    /// Zone rules.
    ZoneRules(ZoneRules),
    /// Component side of SMT pads.
    ComponentSide(ComponentSide),
    /// Pad shape.
    PadShape(ToolPadShape),
    /// Pad width (fiducials: diameter).
    PadWidth(PositiveLength),
    /// Pad height.
    PadHeight(PositiveLength),
    /// Pad corner radius.
    PadRadius(UnsignedLimitedRatio),
    /// Press-fit THT pad.
    PressFit(bool),
    /// Clearance (copper and stop mask) of fiducials.
    FiducialClearance(UnsignedLength),
    /// Store the trace width as board default.
    SaveTraceWidthInBoard,
    /// Store the trace width as net class default.
    SaveTraceWidthInNetClass,
    /// Store the via drill as board default.
    SaveViaDrillInBoard,
    /// Store the via drill as net class default.
    SaveViaDrillInNetClass,
}

/// An action of a context menu entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContextAction {
    /// Open the properties dialog.
    Properties,
    /// Rotate counter-clockwise.
    RotateCcw,
    /// Rotate clockwise.
    RotateCw,
    /// Flip horizontally.
    FlipHorizontal,
    /// Flip vertically.
    FlipVertical,
    /// Remove the selection.
    Remove,
    /// Cut.
    Cut,
    /// Copy.
    Copy,
    /// Snap to grid.
    SnapToGrid,
    /// Lock (`true`) or unlock.
    Lock(bool),
    /// Reset all texts of the device.
    ResetTexts,
    /// Change the device (library device from the library element
    /// source, upstream "Change Device" menu).
    ChangeDevice(Uuid),
    /// Change the footprint of the device.
    ChangeFootprint(Uuid),
    /// Change the 3D model of the device.
    ChangeModel(Option<Uuid>),
    /// Set the width of the selected traces.
    SetLineWidth,
    /// Remove the whole trace (net segment).
    RemoveWholeTrace,
    /// Select the whole trace (net segment).
    SelectWholeTrace,
    /// Measure the length of the selected traces.
    MeasureLength,
    /// Remove the vertices at the clicked position.
    RemoveVertices,
    /// Add a vertex at the clicked position.
    AddVertex,
    /// Show or hide the plane.
    PlaneVisible(bool),
}

/// An entry of a context menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMenuItem {
    /// The action (`None`: separator).
    pub action: Option<ContextAction>,
    /// The (translated) text.
    pub text: String,
    /// Whether the entry is enabled.
    pub enabled: bool,
    /// Check state of checkable entries.
    pub checked: Option<bool>,
    /// Whether this is the default action.
    pub default: bool,
}

/// A request of the FSM to the application (upstream: dialogs, message
/// boxes and context menus the states open themselves).
#[derive(Debug, Clone, PartialEq)]
pub enum BoardRequest {
    /// Show an error (upstream `QMessageBox::critical()`).
    ShowError(String),
    /// Show a message (e.g. a measurement result).
    Message {
        /// Title.
        title: String,
        /// Text.
        text: String,
        /// Whether it is a warning.
        warning: bool,
    },
    /// Open the properties dialog of an item.
    Properties(BoardItemRef),
    /// Show a context menu at a scene position; the chosen entry is
    /// passed back with
    /// [`BoardEditorFsm::context_menu_action()`](super::BoardEditorFsm::context_menu_action).
    ContextMenu {
        /// Position in the scene.
        pos: Point,
        /// The item the menu is for.
        item: BoardItemRef,
        /// The entries.
        items: Vec<ContextMenuItem>,
    },
    /// Ask for a line width (upstream "Set Width" dialog); pass the value
    /// back with
    /// [`BoardEditorFsm::set_line_width()`](super::BoardEditorFsm::set_line_width).
    LineWidthDialog {
        /// The current (median) width.
        current: UnsignedLength,
    },
    /// Devices were placed on the board (refresh the unplaced components
    /// list).
    DevicesChanged(BTreeSet<ComponentInstanceId>),
}

/// The outputs of the FSM, shared by all states.
#[derive(Debug, Default)]
pub(crate) struct Output {
    pub view: ViewState,
    pub highlighted_nets: BTreeSet<NetSignalId>,
    pub cross_probe: crate::fsm::CrossProbe,
    pub hovered: Option<BoardItemRef>,
    pub tool_data: BoardToolData,
    pub requests: Vec<BoardRequest>,
}
