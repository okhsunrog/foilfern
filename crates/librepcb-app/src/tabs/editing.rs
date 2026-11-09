//! Editing support shared by the schematic and board tabs: translating
//! Slint input into editor FSM events, and the FSM outputs into the view.
//!
//! Port of the event and overlay parts of
//! libs/librepcb/editor/graphics/slintgraphicsview.{h,cpp} (double click
//! detection, key conversion `s2key()`) and
//! libs/librepcb/editor/graphics/graphicsscene.{h,cpp} (selection
//! rectangle, ruler, scene cursor, gray-out). Upstream paints the overlays
//! in `drawForeground()`; here they are items of the canvas scene on
//! reserved layers, updated after every FSM call.
//!
//! Differences: the ruler has no tick labels (the measure tool's info box
//! shows the distance), hovered items are not highlighted.

use std::time::{Duration, Instant};

use librepcb_app_ui as ui;
use librepcb_canvas::kurbo::{Circle, Line, Point, Rect, Shape};
use librepcb_canvas::peniko::Color;
use librepcb_canvas::{Item, ItemId, Layer, LayerId, Scene, StrokeStyle, Style, View};
use librepcb_core::types::Length;
use librepcb_editor::fsm::{CursorShape, Key, KeyEvent, Modifiers, PointerEvent, ViewState};
use librepcb_i18n::tr;
use slint::private_unstable_api::re_exports::{BuiltInMouseCursor, MouseCursorInner};

use super::TabRequest;
use crate::notifications::Notification;

/// Maximum time between two presses of a double click (upstream: 500 ms).
const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(500);

/// Maximum distance (logical pixels) between the presses of a double click
/// (upstream: the same scene position).
const DOUBLE_CLICK_DISTANCE: f64 = 2.0;

/// Detects double clicks of the left button (upstream
/// `SlintGraphicsView::pointerEvent()`).
#[derive(Debug, Default, Clone)]
pub struct DoubleClick {
    last: Option<(Instant, Point)>,
}

impl DoubleClick {
    /// Registers a press at a screen position; returns whether it completes
    /// a double click.
    pub fn press(&mut self, pos: Point) -> bool {
        let now = Instant::now();
        let double = self.last.is_some_and(|(t, p)| {
            now.duration_since(t) < DOUBLE_CLICK_INTERVAL
                && (p - pos).hypot() <= DOUBLE_CLICK_DISTANCE
        });
        // A double click does not start the next one.
        self.last = if double { None } else { Some((now, pos)) };
        double
    }
}

/// Converts the canvas modifiers of an event (Meta counts as Control like
/// Qt's Command key on macOS).
pub fn fsm_modifiers(m: librepcb_canvas::Modifiers) -> Modifiers {
    Modifiers {
        shift: m.shift,
        control: m.control || m.meta,
        alt: m.alt,
    }
}

/// A pointer event of the FSM at a world position (mm).
pub fn fsm_pointer_event(world: Point, modifiers: librepcb_canvas::Modifiers) -> PointerEvent {
    PointerEvent::with_modifiers(point_from_world(world), fsm_modifiers(modifiers))
}

/// Converts a world position (mm, Y up) to a model point.
pub fn point_from_world(world: Point) -> librepcb_core::types::Point {
    let conv = |mm: f64| Length::from_mm(mm).unwrap_or(Length::ZERO);
    librepcb_core::types::Point::new(conv(world.x), conv(world.y))
}

/// Converts a model point to a world position (mm).
pub fn point_to_world(p: librepcb_core::types::Point) -> Point {
    Point::new(p.x.to_mm(), p.y.to_mm())
}

/// Converts a Slint key event (upstream `s2key()`); `None` for keys the
/// editor does not handle.
pub fn fsm_key_event(event: &slint::language::KeyEvent) -> Option<KeyEvent> {
    use slint::platform::Key as K;
    let mut chars = event.text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    let key = match c {
        c if c == char::from(K::Escape) => Key::Escape,
        c if c == char::from(K::Return) => Key::Enter,
        '\r' => Key::Enter,
        c if c == char::from(K::Backspace) => Key::Backspace,
        c if c == char::from(K::Delete) => Key::Delete,
        c if c == char::from(K::Tab) || c == char::from(K::Backtab) => Key::Tab,
        c if c == char::from(K::Space) => Key::Space,
        c if c == char::from(K::Shift) || c == char::from(K::ShiftR) => Key::Shift,
        c if c == char::from(K::Control)
            || c == char::from(K::ControlR)
            || c == char::from(K::Meta)
            || c == char::from(K::MetaR) =>
        {
            Key::Control
        }
        c if c == char::from(K::Alt) || c == char::from(K::AltGr) => Key::Alt,
        c if c == char::from(K::LeftArrow) => Key::Left,
        c if c == char::from(K::RightArrow) => Key::Right,
        c if c == char::from(K::UpArrow) => Key::Up,
        c if c == char::from(K::DownArrow) => Key::Down,
        c if !c.is_control() && !('\u{f700}'..='\u{f8ff}').contains(&c) => {
            Key::Char(c.to_lowercase().next().unwrap_or(c))
        }
        _ => return None,
    };
    let m = &event.modifiers;
    Some(KeyEvent {
        key,
        modifiers: Modifiers {
            shift: m.shift,
            control: m.control || m.meta,
            alt: m.alt,
        },
    })
}

/// The mouse cursor of the view (upstream `q2s(Qt::CursorShape)`; a closed
/// hand while panning).
pub fn mouse_cursor(shape: Option<CursorShape>, panning: bool) -> MouseCursorInner {
    let c = if panning {
        BuiltInMouseCursor::Grabbing
    } else {
        match shape.unwrap_or_default() {
            CursorShape::Arrow => BuiltInMouseCursor::Default,
            CursorShape::Cross => BuiltInMouseCursor::Crosshair,
            CursorShape::PointingHand => BuiltInMouseCursor::Pointer,
            CursorShape::OpenHand => BuiltInMouseCursor::Grab,
            CursorShape::ClosedHand => BuiltInMouseCursor::Grabbing,
            CursorShape::SizeAll => BuiltInMouseCursor::Move,
            CursorShape::Forbidden => BuiltInMouseCursor::NotAllowed,
        }
    };
    MouseCursorInner::BuiltIn(c)
}

/// The info box text as plain text (upstream `fsmSetViewInfoBoxText()`).
pub fn info_box_text(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("<br>", "\n")
        .replace("<b>", "")
        .replace("</b>", "")
}

/// Colors of the overlays (upstream color roles `*Overlays` and
/// `*Selection`: primary and secondary colors).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayColors {
    /// Gray-out fill (overlays primary).
    pub fill: Color,
    /// Ruler and scene cursor (overlays secondary).
    pub content: Color,
    /// Selection rectangle border (selection primary).
    pub selection_line: Color,
    /// Selection rectangle fill (selection secondary).
    pub selection_fill: Color,
}

impl OverlayColors {
    /// Upstream `schematicLibrePcbLight()`.
    pub const SCHEMATIC_LIGHT: Self = Self {
        fill: Color::from_rgba8(0xff, 0xff, 0xff, 0x78),
        content: Color::from_rgba8(0, 0, 0, 0xff),
        selection_line: Color::from_rgba8(0x78, 0xaa, 0xff, 0xff),
        selection_fill: Color::from_rgba8(0x96, 0xc8, 0xff, 0x50),
    };

    /// Upstream `boardLibrePcbDark()`.
    pub const BOARD_DARK: Self = Self {
        fill: Color::from_rgba8(0, 0, 0, 0x78),
        content: Color::from_rgba8(0xff, 0xff, 0, 0xff),
        selection_line: Color::from_rgba8(0x78, 0xaa, 0xff, 0xff),
        selection_fill: Color::from_rgba8(0x96, 0xc8, 0xff, 0x50),
    };
}

/// Canvas layers of the overlays (above all layers of the scene builders).
mod layers {
    use librepcb_canvas::LayerId;

    pub const GRAY_OUT: LayerId = LayerId(60_000);
    pub const RULER: LayerId = LayerId(60_001);
    pub const CURSOR: LayerId = LayerId(60_002);
    pub const CURSOR_CIRCLE: LayerId = LayerId(60_003);
    pub const SELECTION_FILL: LayerId = LayerId(60_004);
    pub const SELECTION_LINE: LayerId = LayerId(60_005);
}

/// Z value of the overlay items (above everything).
const OVERLAY_Z: i32 = 1_000_000;

/// What the overlays show (to skip updates when nothing changed).
#[derive(Debug, Clone, PartialEq)]
struct OverlayState {
    view_state: ViewState,
    visible: Rect,
    scale: f64,
}

/// The overlay items of a scene.
#[derive(Debug, Default)]
pub struct Overlays {
    items: Vec<ItemId>,
    state: Option<OverlayState>,
}

impl Overlays {
    /// Forgets the items without removing them (after the scene was
    /// rebuilt from scratch, they are gone).
    pub fn forget(&mut self) {
        self.items.clear();
        self.state = None;
    }

    /// Removes the overlay items from the scene.
    pub fn clear(&mut self, scene: &mut Scene) {
        for id in self.items.drain(..) {
            scene.remove(id);
        }
        self.state = None;
    }

    /// Updates the overlay items from the FSM's view state; returns whether
    /// the scene changed.
    pub fn update(
        &mut self,
        scene: &mut Scene,
        vs: &ViewState,
        view: &View,
        colors: OverlayColors,
    ) -> bool {
        let state = OverlayState {
            view_state: ViewState {
                // Not shown in the scene.
                cursor: None,
                info_box: String::new(),
                status_message: None,
                features: Default::default(),
                ..vs.clone()
            },
            visible: view.visible_rect(),
            scale: view.scale(),
        };
        if self.state.as_ref() == Some(&state) {
            return false;
        }
        let had_items = !self.items.is_empty();
        self.clear(scene);
        Self::ensure_layers(scene, colors);
        let px = |pixels: f64| view.pixels_to_world(pixels);
        let mut items = Vec::new();
        if vs.gray_out {
            items.push(Item::new(
                layers::GRAY_OUT,
                state.visible.inflate(px(10.0), px(10.0)).to_path(0.1),
                Style::fill(),
            ));
        }
        if let Some((a, b)) = vs.rubber_band {
            let rect = Rect::from_points(point_to_world(a), point_to_world(b));
            items.push(Item::new(
                layers::SELECTION_FILL,
                rect.to_path(0.1),
                Style::fill(),
            ));
            items.push(Item::new(
                layers::SELECTION_LINE,
                rect.to_path(0.1),
                Style::stroke(0.0).with_stroke(Some(StrokeStyle::hairline())),
            ));
        }
        if let Some((a, b)) = vs.ruler {
            let (a, b) = (point_to_world(a), point_to_world(b));
            items.push(Item::new(
                layers::RULER,
                Line::new(a, b),
                Style::stroke(px(3.0)),
            ));
            let d = b - a;
            let len = d.hypot();
            if len > 0.0 {
                // Ticks at both ends, perpendicular to the ruler.
                let n = librepcb_canvas::kurbo::Vec2::new(-d.y, d.x) / len * px(20.0);
                for p in [a, b] {
                    items.push(Item::new(
                        layers::RULER,
                        Line::new(p, p + n),
                        Style::stroke(0.0).with_stroke(Some(StrokeStyle::hairline())),
                    ));
                }
                if px(15.0) < len / 2.0 {
                    items.push(Item::new(
                        layers::RULER,
                        Circle::new(a.midpoint(b), px(7.5)),
                        Style::stroke(0.0).with_stroke(Some(StrokeStyle::hairline())),
                    ));
                }
            }
        }
        if let Some(c) = vs.scene_cursor {
            let pos = point_to_world(c.pos);
            let r = px(20.0);
            if c.cross {
                for (dx, dy) in [(0.0, r), (r, 0.0)] {
                    let d = librepcb_canvas::kurbo::Vec2::new(dx, dy);
                    items.push(Item::new(
                        layers::CURSOR,
                        Line::new(pos - d, pos + d),
                        Style::stroke(0.0).with_stroke(Some(StrokeStyle::hairline())),
                    ));
                }
            }
            if c.circle {
                items.push(Item::new(
                    layers::CURSOR_CIRCLE,
                    Circle::new(pos, r / 2.0),
                    Style::stroke(px(2.0)),
                ));
            }
        }
        for item in items {
            self.items.push(scene.insert(item.with_z(OVERLAY_Z)));
        }
        self.state = Some(state);
        had_items || !self.items.is_empty()
    }

    fn ensure_layers(scene: &mut Scene, colors: OverlayColors) {
        let order = i32::MAX - 10;
        let defs = [
            (layers::GRAY_OUT, colors.fill, 0),
            (layers::RULER, colors.content, 1),
            (layers::CURSOR, colors.content, 2),
            (layers::CURSOR_CIRCLE, Color::from_rgb8(0, 0xff, 0), 3),
            (layers::SELECTION_FILL, colors.selection_fill, 4),
            (layers::SELECTION_LINE, colors.selection_line, 5),
        ];
        for (id, color, i) in defs {
            let layer = Layer::new(color)
                .with_highlight_color(color)
                .with_order(order + i);
            if scene.layer(id) != Some(&layer) {
                scene.set_layer(id, layer);
            }
        }
    }

    /// Whether an item is an overlay item.
    pub fn contains(&self, id: ItemId) -> bool {
        self.items.contains(&id)
    }
}

/// Whether a canvas layer is an overlay layer.
pub fn is_overlay_layer(id: LayerId) -> bool {
    id.0 >= layers::GRAY_OUT.0 && id.0 <= layers::SELECTION_LINE.0
}

/// The rectangle to zoom to for the selected (highlighted) items of a
/// scene (upstream `goToObjects()` of the schematic and board tabs): their
/// bounding box plus a margin of 1.5 times its larger dimension, at most
/// 10 mm.
pub fn selection_zoom_rect(scene: &librepcb_canvas::Scene) -> Option<librepcb_canvas::kurbo::Rect> {
    let bbox = scene
        .selection()
        .filter_map(|id| scene.item(id).map(|i| i.bounding_box()))
        .reduce(|a, b| a.union(b))?;
    let margin = (1.5 * bbox.width().max(bbox.height())).min(10.0);
    Some(bbox.inflate(margin, margin))
}

/// Upstream `EditorToolbox::toSingleLine()` (tool bar text inputs).
pub fn to_single_line(text: &str) -> String {
    text.replace('\n', "\\n")
}

/// Upstream `EditorToolbox::toMultiLine()`.
pub fn to_multi_line(text: &str) -> String {
    text.replace("\\n", "\n")
}

/// The notification shown instead of a dialog which is not ported yet
/// (property dialogs come with milestone M3b).
pub fn dialog_not_available() -> TabRequest {
    TabRequest::Notify(Notification::new(
        ui::NotificationType::Info,
        tr!("librepcb::editor::MainWindow", "Not Available Yet"),
        tr!(
            "librepcb::editor::MainWindow",
            "This dialog is not available yet in this version of LibrePCB."
        ),
    ))
}

/// An error notification (upstream: `QMessageBox::critical()`).
pub fn error_notification(message: String) -> TabRequest {
    TabRequest::Notify(Notification {
        auto_popup: true,
        ..Notification::new(
            ui::NotificationType::Critical,
            tr!("librepcb::editor::MainWindow", "Error"),
            message,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_click() {
        let mut d = DoubleClick::default();
        let p = Point::new(10.0, 10.0);
        assert!(!d.press(p));
        assert!(d.press(p));
        assert!(!d.press(p));
        assert!(!d.press(Point::new(50.0, 10.0)));
    }

    #[test]
    fn info_box() {
        assert_eq!(info_box_text("<b>R1</b><br>a&nbsp;b"), "R1\na b");
    }
}
