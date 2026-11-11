//! Exact segment noding for the experimental rational copper union.

use super::{Edge, Error, MAX_COORD, Operand, Point, Statistics, cross, direction};
use dashu_ratio::RBig;
use std::collections::BTreeMap;

struct Segment {
    a: Point,
    b: Point,
    group: usize,
    min: Point,
    max: Point,
    integer: Option<[i64; 4]>,
}

impl Segment {
    fn new(a: Point, b: Point, group: usize) -> Self {
        let integer = [&a.x, &a.y, &b.x, &b.y].map(|n| {
            if n.is_int() {
                i64::try_from(n.numerator()).ok()
            } else {
                None
            }
        });
        Self {
            min: Point::new(a.x.clone().min(b.x.clone()), a.y.clone().min(b.y.clone())),
            max: Point::new(a.x.clone().max(b.x.clone()), a.y.clone().max(b.y.clone())),
            integer: integer
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .and_then(|v| v.try_into().ok()),
            a,
            b,
            group,
        }
    }

    fn contains_box(&self, p: &Point) -> bool {
        self.min.x <= p.x && p.x <= self.max.x && self.min.y <= p.y && p.y <= self.max.y
    }
}

pub(super) fn build(
    operands: &[Operand],
    stats: &mut Statistics,
) -> Result<(Vec<Point>, Vec<Edge>), Error> {
    let limit = RBig::from(MAX_COORD);
    let negative_limit = -&limit;
    let mut segments = Vec::new();
    for (group, operand) in operands.iter().enumerate() {
        for path in &operand.paths {
            for p in path {
                if p.x < negative_limit || p.x > limit || p.y < negative_limit || p.y > limit {
                    return Err(Error::CoordinateOutOfRange);
                }
            }
            if path.len() < 3 {
                continue;
            }
            for (a, b) in path
                .iter()
                .zip(path.iter().cycle().skip(1))
                .take(path.len())
            {
                if a != b {
                    segments.push(Segment::new(a.clone(), b.clone(), group));
                }
            }
        }
    }
    stats.input_segments = segments.len();
    let mut split: Vec<Vec<Point>> = segments
        .iter()
        .map(|s| vec![s.a.clone(), s.b.clone()])
        .collect();
    let mut order: Vec<_> = (0..segments.len()).collect();
    order.sort_by(|&a, &b| segments[a].min.x.cmp(&segments[b].min.x).then(a.cmp(&b)));
    let mut active: Vec<usize> = Vec::new();
    for i in order {
        let a = &segments[i];
        active.retain(|&j| segments[j].max.x >= a.min.x);
        for &j in &active {
            let b = &segments[j];
            if a.min.y > b.max.y || b.min.y > a.max.y {
                continue;
            }
            stats.candidate_pairs += 1;
            for p in intersections(a, b) {
                stats.intersections += 1;
                split[i].push(p.clone());
                split[j].push(p);
            }
        }
        active.push(i);
    }
    let mut point_ids = BTreeMap::new();
    let mut points = Vec::new();
    let mut merged: BTreeMap<(usize, usize), BTreeMap<usize, i64>> = BTreeMap::new();
    for (segment, mut vertices) in segments.iter().zip(split) {
        vertices.sort();
        vertices.dedup();
        let delta = if segment.a < segment.b { 1 } else { -1 };
        for pair in vertices.windows(2) {
            let mut ids = [0; 2];
            for (id, p) in ids.iter_mut().zip(pair) {
                *id = *point_ids.entry(p.clone()).or_insert_with(|| {
                    let id = points.len();
                    points.push(p.clone());
                    id
                });
            }
            *merged
                .entry((ids[0], ids[1]))
                .or_default()
                .entry(segment.group)
                .or_default() += delta;
        }
    }
    let edges: Vec<_> = merged
        .into_iter()
        .filter_map(|((a, b), groups)| {
            let deltas: Vec<_> = groups.into_iter().filter(|&(_, d)| d != 0).collect();
            (!deltas.is_empty()).then_some(Edge { a, b, deltas })
        })
        .collect();
    stats.atomic_edges = edges.len();
    Ok((points, edges))
}

fn intersections(a: &Segment, b: &Segment) -> Vec<Point> {
    if let (Some(a), Some(b)) = (a.integer, b.integer)
        && let Some(result) = integer_intersection(a, b)
    {
        return result;
    }
    let r = direction(&a.a, &a.b);
    let s = direction(&b.a, &b.b);
    let q = direction(&a.a, &b.a);
    let den = cross(&r.0, &r.1, &s.0, &s.1);
    let t = cross(&q.0, &q.1, &s.0, &s.1);
    let u = cross(&q.0, &q.1, &r.0, &r.1);
    if den == RBig::ZERO {
        if u != RBig::ZERO {
            return Vec::new();
        }
        return [&a.a, &a.b, &b.a, &b.b]
            .into_iter()
            .filter(|p| a.contains_box(p) && b.contains_box(p))
            .cloned()
            .collect();
    }
    let between = |v: &RBig| {
        if den > RBig::ZERO {
            v >= &RBig::ZERO && v <= &den
        } else {
            v <= &RBig::ZERO && v >= &den
        }
    };
    if !between(&t) || !between(&u) {
        return Vec::new();
    }
    let t = t / den;
    vec![Point::new(&a.a.x + r.0 * &t, &a.a.y + r.1 * t)]
}

/// Cheap exact path for integer endpoints; overflow falls back to RBig.
/// `None` means fallback, `Some(empty)` means a proven disjoint pair.
fn integer_intersection(a: [i64; 4], b: [i64; 4]) -> Option<Vec<Point>> {
    let [ax, ay, bx, by] = a.map(i128::from);
    let [cx, cy, dx, dy] = b.map(i128::from);
    let (rx, ry, sx, sy, qx, qy) = (bx - ax, by - ay, dx - cx, dy - cy, cx - ax, cy - ay);
    let det = |x: i128, y: i128, u: i128, v: i128| x.checked_mul(v)?.checked_sub(y.checked_mul(u)?);
    let den = det(rx, ry, sx, sy)?;
    let t = det(qx, qy, sx, sy)?;
    let u = det(qx, qy, rx, ry)?;
    if den == 0 {
        if u != 0 {
            return Some(Vec::new());
        }
        // Collinear pairs are less common; use the common exact overlap path.
        return None;
    }
    let between = |v| {
        if den > 0 {
            v >= 0 && v <= den
        } else {
            v <= 0 && v >= den
        }
    };
    if !between(t) || !between(u) {
        return Some(Vec::new());
    }
    let coordinate = |origin: i128, delta: i128| {
        let numerator = origin
            .checked_mul(den)
            .and_then(|v| delta.checked_mul(t).and_then(|w| v.checked_add(w)));
        match numerator {
            Some(n) => RBig::from(n) / RBig::from(den),
            None => RBig::from(origin) + RBig::from(delta) * RBig::from(t) / RBig::from(den),
        }
    };
    Some(vec![Point::new(coordinate(ax, rx), coordinate(ay, ry))])
}
