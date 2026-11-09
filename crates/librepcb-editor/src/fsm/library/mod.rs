//! The symbol and package editor state machines: port of
//! libs/librepcb/editor/library/sym/fsm (symboleditorfsm, symboleditorstate
//! and the states) and libs/librepcb/editor/library/pkg/fsm
//! (packageeditorfsm, packageeditorstate and the states).
//!
//! Both editors share one engine, [`LibraryEditorFsm`], parametrized by an
//! [`ElementHost`] ([`SymbolHost`] or [`PackageHost`]) which knows the
//! element's objects; the states which exist in both editors are
//! implemented once (select, draw line/rect/polygon/arc, draw circle, add
//! names/values/texts, measure), the others per host (add pins; add THT/SMT
//! pads, add holes, draw zones, re-number pads). [`SymbolEditorFsm`] and
//! [`PackageEditorFsm`] are its instances.
//!
//! Usage (like the project FSMs): the application creates one FSM per
//! element tab and calls its methods with a [`LibraryContext`] (the
//! [`LibraryElementEditor`], the view used for hit testing and the
//! clipboard). Afterwards it rebuilds its scene if the editor's
//! [`state_id()`](LibraryElementEditor::state_id) changed (elements are
//! small, a full rebuild is fine), reflects
//! [`selection()`](LibraryEditorFsm::selection) and shows
//! [`view_state()`](LibraryEditorFsm::view_state) and
//! [`tool_data()`](LibraryEditorFsm::tool_data); dialogs are requested
//! through [`take_requests()`](LibraryEditorFsm::take_requests).
//!
//! Differences to upstream:
//!
//! - The FSM owns the selection (a set of items); the view only answers
//!   hit tests ([`LibraryView`]). [`hit_test`] has a model based hit test
//!   which views can use.
//! - Interactive operations keep an undo group of the element editor open
//!   and modify the element directly (upstream: edit commands with
//!   `immediate = true`); the group is committed when the operation
//!   finishes and aborted (restoring the content) when it is canceled.
//! - Info box texts are plain text (upstream HTML).
//! - The "move/align" dialog is the application's: it gets the positions
//!   with [`LibraryEditorFsm::move_align_positions()`] and applies them
//!   with [`LibraryEditorFsm::move_align()`].
//! - Not ported: adding images, DXF import, pasting geometry into pads,
//!   resizing images.

pub mod hit_test;
pub mod package;
pub mod symbol;

mod draw_circle;
mod draw_polygon;
pub mod draw_text;
mod engine;
mod measure;
mod select;

use std::collections::BTreeSet;
use std::fmt::Debug;
use std::hash::Hash;

use librepcb_core::geometry::ComponentSide;
use librepcb_core::geometry::{
    CircleList, PadFunction, PadShape, Path, PolygonList, ZoneLayers, ZoneRules,
};
use librepcb_core::types::{
    Alignment, Angle, HAlign, Layer, Length, LengthUnit, MaskConfig, Point, PositiveLength,
    UnsignedLength, UnsignedLimitedRatio, Uuid, VAlign,
};

use super::{Clipboard, CursorShape, ViewState};
use crate::error::Result;
use crate::library_editor::commands::{ItemContainer, Transformable};
use crate::library_editor::{EditableElement, GridElement, LibraryElementEditor};

pub use draw_text::TextProps;
pub use engine::{LibraryEditorFsm, State};
pub(crate) use engine::{format_angle, format_length};
pub use package::{PackageEditorFsm, PackageHost, PadType};
pub use symbol::{SymbolEditorFsm, SymbolHost};

/// The tools of the library element editors (upstream
/// `SymbolEditorFsm::State`, `PackageEditorFsm::State`, Slint
/// `EditorTool`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum LibraryTool {
    /// Select, move, rotate, ...
    #[default]
    Select,
    /// Add symbol pins.
    AddPins,
    /// Add THT pads.
    AddThtPads,
    /// Add SMT pads of a function (standard, thermal, BGA, edge connector,
    /// test, fiducials).
    AddSmtPads(PadFunction),
    /// Add `{{NAME}}` texts.
    AddNames,
    /// Add `{{VALUE}}` texts.
    AddValues,
    /// Draw lines.
    DrawLine,
    /// Draw rectangles.
    DrawRect,
    /// Draw polygons.
    DrawPolygon,
    /// Draw circles.
    DrawCircle,
    /// Draw arcs.
    DrawArc,
    /// Add texts.
    DrawText,
    /// Draw keepout zones.
    DrawZone,
    /// Add non-plated holes.
    AddHoles,
    /// Measure distances.
    Measure,
    /// Re-number (re-connect) footprint pads.
    RenumberPads,
}

/// The tool bar data of the active tool (the `tool-*` properties of the
/// upstream symbol/package tab data). Only the fields of the active tool
/// are meaningful.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryToolData {
    /// Layer (polygons, circles, texts).
    pub layer: Layer,
    /// Layers to choose from.
    pub available_layers: Vec<Layer>,
    /// Line width (polygons, circles).
    pub line_width: UnsignedLength,
    /// Filled (polygons, circles).
    pub filled: bool,
    /// Grab area (polygons, circles).
    pub grab_area: bool,
    /// Arc angle of the next segment (polygons, arcs, zones).
    pub angle: Angle,
    /// Text (texts).
    pub text: String,
    /// Suggestions for [`text`](Self::text).
    pub text_suggestions: Vec<String>,
    /// Text height.
    pub height: PositiveLength,
    /// Stroke width (stroke texts).
    pub stroke_width: UnsignedLength,
    /// Horizontal alignment (texts).
    pub h_align: HAlign,
    /// Vertical alignment (texts).
    pub v_align: VAlign,
    /// Pin name (add pins).
    pub pin_name: String,
    /// Pin length (add pins).
    pub pin_length: UnsignedLength,
    /// Package pad (add pads).
    pub package_pad: Option<Uuid>,
    /// Component side (SMT pads).
    pub component_side: ComponentSide,
    /// Pad shape.
    pub pad_shape: PadShape,
    /// Pad width.
    pub pad_width: PositiveLength,
    /// Pad height.
    pub pad_height: PositiveLength,
    /// Pad corner radius.
    pub pad_radius: UnsignedLimitedRatio,
    /// Drill diameter (THT pads, holes).
    pub drill: PositiveLength,
    /// Pad copper clearance.
    pub copper_clearance: UnsignedLength,
    /// Pad stop mask.
    pub stop_mask: MaskConfig,
    /// Pad function.
    pub pad_function: PadFunction,
    /// Zone layers.
    pub zone_layers: ZoneLayers,
    /// Zone rules.
    pub zone_rules: ZoneRules,
}

pub(crate) fn len(nm: i64) -> UnsignedLength {
    UnsignedLength::new(Length::new(nm)).unwrap_or_default()
}

pub(crate) fn plen(nm: i64) -> PositiveLength {
    // Only called with positive constants.
    PositiveLength::new(Length::new(nm.max(1))).unwrap_or_else(|_| unreachable!())
}

impl Default for LibraryToolData {
    fn default() -> Self {
        Self {
            layer: Layer::SYMBOL_OUTLINES,
            available_layers: Vec::new(),
            line_width: len(200_000),
            filled: false,
            grab_area: false,
            angle: Angle::DEG0,
            text: String::new(),
            text_suggestions: Vec::new(),
            height: plen(2_500_000),
            stroke_width: len(200_000),
            h_align: HAlign::Left,
            v_align: VAlign::Bottom,
            pin_name: String::new(),
            pin_length: len(2_540_000),
            package_pad: None,
            component_side: ComponentSide::Top,
            pad_shape: PadShape::RoundedRect,
            pad_width: plen(2_500_000),
            pad_height: plen(1_300_000),
            pad_radius: UnsignedLimitedRatio::new(librepcb_core::types::Ratio::ZERO)
                .unwrap_or_else(|_| unreachable!()),
            drill: plen(800_000),
            copper_clearance: UnsignedLength::default(),
            stop_mask: MaskConfig::Automatic,
            pad_function: PadFunction::Unspecified,
            zone_layers: ZoneLayers::TOP,
            zone_rules: ZoneRules::all(),
        }
    }
}

impl LibraryToolData {
    /// The text alignment.
    pub fn alignment(&self) -> Alignment {
        Alignment::new(self.h_align, self.v_align)
    }
}

/// A request of the FSM to the application (dialogs and menus upstream
/// opens itself).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryRequest<I> {
    /// Show an error message.
    ShowError(String),
    /// Open the properties dialog of an item (pin, pad, polygon, ...).
    Properties(I),
    /// Show the context menu for the (selected) item at `pos`; its actions
    /// map to the FSM methods (properties, remove/add vertex, cut, copy,
    /// remove, rotate, mirror, flip, snap to grid).
    ContextMenu {
        /// The item under the cursor.
        item: I,
        /// The position (world coordinates).
        pos: Point,
        /// Indices of the polygon/zone vertices under the cursor (for
        /// "remove vertex").
        vertices: Vec<usize>,
        /// Index of the vertex after the polygon/zone segment under the
        /// cursor (for "add vertex").
        segment: Option<usize>,
    },
    /// Open the "import pins" dialog (upstream
    /// `CircuitIdentifierImportDialog`); the application calls
    /// [`SymbolEditorFsm::import_pins()`] with the names.
    ImportPinsDialog,
    /// Ask for the courtyard excess (upstream "Courtyard Excess" dialog);
    /// the application calls [`PackageEditorFsm::generate_courtyard()`].
    CourtyardOffsetDialog,
    /// Show an information message (e.g. "no content" of "generate
    /// outline").
    ShowInfo {
        /// Title.
        title: String,
        /// Text.
        text: String,
    },
}

/// Settings of the library editor FSMs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryEditorSettings {
    /// Length unit (info box texts).
    pub length_unit: LengthUnit,
    /// Application version used in the clipboard MIME types.
    pub app_version: String,
}

impl Default for LibraryEditorSettings {
    fn default() -> Self {
        Self {
            length_unit: LengthUnit::Millimeters,
            app_version: "2.1.2-unstable".to_owned(),
        }
    }
}

/// What the FSM needs from the view: hit testing on the displayed element
/// (upstream `SymbolGraphicsItem::findItemsAtPos()` /
/// `FootprintGraphicsItem::findItemsAtPos()` and
/// `fsmCalcPosWithTolerance()`). [`hit_test`] implements it on the model.
pub trait LibraryView<I> {
    /// All items at `pos` (their grab area at most `tolerance` away), the
    /// top most first (upstream priority order).
    fn items_at(&self, pos: Point, tolerance: Length) -> Vec<I>;

    /// All items touching the rectangle spanned by `p1` and `p2`.
    fn items_in_rect(&self, p1: Point, p2: Point) -> Vec<I>;

    /// The hit tolerance (world length of 5 screen pixels, see
    /// [`tolerance_for_pixel_size()`](super::tolerance_for_pixel_size)).
    fn tolerance(&self) -> Length;

    /// The current cursor position in world coordinates, if known
    /// (upstream `QCursor::pos()`, used when a tool starts).
    fn cursor_pos(&self) -> Option<Point> {
        None
    }
}

/// The context of an FSM call.
pub struct LibraryContext<'a, H: ElementHost> {
    /// The element editor (undo groups).
    pub editor: &'a mut LibraryElementEditor<H::Element>,
    /// Hit testing on the view.
    pub view: &'a dyn LibraryView<H::Item>,
    /// The clipboard.
    pub clipboard: &'a mut dyn Clipboard,
}

impl<'a, H: ElementHost> LibraryContext<'a, H> {
    /// Creates a context.
    pub fn new(
        editor: &'a mut LibraryElementEditor<H::Element>,
        view: &'a dyn LibraryView<H::Item>,
        clipboard: &'a mut dyn Clipboard,
    ) -> Self {
        Self {
            editor,
            view,
            clipboard,
        }
    }
}

/// The mode of the polygon drawing tool (upstream
/// `*EditorState_DrawPolygonBase::Mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolygonMode {
    /// Lines (open polygons).
    Line,
    /// Rectangles.
    Rect,
    /// Polygons.
    Polygon,
    /// Arcs.
    Arc,
}

/// The mode of the text tool (upstream `*EditorState_DrawTextBase::Mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextMode {
    /// `{{NAME}}`.
    Name,
    /// `{{VALUE}}`.
    Value,
    /// Any text.
    Text,
}

/// The element kind specific part of the library editor FSM: access to
/// the objects of the edited symbol or footprint, host specific states and
/// defaults. Implemented by [`SymbolHost`] and [`PackageHost`].
pub trait ElementHost: Sized + Send + Sync + Debug + 'static {
    /// The edited element.
    type Element: EditableElement + GridElement;
    /// The item identifying an object.
    type Item: Copy + Ord + Hash + Debug + Send + Sync;
    /// A complete object (for texts being placed).
    type Object: Clone + Debug + Transformable;
    /// The host specific states.
    type States: Default + Debug + Send + Sync;

    /// Whether the objects are on a board (footprints: layers can be
    /// flipped, pads).
    const IS_FOOTPRINT: bool;

    /// The polygons of the edited symbol/footprint.
    fn polygons(e: &Self::Element, fpt: Option<Uuid>) -> Option<&PolygonList>;
    /// The polygons for modification.
    fn polygons_mut(e: &mut Self::Element, fpt: Option<Uuid>) -> Option<&mut PolygonList>;
    /// The circles.
    fn circles(e: &Self::Element, fpt: Option<Uuid>) -> Option<&CircleList>;
    /// The circles for modification.
    fn circles_mut(e: &mut Self::Element, fpt: Option<Uuid>) -> Option<&mut CircleList>;
    /// The item of a polygon.
    fn polygon_item(uuid: Uuid) -> Self::Item;
    /// The item of a circle.
    fn circle_item(uuid: Uuid) -> Self::Item;
    /// The path of an item with vertices (polygons, zones).
    fn item_path(e: &Self::Element, fpt: Option<Uuid>, item: Self::Item) -> Option<Path>;
    /// Replaces the path of an item with vertices.
    fn set_item_path(e: &mut Self::Element, fpt: Option<Uuid>, item: Self::Item, path: Path);
    /// Whether the item exists.
    fn item_exists(e: &Self::Element, fpt: Option<Uuid>, item: Self::Item) -> bool;
    /// All items.
    fn all_items(e: &Self::Element, fpt: Option<Uuid>) -> BTreeSet<Self::Item>;
    /// The transformable container of the items.
    fn container(e: &Self::Element, fpt: Option<Uuid>) -> Option<&dyn ItemContainer<Self::Item>>;
    /// The transformable container for modification.
    fn container_mut(
        e: &mut Self::Element,
        fpt: Option<Uuid>,
    ) -> Option<&mut dyn ItemContainer<Self::Item>>;
    /// Removes items.
    fn remove_items(e: &mut Self::Element, fpt: Option<Uuid>, items: &BTreeSet<Self::Item>);
    /// Copies items to the clipboard; returns the MIME type and data.
    fn copy_items(
        e: &Self::Element,
        fpt: Option<Uuid>,
        items: &BTreeSet<Self::Item>,
        cursor_pos: Point,
        app_version: &str,
    ) -> Result<Option<(String, Vec<u8>)>>;
    /// Parses clipboard content; returns the data and its cursor position.
    fn clipboard_data(
        clipboard: &dyn Clipboard,
        app_version: &str,
    ) -> Result<Option<(Self::ClipboardData, Point)>>;
    /// The clipboard data type.
    type ClipboardData;
    /// Pastes clipboard data with an offset; returns the pasted items.
    fn paste(
        e: &mut Self::Element,
        fpt: Option<Uuid>,
        data: Self::ClipboardData,
        offset: Point,
    ) -> Result<BTreeSet<Self::Item>>;

    /// The image of an item (symbol images; `None` for other items).
    fn image(
        _e: &Self::Element,
        _fpt: Option<Uuid>,
        _item: Self::Item,
    ) -> Option<librepcb_core::geometry::Image> {
        None
    }
    /// Replaces the image with the UUID of `image` (see
    /// [`image()`](Self::image)).
    fn set_image(
        _e: &mut Self::Element,
        _fpt: Option<Uuid>,
        _image: librepcb_core::geometry::Image,
    ) {
    }

    /// Whether pasting `data` applies its geometry to the selected items
    /// instead of inserting it (upstream `canPasteGeometry()` of the package
    /// editor): `data` holds exactly one object, and objects of its kind
    /// other than it are selected. Only footprints support it.
    fn can_paste_geometry(
        _e: &Self::Element,
        _fpt: Option<Uuid>,
        _data: &Self::ClipboardData,
        _selection: &BTreeSet<Self::Item>,
    ) -> bool {
        false
    }
    /// Applies the geometry of the object in `data` to the selected objects
    /// of its kind (upstream `pasteGeometryFromClipboard()`).
    fn paste_geometry(
        _e: &mut Self::Element,
        _fpt: Option<Uuid>,
        _data: &Self::ClipboardData,
        _selection: &BTreeSet<Self::Item>,
    ) {
    }

    /// The object of an item.
    fn object(e: &Self::Element, fpt: Option<Uuid>, item: Self::Item) -> Option<Self::Object>;
    /// Adds an object.
    fn add_object(
        e: &mut Self::Element,
        fpt: Option<Uuid>,
        obj: Self::Object,
    ) -> Result<Self::Item>;
    /// Replaces an object.
    fn update_object(e: &mut Self::Element, fpt: Option<Uuid>, obj: Self::Object) -> Result<()>;
    /// Creates a text object (with a new UUID) for the text tool.
    fn new_text(props: &TextProps, pos: Point) -> Self::Object;
    /// Gives `obj` the UUID of `from`.
    fn set_object_uuid(obj: &mut Self::Object, from: &Self::Object);
    /// Reads the properties of a text object back (rotation, alignment,
    /// mirrored) after it was transformed.
    fn text_properties(obj: &Self::Object) -> (Angle, Alignment, bool);

    /// The layers allowed for polygons and circles.
    fn polygon_layers() -> Vec<Layer>;
    /// The layers allowed for texts.
    fn text_layers() -> Vec<Layer>;
    /// Default properties of the polygon tool (layer, width, filled, grab
    /// area).
    fn polygon_defaults(mode: PolygonMode) -> (Layer, UnsignedLength, bool, bool);
    /// Default properties of the circle tool.
    fn circle_defaults() -> (Layer, UnsignedLength, bool, bool);
    /// Default properties of the text tool.
    fn text_defaults(mode: TextMode) -> TextProps;
    /// Suggestions for the text tool.
    fn text_suggestions(mode: TextMode) -> Vec<String>;
    /// Snap candidates of the measure tool.
    fn measure_snap_candidates(e: &Self::Element, fpt: Option<Uuid>) -> BTreeSet<Point>;

    /// Undo text and translation context for adding an object kind
    /// (`"polygon"`, `"circle"`, `"text"`).
    fn add_text(kind: &str) -> String;
    /// Undo text of pasting.
    fn paste_text() -> String;

    /// The host specific state of a tool.
    fn state(states: &mut Self::States, tool: LibraryTool) -> Option<&mut dyn State<Self>>;
}

/// The outputs of the FSM, shared by all states.
#[derive(Debug)]
pub struct Output<I> {
    pub(crate) view: ViewState,
    pub(crate) selection: BTreeSet<I>,
    pub(crate) hovered: Option<I>,
    pub(crate) tool: LibraryTool,
    pub(crate) tool_data: LibraryToolData,
    pub(crate) requests: Vec<LibraryRequest<I>>,
    pub(crate) leave_requested: bool,
    pub(crate) last_pos: Option<Point>,
    pub(crate) footprint: Option<Uuid>,
}

impl<I> Default for Output<I> {
    fn default() -> Self {
        Self {
            view: ViewState::default(),
            selection: BTreeSet::new(),
            hovered: None,
            tool: LibraryTool::Select,
            tool_data: LibraryToolData::default(),
            requests: Vec::new(),
            leave_requested: false,
            last_pos: None,
            footprint: None,
        }
    }
}

/// What a state gets in its handlers.
pub struct Cx<'c, 'a, H: ElementHost> {
    pub(crate) ctx: &'c mut LibraryContext<'a, H>,
    pub(crate) out: &'c mut Output<H::Item>,
    pub(crate) settings: &'c LibraryEditorSettings,
}

impl<H: ElementHost> Cx<'_, '_, H> {
    pub(crate) fn element(&self) -> &H::Element {
        self.ctx.editor.element()
    }

    pub(crate) fn fpt(&self) -> Option<Uuid> {
        self.out.footprint
    }

    /// Grid interval of the element.
    pub(crate) fn grid(&self) -> PositiveLength {
        self.element().grid_interval()
    }

    pub(crate) fn unit(&self) -> LengthUnit {
        self.settings.length_unit
    }

    /// The cursor position (from the view, else the last pointer event).
    pub(crate) fn cursor_pos(&self) -> Point {
        self.ctx
            .view
            .cursor_pos()
            .or(self.out.last_pos)
            .unwrap_or_default()
    }

    pub(crate) fn error(&mut self, e: impl std::fmt::Display) {
        let msg = e.to_string();
        log::warn!("Library editor: {msg}");
        self.out.requests.push(LibraryRequest::ShowError(msg));
    }

    pub(crate) fn set_cursor(&mut self, shape: Option<CursorShape>) {
        self.out.view.cursor = shape;
    }

    pub(crate) fn writable(&self) -> bool {
        self.ctx.editor.is_writable()
    }

    /// Starts an undo group; reports errors.
    pub(crate) fn begin(&mut self, text: String) -> bool {
        match self.ctx.editor.begin_group(text) {
            Ok(()) => true,
            Err(e) => {
                self.error(e);
                false
            }
        }
    }

    /// Commits the active undo group; reports errors.
    pub(crate) fn commit(&mut self) -> bool {
        match self.ctx.editor.commit_group() {
            Ok(_) => true,
            Err(e) => {
                self.error(e);
                false
            }
        }
    }

    /// Aborts the active undo group (if any); reports errors.
    pub(crate) fn abort_group(&mut self) -> bool {
        if !self.ctx.editor.is_group_active() {
            return true;
        }
        match self.ctx.editor.abort_group() {
            Ok(()) => true,
            Err(e) => {
                self.error(e);
                false
            }
        }
    }

    /// Modifies the element in the active group.
    pub(crate) fn modify<R>(
        &mut self,
        f: impl FnOnce(&mut H::Element, Option<Uuid>) -> R,
    ) -> Option<R> {
        let fpt = self.fpt();
        match self.ctx.editor.modify(|e| f(e, fpt)) {
            Ok(r) => Some(r),
            Err(e) => {
                self.error(e);
                None
            }
        }
    }

    /// Executes a command as its own undo group (or in the active one).
    pub(crate) fn execute<C: crate::library_editor::ElementCommand<H::Element>>(
        &mut self,
        cmd: C,
    ) -> Option<C::Output> {
        match self.ctx.editor.execute(cmd) {
            Ok(o) => Some(o),
            Err(e) => {
                self.error(e);
                None
            }
        }
    }

    /// Runs `f` as one undo group (upstream `execCmd()` of an edit command).
    pub(crate) fn exec_group<R>(
        &mut self,
        text: String,
        f: impl FnOnce(&mut H::Element, Option<Uuid>) -> Result<R>,
    ) -> Option<R> {
        let fpt = self.fpt();
        self.execute(FnCommand { text, fpt, f })
    }
}

/// A command from a closure (for FSM internal edits).
struct FnCommand<F> {
    text: String,
    fpt: Option<Uuid>,
    f: F,
}

impl<E, R, F> crate::library_editor::ElementCommand<E> for FnCommand<F>
where
    F: FnOnce(&mut E, Option<Uuid>) -> Result<R>,
{
    type Output = R;

    fn text(&self) -> String {
        self.text.clone()
    }

    fn execute(self, element: &mut E) -> Result<R> {
        (self.f)(element, self.fpt)
    }
}
