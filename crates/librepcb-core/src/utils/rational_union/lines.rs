//! Exact clipping of open cutout boundaries against closed copper regions.

use super::arrangement::{Segment, intersections};
use super::{Error, MAX_COORD, Point, Region, cross, direction};
use dashu_ratio::RBig;

impl Region {
    /// Splits an open polyline at every boundary intersection, retaining the
    /// positive-length intervals inside (or outside) this region. Coincident
    /// boundary intervals count as covered copper; isolated point touches do not
    /// produce intervals. No point is rounded before classification.
    pub fn clip_polyline(&self, path: &[Point], inside: bool) -> Result<Vec<Vec<Point>>, Error> {
        let limit = RBig::from(MAX_COORD);
        for p in path {
            if p.x < -&limit || p.x > limit || p.y < -&limit || p.y > limit {
                return Err(Error::CoordinateOutOfRange);
            }
        }
        let boundary: Vec<_> = self
            .paths
            .iter()
            .flat_map(|ring| {
                ring.iter()
                    .zip(ring.iter().cycle().skip(1))
                    .take(ring.len())
                    .map(|(a, b)| Segment::new(a.clone(), b.clone(), 0))
            })
            .collect();
        let mut result: Vec<Vec<Point>> = Vec::new();
        for pair in path.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if a == b {
                continue;
            }
            let segment = Segment::new(a.clone(), b.clone(), 0);
            let mut split = vec![a.clone(), b.clone()];
            for edge in &boundary {
                if segment.overlaps_box(edge) {
                    split.extend(intersections(&segment, edge));
                }
            }
            split.sort();
            split.dedup();
            if a > b {
                split.reverse();
            }
            for pair in split.windows(2) {
                let middle = Point::new(
                    (&pair[0].x + &pair[1].x) / RBig::from(2),
                    (&pair[0].y + &pair[1].y) / RBig::from(2),
                );
                if self.covers(&middle) == inside {
                    if let Some(last) = result.last_mut()
                        && last.last() == Some(&pair[0])
                    {
                        last.push(pair[1].clone());
                    } else {
                        result.push(pair.to_vec());
                    }
                }
            }
        }
        Ok(result)
    }

    fn covers(&self, p: &Point) -> bool {
        let mut winding = 0_i64;
        for ring in &self.paths {
            for (a, b) in ring
                .iter()
                .zip(ring.iter().cycle().skip(1))
                .take(ring.len())
            {
                let ab = direction(a, b);
                let ap = direction(a, p);
                let det = cross(&ab.0, &ab.1, &ap.0, &ap.1);
                if det == RBig::ZERO
                    && p.x >= a.x.clone().min(b.x.clone())
                    && p.x <= a.x.clone().max(b.x.clone())
                    && p.y >= a.y.clone().min(b.y.clone())
                    && p.y <= a.y.clone().max(b.y.clone())
                {
                    return true;
                }
                if a.y <= p.y && b.y > p.y && det > RBig::ZERO {
                    winding += 1;
                }
                if a.y > p.y && b.y <= p.y && det < RBig::ZERO {
                    winding -= 1;
                }
            }
        }
        winding != 0
    }
}
