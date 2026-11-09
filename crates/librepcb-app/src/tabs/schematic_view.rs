//! Hit testing of the schematic editor FSM on the tab's scene.
//!
//! Port of the scene side of upstream `SchematicEditorFsmAdapter`
//! (`fsmGetGraphicsScene()`, `fsmCalcPosWithTolerance()`,
//! `fsmMapGlobalPosToScenePos()`): implements
//! [`SchematicView`] on a [`SchematicScene`] and the canvas view.

use librepcb_canvas::kurbo::Rect;
use librepcb_canvas::{SelectionMode, View, convert};
use librepcb_core::geometry::NetLineAnchor;
use librepcb_core::project::SymbolId;
use librepcb_core::types::{Length, Point};
use librepcb_editor::fsm::schematic::{SchematicItem, SchematicView};
use librepcb_scene::{SchematicObject, SchematicScene};

/// Hit tolerance in logical pixels (upstream `calcPosWithTolerance()`).
pub const HIT_TOLERANCE_PX: f64 = 5.0;

/// Maps a scene object to the item the FSM works with; `None` for junction
/// dots which are neither a net segment junction nor a pin.
pub fn schematic_item(o: SchematicObject) -> Option<SchematicItem> {
    Some(match o {
        SchematicObject::Symbol(id) => SchematicItem::Symbol(id),
        SchematicObject::SymbolPin(id, pin) => SchematicItem::SymbolPin(id, pin),
        SchematicObject::SymbolText(id, text) => SchematicItem::SymbolText(id, text),
        SchematicObject::NetLine(seg, l) => SchematicItem::NetLine(seg, l),
        SchematicObject::NetJunction(seg, NetLineAnchor::Junction(j)) => {
            SchematicItem::NetPoint(seg, j)
        }
        SchematicObject::NetJunction(_, NetLineAnchor::Pin { symbol, pin }) => {
            SchematicItem::SymbolPin(SymbolId(symbol), pin)
        }
        SchematicObject::NetJunction(..) => return None,
        SchematicObject::NetLabel(seg, l) => SchematicItem::NetLabel(seg, l),
        SchematicObject::BusLine(seg, l) => SchematicItem::BusLine(seg, l),
        SchematicObject::BusJunction(seg, j) => SchematicItem::BusJunction(seg, j),
        SchematicObject::BusLabel(seg, l) => SchematicItem::BusLabel(seg, l),
        SchematicObject::Polygon(id) => SchematicItem::Polygon(id),
        SchematicObject::Text(id) => SchematicItem::Text(id),
        SchematicObject::Image(id) => SchematicItem::Image(id),
    })
}

/// The view of a schematic tab as the FSM sees it.
pub struct SchematicSceneView<'a> {
    /// The scene.
    pub scene: &'a SchematicScene,
    /// The canvas view (zoom level for the tolerance).
    pub view: &'a View,
    /// The last pointer position (world coordinates).
    pub cursor: Option<Point>,
}

impl SchematicView for SchematicSceneView<'_> {
    fn items_at(&self, pos: Point, tolerance: Length) -> Vec<SchematicItem> {
        let mut items: Vec<SchematicItem> = Vec::new();
        for id in self
            .scene
            .scene()
            .items_at(convert::point(pos), tolerance.to_mm())
        {
            if let Some(item) = self.scene.object(id).and_then(schematic_item)
                && !items.contains(&item)
            {
                items.push(item);
            }
        }
        items
    }

    fn items_in_rect(&self, p1: Point, p2: Point) -> Vec<SchematicItem> {
        let rect = Rect::from_points(convert::point(p1), convert::point(p2));
        let mut items: Vec<SchematicItem> = Vec::new();
        for id in self
            .scene
            .scene()
            .items_in_rect(rect, SelectionMode::Intersects)
        {
            if let Some(item) = self.scene.object(id).and_then(schematic_item)
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

    fn cursor_pos(&self) -> Option<Point> {
        self.cursor
    }
}
