//! Synthetic scenes for benchmarks, tests and examples: the stress board of
//! the canvas spike (`spikes/canvas`), with the same random sequence and
//! geometry, and the board layer colors of the "LibrePCB Dark" color scheme
//! (libs/librepcb/core/workspace/basecolorscheme.cpp).

use kurbo::{Affine, BezPath, Line, Point, Rect, Shape, Vec2};
use peniko::Color;

use crate::{Item, Layer, LayerId, Scene, Style};

/// Board layers in draw order: name and ARGB color.
pub const BOARD_LAYERS: &[(&str, u32)] = &[
    ("zones", 0x80494949),
    ("bot_courtyard", 0xc0ff00ff),
    ("bot_documentation", 0x76fbc697),
    ("bot_package_outlines", 0xc000ffff),
    ("bot_glue", 0x64e0e0e0),
    ("bot_legend", 0xbbffffff),
    ("bot_names", 0x96edffd8),
    ("bot_values", 0x96d8f2ff),
    ("bot_cu", 0x964578cc),
    ("in6_cu", 0x967b20a3),
    ("in5_cu", 0x96a70049),
    ("in4_cu", 0x96e2a1ff),
    ("in3_cu", 0x96ee5c9b),
    ("in2_cu", 0x96e50063),
    ("in1_cu", 0x96cc57ff),
    ("top_cu", 0x96cc0802),
    ("vias", 0x966db515),
    ("pads", 0x966db515),
    ("drills", 0xff000000),
    ("holes", 0xc8ffffff),
    ("top_glue", 0x64e0e0e0),
    ("top_documentation", 0x76fbc697),
    ("top_package_outlines", 0xc000ffff),
    ("top_courtyard", 0xc0ff00ff),
    ("top_legend", 0xbbffffff),
    ("top_names", 0x96edffd8),
    ("top_values", 0x96d8f2ff),
    ("brd_documentation", 0x76fbc697),
    ("brd_plated_cutouts", 0xc800ddff),
    ("brd_cutouts", 0xc8ffffff),
    ("brd_outlines", 0xc8ffffff),
];

/// Converts an ARGB value to a color.
pub fn argb(v: u32) -> Color {
    Color::from_rgba8((v >> 16) as u8, (v >> 8) as u8, v as u8, (v >> 24) as u8)
}

/// The ID of a board layer by name (its index in [`BOARD_LAYERS`]).
pub fn board_layer(name: &str) -> Option<LayerId> {
    BOARD_LAYERS
        .iter()
        .position(|(n, _)| *n == name)
        .map(|i| LayerId(i as u16))
}

/// Registers all [`BOARD_LAYERS`] (draw order = index).
pub fn add_board_layers(scene: &mut Scene) {
    for (i, (_, color)) in BOARD_LAYERS.iter().enumerate() {
        scene.set_layer(
            LayerId(i as u16),
            Layer::new(argb(*color)).with_order(i as i32),
        );
    }
}

/// The spike's xorshift generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn f(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn pick<T: Copy>(&mut self, v: &[T]) -> T {
        v[(self.next() % v.len() as u64) as usize]
    }
}

fn layer(name: &str) -> LayerId {
    // Only called with names from BOARD_LAYERS.
    board_layer(name).unwrap_or_default()
}

/// A dot: zero-length round-capped stroke (1 µm long, like the spike).
fn dot(c: Point, diameter: f64, layer: LayerId) -> Item {
    Item::new(
        layer,
        Line::new(c, c + Vec2::new(1e-3, 0.0)),
        Style::stroke(diameter),
    )
}

/// The spike's stress board (300 × 200 mm): `traces` trace segments on 4
/// copper layers, `pads` SMT pads and `vias` vias with drills, plus the
/// board outline. `stress_items(100_000, 16_000, 4_000)` gives the 124k
/// items of the spike's measurements.
pub fn stress_items(traces: usize, pads: usize, vias: usize) -> Vec<Item> {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let (bw, bh) = (300.0, 200.0);
    let mut items = Vec::with_capacity(traces + pads + 2 * vias + 1);
    let outline = Rect::new(0.0, 0.0, bw, bh).to_path(0.01);
    items.push(Item::new(
        layer("brd_outlines"),
        outline,
        Style::stroke(0.0),
    ));
    let cu = ["top_cu", "bot_cu", "in1_cu", "in2_cu"].map(layer);
    let mut anchors = Vec::new();
    for _ in 0..pads {
        let c = Point::new(2.0 + rng.f() * (bw - 4.0), 2.0 + rng.f() * (bh - 4.0));
        let (w, h): (f64, f64) = (rng.pick(&[0.6, 1.0, 1.5]), rng.pick(&[0.3, 0.6, 0.8]));
        let rr = Rect::from_center_size(c, (w, h)).to_rounded_rect(w.min(h) * 0.25);
        let rot = Affine::rotate_about(rng.pick(&[0.0, std::f64::consts::FRAC_PI_2]), c);
        let path: BezPath = rot * rr.to_path(0.01);
        items.push(Item::new(rng.pick(&[cu[0], cu[1]]), path, Style::fill()));
        anchors.push(c);
    }
    for _ in 0..vias {
        let c = Point::new(2.0 + rng.f() * (bw - 4.0), 2.0 + rng.f() * (bh - 4.0));
        items.push(dot(c, 0.6, layer("vias")));
        items.push(dot(c, 0.3, layer("drills")));
        anchors.push(c);
    }
    if anchors.is_empty() {
        anchors.push(Point::new(bw / 2.0, bh / 2.0));
    }
    let dirs: Vec<Vec2> = (0..8)
        .map(|k| Vec2::from_angle(k as f64 * std::f64::consts::FRAC_PI_4))
        .collect();
    let mut n = 0;
    while n < traces {
        let mut p = anchors[(rng.next() % anchors.len() as u64) as usize];
        let layer = rng.pick(&cu);
        let w = rng.pick(&[0.15, 0.2, 0.25, 0.4]);
        for _ in 0..8 {
            let d = rng.pick(&dirs) * (0.5 + rng.f() * 4.5);
            let q = Point::new(
                (p.x + d.x).clamp(0.5, bw - 0.5),
                (p.y + d.y).clamp(0.5, bh - 0.5),
            );
            items.push(Item::new(layer, Line::new(p, q), Style::stroke(w)));
            p = q;
            n += 1;
        }
    }
    items
}

/// A scene with the board layers and the 124k items stress board.
pub fn stress_scene() -> Scene {
    let mut scene = Scene::new();
    add_board_layers(&mut scene);
    scene.extend(stress_items(100_000, 16_000, 4_000));
    scene
}
