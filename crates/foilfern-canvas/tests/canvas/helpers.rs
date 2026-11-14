//! Shared test helpers.

use foilfern_canvas::kurbo::{Circle, Line, Rect};
use foilfern_canvas::peniko::Color;
use foilfern_canvas::{Item, ItemId, Layer, LayerId, Scene, Style};

pub const RED: LayerId = LayerId(1);
pub const GREEN: LayerId = LayerId(2);
pub const BLUE: LayerId = LayerId(3);

/// A scene with three opaque layers (red below green below blue).
pub fn scene() -> Scene {
    let mut s = Scene::with_tile_size(10.0);
    s.set_layer(RED, Layer::new(Color::from_rgb8(255, 0, 0)).with_order(1));
    s.set_layer(GREEN, Layer::new(Color::from_rgb8(0, 255, 0)).with_order(2));
    s.set_layer(BLUE, Layer::new(Color::from_rgb8(0, 0, 255)).with_order(3));
    s
}

pub fn square(layer: LayerId, x: f64, y: f64, size: f64) -> Item {
    Item::new(layer, Rect::new(x, y, x + size, y + size), Style::fill())
}

pub fn trace(layer: LayerId, x0: f64, y0: f64, x1: f64, y1: f64, width: f64) -> Item {
    Item::new(layer, Line::new((x0, y0), (x1, y1)), Style::stroke(width))
}

pub fn dot(layer: LayerId, x: f64, y: f64, r: f64) -> Item {
    Item::new(layer, Circle::new((x, y), r), Style::fill())
}

/// Some items spread over several tiles.
pub fn populate(s: &mut Scene) -> Vec<ItemId> {
    let mut ids = Vec::new();
    for i in 0..10 {
        let f = f64::from(i);
        ids.push(s.insert(square(RED, f * 7.0, 0.0, 3.0)));
        ids.push(s.insert(trace(GREEN, f * 7.0, 5.0, f * 7.0 + 5.0, 12.0, 0.5)));
        ids.push(s.insert(dot(BLUE, f * 7.0 + 1.5, 1.5, 0.8)));
    }
    ids
}
