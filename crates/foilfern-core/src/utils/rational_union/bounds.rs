//! Exact clipping to a query box before building a boolean arrangement.
//!
//! Sutherland-Hodgman clipping retains the signed winding of each contour,
//! including holes and the cancelling boundary bridges of disconnected pieces.
//! The arrangement resolves those bridges; no rounded polygon is constructed.

use super::{Operand, Point, Region};
use clipper::PolyFillType;

impl Region {
    pub(super) fn operand_within(&self, min: &Point, max: &Point) -> Operand {
        if self
            .bounds
            .as_ref()
            .is_some_and(|(a, b)| a.x >= min.x && a.y >= min.y && b.x <= max.x && b.y <= max.y)
        {
            return self.operand();
        }
        let mut paths = Vec::new();
        for (original, (a, b)) in self.paths.iter().zip(&self.path_bounds) {
            if a.x >= max.x || b.x <= min.x || a.y >= max.y || b.y <= min.y {
                continue;
            }
            let mut path = original.clone();
            for (x_axis, limit, lower, bound) in [
                (true, &min.x, true, &a.x),
                (true, &max.x, false, &b.x),
                (false, &min.y, true, &a.y),
                (false, &max.y, false, &b.y),
            ] {
                if (lower && bound >= limit) || (!lower && bound <= limit) {
                    continue;
                }

                let mut clipped = Vec::with_capacity(path.len());
                if let Some(mut prev) = path.last() {
                    let mut prev_coord = if x_axis { &prev.x } else { &prev.y };
                    let mut prev_inside = if lower {
                        prev_coord >= limit
                    } else {
                        prev_coord <= limit
                    };
                    for current in &path {
                        let current_coord = if x_axis { &current.x } else { &current.y };
                        let current_inside = if lower {
                            current_coord >= limit
                        } else {
                            current_coord <= limit
                        };
                        if prev_inside != current_inside {
                            let t = (limit - prev_coord) / (current_coord - prev_coord);
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
                path = clipped;
            }
            paths.push(path);
        }
        paths.retain(|p| p.len() >= 3);
        Operand {
            paths,
            fill: PolyFillType::NonZero,
        }
    }
}
