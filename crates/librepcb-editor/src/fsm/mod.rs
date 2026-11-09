//! Interactive editor state machines (port of
//! libs/librepcb/editor/project/{schematic,board}/fsm and the event types
//! of libs/librepcb/editor/graphics/graphicsscene.h).
//!
//! The state machines are UI-toolkit independent (see
//! `docs/ui-design.md`, decision 2): the application translates its pointer
//! and key events into the types of this module (positions already mapped
//! to world coordinates), calls the FSM, and afterwards reads the FSM's
//! [`ViewState`] (cursor, overlays, info box text, status bar message,
//! enabled features) and its tool data. Model changes go through the
//! [`ProjectEditor`](crate::ProjectEditor) as undo groups; interactive
//! operations (dragging, drawing a wire, placing a component) keep a group
//! open and apply mutations to it while the pointer moves, like upstream's
//! `CmdDrag*`/`beginCmdGroup()`, so the scene (updated from the change
//! journal) is the live preview. The group is committed when the operation
//! finishes and aborted when it is canceled.
//!
//! Shared here: input events ([`PointerEvent`], [`KeyEvent`],
//! [`Modifiers`], [`Key`]), outputs ([`ViewState`], [`CursorShape`],
//! [`SceneCursor`], [`StatusMessage`], [`Features`]) and the
//! [`Clipboard`]. Hit testing is specific to each editor (a trait per FSM,
//! implemented by the application on its scene).
//!
//! Differences to upstream: no adapter object with callbacks
//! (`SchematicEditorFsmAdapter`); outputs are plain data which the
//! application polls after each call. Dialogs are not opened by the FSM;
//! it reports requests instead (see the per-editor request types).

pub mod board;
pub mod find;
pub mod library;
pub mod measure;
pub mod schematic;

use librepcb_core::types::{Length, Point};

/// The objects of the selection to highlight in the other editors
/// (upstream `fsmCrossProbe()` of `processSelection()`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CrossProbe {
    /// Nets of the selected wires, labels, traces, vias and planes.
    pub nets: std::collections::BTreeSet<librepcb_core::project::NetSignalId>,
    /// Components of the selected symbols or devices.
    pub components: std::collections::BTreeSet<librepcb_core::project::ComponentInstanceId>,
    /// Component signals of the selected pins or pads.
    pub component_signals: std::collections::BTreeSet<librepcb_core::project::ComponentSignalRef>,
    /// Buses of the selected bus lines and labels (schematic only).
    pub buses: std::collections::BTreeSet<librepcb_core::project::BusId>,
}

impl CrossProbe {
    /// Whether nothing is to be highlighted.
    pub fn is_empty(&self) -> bool {
        self.nets.is_empty()
            && self.components.is_empty()
            && self.component_signals.is_empty()
            && self.buses.is_empty()
    }
}

/// Keyboard modifiers of an input event (upstream `Qt::KeyboardModifiers`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    /// Shift is pressed.
    pub shift: bool,
    /// Control (Command on macOS) is pressed.
    pub control: bool,
    /// Alt (Option on macOS) is pressed.
    pub alt: bool,
}

impl Modifiers {
    /// No modifier pressed.
    pub const NONE: Self = Self {
        shift: false,
        control: false,
        alt: false,
    };
    /// Only Shift pressed.
    pub const SHIFT: Self = Self {
        shift: true,
        control: false,
        alt: false,
    };
    /// Only Control pressed.
    pub const CONTROL: Self = Self {
        shift: false,
        control: true,
        alt: false,
    };
    /// Only Alt pressed.
    pub const ALT: Self = Self {
        shift: false,
        control: false,
        alt: true,
    };
}

/// A pointer event on the scene (upstream `GraphicsSceneMouseEvent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerEvent {
    /// Position in world coordinates (not snapped to the grid).
    pub pos: Point,
    /// Pressed keyboard modifiers.
    pub modifiers: Modifiers,
}

impl PointerEvent {
    /// An event at `pos` without modifiers.
    pub fn new(pos: Point) -> Self {
        Self {
            pos,
            modifiers: Modifiers::NONE,
        }
    }

    /// An event at `pos` with modifiers.
    pub fn with_modifiers(pos: Point, modifiers: Modifiers) -> Self {
        Self { pos, modifiers }
    }

    /// Position snapped to the grid (upstream
    /// `scenePos.mappedToGrid(getGridInterval())`).
    pub fn snapped(&self, grid: librepcb_core::types::PositiveLength) -> Point {
        self.pos.mapped_to_grid(grid)
    }
}

/// A key (the subset of `Qt::Key` the editor states handle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// Escape.
    Escape,
    /// Return or Enter.
    Enter,
    /// Backspace.
    Backspace,
    /// Delete.
    Delete,
    /// Tab.
    Tab,
    /// Space bar.
    Space,
    /// Shift (pressed or released alone).
    Shift,
    /// Control.
    Control,
    /// Alt.
    Alt,
    /// Arrow left.
    Left,
    /// Arrow right.
    Right,
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// A character key (lowercase letters, digits, punctuation).
    Char(char),
    /// Any other key.
    Other,
}

/// A key event (upstream `GraphicsSceneKeyEvent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    /// The key.
    pub key: Key,
    /// Pressed keyboard modifiers (after the event).
    pub modifiers: Modifiers,
}

impl KeyEvent {
    /// A key event without modifiers.
    pub fn new(key: Key) -> Self {
        Self {
            key,
            modifiers: Modifiers::NONE,
        }
    }
}

/// Mouse cursor shape of the view (upstream `Qt::CursorShape` as used by
/// the editor states, Slint `MouseCursor`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum CursorShape {
    /// The default arrow.
    #[default]
    Arrow,
    /// A crosshair (drawing tools).
    Cross,
    /// A pointing hand (hovering a clickable item).
    PointingHand,
    /// An open hand.
    OpenHand,
    /// A closed hand (dragging).
    ClosedHand,
    /// Move in all directions.
    SizeAll,
    /// Operation not allowed.
    Forbidden,
}

/// The cursor drawn into the scene at the snapped position (upstream
/// `fsmSetSceneCursor()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneCursor {
    /// Position.
    pub pos: Point,
    /// Draw a cross.
    pub cross: bool,
    /// Draw a circle.
    pub circle: bool,
}

/// A status bar message (upstream `fsmSetStatusBarMessage()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusMessage {
    /// The (translated) text; empty clears the status bar.
    pub text: String,
    /// How long to show it, `None` until replaced.
    pub timeout_ms: Option<u32>,
}

/// The editing features available in the current state (upstream
/// `SchematicEditorFsmAdapter::Feature` / `BoardEditorFsmAdapter::Feature`),
/// used by the application to enable its actions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Features {
    /// Select all.
    pub select: bool,
    /// Cut.
    pub cut: bool,
    /// Copy.
    pub copy: bool,
    /// Paste.
    pub paste: bool,
    /// Remove.
    pub remove: bool,
    /// Rotate.
    pub rotate: bool,
    /// Mirror (schematic).
    pub mirror: bool,
    /// Flip (board).
    pub flip: bool,
    /// Snap to grid.
    pub snap_to_grid: bool,
    /// Reset texts.
    pub reset_texts: bool,
    /// Lock (board).
    pub lock: bool,
    /// Unlock (board).
    pub unlock: bool,
    /// Edit properties.
    pub properties: bool,
    /// Modify line width (board).
    pub modify_line_width: bool,
    /// Import graphics (board).
    pub import_graphics: bool,
}

/// What the view shows besides the scene, as set by the current FSM state
/// (upstream: the `fsmSet*()` adapter calls). The application reads it
/// after each FSM call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ViewState {
    /// Cursor of the view (`None`: default).
    pub cursor: Option<CursorShape>,
    /// Cursor drawn into the scene.
    pub scene_cursor: Option<SceneCursor>,
    /// Selection rectangle (two corners).
    pub rubber_band: Option<(Point, Point)>,
    /// Measure ruler (start, end).
    pub ruler: Option<(Point, Point)>,
    /// Text of the info box overlay (plain text, lines separated by `\n`).
    pub info_box: String,
    /// Whether the scene is grayed out (e.g. while placing an item).
    pub gray_out: bool,
    /// The last status bar message (the application clears it when shown).
    pub status_message: Option<StatusMessage>,
    /// Available features.
    pub features: Features,
}

impl ViewState {
    /// Sets the status bar message (upstream `fsmSetStatusBarMessage()`).
    pub fn set_status(&mut self, text: impl Into<String>, timeout_ms: Option<u32>) {
        self.status_message = Some(StatusMessage {
            text: text.into(),
            timeout_ms,
        });
    }
}

/// The clipboard the FSMs copy to and paste from (upstream
/// `QApplication::clipboard()` with `QMimeData`). The application
/// implements it on the system clipboard; [`MemoryClipboard`] keeps the
/// data in memory (tests, or the application without system clipboard).
pub trait Clipboard {
    /// Replaces the clipboard content with `data` of the given MIME type.
    fn set(&mut self, mime_type: &str, data: Vec<u8>);

    /// Returns the clipboard content of the given MIME type, if any.
    fn get(&self, mime_type: &str) -> Option<Vec<u8>>;
}

/// A clipboard in memory (one entry).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryClipboard {
    content: Option<(String, Vec<u8>)>,
}

impl MemoryClipboard {
    /// An empty clipboard.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Clipboard for MemoryClipboard {
    fn set(&mut self, mime_type: &str, data: Vec<u8>) {
        self.content = Some((mime_type.to_owned(), data));
    }

    fn get(&self, mime_type: &str) -> Option<Vec<u8>> {
        self.content
            .as_ref()
            .filter(|(m, _)| m == mime_type)
            .map(|(_, d)| d.clone())
    }
}

/// Returns the hit tolerance for a view scale: upstream uses a tolerance
/// of about 5 pixels around the cursor (`calcPosWithTolerance()`), so
/// `pixel_size` (world length of one screen pixel) times 5.
pub fn tolerance_for_pixel_size(pixel_size: Length) -> Length {
    pixel_size * 5
}

/// Reads the DXF file of an import (upstream `processImportDxf()` of the
/// board, symbol and package editors): the polygon paths (joined if
/// requested) and the circles. Fails if the file contains nothing to
/// import.
pub(crate) fn read_dxf_import(
    settings: &board::DxfImportSettings,
) -> crate::Result<(
    Vec<librepcb_core::geometry::Path>,
    Vec<librepcb_core::import::DxfCircle>,
)> {
    let mut reader = librepcb_core::import::DxfReader::new();
    reader.set_scale_factor(settings.scale_factor);
    reader
        .parse_file(&settings.file)
        .map_err(|e| crate::Error::InvalidArgument(e.to_string()))?;
    let mut paths = reader.polygons().to_vec();
    if settings.join_tangent_polylines {
        paths = librepcb_core::utils::tangent_path_joiner::join(
            paths,
            Some(std::time::Duration::from_secs(2)),
        )
        .paths;
    }
    if paths.is_empty() && reader.circles().is_empty() {
        return Err(crate::Error::InvalidArgument(librepcb_i18n::tr!(
            "librepcb::editor::DxfImportDialog",
            "The selected file does not contain any objects to import."
        )));
    }
    Ok((paths, reader.circles().to_vec()))
}
