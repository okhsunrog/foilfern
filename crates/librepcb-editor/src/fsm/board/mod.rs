//! The board editor state machine: port of
//! libs/librepcb/editor/project/board/fsm/boardeditorfsm.{h,cpp},
//! boardeditorfsmadapter.h, boardeditorstate.{h,cpp} and the states
//! `boardeditorstate_*.{h,cpp}` (submodules `select`, `draw_trace`,
//! `add_via`, `draw_polygon` (polygons, planes, zones), `add_hole`,
//! `add_stroke_text`, `add_device`, `measure`), UI-toolkit independent.
//!
//! # Usage
//!
//! The application creates one [`BoardEditorFsm`] per board tab and calls
//! its methods (tools, actions, pointer and key events in world
//! coordinates, tool bar changes) with a [`BoardContext`]: the project
//! editor, its [`BoardView`] implementation on the board scene (hit
//! testing) and the clipboard. Afterwards it syncs its scene with the
//! project's change journal, reflects [`BoardEditorFsm::selection()`] in
//! the scene and shows [`BoardEditorFsm::view_state()`] (cursor, info box,
//! ruler, rubber band, status message, available features),
//! [`BoardEditorFsm::highlighted_nets()`] and
//! [`BoardEditorFsm::tool_data()`]; dialogs, messages and context menus
//! are requested through [`BoardEditorFsm::take_requests()`].
//!
//! # Model changes and undo
//!
//! Every user action is one undo group of the [`ProjectEditor`]
//! (upstream `UndoStack::beginCmdGroup()`). Interactive tools (dragging,
//! drawing a trace, placing a via, device, text or hole, drawing planes,
//! polygons and zones) keep a group open and replace their live preview
//! inside it (`ProjectEditor::rollback_group_to()`), so the scene (updated
//! from the change journal) shows the preview; they commit the group when
//! the action is finished (mouse release, click) and abort it on
//! [`BoardEditorFsm::abort()`]. Air wires are rebuilt after every change,
//! also during previews.
//!
//! # Differences to upstream
//!
//! - The selection is held by the FSM ([`BoardSelection`]) instead of the
//!   graphics items; selecting a device implicitly selects its texts.
//!   Junctions are hit tested on the model (upstream: graphics items).
//! - Dialogs, message boxes and context menus are [`BoardRequest`]s;
//!   answers come back as method calls
//!   ([`BoardEditorFsm::context_menu_action()`],
//!   [`BoardEditorFsm::set_line_width()`]).
//! - Tool bar values are polled ([`BoardToolData`]) instead of signals;
//!   changes are [`ToolSetting`]s.
//! - Cross-probing is reported through [`BoardEditorFsm::cross_probe()`]
//!   and [`BoardEditorFsm::highlighted_nets()`]; the application forwards
//!   it to the schematic editors.
//! - Not ported: aborting blocking tools in other editors (the
//!   application must abort them before switching tabs) and the plane
//!   visibility of the context menu (a view setting of the application).

mod clipboard;
mod context;
mod find;
mod output;
mod selection;
mod simplify;
mod transform;
mod view;

mod add_device;
mod add_hole;
mod add_pad;
mod add_stroke_text;
mod add_via;
mod draw_polygon;
mod draw_trace;
mod measure;
mod select;

use std::collections::BTreeSet;

use librepcb_core::project::{BoardId, ComponentInstanceId, NetSignalId, Project};
use librepcb_core::types::{Angle, Orientation, Point, UnsignedLength, Uuid};

pub use clipboard::{
    BOARD_CLIPBOARD_MIME_PREFIX, BoardClipboardData, ClipboardDevice, ClipboardNetSegment,
    ClipboardPlane, PasteBoardItems, board_clipboard_mime_type,
};
pub use context::BoardContext;
pub use output::{
    BoardRequest, BoardTool, BoardToolData, ContextAction, ContextMenuItem, ToolNet, ToolPadShape,
    ToolSetting, WireMode,
};
pub use selection::{BoardSelection, SelectionQuery};
pub use simplify::SimplifyBoardNetSegments;
pub use transform::DragItems;
pub use view::{BoardItemRef, BoardView, FindFilter, FindFlags, find_items_at_pos};

use super::{KeyEvent, PointerEvent, StatusMessage, ViewState};
use context::Cx;
use output::Output;

#[allow(unused_imports)] // referenced by the documentation
use crate::editor::ProjectEditor;

/// Settings of the board editor FSM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardEditorSettings {
    /// Ignore placement locks (upstream "ignore placement locks" of the
    /// tab).
    pub ignore_locks: bool,
    /// Application version used in the clipboard MIME type (upstream
    /// `Application::getVersion()`; clipboard data is only exchanged
    /// between identical versions).
    pub app_version: String,
}

impl Default for BoardEditorSettings {
    fn default() -> Self {
        Self {
            ignore_locks: false,
            app_version: "2.1.2-unstable".to_owned(),
        }
    }
}

/// An input of the board editor FSM (upstream `BoardEditorFsm::process*()`
/// methods); the methods of [`BoardEditorFsm`] create these.
#[derive(Debug, Clone, PartialEq)]
pub enum BoardFsmInput {
    /// Switch to a tool. [`BoardTool::AddDevice`] is started with
    /// [`AddDevice`](Self::AddDevice).
    Tool(BoardTool),
    /// Start placing the device of a component.
    AddDevice {
        /// The component.
        component: ComponentInstanceId,
        /// The library device.
        device: Uuid,
        /// The footprint (`None`: the first one).
        footprint: Option<Uuid>,
    },
    /// Abort the current command (Escape).
    Abort,
    /// Select all items.
    SelectAll,
    /// Cut the selection.
    Cut,
    /// Copy the selection.
    Copy,
    /// Paste the clipboard content.
    Paste,
    /// Move the selection by a vector (arrow keys).
    Move(Point),
    /// Rotate the selection (or the item being placed).
    Rotate(Angle),
    /// Flip the selection (or the item being placed).
    Flip(Orientation),
    /// Snap the selection to the grid.
    SnapToGrid,
    /// Lock or unlock the selection.
    SetLocked(bool),
    /// Change the line width of the selection (step, see
    /// [`BoardEditorFsm::change_line_width()`]).
    ChangeLineWidth(i32),
    /// The answer of [`BoardRequest::LineWidthDialog`].
    SetLineWidth(UnsignedLength),
    /// Reset the texts of the selected devices.
    ResetAllTexts,
    /// Remove the selection.
    Remove,
    /// Open the properties of the selection.
    EditProperties,
    /// Add a plane around the board outline (plane tool).
    AutoAddPlane,
    /// A key was pressed.
    KeyPressed(KeyEvent),
    /// A key was released.
    KeyReleased(KeyEvent),
    /// The pointer moved.
    PointerMoved(PointerEvent),
    /// The left button was pressed.
    LeftPressed(PointerEvent),
    /// The left button was released.
    LeftReleased(PointerEvent),
    /// The left button was double-clicked.
    LeftDoubleClicked(PointerEvent),
    /// The right button was released (without panning).
    RightReleased(PointerEvent),
    /// A context menu entry was chosen.
    ContextMenu(ContextAction),
    /// Import a DXF file (select tool, upstream `processImportDxf()` with
    /// the choices of the DXF import dialog).
    ImportDxf(DxfImportSettings),
    /// A tool bar value was changed.
    ToolSetting(ToolSetting),
}

/// The choices of the DXF import dialog (upstream `DxfImportDialog`).
#[derive(Debug, Clone, PartialEq)]
pub struct DxfImportSettings {
    /// The DXF file.
    pub file: librepcb_core::fileio::FilePath,
    /// Layer of the imported polygons.
    pub layer: librepcb_core::types::Layer,
    /// Line width of the imported polygons.
    pub line_width: UnsignedLength,
    /// Factor the coordinates are multiplied with.
    pub scale_factor: f64,
    /// Join tangent polylines to (closed) paths.
    pub join_tangent_polylines: bool,
    /// Import circles as (non-plated) holes instead of polygons.
    pub circles_as_drills: bool,
    /// Fixed placement offset; `None`: the items follow the cursor until
    /// the next click (like pasting).
    pub placement: Option<Point>,
}

/// Interface of the states (upstream `BoardEditorState`).
trait State {
    /// Enters the state; `false` if it cannot be entered.
    fn entry(&mut self, _cx: &mut Cx<'_, '_>) -> bool {
        true
    }
    /// Leaves the state; `false` if it cannot be left.
    fn exit(&mut self, _cx: &mut Cx<'_, '_>) -> bool {
        true
    }
    /// Processes an input; `true` if handled.
    fn process(&mut self, cx: &mut Cx<'_, '_>, input: &BoardFsmInput) -> bool;
    /// Tool bar data.
    fn tool_data(&self, _cx: &Cx<'_, '_>) -> BoardToolData {
        BoardToolData::default()
    }
}

/// The states (all kept, so tool settings persist like upstream).
#[derive(Debug)]
struct States {
    select: select::SelectState,
    draw_trace: draw_trace::DrawTraceState,
    add_via: add_via::AddViaState,
    draw_polygon: draw_polygon::DrawPolygonState,
    draw_plane: draw_polygon::DrawPlaneState,
    draw_zone: draw_polygon::DrawZoneState,
    add_hole: add_hole::AddHoleState,
    add_tht_pad: add_pad::AddPadState,
    add_smt_pads:
        std::collections::BTreeMap<librepcb_core::geometry::PadFunction, add_pad::AddPadState>,
    add_stroke_text: add_stroke_text::AddStrokeTextState,
    add_device: add_device::AddDeviceState,
    measure: measure::MeasureState,
}

impl States {
    fn get(&mut self, tool: BoardTool) -> &mut dyn State {
        match tool {
            BoardTool::Select => &mut self.select,
            BoardTool::DrawTrace => &mut self.draw_trace,
            BoardTool::AddVia => &mut self.add_via,
            BoardTool::DrawPolygon => &mut self.draw_polygon,
            BoardTool::DrawPlane => &mut self.draw_plane,
            BoardTool::DrawZone => &mut self.draw_zone,
            BoardTool::AddHole => &mut self.add_hole,
            BoardTool::AddThtPad => &mut self.add_tht_pad,
            BoardTool::AddSmtPad(f) => self
                .add_smt_pads
                .entry(f)
                .or_insert_with(|| add_pad::AddPadState::smt(f)),
            BoardTool::AddStrokeText => &mut self.add_stroke_text,
            BoardTool::AddDevice => &mut self.add_device,
            BoardTool::Measure => &mut self.measure,
        }
    }

    fn get_ref(&self, tool: BoardTool) -> &dyn State {
        match tool {
            BoardTool::Select => &self.select,
            BoardTool::DrawTrace => &self.draw_trace,
            BoardTool::AddVia => &self.add_via,
            BoardTool::DrawPolygon => &self.draw_polygon,
            BoardTool::DrawPlane => &self.draw_plane,
            BoardTool::DrawZone => &self.draw_zone,
            BoardTool::AddHole => &self.add_hole,
            BoardTool::AddThtPad => &self.add_tht_pad,
            BoardTool::AddSmtPad(f) => match self.add_smt_pads.get(&f) {
                Some(s) => s,
                None => &self.select,
            },
            BoardTool::AddStrokeText => &self.add_stroke_text,
            BoardTool::AddDevice => &self.add_device,
            BoardTool::Measure => &self.measure,
        }
    }
}

/// The board editor state machine of one board (upstream
/// `BoardEditorFsm`).
#[derive(Debug)]
pub struct BoardEditorFsm {
    board: BoardId,
    settings: BoardEditorSettings,
    /// `None`: no state active (upstream `IDLE`).
    current: Option<BoardTool>,
    previous: Option<BoardTool>,
    entered: bool,
    states: States,
    out: Output,
    selection: BoardSelection,
    cursor_pos: Option<Point>,
    left_button: bool,
    search: crate::fsm::find::SearchContext,
}

impl BoardEditorFsm {
    /// Creates the FSM for a board; it starts in the select tool.
    pub fn new(board: BoardId, settings: BoardEditorSettings) -> Self {
        Self {
            board,
            settings,
            current: Some(BoardTool::Select),
            previous: None,
            entered: false,
            states: States {
                select: select::SelectState::default(),
                draw_trace: draw_trace::DrawTraceState::new(None),
                add_via: add_via::AddViaState::default(),
                draw_polygon: draw_polygon::DrawPolygonState::default(),
                draw_plane: draw_polygon::DrawPlaneState::default(),
                draw_zone: draw_polygon::DrawZoneState::default(),
                add_hole: add_hole::AddHoleState::default(),
                add_tht_pad: add_pad::AddPadState::tht(),
                add_smt_pads: Default::default(),
                add_stroke_text: add_stroke_text::AddStrokeTextState::default(),
                add_device: add_device::AddDeviceState::default(),
                measure: measure::MeasureState::default(),
            },
            out: Output::default(),
            selection: BoardSelection::default(),
            cursor_pos: None,
            left_button: false,
            search: crate::fsm::find::SearchContext::new(),
        }
    }

    /// The board.
    pub fn board(&self) -> BoardId {
        self.board
    }

    /// The settings.
    pub fn settings(&self) -> &BoardEditorSettings {
        &self.settings
    }

    /// Changes the settings.
    pub fn set_settings(&mut self, settings: BoardEditorSettings) {
        self.settings = settings;
    }

    // --- Outputs ---

    /// The active tool.
    pub fn tool(&self) -> BoardTool {
        self.current.unwrap_or_default()
    }

    /// The tool bar data of the active tool.
    pub fn tool_data(&self) -> &BoardToolData {
        &self.out.tool_data
    }

    /// What the view shows besides the scene.
    pub fn view_state(&self) -> &ViewState {
        &self.out.view
    }

    /// Takes the status bar message set since the last call.
    pub fn take_status_message(&mut self) -> Option<StatusMessage> {
        self.out.view.status_message.take()
    }

    /// Nets to highlight (upstream cross probing of the current tool).
    pub fn highlighted_nets(&self) -> &BTreeSet<NetSignalId> {
        &self.out.highlighted_nets
    }

    /// The selected objects to highlight in the schematic editors
    /// (upstream `fsmCrossProbe()` of the select tool: nets, components of
    /// devices and component signals of pads).
    pub fn cross_probe(&self) -> &crate::fsm::CrossProbe {
        &self.out.cross_probe
    }

    /// The item under the cursor (select tool, idle).
    pub fn hovered(&self) -> Option<BoardItemRef> {
        self.out.hovered
    }

    /// Takes the requests produced since the last call.
    pub fn take_requests(&mut self) -> Vec<BoardRequest> {
        std::mem::take(&mut self.out.requests)
    }

    /// The selection.
    pub fn selection(&self) -> &BoardSelection {
        &self.selection
    }

    /// Replaces the selection (e.g. "find", cross probing; upstream
    /// `processChangedSelection()`).
    pub fn set_selection(
        &mut self,
        ctx: &mut BoardContext<'_>,
        items: impl IntoIterator<Item = BoardItemRef>,
    ) {
        self.selection.replace(items);
        self.with_cx(ctx, |fsm, cx| {
            if fsm.current == Some(BoardTool::Select) {
                fsm.states.select.update_features(cx, false);
            }
        });
    }

    /// Removes items which no longer exist from the selection and updates
    /// the net segments of selected elements; call after the project was
    /// changed from outside the FSM (undo, redo, MCP).
    pub fn project_changed(&mut self, project: &Project) {
        if let Some(board) = project.board(self.board) {
            self.selection.update(board);
        }
    }

    // --- Tools ---

    /// Switches to a tool (upstream `processSelect()`, `processDrawTrace()`,
    /// ...; see [`add_device()`](Self::add_device) for placing devices).
    pub fn set_tool(&mut self, ctx: &mut BoardContext<'_>, tool: BoardTool) -> bool {
        self.process(ctx, BoardFsmInput::Tool(tool))
    }

    /// Starts placing the device of a component (upstream
    /// `processAddDevice()`, from the "place devices" panel).
    pub fn add_device(
        &mut self,
        ctx: &mut BoardContext<'_>,
        component: ComponentInstanceId,
        device: Uuid,
        footprint: Option<Uuid>,
    ) -> bool {
        self.process(
            ctx,
            BoardFsmInput::AddDevice {
                component,
                device,
                footprint,
            },
        )
    }

    /// Adds a plane around the board outline (plane tool, upstream
    /// `autoAdd()`).
    pub fn auto_add_plane(&mut self, ctx: &mut BoardContext<'_>) -> bool {
        self.process(ctx, BoardFsmInput::AutoAddPlane)
    }

    /// Changes a tool bar value.
    pub fn tool_setting(&mut self, ctx: &mut BoardContext<'_>, setting: ToolSetting) -> bool {
        self.process(ctx, BoardFsmInput::ToolSetting(setting))
    }

    // --- Actions ---

    /// Aborts the current command (Escape).
    pub fn abort(&mut self, ctx: &mut BoardContext<'_>) -> bool {
        self.process(ctx, BoardFsmInput::Abort)
    }

    /// Selects all items.
    pub fn select_all(&mut self, ctx: &mut BoardContext<'_>) -> bool {
        self.process(ctx, BoardFsmInput::SelectAll)
    }

    /// Cuts the selection to the clipboard.
    pub fn cut(&mut self, ctx: &mut BoardContext<'_>) -> bool {
        self.process(ctx, BoardFsmInput::Cut)
    }

    /// Copies the selection to the clipboard.
    pub fn copy(&mut self, ctx: &mut BoardContext<'_>) -> bool {
        self.process(ctx, BoardFsmInput::Copy)
    }

    /// Pastes the clipboard content (it follows the cursor until a click).
    pub fn paste(&mut self, ctx: &mut BoardContext<'_>) -> bool {
        self.process(ctx, BoardFsmInput::Paste)
    }

    /// Moves the selection (arrow keys).
    pub fn move_by(&mut self, ctx: &mut BoardContext<'_>, delta: Point) -> bool {
        self.process(ctx, BoardFsmInput::Move(delta))
    }

    /// Rotates the selection or the item being placed.
    pub fn rotate(&mut self, ctx: &mut BoardContext<'_>, angle: Angle) -> bool {
        self.process(ctx, BoardFsmInput::Rotate(angle))
    }

    /// Flips the selection or the item being placed to the other side.
    pub fn flip(&mut self, ctx: &mut BoardContext<'_>, orientation: Orientation) -> bool {
        self.process(ctx, BoardFsmInput::Flip(orientation))
    }

    /// Snaps the selection to the grid.
    pub fn snap_to_grid(&mut self, ctx: &mut BoardContext<'_>) -> bool {
        self.process(ctx, BoardFsmInput::SnapToGrid)
    }

    /// Locks or unlocks the selection.
    pub fn set_locked(&mut self, ctx: &mut BoardContext<'_>, locked: bool) -> bool {
        self.process(ctx, BoardFsmInput::SetLocked(locked))
    }

    /// Changes the line width of the selected traces, polygons and texts to
    /// the next larger (`step > 0`) or smaller (`step < 0`) width used on
    /// the board, or asks for one (`0`, or no such width).
    pub fn change_line_width(&mut self, ctx: &mut BoardContext<'_>, step: i32) -> bool {
        self.process(ctx, BoardFsmInput::ChangeLineWidth(step))
    }

    /// The answer of [`BoardRequest::LineWidthDialog`].
    pub fn set_line_width(&mut self, ctx: &mut BoardContext<'_>, width: UnsignedLength) -> bool {
        self.process(ctx, BoardFsmInput::SetLineWidth(width))
    }

    /// Resets the texts of the selected devices.
    pub fn reset_all_texts(&mut self, ctx: &mut BoardContext<'_>) -> bool {
        self.process(ctx, BoardFsmInput::ResetAllTexts)
    }

    /// Removes the selection.
    pub fn remove(&mut self, ctx: &mut BoardContext<'_>) -> bool {
        self.process(ctx, BoardFsmInput::Remove)
    }

    /// Requests the properties dialog of the selection.
    pub fn edit_properties(&mut self, ctx: &mut BoardContext<'_>) -> bool {
        self.process(ctx, BoardFsmInput::EditProperties)
    }

    /// Imports a DXF file (upstream `processImportDxf()`): its polygons
    /// (and holes) are pasted.
    pub fn import_dxf(&mut self, ctx: &mut BoardContext<'_>, settings: DxfImportSettings) -> bool {
        self.process(ctx, BoardFsmInput::ImportDxf(settings))
    }

    /// A context menu entry was chosen.
    pub fn context_menu_action(
        &mut self,
        ctx: &mut BoardContext<'_>,
        action: ContextAction,
    ) -> bool {
        self.process(ctx, BoardFsmInput::ContextMenu(action))
    }

    // --- Events ---

    /// A key was pressed.
    pub fn key_pressed(&mut self, ctx: &mut BoardContext<'_>, e: KeyEvent) -> bool {
        self.process(ctx, BoardFsmInput::KeyPressed(e))
    }

    /// A key was released.
    pub fn key_released(&mut self, ctx: &mut BoardContext<'_>, e: KeyEvent) -> bool {
        self.process(ctx, BoardFsmInput::KeyReleased(e))
    }

    /// The pointer moved.
    pub fn pointer_moved(&mut self, ctx: &mut BoardContext<'_>, e: PointerEvent) -> bool {
        self.process(ctx, BoardFsmInput::PointerMoved(e))
    }

    /// The left button was pressed.
    pub fn left_pressed(&mut self, ctx: &mut BoardContext<'_>, e: PointerEvent) -> bool {
        self.process(ctx, BoardFsmInput::LeftPressed(e))
    }

    /// The left button was released.
    pub fn left_released(&mut self, ctx: &mut BoardContext<'_>, e: PointerEvent) -> bool {
        self.process(ctx, BoardFsmInput::LeftReleased(e))
    }

    /// The left button was double-clicked.
    pub fn left_double_clicked(&mut self, ctx: &mut BoardContext<'_>, e: PointerEvent) -> bool {
        self.process(ctx, BoardFsmInput::LeftDoubleClicked(e))
    }

    /// The right button was released (without panning); switches back to
    /// the previous tool in the select tool (upstream behavior).
    pub fn right_released(&mut self, ctx: &mut BoardContext<'_>, e: PointerEvent) -> bool {
        self.process(ctx, BoardFsmInput::RightReleased(e))
    }

    /// Processes an input; returns whether it was handled.
    pub fn process(&mut self, ctx: &mut BoardContext<'_>, input: BoardFsmInput) -> bool {
        match &input {
            BoardFsmInput::PointerMoved(e)
            | BoardFsmInput::LeftReleased(e)
            | BoardFsmInput::RightReleased(e) => self.cursor_pos = Some(e.pos),
            BoardFsmInput::LeftPressed(e) | BoardFsmInput::LeftDoubleClicked(e) => {
                self.cursor_pos = Some(e.pos);
                self.left_button = true;
            }
            _ => {}
        }
        let handled = self.with_cx(ctx, |fsm, cx| fsm.dispatch(cx, &input));
        if matches!(input, BoardFsmInput::LeftReleased(_)) {
            self.left_button = false;
        }
        handled
    }

    fn with_cx<R>(
        &mut self,
        ctx: &mut BoardContext<'_>,
        f: impl FnOnce(&mut Self, &mut Cx<'_, '_>) -> R,
    ) -> R {
        let mut out = std::mem::take(&mut self.out);
        let mut selection = std::mem::take(&mut self.selection);
        let settings = self.settings.clone();
        let cursor_pos = ctx
            .view
            .cursor_pos()
            .or(self.cursor_pos)
            .unwrap_or_default();
        let result = {
            let mut cx = Cx {
                ctx,
                out: &mut out,
                selection: &mut selection,
                board: self.board,
                settings: &settings,
                cursor_pos,
                left_button: self.left_button,
                leave_requested: false,
            };
            if !self.entered {
                self.entered = true;
                if let Some(tool) = self.current {
                    self.states.get(tool).entry(&mut cx);
                }
            }
            let result = f(self, &mut cx);
            // Upstream: `requestLeavingState()` is queued.
            while cx.leave_requested {
                cx.leave_requested = false;
                self.set_state(&mut cx, BoardTool::Select);
            }
            // Keep the selection valid after model changes.
            if let Some(board) = cx.ctx.editor.project().board(cx.board) {
                cx.selection.update(board);
            }
            if !cx.is_group_active() {
                cx.rebuild_air_wires();
            }
            let tool = self.tool();
            let mut data = self.states.get_ref(tool).tool_data(&cx);
            data.tool = tool;
            cx.out.tool_data = data;
            result
        };
        self.out = out;
        self.selection = selection;
        result
    }

    fn dispatch(&mut self, cx: &mut Cx<'_, '_>, input: &BoardFsmInput) -> bool {
        match input {
            BoardFsmInput::Tool(tool) => {
                if *tool == BoardTool::AddDevice {
                    return false;
                }
                self.set_state(cx, *tool)
            }
            BoardFsmInput::AddDevice { .. } => {
                // upstream `processAddDevice()`
                let old = self.tool();
                if !self.set_state(cx, BoardTool::AddDevice) {
                    return false;
                }
                if self.states.add_device_process(cx, input) {
                    return true;
                }
                self.set_state(cx, old);
                false
            }
            BoardFsmInput::Abort => {
                let handled = self
                    .current
                    .is_some_and(|t| self.states.get(t).process(cx, input));
                handled || self.set_state(cx, BoardTool::Select)
            }
            BoardFsmInput::RightReleased(_) => {
                let handled = self
                    .current
                    .is_some_and(|t| self.states.get(t).process(cx, input));
                if handled {
                    true
                } else if self.current != Some(BoardTool::Select) {
                    // If a right click is not handled, abort the command.
                    self.dispatch(cx, &BoardFsmInput::Abort)
                } else {
                    // In the select tool, switch back to the last tool.
                    let next = match self.previous {
                        Some(p) if Some(p) != self.current => p,
                        _ => BoardTool::Select,
                    };
                    self.set_state(cx, next)
                }
            }
            _ => self
                .current
                .is_some_and(|t| self.states.get(t).process(cx, input)),
        }
    }

    fn set_state(&mut self, cx: &mut Cx<'_, '_>, tool: BoardTool) -> bool {
        if self.current == Some(tool) {
            return true;
        }
        if let Some(current) = self.current {
            if !self.states.get(current).exit(cx) {
                return false;
            }
            // Only memorize states other than select and add device.
            if !matches!(current, BoardTool::Select | BoardTool::AddDevice) {
                self.previous = Some(current);
            }
            self.current = None;
        }
        if self.states.get(tool).entry(cx) {
            self.current = Some(tool);
            true
        } else {
            // Not upstream (which stays idle): fall back to the select tool.
            if tool != BoardTool::Select && self.states.get(BoardTool::Select).entry(cx) {
                self.current = Some(BoardTool::Select);
            }
            false
        }
    }
}

impl States {
    fn add_device_process(&mut self, cx: &mut Cx<'_, '_>, input: &BoardFsmInput) -> bool {
        self.add_device.process(cx, input)
    }
}
