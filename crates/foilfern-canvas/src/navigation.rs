//! Pointer and scroll handling for the view, with upstream's behavior
//! (`SlintGraphicsView::pointerEvent()` / `scrollEvent()`):
//!
//! - middle button drag pans;
//! - right button drag pans once moved more than 5 px, otherwise the release
//!   is a context menu request;
//! - the wheel zooms around the cursor by 1.3 per 120 units; with Shift it
//!   scrolls horizontally, with Ctrl vertically; horizontal wheel deltas
//!   scroll horizontally;
//! - left button events and moves are passed on (in world coordinates) to
//!   the editor tools.
//!
//! This module is toolkit independent; the Slint adapter converts Slint's
//! event types.

use kurbo::{Point, Vec2};

use crate::View;

/// Distance (logical pixels) the pointer must move with the right button
/// pressed before panning starts.
const RIGHT_PAN_THRESHOLD: f64 = 5.0;

/// A pointer button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    /// Left (primary).
    Left,
    /// Right (secondary).
    Right,
    /// Middle (wheel).
    Middle,
    /// Any other or no button.
    Other,
}

/// The kind of a pointer event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerKind {
    /// Button pressed.
    Down,
    /// Button released.
    Up,
    /// Pointer moved.
    Move,
    /// Pointer grab cancelled.
    Cancel,
}

/// Keyboard modifiers of an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    /// Shift key.
    pub shift: bool,
    /// Control key.
    pub control: bool,
    /// Alt key.
    pub alt: bool,
    /// Meta/Super key.
    pub meta: bool,
}

/// What an event means for the application.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointerAction {
    /// Nothing to do.
    None,
    /// The view was panned; render a new frame.
    ViewChanged,
    /// The left button was pressed at a world position.
    LeftPressed(Point),
    /// The left button was released at a world position.
    LeftReleased(Point),
    /// The pointer moved (not panning) to a world position.
    Moved(Point),
    /// The right button was released without panning (context menu).
    ContextMenu(Point),
}

/// Tracks panning state between pointer events.
#[derive(Debug, Clone, Default)]
pub struct Navigator {
    /// Last screen position while panning.
    panning: Option<Point>,
    /// Press position of the right button (until panning starts).
    right_down: Option<Point>,
}

impl Navigator {
    /// Creates a navigator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns whether a pan drag is in progress (e.g. for the cursor).
    pub fn is_panning(&self) -> bool {
        self.panning.is_some()
    }

    /// Handles a pointer event at `pos` (logical pixels relative to the
    /// canvas), updating the view when panning.
    pub fn pointer_event(
        &mut self,
        view: &mut View,
        kind: PointerKind,
        button: PointerButton,
        pos: Point,
    ) -> PointerAction {
        let world = view.screen_to_world(pos);
        match (kind, button) {
            (PointerKind::Down, PointerButton::Left) => PointerAction::LeftPressed(world),
            (PointerKind::Up, PointerButton::Left) => PointerAction::LeftReleased(world),
            (PointerKind::Down, PointerButton::Middle) => {
                self.panning = Some(pos);
                PointerAction::None
            }
            (PointerKind::Up | PointerKind::Cancel, PointerButton::Middle) => {
                self.panning = None;
                PointerAction::None
            }
            (PointerKind::Down, PointerButton::Right) => {
                self.right_down = Some(pos);
                PointerAction::None
            }
            (PointerKind::Up | PointerKind::Cancel, PointerButton::Right) => {
                let was_panning = self.panning.take().is_some();
                let pressed = self.right_down.take().is_some();
                if !was_panning && pressed && kind == PointerKind::Up {
                    PointerAction::ContextMenu(world)
                } else {
                    PointerAction::None
                }
            }
            (PointerKind::Move, _) => {
                if self.panning.is_none()
                    && let Some(start) = self.right_down
                {
                    if (pos - start).hypot() > RIGHT_PAN_THRESHOLD {
                        self.panning = Some(start);
                        self.right_down = None;
                    } else {
                        return PointerAction::None;
                    }
                }
                if let Some(last) = self.panning {
                    view.pan(pos - last);
                    self.panning = Some(pos);
                    PointerAction::ViewChanged
                } else {
                    PointerAction::Moved(world)
                }
            }
            _ => PointerAction::None,
        }
    }

    /// Handles a scroll event at `pos` (logical pixels). Returns whether the
    /// view changed.
    pub fn scroll_event(
        &mut self,
        view: &mut View,
        pos: Point,
        delta: Vec2,
        modifiers: Modifiers,
    ) -> bool {
        if !delta.is_finite() || delta == Vec2::ZERO {
            return false;
        }
        if delta.x.abs() > delta.y.abs() {
            view.pan(Vec2::new(delta.x, 0.0));
        } else if modifiers.shift {
            view.pan(Vec2::new(delta.y, 0.0));
        } else if modifiers.control {
            view.pan(Vec2::new(0.0, delta.y));
        } else {
            view.zoom_at(pos, 1.3f64.powf(delta.y / 120.0));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn middle_drag_pans() {
        let mut v = View::new((100.0, 100.0), 1.0);
        let mut n = Navigator::new();
        let p0 = Point::new(10.0, 10.0);
        n.pointer_event(&mut v, PointerKind::Down, PointerButton::Middle, p0);
        let w0 = v.screen_to_world(p0);
        let p1 = Point::new(30.0, 25.0);
        let a = n.pointer_event(&mut v, PointerKind::Move, PointerButton::Other, p1);
        assert_eq!(a, PointerAction::ViewChanged);
        // The grabbed world point follows the cursor.
        assert!((v.screen_to_world(p1) - w0).hypot() < 1e-12);
        n.pointer_event(&mut v, PointerKind::Up, PointerButton::Middle, p1);
        assert!(!n.is_panning());
    }

    #[test]
    fn right_click_vs_right_drag() {
        let mut v = View::new((100.0, 100.0), 1.0);
        let mut n = Navigator::new();
        let p = Point::new(50.0, 50.0);
        n.pointer_event(&mut v, PointerKind::Down, PointerButton::Right, p);
        let small = Point::new(52.0, 51.0);
        assert_eq!(
            n.pointer_event(&mut v, PointerKind::Move, PointerButton::Other, small),
            PointerAction::None
        );
        assert!(matches!(
            n.pointer_event(&mut v, PointerKind::Up, PointerButton::Right, small),
            PointerAction::ContextMenu(_)
        ));

        n.pointer_event(&mut v, PointerKind::Down, PointerButton::Right, p);
        let far = Point::new(70.0, 50.0);
        assert_eq!(
            n.pointer_event(&mut v, PointerKind::Move, PointerButton::Other, far),
            PointerAction::ViewChanged
        );
        assert_eq!(
            n.pointer_event(&mut v, PointerKind::Up, PointerButton::Right, far),
            PointerAction::None
        );
    }

    #[test]
    fn wheel_zooms_or_scrolls() {
        let mut v = View::new((100.0, 100.0), 1.0);
        let mut n = Navigator::new();
        let s = v.scale();
        let pos = Point::new(20.0, 30.0);
        let w = v.screen_to_world(pos);
        n.scroll_event(&mut v, pos, Vec2::new(0.0, 120.0), Modifiers::default());
        assert!((v.scale() - s * 1.3).abs() < 1e-9);
        assert!((v.screen_to_world(pos) - w).hypot() < 1e-9);
        let off = v.offset();
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        n.scroll_event(&mut v, pos, Vec2::new(0.0, 10.0), shift);
        assert_eq!(v.offset(), off + Vec2::new(10.0, 0.0));
    }
}
