//! Areas for the geometric package checks (see
//! [`run_package_checks()`](super::run_package_checks)).
//!
//! Upstream compares `QPainterPath`s of the pad outlines and the legend
//! with `QPainterPath::intersects()` and `contains()`, which approximate
//! curves differently depending on the case (`QPathClipper` samples each
//! Bézier curve at only a few points, e.g. 3 points per quarter circle of a
//! typical pad corner, while the rectangle and point predicates evaluate
//! curves exactly). These approximations decide e.g. whether a legend line
//! 20 µm too close to a rounded pad corner is reported for real libraries,
//! so the painter paths are built like upstream and compared with the
//! ported Qt predicates of [`PainterPathPx`] instead of exact polygon
//! operations (e.g. Clipper, whose results differ from upstream for such
//! cases).

use crate::geometry::Path;
use crate::types::{Angle, Length, Point, UnsignedLength};
use crate::utils::painter_path::PainterPathPx;

/// A filled area as `QPainterPath` in footprint pixel coordinates (e.g. pad
/// copper).
#[derive(Debug, Clone)]
pub struct Area {
    path: PainterPathPx,
}

impl Area {
    /// Builds the area of closed outlines (upstream
    /// `Path::toQPainterPathPx()` added to one painter path), optionally
    /// mapped like upstream `Transform::mapPx()` (position and rotation of a
    /// pad).
    pub fn from_outlines(
        outlines: &[Path],
        transform: Option<(Point, Angle)>,
        winding_fill: bool,
    ) -> Self {
        let mut path = PainterPathPx::from_paths(outlines);
        if let Some((position, rotation)) = transform {
            path = path.mapped(position, rotation);
        }
        path.set_winding_fill(winding_fill);
        Self { path }
    }

    /// Returns whether the areas overlap or touch (upstream
    /// `QPainterPath::intersects(QPainterPath)`).
    pub fn intersects(&self, other: &Area) -> bool {
        self.path.intersects(&other.path)
    }

    /// Returns whether `other` lies inside this area without touching its
    /// boundary (upstream `QPainterPath::contains(QPainterPath)`).
    pub fn contains(&self, other: &Area) -> bool {
        self.path.contains_path(&other.path)
    }

    /// Returns whether the origin lies inside the area (upstream
    /// `QPainterPath::contains(QPointF(0, 0))`).
    pub fn contains_origin(&self) -> bool {
        self.path.contains_point((0.0, 0.0))
    }
}

/// The legend of a footprint side as `QPainterPath` (odd-even fill), built
/// like upstream from the legend polygons with `Toolbox::shapeFromPath()`.
#[derive(Debug, Default)]
pub struct Legend {
    path: PainterPathPx,
}

impl Legend {
    /// Adds a polygon: with a line width, its stroke (the default `QPen`:
    /// square caps, bevel joins) plus its area if `filled`; without line
    /// width, the (implicitly closed) path itself.
    pub fn add_polygon(&mut self, path: &Path, line_width: UnsignedLength, filled: bool) {
        let painter_path = PainterPathPx::from_paths([path]);
        if painter_path.is_empty() {
            return;
        }
        if *line_width > Length::ZERO {
            let width = line_width.to_px().max(0.00000001);
            self.path.append(&painter_path.stroked(width));
            if filled {
                self.path.append(&painter_path);
            }
        } else {
            self.path.append(&painter_path);
        }
    }

    /// Returns whether `area` overlaps or touches the legend (upstream
    /// `area.intersects(legend)`).
    pub fn intersects(&self, area: &Area) -> bool {
        area.path.intersects(&self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Vertex;

    fn rect(x1: i64, y1: i64, x2: i64, y2: i64) -> Path {
        Path::rect(Point::from_nm(x1, y1), Point::from_nm(x2, y2))
    }

    fn line(x1: i64, y1: i64, x2: i64, y2: i64) -> Path {
        Path::new(vec![
            Vertex::at(Point::from_nm(x1, y1)),
            Vertex::at(Point::from_nm(x2, y2)),
        ])
    }

    #[test]
    fn touching_areas_intersect() {
        let a = Area::from_outlines(&[rect(0, 0, 1_000_000, 1_000_000)], None, false);
        let b = Area::from_outlines(&[rect(1_000_000, 0, 2_000_000, 1_000_000)], None, false);
        let c = Area::from_outlines(&[rect(1_000_100, 0, 2_000_000, 1_000_000)], None, false);
        assert!(a.intersects(&b));
        assert!(!a.intersects(&c));
        // Rotated by 45°: corner at x = 1.4929 mm.
        let d = Area::from_outlines(
            &[rect(-500_000, -500_000, 500_000, 500_000)],
            Some((Point::from_nm(2_200_000, 500_000), Angle::DEG45)),
            false,
        );
        assert!(!d.intersects(&a));
        assert!(d.intersects(&b));
    }

    #[test]
    fn containment() {
        let outer = Area::from_outlines(&[rect(0, 0, 1_000_000, 1_000_000)], None, false);
        let inner = Area::from_outlines(&[rect(100, 100, 999_900, 999_900)], None, false);
        assert!(outer.contains(&inner));
        assert!(!inner.contains(&outer));
    }

    #[test]
    fn origin() {
        // On the left edge: inside, on the right edge: outside.
        let area = Area::from_outlines(&[rect(0, -10, 1_000, 10)], None, false);
        assert!(area.contains_origin());
        let area = Area::from_outlines(&[rect(-1_000, -10, 0, 10)], None, false);
        assert!(!area.contains_origin());
    }

    #[test]
    fn legend() {
        let pad = Area::from_outlines(&[rect(0, 0, 1_000_000, 1_000_000)], None, false);
        let width = UnsignedLength::new(Length::new(200_000)).unwrap();
        // A 0.2 mm wide line ending 0.15 mm left of the pad (square cap).
        let mut legend = Legend::default();
        legend.add_polygon(&line(-1_000_000, 500_000, -250_000, 500_000), width, false);
        assert!(!legend.intersects(&pad));
        legend.add_polygon(&line(-1_000_000, 500_000, -100_000, 500_000), width, false);
        assert!(legend.intersects(&pad));

        // Zero-width lines crossing the pad.
        let mut legend = Legend::default();
        legend.add_polygon(
            &line(-1_000_000, 500_000, 2_000_000, 500_000),
            UnsignedLength::ZERO,
            false,
        );
        assert!(legend.intersects(&pad));
    }
}
