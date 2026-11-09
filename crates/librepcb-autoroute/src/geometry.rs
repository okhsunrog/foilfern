//! Exact geometry used for clearance checks and rasterization.
//!
//! Every copper object is a *core* (point, segment or polygon area) plus a
//! distance: a trace is a segment core with the half width, a via a point
//! core with the radius, a pad polygon a polygon core with distance zero.
//! Clearance checks compare core-to-core distances with the sum of the
//! radii and the clearance, so round caps and circles are handled exactly
//! instead of being approximated by polygons.
//!
//! Coordinates are integer nanometers. Intersection and inside tests use
//! exact `i128` arithmetic; distances are `f64` (exact enough: coordinates
//! are below 2^53 nm).

use librepcb_core::types::Point;

/// A point in nanometers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct P {
    pub x: i64,
    pub y: i64,
}

impl P {
    pub const fn new(x: i64, y: i64) -> Self {
        Self { x, y }
    }

    pub fn to_point(self) -> Point {
        Point::from_nm(self.x, self.y)
    }
}

impl From<Point> for P {
    fn from(p: Point) -> Self {
        Self::new(p.x.to_nm(), p.y.to_nm())
    }
}

/// The core of a copper object (see module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Core {
    Point(P),
    Segment(P, P),
    Polygon(Vec<P>),
}

impl Core {
    /// Bounding box `(min, max)`.
    pub fn bounds(&self) -> (P, P) {
        match self {
            Core::Point(p) => (*p, *p),
            Core::Segment(a, b) => (
                P::new(a.x.min(b.x), a.y.min(b.y)),
                P::new(a.x.max(b.x), a.y.max(b.y)),
            ),
            Core::Polygon(v) => bounds_of(v),
        }
    }

    /// Distance between this core and the segment `a`-`b` (zero if they
    /// touch or overlap).
    pub fn distance_to_segment(&self, a: P, b: P) -> f64 {
        match self {
            Core::Point(p) => point_segment_distance(*p, a, b),
            Core::Segment(c, d) => segment_segment_distance(a, b, *c, *d),
            Core::Polygon(v) => {
                if v.is_empty() {
                    return f64::INFINITY;
                }
                if point_in_polygon(v, a) || point_in_polygon(v, b) {
                    return 0.0;
                }
                polygon_edges(v)
                    .map(|(c, d)| segment_segment_distance(a, b, c, d))
                    .fold(f64::INFINITY, f64::min)
            }
        }
    }
}

/// Bounding box of points; `(0,0),(0,0)` for an empty slice.
pub(crate) fn bounds_of(points: &[P]) -> (P, P) {
    let mut it = points.iter();
    let Some(first) = it.next() else {
        return (P::new(0, 0), P::new(0, 0));
    };
    it.fold((*first, *first), |(lo, hi), p| {
        (
            P::new(lo.x.min(p.x), lo.y.min(p.y)),
            P::new(hi.x.max(p.x), hi.y.max(p.y)),
        )
    })
}

/// Iterates over the edges of a closed polygon.
pub(crate) fn polygon_edges(v: &[P]) -> impl Iterator<Item = (P, P)> + '_ {
    v.iter()
        .enumerate()
        .map(move |(i, a)| (*a, v[(i + 1) % v.len()]))
}

/// Sign of the cross product (b - a) x (c - a).
fn orientation(a: P, b: P, c: P) -> i32 {
    let v = i128::from(b.x - a.x) * i128::from(c.y - a.y)
        - i128::from(b.y - a.y) * i128::from(c.x - a.x);
    v.signum() as i32
}

fn on_segment_bbox(a: P, b: P, p: P) -> bool {
    p.x >= a.x.min(b.x) && p.x <= a.x.max(b.x) && p.y >= a.y.min(b.y) && p.y <= a.y.max(b.y)
}

/// Whether the closed segments `a`-`b` and `c`-`d` intersect.
pub(crate) fn segments_intersect(a: P, b: P, c: P, d: P) -> bool {
    let o1 = orientation(a, b, c);
    let o2 = orientation(a, b, d);
    let o3 = orientation(c, d, a);
    let o4 = orientation(c, d, b);
    if o1 != o2 && o3 != o4 && o1 * o2 <= 0 && o3 * o4 <= 0 {
        return true;
    }
    (o1 == 0 && on_segment_bbox(a, b, c))
        || (o2 == 0 && on_segment_bbox(a, b, d))
        || (o3 == 0 && on_segment_bbox(c, d, a))
        || (o4 == 0 && on_segment_bbox(c, d, b))
}

/// Whether `p` lies exactly on the closed segment `a`-`b`.
pub(crate) fn point_on_segment(p: P, a: P, b: P) -> bool {
    orientation(a, b, p) == 0 && on_segment_bbox(a, b, p)
}

/// Distance between point `p` and segment `a`-`b`.
pub(crate) fn point_segment_distance(p: P, a: P, b: P) -> f64 {
    let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
    let (px, py) = ((p.x - a.x) as f64, (p.y - a.y) as f64);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        ((px * dx + py * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (ex, ey) = (px - t * dx, py - t * dy);
    (ex * ex + ey * ey).sqrt()
}

/// Distance between the segments `a`-`b` and `c`-`d`.
pub(crate) fn segment_segment_distance(a: P, b: P, c: P, d: P) -> f64 {
    if segments_intersect(a, b, c, d) {
        return 0.0;
    }
    point_segment_distance(a, c, d)
        .min(point_segment_distance(b, c, d))
        .min(point_segment_distance(c, a, b))
        .min(point_segment_distance(d, a, b))
}

/// Even-odd point in polygon test (points on the boundary count as inside).
pub(crate) fn point_in_polygon(v: &[P], p: P) -> bool {
    let mut inside = false;
    for (a, b) in polygon_edges(v) {
        if orientation(a, b, p) == 0 && on_segment_bbox(a, b, p) {
            return true;
        }
        if (a.y > p.y) != (b.y > p.y) {
            // x coordinate of the edge at p.y compared with p.x, exactly:
            // p.x < a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y)
            let lhs = i128::from(p.x - a.x) * i128::from(b.y - a.y);
            let rhs = i128::from(p.y - a.y) * i128::from(b.x - a.x);
            let crosses = if b.y > a.y { lhs < rhs } else { lhs > rhs };
            if crosses {
                inside = !inside;
            }
        }
    }
    inside
}

/// The x interval `[lo, hi]` of the row `y` covered by the core dilated by
/// `d`, for point and segment cores. Returns `None` if the row misses it.
pub(crate) fn capsule_row_interval(a: P, b: P, d: f64, y: f64) -> Option<(f64, f64)> {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut add = |iv: Option<(f64, f64)>| {
        if let Some((l, h)) = iv {
            lo = lo.min(l);
            hi = hi.max(h);
        }
    };
    add(circle_row_interval(a, d, y));
    add(circle_row_interval(b, d, y));
    let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
    let len = (dx * dx + dy * dy).sqrt();
    if len > 0.0 {
        let (ux, uy) = (dx / len, dy / len);
        let ry = y - a.y as f64;
        // Band: 0 <= (x - ax) * ux + ry * uy <= len and
        // -d <= (x - ax) * uy - ry * ux <= d.
        let band = linear_range(ux, ry * uy, 0.0, len)
            .and_then(|r1| linear_range(uy, -ry * ux, -d, d).map(|r2| (r1, r2)))
            .and_then(|((l1, h1), (l2, h2))| {
                let (l, h) = (l1.max(l2), h1.min(h2));
                (l <= h).then_some((l + a.x as f64, h + a.x as f64))
            });
        add(band);
    }
    (lo <= hi).then_some((lo, hi))
}

/// Solves `lo <= c * t + k <= hi` for `t`.
fn linear_range(c: f64, k: f64, lo: f64, hi: f64) -> Option<(f64, f64)> {
    if c.abs() < 1e-12 {
        return (k >= lo && k <= hi).then_some((f64::NEG_INFINITY, f64::INFINITY));
    }
    let (t1, t2) = ((lo - k) / c, (hi - k) / c);
    Some((t1.min(t2), t1.max(t2)))
}

fn circle_row_interval(c: P, r: f64, y: f64) -> Option<(f64, f64)> {
    let dy = y - c.y as f64;
    let s = r * r - dy * dy;
    (s >= 0.0).then(|| {
        let w = s.sqrt();
        (c.x as f64 - w, c.x as f64 + w)
    })
}

/// The x coordinates where the row `y` crosses the edges of the polygons,
/// sorted; consecutive pairs are the inside intervals (even-odd rule).
pub(crate) fn polygon_row_crossings<'a>(
    polygons: impl IntoIterator<Item = &'a [P]>,
    y: f64,
) -> Vec<f64> {
    let mut xs = Vec::new();
    for v in polygons {
        for (a, b) in polygon_edges(v) {
            let (ay, by) = (a.y as f64, b.y as f64);
            if (ay > y) != (by > y) {
                let t = (y - ay) / (by - ay);
                xs.push(a.x as f64 + t * (b.x - a.x) as f64);
            }
        }
    }
    xs.sort_by(f64::total_cmp);
    xs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distances() {
        let a = P::new(0, 0);
        let b = P::new(10, 0);
        assert_eq!(point_segment_distance(P::new(5, 3), a, b), 3.0);
        assert_eq!(point_segment_distance(P::new(13, 4), a, b), 5.0);
        assert_eq!(
            segment_segment_distance(a, b, P::new(5, -5), P::new(5, 5)),
            0.0
        );
        assert_eq!(
            segment_segment_distance(a, b, P::new(0, 2), P::new(10, 2)),
            2.0
        );
    }

    #[test]
    fn polygon() {
        let sq = vec![P::new(0, 0), P::new(10, 0), P::new(10, 10), P::new(0, 10)];
        assert!(point_in_polygon(&sq, P::new(5, 5)));
        assert!(point_in_polygon(&sq, P::new(10, 5)));
        assert!(!point_in_polygon(&sq, P::new(11, 5)));
        let core = Core::Polygon(sq);
        assert_eq!(core.distance_to_segment(P::new(2, 2), P::new(3, 3)), 0.0);
        assert_eq!(core.distance_to_segment(P::new(13, 2), P::new(13, 8)), 3.0);
    }

    #[test]
    fn capsule_rows() {
        let (lo, hi) = capsule_row_interval(P::new(0, 0), P::new(10, 0), 2.0, 0.0).unwrap();
        assert_eq!((lo, hi), (-2.0, 12.0));
        let (lo, hi) = capsule_row_interval(P::new(0, 0), P::new(10, 0), 2.0, 2.0).unwrap();
        assert_eq!((lo, hi), (0.0, 10.0));
        assert!(capsule_row_interval(P::new(0, 0), P::new(10, 0), 2.0, 2.5).is_none());
        // Diagonal segment: the row through the middle.
        let (lo, hi) = capsule_row_interval(P::new(0, 0), P::new(10, 10), 1.0, 5.0).unwrap();
        let w = std::f64::consts::SQRT_2;
        assert!((lo - (5.0 - w)).abs() < 1e-9 && (hi - (5.0 + w)).abs() < 1e-9);
    }
}
