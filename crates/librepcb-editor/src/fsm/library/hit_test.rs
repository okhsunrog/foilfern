//! Hit testing on the model of a symbol or footprint (port of the item
//! ordering of upstream `SymbolGraphicsItem::findItemsAtPos()` and
//! `FootprintGraphicsItem::findItemsAtPos()`, and of the vertex/line lookup
//! of `PolygonGraphicsItem`).
//!
//! [`ModelHitTester`] is a [`LibraryView`] built from a snapshot of the
//! element: the application can use it directly (rebuilt whenever the
//! element changes) or implement the view on its scene.
//!
//! Differences to upstream: grab areas are approximated from the model
//! instead of the graphics items' painter paths: texts are rectangles
//! estimated from height and number of characters, pins are their line.

use librepcb_core::geometry::Path;
use librepcb_core::library::pkg::Footprint;
use librepcb_core::library::sym::Symbol;
use librepcb_core::types::{
    Alignment, Angle, HAlign, Layer, Length, Point, PositiveLength, VAlign,
};

use super::LibraryView;
use crate::library_editor::commands::{FootprintItem, SymbolItem};

type P = (f64, f64);

fn mm(p: Point) -> P {
    p.to_mm()
}

/// A grab area.
#[derive(Debug, Clone, PartialEq)]
enum Shape {
    /// Polylines with a half stroke width, optionally closed areas.
    Paths {
        lines: Vec<Vec<P>>,
        half_width: f64,
        areas: Vec<Vec<P>>,
    },
    /// A disk (or ring if `inner` is set).
    Circle {
        center: P,
        outer: f64,
        inner: Option<f64>,
    },
}

impl Shape {
    /// Distance from `p` to the shape (0 if inside).
    fn distance(&self, p: P) -> f64 {
        match self {
            Self::Paths {
                lines,
                half_width,
                areas,
            } => {
                if areas.iter().any(|a| point_in_polygon(p, a)) {
                    return 0.0;
                }
                let mut d = f64::MAX;
                for line in lines.iter().chain(areas.iter()) {
                    if line.len() == 1 {
                        d = d.min(dist(p, line[0]) - half_width);
                    }
                    for w in line.windows(2) {
                        d = d.min(segment_distance(p, w[0], w[1]) - half_width);
                    }
                    if areas.contains(line) && line.len() > 2 {
                        let (a, b) = (line[line.len() - 1], line[0]);
                        d = d.min(segment_distance(p, a, b) - half_width);
                    }
                }
                d.max(0.0)
            }
            Self::Circle {
                center,
                outer,
                inner,
            } => {
                let d = dist(p, *center);
                match inner {
                    Some(inner) if d < *inner => inner - d,
                    _ => (d - outer).max(0.0),
                }
            }
        }
    }

    /// Bounding box `(min, max)`.
    fn bounds(&self) -> (P, P) {
        match self {
            Self::Paths {
                lines,
                half_width,
                areas,
            } => {
                let mut min = (f64::MAX, f64::MAX);
                let mut max = (f64::MIN, f64::MIN);
                for p in lines.iter().chain(areas.iter()).flatten() {
                    min = (min.0.min(p.0), min.1.min(p.1));
                    max = (max.0.max(p.0), max.1.max(p.1));
                }
                (
                    (min.0 - half_width, min.1 - half_width),
                    (max.0 + half_width, max.1 + half_width),
                )
            }
            Self::Circle { center, outer, .. } => (
                (center.0 - outer, center.1 - outer),
                (center.0 + outer, center.1 + outer),
            ),
        }
    }
}

fn dist(a: P, b: P) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn segment_distance(p: P, a: P, b: P) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    if len2 == 0.0 {
        return dist(p, a);
    }
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0);
    dist(p, (a.0 + t * dx, a.1 + t * dy))
}

/// Even-odd point in polygon test.
fn point_in_polygon(p: P, poly: &[P]) -> bool {
    let mut inside = false;
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if ((a.1 > p.1) != (b.1 > p.1)) && (p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn flat(path: &Path) -> Vec<P> {
    let tol = PositiveLength::new(Length::new(5_000)).unwrap_or_else(|_| unreachable!());
    path.flattened_arcs(tol)
        .vertices()
        .iter()
        .map(|v| mm(v.pos))
        .collect()
}

fn path_shape(path: &Path, line_width: Length, area: bool) -> Shape {
    let points = flat(path);
    let half_width = line_width.to_mm() / 2.0;
    if area {
        Shape::Paths {
            lines: Vec::new(),
            half_width,
            areas: vec![points],
        }
    } else {
        Shape::Paths {
            lines: vec![points],
            half_width,
            areas: Vec::new(),
        }
    }
}

fn circle_shape(center: Point, diameter: Length, line_width: Length, area: bool) -> Shape {
    let (r, hw) = (diameter.to_mm() / 2.0, line_width.to_mm() / 2.0);
    Shape::Circle {
        center: mm(center),
        outer: r + hw,
        inner: (!area).then_some((r - hw).max(0.0)),
    }
}

/// Approximated rectangle of a text: character width 0.7 × height.
fn text_shape(text: &str, pos: Point, rotation: Angle, height: Length, align: Alignment) -> Shape {
    let h = height.to_mm();
    let w = 0.7 * h * text.chars().count().max(1) as f64;
    let x0 = match align.h {
        HAlign::Left => 0.0,
        HAlign::Center => -w / 2.0,
        HAlign::Right => -w,
    };
    let y0 = match align.v {
        VAlign::Bottom => 0.0,
        VAlign::Center => -h / 2.0,
        VAlign::Top => -h,
    };
    rect_shape(pos, rotation, (x0, y0), (x0 + w, y0 + h))
}

fn rect_shape(pos: Point, rotation: Angle, p0: P, p1: P) -> Shape {
    let (s, c) = libm::sincos(rotation.to_rad());
    let origin = mm(pos);
    let map = |(x, y): P| (origin.0 + x * c - y * s, origin.1 + x * s + y * c);
    Shape::Paths {
        lines: Vec::new(),
        half_width: 0.0,
        areas: vec![vec![map(p0), map((p1.0, p0.1)), map(p1), map((p0.0, p1.1))]],
    }
}

/// Upstream layer priority of footprint items (top 100, inner 200, bottom
/// 300, others 0).
fn layer_priority(layer: Layer) -> i32 {
    if layer.is_top() {
        100
    } else if layer.is_inner() {
        200
    } else if layer.is_bottom() {
        300
    } else {
        0
    }
}

/// A model based hit tester (a [`LibraryView`]) of a symbol or footprint.
#[derive(Debug, Clone)]
pub struct ModelHitTester<I> {
    items: Vec<(I, Shape, i32, bool)>,
    tolerance: Length,
    cursor: Option<Point>,
}

impl<I: Copy> ModelHitTester<I> {
    /// Sets the hit tolerance (default 0.2 mm).
    pub fn with_tolerance(mut self, tolerance: Length) -> Self {
        self.tolerance = tolerance;
        self
    }

    /// Sets the cursor position reported to the FSM.
    pub fn set_cursor_pos(&mut self, pos: Option<Point>) {
        self.cursor = pos;
    }

    fn new(items: Vec<(I, Shape, i32, bool)>) -> Self {
        Self {
            items,
            tolerance: Length::new(200_000),
            cursor: None,
        }
    }
}

impl ModelHitTester<SymbolItem> {
    /// Builds the grab areas of a symbol.
    pub fn for_symbol(symbol: &Symbol) -> Self {
        let mut items = Vec::new();
        for pin in symbol.pins().iter() {
            let end = pin.position()
                + Point::new(*pin.length(), Length::ZERO).rotated(pin.rotation(), Point::ORIGIN);
            let shape = Shape::Paths {
                lines: vec![vec![mm(pin.position()), mm(end)]],
                half_width: 0.3,
                areas: Vec::new(),
            };
            items.push((SymbolItem::Pin(pin.uuid()), shape, 0, false));
        }
        for text in symbol.texts().iter() {
            let shape = text_shape(
                text.text(),
                text.position(),
                text.rotation(),
                *text.height(),
                text.align(),
            );
            items.push((SymbolItem::Text(text.uuid()), shape, 10, false));
        }
        for c in symbol.circles().iter() {
            let area = c.is_filled() || c.is_grab_area();
            let shape = circle_shape(c.center(), *c.diameter(), *c.line_width(), area);
            items.push((SymbolItem::Circle(c.uuid()), shape, 20, true));
        }
        for p in symbol.polygons().iter() {
            let area = (p.is_filled() || p.is_grab_area()) && p.path().is_closed();
            let shape = path_shape(p.path(), *p.line_width(), area);
            items.push((SymbolItem::Polygon(p.uuid()), shape, 20, true));
        }
        for i in symbol.images().iter() {
            let shape = rect_shape(
                i.position(),
                i.rotation(),
                (0.0, 0.0),
                (i.width().to_mm(), i.height().to_mm()),
            );
            items.push((SymbolItem::Image(i.uuid()), shape, 21, false));
        }
        Self::new(items)
    }
}

impl ModelHitTester<FootprintItem> {
    /// Builds the grab areas of a footprint.
    pub fn for_footprint(footprint: &Footprint) -> Self {
        let mut items = Vec::new();
        for h in footprint.holes().iter() {
            let shape = path_shape(h.path().get(), *h.diameter(), false);
            items.push((FootprintItem::Hole(h.uuid()), shape, 0, false));
        }
        for fp in footprint.pads().iter() {
            let pad = fp.pad();
            let areas: Vec<Vec<P>> = pad
                .geometry()
                .to_outlines()
                .unwrap_or_default()
                .iter()
                .map(|o| {
                    flat(
                        &o.rotated(pad.rotation(), Point::ORIGIN)
                            .translated(pad.position()),
                    )
                })
                .collect();
            let shape = Shape::Paths {
                lines: Vec::new(),
                half_width: 0.0,
                areas,
            };
            let prio = if pad.is_tht() {
                10
            } else {
                10 + layer_priority(pad.smt_layer())
            };
            items.push((FootprintItem::Pad(fp.uuid()), shape, prio, false));
        }
        for t in footprint.stroke_texts().iter() {
            let shape = text_shape(t.text(), t.position(), t.rotation(), *t.height(), t.align());
            items.push((
                FootprintItem::StrokeText(t.uuid()),
                shape,
                20 + layer_priority(t.layer()),
                false,
            ));
        }
        for c in footprint.circles().iter() {
            let area = c.is_filled() || c.is_grab_area();
            let shape = circle_shape(c.center(), *c.diameter(), *c.line_width(), area);
            items.push((
                FootprintItem::Circle(c.uuid()),
                shape,
                30 + layer_priority(c.layer()),
                true,
            ));
        }
        for p in footprint.polygons().iter() {
            let area = (p.is_filled() || p.is_grab_area()) && p.path().is_closed();
            let shape = path_shape(p.path(), *p.line_width(), area);
            items.push((
                FootprintItem::Polygon(p.uuid()),
                shape,
                30 + layer_priority(p.layer()),
                true,
            ));
        }
        for z in footprint.zones().iter() {
            use librepcb_core::geometry::ZoneLayers;
            let shape = path_shape(z.outline(), Length::ZERO, true);
            let prio = 40
                + if z.layers().contains(ZoneLayers::TOP) {
                    100
                } else if z.layers().contains(ZoneLayers::INNER) {
                    200
                } else if z.layers().contains(ZoneLayers::BOTTOM) {
                    300
                } else {
                    0
                };
            items.push((FootprintItem::Zone(z.uuid()), shape, prio, true));
        }
        Self::new(items)
    }
}

impl<I: Copy> LibraryView<I> for ModelHitTester<I> {
    fn items_at(&self, pos: Point, tolerance: Length) -> Vec<I> {
        let p = mm(pos);
        let small = tolerance.to_mm();
        let mut hits: Vec<((i32, f64), I)> = Vec::new();
        for (item, shape, priority, large) in &self.items {
            let d = shape.distance(p);
            let (min, max) = shape.bounds();
            let center = ((min.0 + max.0) / 2.0, (min.1 + max.1) / 2.0);
            let center_dist = dist(center, p).powi(2);
            let near = if *large { 2.0 * small } else { small };
            if d <= 0.0 {
                hits.push(((*priority, center_dist), *item));
            } else if d <= near {
                hits.push(((*priority + 1000, center_dist), *item));
            }
        }
        hits.sort_by(|a, b| a.0.0.cmp(&b.0.0).then(a.0.1.total_cmp(&b.0.1)));
        hits.into_iter().map(|(_, i)| i).collect()
    }

    fn items_in_rect(&self, p1: Point, p2: Point) -> Vec<I> {
        let (a, b) = (mm(p1), mm(p2));
        let (min, max) = ((a.0.min(b.0), a.1.min(b.1)), (a.0.max(b.0), a.1.max(b.1)));
        self.items
            .iter()
            .filter(|(_, shape, _, _)| {
                let (smin, smax) = shape.bounds();
                smin.0 <= max.0 && smax.0 >= min.0 && smin.1 <= max.1 && smax.1 >= min.1
            })
            .map(|(i, _, _, _)| *i)
            .collect()
    }

    fn tolerance(&self) -> Length {
        self.tolerance
    }

    fn cursor_pos(&self) -> Option<Point> {
        self.cursor
    }
}

/// Indices of the vertices of `path` at most `tolerance` away from `pos`
/// (upstream `PolygonGraphicsItem::getVertexIndicesAtPosition()`).
pub fn vertex_indices_at(path: &Path, pos: Point, tolerance: Length) -> Vec<usize> {
    path.vertices()
        .iter()
        .enumerate()
        .filter(|(_, v)| *(v.pos - pos).length() <= tolerance)
        .map(|(i, _)| i)
        .collect()
}

/// The index of the vertex after the (straight) segment of `path` at most
/// `tolerance` away from `pos` (upstream
/// `PolygonGraphicsItem::getLineIndexAtPosition()`, which returns the
/// index of the segment's end vertex).
pub fn segment_index_at(path: &Path, pos: Point, tolerance: Length) -> Option<usize> {
    let p = mm(pos);
    let tol = tolerance.to_mm();
    path.vertices()
        .windows(2)
        .position(|w| segment_distance(p, mm(w[0].pos), mm(w[1].pos)) <= tol)
        .map(|i| i + 1)
}
