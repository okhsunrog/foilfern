//! The state machine engine shared by the symbol and package editors: port
//! of libs/librepcb/editor/library/sym/fsm/symboleditorfsm.{h,cpp},
//! symboleditorstate.{h,cpp}, libs/librepcb/editor/library/pkg/fsm/
//! packageeditorfsm.{h,cpp} and packageeditorstate.{h,cpp}.

use std::collections::BTreeSet;
use std::fmt::Debug;

use librepcb_core::types::{Angle, Length, LengthUnit, Orientation, Point, Uuid};

use super::{
    Cx, ElementHost, LibraryContext, LibraryEditorSettings, LibraryRequest, LibraryTool,
    LibraryToolData, Output, PolygonMode, TextMode, draw_circle, draw_polygon, draw_text, measure,
    select,
};
use crate::fsm::{KeyEvent, PointerEvent, StatusMessage, ViewState};

/// The event handlers of a state (upstream `SymbolEditorState` /
/// `PackageEditorState`); each returns whether the event was handled.
#[allow(unused_variables)]
pub trait State<H: ElementHost>: Debug + Send + Sync {
    /// Enters the state; returns `false` if it cannot be entered.
    fn entry(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        true
    }
    /// Leaves the state; returns `false` if it cannot be left.
    fn exit(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        true
    }
    /// Aborts the current command (Escape).
    fn abort(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        false
    }
    /// Accepts the current command (Enter).
    fn accept(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        false
    }
    /// Select all.
    fn select_all(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        false
    }
    /// Cut.
    fn cut(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        false
    }
    /// Copy.
    fn copy(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        false
    }
    /// Paste.
    fn paste(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        false
    }
    /// Move by an offset (arrow keys).
    fn move_by(&mut self, cx: &mut Cx<'_, '_, H>, delta: Point) -> bool {
        false
    }
    /// Rotate.
    fn rotate(&mut self, cx: &mut Cx<'_, '_, H>, angle: Angle) -> bool {
        false
    }
    /// Mirror (geometry only).
    fn mirror(&mut self, cx: &mut Cx<'_, '_, H>, orientation: Orientation) -> bool {
        false
    }
    /// Flip (mirror geometry and move to the other board side).
    fn flip(&mut self, cx: &mut Cx<'_, '_, H>, orientation: Orientation) -> bool {
        false
    }
    /// Snap to grid.
    fn snap_to_grid(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        false
    }
    /// Remove.
    fn remove(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        false
    }
    /// Edit properties.
    fn edit_properties(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        false
    }
    /// A key was pressed.
    fn key_pressed(&mut self, cx: &mut Cx<'_, '_, H>, e: KeyEvent) -> bool {
        false
    }
    /// A key was released.
    fn key_released(&mut self, cx: &mut Cx<'_, '_, H>, e: KeyEvent) -> bool {
        false
    }
    /// The pointer moved.
    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_, H>, e: PointerEvent) -> bool {
        false
    }
    /// Left button pressed.
    fn left_pressed(&mut self, cx: &mut Cx<'_, '_, H>, e: PointerEvent) -> bool {
        false
    }
    /// Left button released.
    fn left_released(&mut self, cx: &mut Cx<'_, '_, H>, e: PointerEvent) -> bool {
        false
    }
    /// Left button double-clicked.
    fn left_double_clicked(&mut self, cx: &mut Cx<'_, '_, H>, e: PointerEvent) -> bool {
        false
    }
    /// Right button released.
    fn right_released(&mut self, cx: &mut Cx<'_, '_, H>, e: PointerEvent) -> bool {
        false
    }
    /// The tool bar data changed (`old`: before the change); the state
    /// takes over the new values and updates the object being placed.
    fn tool_data_changed(&mut self, cx: &mut Cx<'_, '_, H>, old: &LibraryToolData) {}
    /// Called after every event (feature updates).
    fn update(&mut self, cx: &mut Cx<'_, '_, H>) {}
}

/// All states of an FSM.
#[derive(Debug)]
struct States<H: ElementHost> {
    select: select::SelectState<H>,
    line: draw_polygon::DrawPolygonState,
    rect: draw_polygon::DrawPolygonState,
    polygon: draw_polygon::DrawPolygonState,
    arc: draw_polygon::DrawPolygonState,
    circle: draw_circle::DrawCircleState,
    names: draw_text::DrawTextState<H>,
    values: draw_text::DrawTextState<H>,
    texts: draw_text::DrawTextState<H>,
    measure: measure::MeasureState,
    host: H::States,
}

impl<H: ElementHost> Default for States<H> {
    fn default() -> Self {
        Self {
            select: select::SelectState::default(),
            line: draw_polygon::DrawPolygonState::new::<H>(PolygonMode::Line),
            rect: draw_polygon::DrawPolygonState::new::<H>(PolygonMode::Rect),
            polygon: draw_polygon::DrawPolygonState::new::<H>(PolygonMode::Polygon),
            arc: draw_polygon::DrawPolygonState::new::<H>(PolygonMode::Arc),
            circle: draw_circle::DrawCircleState::new::<H>(),
            names: draw_text::DrawTextState::new(TextMode::Name),
            values: draw_text::DrawTextState::new(TextMode::Value),
            texts: draw_text::DrawTextState::new(TextMode::Text),
            measure: measure::MeasureState::default(),
            host: H::States::default(),
        }
    }
}

impl<H: ElementHost> States<H> {
    fn get(&mut self, tool: Option<LibraryTool>) -> Option<&mut dyn State<H>> {
        Some(match tool? {
            LibraryTool::Select => &mut self.select,
            LibraryTool::DrawLine => &mut self.line,
            LibraryTool::DrawRect => &mut self.rect,
            LibraryTool::DrawPolygon => &mut self.polygon,
            LibraryTool::DrawArc => &mut self.arc,
            LibraryTool::DrawCircle => &mut self.circle,
            LibraryTool::AddNames => &mut self.names,
            LibraryTool::AddValues => &mut self.values,
            LibraryTool::DrawText => &mut self.texts,
            LibraryTool::Measure => &mut self.measure,
            other => return H::state(&mut self.host, other),
        })
    }
}

/// The library element editor state machine (upstream `SymbolEditorFsm`,
/// `PackageEditorFsm`).
#[derive(Debug)]
pub struct LibraryEditorFsm<H: ElementHost> {
    settings: LibraryEditorSettings,
    /// `None`: idle (between leaving and entering a state).
    current: Option<LibraryTool>,
    previous: Option<LibraryTool>,
    entered: bool,
    states: States<H>,
    out: Output<H::Item>,
}

macro_rules! lib_dispatch {
    ($self:ident, $ctx:ident, |$state:ident, $cx:ident| $body:expr) => {{
        let tool = $self.ensure_entered($ctx);
        let handled = if H::IS_FOOTPRINT && $self.out.footprint.is_none() {
            false
        } else {
            let mut $cx = Cx {
                ctx: $ctx,
                out: &mut $self.out,
                settings: &$self.settings,
            };
            match $self.states.get(tool) {
                Some($state) => $body,
                None => false,
            }
        };
        $self.after_event($ctx);
        handled
    }};
}

impl<H: ElementHost> LibraryEditorFsm<H> {
    /// Creates the FSM; it starts in the select state.
    pub fn new(settings: LibraryEditorSettings) -> Self {
        Self {
            settings,
            current: Some(LibraryTool::Select),
            previous: None,
            entered: false,
            states: States::default(),
            out: Output::default(),
        }
    }

    /// The settings.
    pub fn settings(&self) -> &LibraryEditorSettings {
        &self.settings
    }

    /// Changes the settings.
    pub fn set_settings(&mut self, settings: LibraryEditorSettings) {
        self.settings = settings;
    }

    // --- Outputs ---

    /// The active tool.
    pub fn tool(&self) -> LibraryTool {
        self.out.tool
    }

    /// The tool bar data of the active tool.
    pub fn tool_data(&self) -> &LibraryToolData {
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

    /// The selected items.
    pub fn selection(&self) -> &BTreeSet<H::Item> {
        &self.out.selection
    }

    /// Replaces the selection.
    pub fn set_selection(&mut self, items: impl IntoIterator<Item = H::Item>) {
        self.out.selection = items.into_iter().collect();
    }

    /// The item under the cursor in the select state.
    pub fn hovered(&self) -> Option<H::Item> {
        self.out.hovered
    }

    /// Takes the pending requests (dialogs, errors, menus).
    pub fn take_requests(&mut self) -> Vec<LibraryRequest<H::Item>> {
        std::mem::take(&mut self.out.requests)
    }

    /// Removes items which no longer exist from the selection; call after
    /// changing the element outside the FSM (undo, redo, commands).
    pub fn element_changed(&mut self, element: &H::Element) {
        let fpt = self.out.footprint;
        self.out
            .selection
            .retain(|i| H::item_exists(element, fpt, *i));
        if self
            .out
            .hovered
            .is_some_and(|i| !H::item_exists(element, fpt, i))
        {
            self.out.hovered = None;
        }
    }

    // --- Tool switching ---

    /// Switches to a tool (upstream `processStart*()`); returns `false` if
    /// the current tool cannot be left or the new one cannot be entered
    /// (e.g. a package tool without footprint).
    pub fn set_tool(&mut self, ctx: &mut LibraryContext<'_, H>, tool: LibraryTool) -> bool {
        self.ensure_entered(ctx);
        let ok = self.set_next_state(ctx, Some(tool));
        self.after_event(ctx);
        ok
    }

    /// Switches to the select tool (upstream `processStartSelecting()`).
    pub fn select_tool(&mut self, ctx: &mut LibraryContext<'_, H>) -> bool {
        self.set_tool(ctx, LibraryTool::Select)
    }

    // --- Actions ---

    /// Aborts the current command, or goes back to the select tool
    /// (upstream `processAbortCommand()`, Escape).
    pub fn abort(&mut self, ctx: &mut LibraryContext<'_, H>) -> bool {
        let tool = self.ensure_entered(ctx);
        let handled = {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                settings: &self.settings,
            };
            self.states.get(tool).is_some_and(|s| s.abort(&mut cx))
        };
        let result = handled || self.set_next_state(ctx, Some(LibraryTool::Select));
        self.after_event(ctx);
        result
    }

    /// Accepts the current command (upstream `processAcceptCommand()`,
    /// Enter; e.g. finishes re-numbering pads).
    pub fn accept(&mut self, ctx: &mut LibraryContext<'_, H>) -> bool {
        lib_dispatch!(self, ctx, |s, cx| s.accept(&mut cx))
    }

    /// Selects all items.
    pub fn select_all(&mut self, ctx: &mut LibraryContext<'_, H>) -> bool {
        lib_dispatch!(self, ctx, |s, cx| s.select_all(&mut cx))
    }

    /// Cuts the selected items to the clipboard.
    pub fn cut(&mut self, ctx: &mut LibraryContext<'_, H>) -> bool {
        lib_dispatch!(self, ctx, |s, cx| s.cut(&mut cx))
    }

    /// Copies the selected items (or the measured distance) to the
    /// clipboard.
    pub fn copy(&mut self, ctx: &mut LibraryContext<'_, H>) -> bool {
        lib_dispatch!(self, ctx, |s, cx| s.copy(&mut cx))
    }

    /// Pastes items from the clipboard; they follow the cursor until the
    /// next click.
    pub fn paste(&mut self, ctx: &mut LibraryContext<'_, H>) -> bool {
        lib_dispatch!(self, ctx, |s, cx| s.paste(&mut cx))
    }

    /// Whether the clipboard holds one object whose geometry can be pasted
    /// onto the selected objects (upstream `canPasteGeometry()`, package
    /// editor; for the "Paste Geometry" context menu entry). [`paste()`](Self::paste)
    /// then applies the geometry instead of inserting the object.
    pub fn can_paste_geometry(&mut self, ctx: &mut LibraryContext<'_, H>) -> bool {
        self.with_select_state(ctx, |s, cx| s.can_paste_geometry(cx))
            .unwrap_or(false)
    }

    /// Applies the geometry of the clipboard object to the selected objects
    /// of its kind (upstream `pasteGeometryFromClipboard()`, "Paste
    /// Geometry" in the context menu).
    pub fn paste_geometry(&mut self, ctx: &mut LibraryContext<'_, H>) -> bool {
        self.with_select_state(ctx, |s, cx| s.paste_geometry(cx))
            .unwrap_or(false)
    }

    /// Moves the selected items (arrow keys).
    pub fn move_by(&mut self, ctx: &mut LibraryContext<'_, H>, delta: Point) -> bool {
        lib_dispatch!(self, ctx, |s, cx| s.move_by(&mut cx, delta))
    }

    /// Rotates the selected items or the item being placed.
    pub fn rotate(&mut self, ctx: &mut LibraryContext<'_, H>, angle: Angle) -> bool {
        lib_dispatch!(self, ctx, |s, cx| s.rotate(&mut cx, angle))
    }

    /// Mirrors the selected items or the item being placed.
    pub fn mirror(&mut self, ctx: &mut LibraryContext<'_, H>, orientation: Orientation) -> bool {
        lib_dispatch!(self, ctx, |s, cx| s.mirror(&mut cx, orientation))
    }

    /// Snaps the selected items to the grid.
    pub fn snap_to_grid(&mut self, ctx: &mut LibraryContext<'_, H>) -> bool {
        lib_dispatch!(self, ctx, |s, cx| s.snap_to_grid(&mut cx))
    }

    /// Removes the selected items (or clears the measurement).
    pub fn remove(&mut self, ctx: &mut LibraryContext<'_, H>) -> bool {
        lib_dispatch!(self, ctx, |s, cx| s.remove(&mut cx))
    }

    /// Requests the properties dialog of the selected item.
    pub fn edit_properties(&mut self, ctx: &mut LibraryContext<'_, H>) -> bool {
        lib_dispatch!(self, ctx, |s, cx| s.edit_properties(&mut cx))
    }

    /// Removes vertices of a polygon or zone (context menu "remove
    /// vertex").
    pub fn remove_vertices(
        &mut self,
        ctx: &mut LibraryContext<'_, H>,
        item: H::Item,
        vertices: &BTreeSet<usize>,
    ) -> bool {
        self.with_select_state(ctx, |s, cx| s.remove_vertices(cx, item, vertices))
            .unwrap_or(false)
    }

    /// Inserts a vertex into a polygon or zone before vertex `index` at
    /// `pos` and starts moving it (context menu "add vertex").
    pub fn add_vertex(
        &mut self,
        ctx: &mut LibraryContext<'_, H>,
        item: H::Item,
        index: usize,
        pos: Point,
    ) -> bool {
        self.with_select_state(ctx, |s, cx| s.start_adding_vertex(cx, item, index, pos))
            .unwrap_or(false)
    }

    /// The positions of the selected items for the "move/align" dialog
    /// (upstream `processMoveAlign()`); `None` if nothing is selected or
    /// another tool is active.
    pub fn move_align_positions(&mut self, ctx: &mut LibraryContext<'_, H>) -> Option<Vec<Point>> {
        self.with_select_state(ctx, |s, cx| s.move_align_positions(cx))
            .flatten()
    }

    /// Moves the selected items to the positions chosen in the
    /// "move/align" dialog (same order as
    /// [`move_align_positions()`](Self::move_align_positions)).
    pub fn move_align(&mut self, ctx: &mut LibraryContext<'_, H>, positions: &[Point]) -> bool {
        self.with_select_state(ctx, |s, cx| s.move_align(cx, positions))
            .unwrap_or(false)
    }

    // --- Input events ---

    /// A key was pressed.
    pub fn key_pressed(&mut self, ctx: &mut LibraryContext<'_, H>, e: KeyEvent) -> bool {
        lib_dispatch!(self, ctx, |s, cx| s.key_pressed(&mut cx, e))
    }

    /// A key was released.
    pub fn key_released(&mut self, ctx: &mut LibraryContext<'_, H>, e: KeyEvent) -> bool {
        lib_dispatch!(self, ctx, |s, cx| s.key_released(&mut cx, e))
    }

    /// The pointer moved.
    pub fn pointer_moved(&mut self, ctx: &mut LibraryContext<'_, H>, e: PointerEvent) -> bool {
        self.out.last_pos = Some(e.pos);
        lib_dispatch!(self, ctx, |s, cx| s.pointer_moved(&mut cx, e))
    }

    /// The left button was pressed.
    pub fn left_pressed(&mut self, ctx: &mut LibraryContext<'_, H>, e: PointerEvent) -> bool {
        self.out.last_pos = Some(e.pos);
        lib_dispatch!(self, ctx, |s, cx| s.left_pressed(&mut cx, e))
    }

    /// The left button was released.
    pub fn left_released(&mut self, ctx: &mut LibraryContext<'_, H>, e: PointerEvent) -> bool {
        self.out.last_pos = Some(e.pos);
        lib_dispatch!(self, ctx, |s, cx| s.left_released(&mut cx, e))
    }

    /// The left button was double-clicked.
    pub fn left_double_clicked(
        &mut self,
        ctx: &mut LibraryContext<'_, H>,
        e: PointerEvent,
    ) -> bool {
        self.out.last_pos = Some(e.pos);
        lib_dispatch!(self, ctx, |s, cx| s.left_double_clicked(&mut cx, e))
    }

    /// A click (move, press and release) at `e`: convenience for scripted
    /// input and tests.
    pub fn click(&mut self, ctx: &mut LibraryContext<'_, H>, e: PointerEvent) -> bool {
        let a = self.pointer_moved(ctx, e);
        let b = self.left_pressed(ctx, e);
        let c = self.left_released(ctx, e);
        a || b || c
    }

    /// The right button was released: handled by the state (e.g. rotate
    /// while placing), otherwise aborts the tool, or in the select state
    /// switches back to the previous tool.
    pub fn right_released(&mut self, ctx: &mut LibraryContext<'_, H>, e: PointerEvent) -> bool {
        self.out.last_pos = Some(e.pos);
        let tool = self.ensure_entered(ctx);
        if H::IS_FOOTPRINT && self.out.footprint.is_none() {
            return false;
        }
        let handled = {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                settings: &self.settings,
            };
            self.states
                .get(tool)
                .is_some_and(|s| s.right_released(&mut cx, e))
        };
        let result = if handled {
            true
        } else if tool != Some(LibraryTool::Select) {
            self.after_event(ctx);
            return self.abort(ctx);
        } else {
            self.switch_to_previous_state(ctx)
        };
        self.after_event(ctx);
        result
    }

    // --- Tool bar ---

    /// Modifies the tool bar data (layer, width, text, pad properties,
    /// ...); the active tool takes over the new values.
    pub fn update_tool_data(
        &mut self,
        ctx: &mut LibraryContext<'_, H>,
        f: impl FnOnce(&mut LibraryToolData),
    ) {
        let tool = self.ensure_entered(ctx);
        let old = self.out.tool_data.clone();
        f(&mut self.out.tool_data);
        if self.out.tool_data != old {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                settings: &self.settings,
            };
            if let Some(s) = self.states.get(tool) {
                s.tool_data_changed(&mut cx, &old);
            }
        }
        self.after_event(ctx);
    }

    // --- State handling ---

    /// Enters the initial state on first use (it needs a context).
    fn ensure_entered(&mut self, ctx: &mut LibraryContext<'_, H>) -> Option<LibraryTool> {
        if !self.entered {
            self.entered = true;
            let tool = self.current;
            self.current = None;
            if !self.enter_next_state(ctx, tool) {
                self.enter_next_state(ctx, Some(LibraryTool::Select));
            }
        }
        self.current
    }

    fn set_next_state(
        &mut self,
        ctx: &mut LibraryContext<'_, H>,
        tool: Option<LibraryTool>,
    ) -> bool {
        if tool == self.current {
            return true;
        }
        if !self.leave_current_state(ctx) {
            return false;
        }
        if !self.enter_next_state(ctx, tool) {
            self.enter_next_state(ctx, Some(LibraryTool::Select));
            return false;
        }
        true
    }

    fn leave_current_state(&mut self, ctx: &mut LibraryContext<'_, H>) -> bool {
        let tool = self.current;
        let ok = {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                settings: &self.settings,
            };
            self.states.get(tool).is_none_or(|s| s.exit(&mut cx))
        };
        if !ok {
            return false;
        }
        // Only memorize states other than select.
        if tool.is_some_and(|t| t != LibraryTool::Select) {
            self.previous = tool;
        }
        self.current = None;
        self.out.tool = LibraryTool::Select;
        self.out.view.cursor = None;
        self.out.view.scene_cursor = None;
        self.out.view.rubber_band = None;
        self.out.view.ruler = None;
        self.out.view.gray_out = false;
        self.out.view.info_box.clear();
        self.out.view.features = Default::default();
        self.out.hovered = None;
        true
    }

    fn enter_next_state(
        &mut self,
        ctx: &mut LibraryContext<'_, H>,
        tool: Option<LibraryTool>,
    ) -> bool {
        // Package tools other than select need a footprint.
        if H::IS_FOOTPRINT
            && self.out.footprint.is_none()
            && tool.is_some_and(|t| t != LibraryTool::Select)
        {
            return false;
        }
        let ok = {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                settings: &self.settings,
            };
            self.states.get(tool).is_none_or(|s| s.entry(&mut cx))
        };
        if ok {
            self.current = tool;
            if let Some(t) = tool {
                self.out.tool = t;
            }
        }
        ok
    }

    fn switch_to_previous_state(&mut self, ctx: &mut LibraryContext<'_, H>) -> bool {
        let mut next = self.previous;
        if next == self.current || next.is_none() {
            next = Some(LibraryTool::Select);
        }
        self.set_next_state(ctx, next)
    }

    /// Processes queued state requests and lets the state update its
    /// outputs.
    fn after_event(&mut self, ctx: &mut LibraryContext<'_, H>) {
        if std::mem::take(&mut self.out.leave_requested) {
            self.set_next_state(ctx, Some(LibraryTool::Select));
        }
        self.element_changed(ctx.editor.element());
        let tool = self.current;
        let mut cx = Cx {
            ctx,
            out: &mut self.out,
            settings: &self.settings,
        };
        if let Some(state) = self.states.get(tool) {
            state.update(&mut cx);
        }
    }

    /// Changes the edited footprint (packages; upstream
    /// `processChangeCurrentFootprint()`): the current tool is left and
    /// entered again for the new footprint (select tool if `None`).
    pub(crate) fn change_footprint(
        &mut self,
        ctx: &mut LibraryContext<'_, H>,
        footprint: Option<Uuid>,
    ) -> bool {
        self.ensure_entered(ctx);
        if footprint == self.out.footprint {
            return false;
        }
        let previous = self.current;
        if !self.leave_current_state(ctx) {
            return false;
        }
        self.out.footprint = footprint;
        self.out.selection.clear();
        let next = if footprint.is_some() {
            previous
        } else {
            Some(LibraryTool::Select)
        };
        let ok = if self.enter_next_state(ctx, next) {
            true
        } else {
            self.enter_next_state(ctx, Some(LibraryTool::Select));
            false
        };
        self.after_event(ctx);
        ok
    }

    /// Runs `f` with the host specific states (host specific FSM methods).
    pub(crate) fn with_host_state<R>(
        &mut self,
        ctx: &mut LibraryContext<'_, H>,
        f: impl FnOnce(&mut H::States, Option<LibraryTool>, &mut Cx<'_, '_, H>) -> R,
    ) -> R {
        let tool = self.ensure_entered(ctx);
        let r = {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                settings: &self.settings,
            };
            f(&mut self.states.host, tool, &mut cx)
        };
        self.after_event(ctx);
        r
    }

    /// The current footprint (packages).
    pub(crate) fn current_footprint(&self) -> Option<Uuid> {
        self.out.footprint
    }

    /// Runs `f` in the select state (e.g. flip, context menu actions);
    /// `None` if another tool is active.
    pub(crate) fn with_select_state<R>(
        &mut self,
        ctx: &mut LibraryContext<'_, H>,
        f: impl FnOnce(&mut select::SelectState<H>, &mut Cx<'_, '_, H>) -> R,
    ) -> Option<R> {
        let tool = self.ensure_entered(ctx);
        let r = (tool == Some(LibraryTool::Select)
            && !(H::IS_FOOTPRINT && self.out.footprint.is_none()))
        .then(|| {
            let mut cx = Cx {
                ctx,
                out: &mut self.out,
                settings: &self.settings,
            };
            f(&mut self.states.select, &mut cx)
        });
        self.after_event(ctx);
        r
    }
}

/// Formats a length for the info box (upstream `formatLength` lambdas of
/// the drawing states): `"<name>: <value> <unit>"`, right aligned.
pub(crate) fn format_length(unit: LengthUnit, name: &str, value: Length) -> String {
    let decimals = unit.reasonable_number_of_decimals();
    let width = 11usize.saturating_sub(name.chars().count());
    format!(
        "{name}: {:>width$.decimals$} {}",
        unit.convert_to_unit(value),
        unit.to_short_str()
    )
}

/// Formats an angle for the info box.
pub(crate) fn format_angle(unit: LengthUnit, name: &str, value: Angle) -> String {
    let decimals = unit.reasonable_number_of_decimals();
    let width = 14usize
        .saturating_sub(decimals)
        .saturating_sub(name.chars().count());
    format!("{name}: {:>width$.3}°", value.to_deg())
}
