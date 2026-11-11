//! Exact clipping to a query box before building a boolean arrangement.
//!
//! Sutherland-Hodgman clipping retains the signed winding of each contour,
//! including holes and the cancelling boundary bridges of disconnected pieces.
//! The arrangement resolves those bridges; no rounded polygon is constructed.

use super::{Operand, Point, Region};
use clipper::PolyFillType;
use dashu_ratio::RBig;

impl Region {
    pub(super) fn operand_within(&self, min: &Point, max: &Point) -> Operand {
        if self
            .bounds
            .as_ref()
            .is_some_and(|(a, b)| a.x >= min.x && a.y >= min.y && b.x <= max.x && b.y <= max.y)
        {
            return self.operand();
        }
        let mut paths = self.paths.clone();
        for (x_axis, limit, lower) in [
            (true, &min.x, true),
            (true, &max.x, false),
            (false, &min.y, true),
            (false, &max.y, false),
        ] {
            let coord = |p: &Point| -> RBig { if x_axis { p.x.clone() } else { p.y.clone() } };
            for path in &mut paths {
                let mut clipped = Vec::new();
                if let Some(mut prev) = path.last() {
                    let mut prev_coord = coord(prev);
                    let mut prev_inside = if lower {
                        &prev_coord >= limit
                    } else {
                        &prev_coord <= limit
                    };
                    for current in path.iter() {
                        let current_coord = coord(current);
                        let current_inside = if lower {
                            &current_coord >= limit
                        } else {
                            &current_coord <= limit
                        };
                        if prev_inside != current_inside {
                            // A crossing has distinct coordinates on this axis.
                            let t = (limit - &prev_coord) / (&current_coord - &prev_coord);
                            clipped.push(if x_axis {
                                Point::new(limit.clone(), &prev.y + (&current.y - &prev.y) * t)
                            } else {
                                Point::new(&prev.x + (&current.x - &prev.x) * t, limit.clone())
                            });
                        }
                        if current_inside {
                            clipped.push(current.clone());
                        }
                        prev = current;
                        prev_coord = current_coord;
                        prev_inside = current_inside;
                    }
                }
                clipped.dedup();
                *path = clipped;
            }
        }
        paths.retain(|p| p.len() >= 3);
        Operand {
            paths,
            fill: PolyFillType::NonZero,
        }
    }
}
