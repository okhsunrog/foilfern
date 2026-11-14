//! Demo content: the spike's stress board plus real LibrePCB geometry
//! (paths with arcs, polygons, circles, holes, stroke texts, a transformed
//! "footprint"), built with `foilfern-core` types and `convert`.

use std::collections::HashMap;

use foilfern_canvas::kurbo::Point as KPoint;
use foilfern_canvas::stress::{self, board_layer};
use foilfern_canvas::{Item, ItemId, LayerId, Scene, Style, convert};
use foilfern_core::font::StrokeFont;
use foilfern_core::geometry::{Circle, Hole, NonEmptyPath, Path, Polygon, StrokeText, Vertex};
use foilfern_core::types::{
    Alignment, Angle, HAlign, Layer, Length, MaskConfig, Point, PositiveLength, StrokeTextSpacing,
    UnsignedLength, Uuid, VAlign,
};
use foilfern_core::utils::transform::Transform;

/// A scene plus a description of every item (for click feedback).
pub struct Demo {
    pub scene: Scene,
    pub labels: HashMap<ItemId, String>,
}

fn mm(v: f64) -> Length {
    Length::from_mm(v).expect("demo lengths are valid")
}

fn pos(v: f64) -> PositiveLength {
    PositiveLength::new(mm(v)).expect("demo lengths are positive")
}

fn uns(v: f64) -> UnsignedLength {
    UnsignedLength::new(mm(v)).expect("demo lengths are not negative")
}

fn pt(x: f64, y: f64) -> Point {
    Point::from_mm(x, y).expect("demo points are valid")
}

fn deg(v: f64) -> Angle {
    Angle::from_deg(v).expect("demo angles are valid")
}

fn core_layer(id: &str) -> Layer {
    Layer::from_id(id).expect("demo layer exists")
}

/// Canvas layer of a core layer (same identifiers as the demo color table).
fn canvas_layer(layer: Layer) -> LayerId {
    board_layer(layer.id()).unwrap_or_default()
}

fn load_font() -> Option<StrokeFont> {
    // See `LIBREPCB_UPSTREAM_DIR` in `.cargo/config.toml`.
    let path = std::path::Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("share/librepcb/fontobene/newstroke.bene");
    match std::fs::read(&path) {
        Ok(bytes) => Some(StrokeFont::new(bytes)),
        Err(e) => {
            eprintln!("stroke font {} not loaded: {e}", path.display());
            None
        }
    }
}

struct Builder {
    scene: Scene,
    labels: HashMap<ItemId, String>,
}

impl Builder {
    fn add(&mut self, item: Item, label: impl Into<String>) -> ItemId {
        let id = self.scene.insert(item);
        self.labels.insert(id, label.into());
        id
    }
}

/// Builds the demo. `stress` adds the 124k items board at (0, 0)..(300,
/// 200); the real geometry is placed right of it at x = 320 mm.
pub fn build(stress: bool) -> Demo {
    let mut b = Builder {
        scene: Scene::new(),
        labels: HashMap::new(),
    };
    stress::add_board_layers(&mut b.scene);
    if stress {
        let items = stress::stress_items(100_000, 16_000, 4_000);
        let ids = b.scene.extend(items);
        for (i, id) in ids.into_iter().enumerate() {
            b.labels.insert(id, format!("stress item #{i}"));
        }
    }
    add_real_geometry(&mut b, 320.0, 0.0);
    Demo {
        scene: b.scene,
        labels: b.labels,
    }
}

fn add_real_geometry(b: &mut Builder, x0: f64, y0: f64) {
    let p = |x: f64, y: f64| pt(x0 + x, y0 + y);

    // Board outline: rounded rectangle with 90° arcs.
    let r = 5.0;
    let (w, h) = (120.0, 90.0);
    let outline = Path::new(vec![
        Vertex::at(p(r, 0.0)),
        Vertex::new(p(w - r, 0.0), deg(90.0)),
        Vertex::at(p(w, r)),
        Vertex::new(p(w, h - r), deg(90.0)),
        Vertex::at(p(w - r, h)),
        Vertex::new(p(r, h), deg(90.0)),
        Vertex::at(p(0.0, h - r)),
        Vertex::new(p(0.0, r), deg(90.0)),
        Vertex::at(p(r, 0.0)),
    ]);
    let outline = Polygon::new(
        Uuid::new_random(),
        core_layer("brd_outlines"),
        uns(0.0),
        false,
        false,
        outline,
    );
    let item = convert::polygon_item(&outline, canvas_layer(outline.layer()));
    b.add(item, "board outline (polygon with arcs)");

    // Plane-like filled area with a cut-in hole, below the copper.
    let zone = Path::new(vec![
        Vertex::at(p(10.0, 10.0)),
        Vertex::at(p(60.0, 10.0)),
        Vertex::at(p(60.0, 40.0)),
        Vertex::at(p(35.0, 40.0)),
        // Cut-in to a clockwise circle: a hole in the counter-clockwise area.
        Vertex::new(p(35.0, 30.0), deg(-180.0)),
        Vertex::new(p(35.0, 20.0), deg(-180.0)),
        Vertex::at(p(35.0, 30.0)),
        Vertex::at(p(35.0, 40.0)),
        Vertex::at(p(10.0, 40.0)),
        Vertex::at(p(10.0, 10.0)),
    ]);
    let plane = Polygon::new(
        Uuid::new_random(),
        core_layer("bot_cu"),
        uns(0.0),
        true,
        false,
        zone,
    );
    let item = convert::polygon_item(&plane, canvas_layer(plane.layer()));
    b.add(item, "filled polygon with arc cut-in (bot_cu)");

    // Traces with arcs.
    for (i, angle) in [0.0, 45.0, 90.0, -120.0].into_iter().enumerate() {
        let y = 50.0 + i as f64 * 8.0;
        let path = Path::line(p(10.0, y), p(50.0, y + 4.0), deg(angle));
        let item = Item::new(
            canvas_layer(core_layer("top_cu")),
            convert::path(&path),
            Style::stroke(0.5 + i as f64 * 0.25),
        );
        b.add(item, format!("trace with {angle}° arc"));
    }

    // Circles: filled, outline, both.
    for (i, (filled, width)) in [(true, 0.0), (false, 0.3), (true, 0.5)]
        .into_iter()
        .enumerate()
    {
        let c = Circle::new(
            Uuid::new_random(),
            core_layer("top_documentation"),
            uns(width),
            filled,
            false,
            p(75.0 + i as f64 * 12.0, 15.0),
            pos(8.0),
        );
        let item = convert::circle_item(&c, canvas_layer(c.layer()));
        b.add(item, format!("circle filled={filled} width={width}"));
    }

    // Holes: round, straight slot, curved slot (hairline outlines like
    // upstream).
    let holes = [
        NonEmptyPath::from_point(p(75.0, 35.0)),
        NonEmptyPath::new(Path::line(p(85.0, 32.0), p(95.0, 38.0), Angle::DEG0))
            .expect("slot path is not empty"),
        NonEmptyPath::new(Path::line(p(100.0, 32.0), p(110.0, 32.0), deg(120.0)))
            .expect("slot path is not empty"),
    ];
    for (i, path) in holes.into_iter().enumerate() {
        let hole = Hole::new(Uuid::new_random(), pos(2.0), path, MaskConfig::Automatic);
        let item = convert::hole_item(&hole, board_layer("holes").unwrap_or_default());
        b.add(item, format!("hole #{i}"));
    }

    // A "footprint": pads, legend and courtyard in local coordinates, placed
    // rotated and mirrored like a device on the bottom side.
    let placements = [
        (Transform::new(p(80.0, 60.0), deg(0.0), false), "U1"),
        (Transform::new(p(105.0, 60.0), deg(30.0), false), "U2"),
        (
            Transform::new(p(92.0, 80.0), deg(90.0), true),
            "U3 (mirrored)",
        ),
    ];
    for (placement, name) in placements {
        let xf = convert::transform(&placement);
        let mirrored = placement.mirrored;
        let side =
            |top: &str, bot: &str| canvas_layer(core_layer(if mirrored { bot } else { top }));
        for k in 0..4 {
            let pad = Path::centered_rect(pos(1.2), pos(0.6), uns(0.15))
                .translated(pt(-3.0, -1.5 + k as f64));
            let item = Item::new(side("top_cu", "bot_cu"), convert::path(&pad), Style::fill());
            let item = Item {
                geometry: item.geometry.transformed(xf),
                ..item
            };
            b.add(item, format!("{name} pad {}", k + 1));
            let pad = Path::obround(pos(1.2), pos(0.6)).translated(pt(3.0, -1.5 + k as f64));
            let item = Item::new(side("top_cu", "bot_cu"), convert::path(&pad), Style::fill());
            let item = Item {
                geometry: item.geometry.transformed(xf),
                ..item
            };
            b.add(item, format!("{name} pad {}", 8 - k));
        }
        let body = Polygon::new(
            Uuid::new_random(),
            core_layer("top_legend"),
            uns(0.2),
            false,
            false,
            Path::centered_rect(pos(4.0), pos(5.0), uns(0.0)),
        );
        let mut item = convert::polygon_item(&body, side("top_legend", "bot_legend"));
        item.geometry = item.geometry.transformed(xf);
        b.add(item, format!("{name} legend"));
        let courtyard = Polygon::new(
            Uuid::new_random(),
            core_layer("top_courtyard"),
            uns(0.0),
            false,
            false,
            Path::centered_rect(pos(8.0), pos(6.0), uns(0.0)),
        );
        let mut item = convert::polygon_item(&courtyard, side("top_courtyard", "bot_courtyard"));
        item.geometry = item.geometry.transformed(xf);
        // Dashed outline, drawn on top of everything.
        if let Some(s) = &mut item.style.stroke {
            *s = s.clone().with_dash([0.4, 0.3], 0.0);
        }
        b.add(item.with_z(1), format!("{name} courtyard (dashed)"));
    }

    // Stroke texts.
    let Some(font) = load_font() else {
        return;
    };
    let texts = [
        (
            "LibrePCB canvas: vello_cpu + kurbo + rstar",
            p(8.0, 84.0),
            0.0,
            3.0,
            false,
            "top_legend",
        ),
        (
            "R1 C2 U3 1.5mm ±0.1 Ω µF",
            p(8.0, 76.0),
            0.0,
            2.0,
            false,
            "top_names",
        ),
        (
            "rotated 30°",
            p(70.0, 45.0),
            30.0,
            2.5,
            false,
            "top_documentation",
        ),
        (
            "mirrored (bottom)",
            p(115.0, 5.0),
            0.0,
            2.5,
            true,
            "bot_legend",
        ),
    ];
    for (text, at, rot, height, mirrored, layer) in texts {
        let t = StrokeText::new(
            Uuid::new_random(),
            core_layer(layer),
            text,
            at,
            deg(rot),
            pos(height),
            uns(height * 0.1),
            StrokeTextSpacing::Auto,
            StrokeTextSpacing::Auto,
            Alignment::new(HAlign::Left, VAlign::Bottom),
            mirrored,
            false,
            false,
        );
        let item = convert::stroke_text_item(&t, &font, None, canvas_layer(t.layer()));
        b.add(item, format!("stroke text \"{text}\""));
    }
}

/// World position of the real geometry (for the headless zoom views).
pub fn real_geometry_center() -> KPoint {
    KPoint::new(380.0, 45.0)
}
