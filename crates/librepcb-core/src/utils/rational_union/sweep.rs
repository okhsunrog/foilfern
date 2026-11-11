//! Winding classification of the noded arrangement, without coordinate rounding.

use super::{Edge, Error, Operand, Point};
use clipper::PolyFillType;
use dashu_ratio::RBig;
use std::collections::BTreeMap;

#[derive(Default)]
struct Event {
    starts: Vec<usize>,
    ends: Vec<usize>,
    vertical: Vec<usize>,
}
struct Line {
    slope: RBig,
    intercept: RBig,
}
impl Line {
    fn y(&self, x: &RBig) -> RBig {
        &self.slope * x + &self.intercept
    }
}

fn filled(winding: i64, fill: PolyFillType) -> bool {
    match fill {
        PolyFillType::EvenOdd => winding % 2 != 0,
        PolyFillType::NonZero => winding != 0,
        PolyFillType::Positive => winding > 0,
        PolyFillType::Negative => winding < 0,
    }
}

pub(super) fn boundary(
    points: &[Point],
    edges: &[Edge],
    operands: &[Operand],
) -> Result<Vec<(usize, usize)>, Error> {
    let mut events: BTreeMap<RBig, Event> = BTreeMap::new();
    let mut lines = Vec::with_capacity(edges.len());
    for (i, edge) in edges.iter().enumerate() {
        let (a, b) = (&points[edge.a], &points[edge.b]);
        if a.x == b.x {
            events.entry(a.x.clone()).or_default().vertical.push(i);
            lines.push(Line {
                slope: RBig::ZERO,
                intercept: RBig::ZERO,
            });
        } else {
            events.entry(a.x.clone()).or_default().starts.push(i);
            events.entry(b.x.clone()).or_default().ends.push(i);
            let slope = (&b.y - &a.y) / (&b.x - &a.x);
            let intercept = &a.y - &slope * &a.x;
            lines.push(Line { slope, intercept });
        }
    }
    let mut active: Vec<usize> = Vec::new();
    let mut ended = vec![false; edges.len()];
    let mut seen = vec![false; edges.len()];
    let mut prefix = vec![false];
    let mut boundary = Vec::new();
    let mut winding = vec![0_i64; operands.len()];
    for (x, event) in events {
        let midpoint =
            |id: usize| (&points[edges[id].a].y + &points[edges[id].b].y) / RBig::from(2);
        let left: Vec<_> = event
            .vertical
            .iter()
            .map(|&v| {
                let y = midpoint(v);
                let rank = active.partition_point(|&e| lines[e].y(&x) < y);
                prefix[rank]
            })
            .collect();
        for e in event.ends {
            ended[e] = true;
        }
        active.retain(|&e| !ended[e]);
        for e in event.starts {
            let y = lines[e].y(&x);
            // At a shared endpoint, slope orders edges immediately to its right.
            let at = active.partition_point(|&other| {
                lines[other]
                    .y(&x)
                    .cmp(&y)
                    .then_with(|| lines[other].slope.cmp(&lines[e].slope))
                    .is_lt()
            });
            active.insert(at, e);
        }
        winding.fill(0);
        let mut inside_count = 0_usize;
        prefix.clear();
        prefix.push(false);
        for &id in &active {
            let below = inside_count != 0;
            for &(group, delta) in &edges[id].deltas {
                let was = filled(winding[group], operands[group].fill);
                winding[group] += delta;
                let now = filled(winding[group], operands[group].fill);
                match (was, now) {
                    (false, true) => inside_count += 1,
                    (true, false) => inside_count -= 1,
                    _ => {}
                }
            }
            let above = inside_count != 0;
            prefix.push(above);
            if !seen[id] {
                seen[id] = true;
                if below != above {
                    let e = &edges[id];
                    boundary.push(if above { (e.a, e.b) } else { (e.b, e.a) });
                }
            }
        }
        if winding.iter().any(|&w| w != 0) {
            return Err(Error::InvalidTopology);
        }
        for (&id, left) in event.vertical.iter().zip(left) {
            let y = midpoint(id);
            let rank = active.partition_point(|&e| lines[e].y(&x) < y);
            let right = prefix[rank];
            if left != right {
                let e = &edges[id];
                boundary.push(if left { (e.a, e.b) } else { (e.b, e.a) });
            }
        }
    }
    if !active.is_empty() {
        return Err(Error::InvalidTopology);
    }
    Ok(boundary)
}
