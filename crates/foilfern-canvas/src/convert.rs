//! Conversions from `foilfern-core` geometry to canvas geometry and items.
//!
//! World units are millimetres with the Y axis up, so core points map
//! directly (`x_nm * 1e-6`). Arcs are converted exactly like upstream
//! (`Path::toQPainterPathPx()`, Qt's cubic approximation) via
//! [`foilfern_core::utils::painter_path::painter_path_px`].
//!
//! The item helpers mirror upstream's graphics items (`PolygonGraphicsItem`,
//! `CircleGraphicsItem`, `PrimitiveHoleGraphicsItem`,
//! `StrokeTextGraphicsItem`): round caps and joins, outlines as strokes,
//! fills only for closed paths. Mapping model layers to [`LayerId`]s is up to
//! the scene builder.

use foilfern_core::font::StrokeFont;
use foilfern_core::geometry::{Circle, Hole, Path, Polygon, StrokeText};
use foilfern_core::types::{Angle, Length, Point};
use foilfern_core::utils::painter_path::{PathElement, painter_path_px};
use foilfern_core::utils::transform::Transform;
use kurbo::{Affine, BezPath, Circle as KCircle, Point as KPoint};

use crate::style::{StrokeStyle, Style};
use crate::{Geometry, Item, LayerId};

/// Millimetres per upstream scene pixel (1/72 inch).
fn mm_per_px() -> f64 {
    1.0 / Length::new(1_000_000).to_px()
}

/// Converts a length to world units (mm).
pub fn length(l: Length) -> f64 {
    l.to_mm()
}

/// Converts a point to world coordinates (mm, Y up).
pub fn point(p: Point) -> KPoint {
    let (x, y) = p.to_mm();
    KPoint::new(x, y)
}

/// Converts an angle (counter-clockwise) to radians.
pub fn angle(a: Angle) -> f64 {
    a.to_rad()
}

/// The world transformation of a core [`Transform`] (mirror horizontally,
/// rotate counter-clockwise, translate), e.g. of a placed footprint.
pub fn transform(t: &Transform) -> Affine {
    let mirror = if t.mirrored {
        Affine::FLIP_X
    } else {
        Affine::IDENTITY
    };
    Affine::translate(point(t.position).to_vec2()) * Affine::rotate(angle(t.rotation)) * mirror
}

/// Converts a path (with arc segments) to a Bézier path.
pub fn path(p: &Path) -> BezPath {
    let mut out = BezPath::new();
    append_path(&mut out, p);
    out
}

/// Converts several paths into one Bézier path with a subpath each (like
/// upstream `Path::toQPainterPathPx(paths, false)`).
pub fn paths<'a>(ps: impl IntoIterator<Item = &'a Path>) -> BezPath {
    let mut out = BezPath::new();
    for p in ps {
        append_path(&mut out, p);
    }
    out
}

fn append_path(out: &mut BezPath, p: &Path) {
    let k = mm_per_px();
    // Pixels have the Y axis pointing down.
    let pt = |(x, y): (f64, f64)| KPoint::new(x * k, -y * k);
    for el in painter_path_px(p) {
        match el {
            PathElement::MoveTo(a) => out.move_to(pt(a)),
            PathElement::LineTo(a) => out.line_to(pt(a)),
            PathElement::CubicTo(a, b, c) => out.curve_to(pt(a), pt(b), pt(c)),
        }
    }
}

/// The item of a polygon: stroked outline (a hairline for zero width, like
/// upstream's cosmetic pen) and fill (if filled and closed; grab areas are
/// left to the builder).
pub fn polygon_item(p: &Polygon, layer: LayerId) -> Item {
    let path = p.path_for_rendering();
    let filled = p.is_filled() && path.is_closed();
    let style = Style {
        fill: filled.then_some(crate::Brush::Layer),
        stroke: Some(StrokeStyle::new(length(*p.line_width()))),
    };
    Item::new(layer, self::path(&path), style)
}

/// The item of a circle: stroked outline (a hairline for zero width) and
/// fill (if filled).
pub fn circle_item(c: &Circle, layer: LayerId) -> Item {
    let style = Style {
        fill: c.is_filled().then_some(crate::Brush::Layer),
        stroke: Some(StrokeStyle::new(length(*c.line_width()))),
    };
    let circle = KCircle::new(point(c.center()), length(*c.diameter()) / 2.0);
    Item::new(layer, circle, style)
}

/// The item of a hole: its outline (circle or slot) as a hairline, like
/// upstream's `PrimitiveHoleGraphicsItem`.
pub fn hole_item(h: &Hole, layer: LayerId) -> Item {
    let outlines = h.path().to_outline_strokes(h.diameter());
    Item::new(layer, paths(&outlines), Style::stroke(0.0))
}

/// The item of a stroke text (strokes with round caps), transformed by the
/// text's position, rotation and mirroring. `text` overrides the text
/// (e.g. with substituted attributes).
pub fn stroke_text_item(
    t: &StrokeText,
    font: &StrokeFont,
    text: Option<&str>,
    layer: LayerId,
) -> Item {
    let local = match text {
        Some(s) => t.generate_paths_for(font, s),
        None => t.generate_paths(font),
    };
    let xf = transform(&Transform::new(t.position(), t.rotation(), t.mirrored()));
    let geometry = Geometry::Path(xf * paths(&local));
    Item::new(layer, geometry, Style::stroke(length(*t.stroke_width())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use foilfern_core::geometry::Vertex;
    use foilfern_core::types::PositiveLength;
    use kurbo::Shape;

    #[test]
    fn points_are_mm_y_up() {
        assert_eq!(
            point(Point::from_nm(1_500_000, -2_000_000)),
            KPoint::new(1.5, -2.0)
        );
        assert_eq!(length(Length::new(250_000)), 0.25);
    }

    #[test]
    fn straight_path() {
        let p = Path::new(vec![
            Vertex::at(Point::from_nm(0, 0)),
            Vertex::at(Point::from_nm(10_000_000, 0)),
            Vertex::at(Point::from_nm(10_000_000, 5_000_000)),
        ]);
        let b = path(&p);
        let pts: Vec<_> = b.elements().iter().filter_map(|e| e.end_point()).collect();
        assert_eq!(pts.len(), 3);
        for (a, e) in pts.iter().zip([(0.0, 0.0), (10.0, 0.0), (10.0, 5.0)]) {
            assert!((*a - KPoint::from(e)).hypot() < 1e-9, "{a:?} vs {e:?}");
        }
    }

    #[test]
    fn arc_matches_true_circle() {
        // Full circle of 10 mm diameter: two 180° arcs.
        let c = Path::circle(PositiveLength::new(Length::new(10_000_000)).unwrap());
        let b = path(&c);
        let bbox = b.bounding_box();
        assert!((bbox.width() - 10.0).abs() < 1e-3, "{bbox:?}");
        assert!((bbox.height() - 10.0).abs() < 1e-3, "{bbox:?}");
        // Every curve point is on the circle (Qt's approximation is within
        // 0.03 % of the radius).
        for seg in b.segments() {
            for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
                use kurbo::ParamCurve;
                let r = seg.eval(t).to_vec2().hypot();
                assert!((r - 5.0).abs() < 5.0 * 3e-4, "radius {r}");
            }
        }
    }

    #[test]
    fn arc_direction() {
        // 90° counter-clockwise from (1, 0) to (0, 1) around the origin:
        // passes through the first quadrant.
        let p = Path::new(vec![
            Vertex::new(Point::from_nm(1_000_000, 0), Angle::DEG90),
            Vertex::at(Point::from_nm(0, 1_000_000)),
        ]);
        let b = path(&p);
        let bbox = b.bounding_box();
        assert!(bbox.x0 > -1e-6 && bbox.y0 > -1e-6, "{bbox:?}");
        assert!((bbox.x1 - 1.0).abs() < 1e-6 && (bbox.y1 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn footprint_transform() {
        let t = Transform::new(Point::from_nm(10_000_000, 0), Angle::DEG90, true);
        let a = transform(&t);
        // Mirror (x -> -x), rotate 90° CCW, translate.
        let p = a * KPoint::new(1.0, 0.0);
        assert!((p - KPoint::new(10.0, -1.0)).hypot() < 1e-12, "{p:?}");
        // Same as the core transform.
        let core = t.map(&Point::from_nm(1_000_000, 0));
        assert!((point(core) - p).hypot() < 1e-6);
    }
}
