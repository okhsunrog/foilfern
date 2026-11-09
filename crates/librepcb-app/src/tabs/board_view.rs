//! Hit testing of the board editor FSM on the tab's scene.
//!
//! Port of the scene side of upstream `BoardEditorFsmAdapter`
//! (`fsmGetGraphicsScene()`, `fsmCalcPosWithTolerance()`,
//! `fsmMapGlobalPosToScenePos()`): implements [`BoardView`] on a
//! [`BoardScene`] and the canvas view.

use librepcb_canvas::kurbo::Rect;
use librepcb_canvas::{SelectionMode, View, convert};
use librepcb_core::types::{Layer, Length, Point};
use librepcb_editor::fsm::board::{BoardItemRef, BoardView};
use librepcb_scene::{BoardObject, BoardScene, BoardSceneLayer, BoardSide};

/// Hit tolerance in logical pixels (upstream `calcPosWithTolerance()`).
pub const HIT_TOLERANCE_PX: f64 = 5.0;

/// Maps a scene object to the item the FSM works with (air wires are not
/// selectable).
pub fn board_item(o: BoardObject) -> Option<BoardItemRef> {
    Some(match o {
        BoardObject::Device(c) => BoardItemRef::Device(c),
        BoardObject::FootprintPad(c, u) => BoardItemRef::FootprintPad(c, u),
        BoardObject::Pad(s, u) => BoardItemRef::Pad(s, u),
        BoardObject::Via(s, u) => BoardItemRef::Via(s, u),
        BoardObject::Trace(s, u) => BoardItemRef::Trace(s, u),
        BoardObject::Plane(p) => BoardItemRef::Plane(p),
        BoardObject::Zone(u) => BoardItemRef::Zone(u),
        BoardObject::Polygon(u) => BoardItemRef::Polygon(u),
        BoardObject::StrokeText(u) => BoardItemRef::StrokeText(u),
        BoardObject::Hole(u) => BoardItemRef::Hole(u),
        BoardObject::AirWire(_) => return None,
    })
}

/// The view of a board tab as the FSM sees it.
pub struct BoardSceneView<'a> {
    /// The scene.
    pub scene: &'a BoardScene,
    /// The canvas view (zoom level for the tolerance).
    pub view: &'a View,
    /// The last pointer position (world coordinates).
    pub cursor: Option<Point>,
}

impl BoardView for BoardSceneView<'_> {
    fn items_at(&self, pos: Point, tolerance: Length) -> Vec<BoardItemRef> {
        let mut items: Vec<BoardItemRef> = Vec::new();
        for id in self
            .scene
            .scene()
            .items_at(convert::point(pos), tolerance.to_mm())
        {
            if let Some(item) = self.scene.object(id).and_then(board_item)
                && !items.contains(&item)
            {
                items.push(item);
            }
        }
        items
    }

    fn items_in_rect(&self, p1: Point, p2: Point) -> Vec<BoardItemRef> {
        let rect = Rect::from_points(convert::point(p1), convert::point(p2));
        let mut items: Vec<BoardItemRef> = Vec::new();
        for id in self
            .scene
            .scene()
            .items_in_rect(rect, SelectionMode::Intersects)
        {
            if let Some(item) = self.scene.object(id).and_then(board_item)
                && !items.contains(&item)
            {
                items.push(item);
            }
        }
        items
    }

    fn tolerance(&self) -> Length {
        Length::from_mm(self.view.pixels_to_world(HIT_TOLERANCE_PX)).unwrap_or(Length::ZERO)
    }

    fn is_layer_visible(&self, layer: Layer) -> bool {
        self.scene
            .scene()
            .is_layer_visible(BoardSceneLayer::Board(layer).id())
    }

    fn is_flipped(&self) -> bool {
        self.scene.side() == BoardSide::Bottom
    }

    fn cursor_pos(&self) -> Option<Point> {
        self.cursor
    }
}
