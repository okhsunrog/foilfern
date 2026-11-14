//! Port of the `QPainterPath` predicates of Qt 6 (`qpainterpath.cpp`,
//! `qpathclipper.cpp`, LGPL-3.0/GPL): `intersects()` and `contains()` for
//! paths, rectangles and points, with their curve approximations.

use super::{
    Bezier, Element, ElementType, PainterPathPx, RectF, fuzzy_compare, fuzzy_point_eq,
    is_bezier_line, isect_line,
};

type PointF = (f64, f64);

/// The approximation of π used by `QPathSegments::addPath()`.
#[allow(clippy::approx_constant)]
const QT_PI: f64 = 3.14;

/// `fuzzyIsNull()` of qpathclipper.cpp.
fn clipper_fuzzy_is_null(d: f64) -> bool {
    d.abs() <= 1e-12
}

/// `comparePoints()` of qpathclipper.cpp.
fn compare_points(a: PointF, b: PointF) -> bool {
    clipper_fuzzy_is_null(a.0 - b.0) && clipper_fuzzy_is_null(a.1 - b.1)
}

/// `qFuzzyIsNull(double)`
fn q_fuzzy_is_null(d: f64) -> bool {
    d.abs() <= 0.000000000001
}

fn dot(a: PointF, b: PointF) -> f64 {
    a.0 * b.0 + a.1 * b.1
}

fn sub(a: PointF, b: PointF) -> PointF {
    (a.0 - b.0, a.1 - b.1)
}

/// `QRectF::contains(QPointF)` (false for null rectangles).
fn rect_contains(r: RectF, p: PointF) -> bool {
    let (mut l, mut rr) = (r.x, r.x);
    if r.width < 0.0 {
        l += r.width;
    } else {
        rr += r.width;
    }
    if l == rr {
        return false;
    }
    if p.0 < l || p.0 > rr {
        return false;
    }
    let (mut t, mut b) = (r.y, r.y);
    if r.height < 0.0 {
        t += r.height;
    } else {
        b += r.height;
    }
    if t == b {
        return false;
    }
    !(p.1 < t || p.1 > b)
}

/// `QRectF::contains(QRectF)` for normalized rectangles.
fn rect_contains_rect(r1: RectF, r2: RectF) -> bool {
    if r1.width == 0.0 || r1.height == 0.0 || r2.width == 0.0 || r2.height == 0.0 {
        return false;
    }
    !(r2.x < r1.x || r2.right() > r1.right() || r2.y < r1.y || r2.bottom() > r1.bottom())
}

/// The early exit of `QPathClipper::intersect()` and `contains()`.
fn rects_disjoint(r1: RectF, r2: RectF) -> bool {
    r1.x.max(r2.x) > (r1.x + r1.width).min(r2.x + r2.width)
        || r1.y.max(r2.y) > (r1.y + r1.height).min(r2.y + r2.height)
}

/// A segment of `QPathSegments` with its bounds.
#[derive(Debug, Clone, Copy)]
struct Segment {
    p1: PointF,
    p2: PointF,
    bounds: RectF,
}

impl Bezier {
    /// `QBezier::split()`
    pub(super) fn split(&self) -> (Self, Self) {
        let mid = |a: PointF, b: PointF| ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5);
        let (p1, p2, p3, p4) = (
            (self.x1, self.y1),
            (self.x2, self.y2),
            (self.x3, self.y3),
            (self.x4, self.y4),
        );
        let mid_12 = mid(p1, p2);
        let mid_23 = mid(p2, p3);
        let mid_34 = mid(p3, p4);
        let mid_12_23 = mid(mid_12, mid_23);
        let mid_23_34 = mid(mid_23, mid_34);
        let mid_12_23_23_34 = mid(mid_12_23, mid_23_34);
        (
            Self::from_points(p1, mid_12, mid_12_23, mid_12_23_23_34),
            Self::from_points(mid_12_23_23_34, mid_23_34, mid_34, p4),
        )
    }
}

/// `qt_painterpath_isect_curve()`
fn isect_curve(bezier: &Bezier, pt: PointF, winding: &mut i32, depth: u32) {
    let (x, y) = pt;
    let bounds = bezier.control_point_bounds();
    // Potential intersection, divide and try again. Note that a side effect
    // of the bottom exclusion is that horizontal lines are dropped, but this
    // is correct according to scan conversion rules.
    if y >= bounds.y && y < bounds.y + bounds.height {
        // Hit lower limit: a rough threshold, but a tradeoff between speed
        // and precision.
        let lower_bound = 0.001;
        if depth == 32 || (bounds.width < lower_bound && bounds.height < lower_bound) {
            // Assume that the curve approximates a line after a while.
            if bezier.x1 <= x {
                *winding += if bezier.y4 > bezier.y1 { 1 } else { -1 };
            }
            return;
        }
        let (first, second) = bezier.split();
        isect_curve(&first, pt, winding, depth + 1);
        isect_curve(&second, pt, winding, depth + 1);
    }
}

/// `qt_isect_curve_horizontal()`
fn isect_curve_horizontal(bezier: &Bezier, y: f64, x1: f64, x2: f64, depth: u32) -> bool {
    let bounds = bezier.control_point_bounds();
    if y >= bounds.top() && y < bounds.bottom() && bounds.right() >= x1 && bounds.left() < x2 {
        let lower_bound = 0.01;
        if depth == 32 || (bounds.width < lower_bound && bounds.height < lower_bound) {
            return true;
        }
        let (first, second) = bezier.split();
        return isect_curve_horizontal(&first, y, x1, x2, depth + 1)
            || isect_curve_horizontal(&second, y, x1, x2, depth + 1);
    }
    false
}

/// `qt_isect_curve_vertical()`
fn isect_curve_vertical(bezier: &Bezier, x: f64, y1: f64, y2: f64, depth: u32) -> bool {
    let bounds = bezier.control_point_bounds();
    if x >= bounds.left() && x < bounds.right() && bounds.bottom() >= y1 && bounds.top() < y2 {
        let lower_bound = 0.01;
        if depth == 32 || (bounds.width < lower_bound && bounds.height < lower_bound) {
            return true;
        }
        let (first, second) = bezier.split();
        return isect_curve_vertical(&first, x, y1, y2, depth + 1)
            || isect_curve_vertical(&second, x, y1, y2, depth + 1);
    }
    false
}

/// `qt_painterpath_isect_line_rect()` (Cohen-Sutherland clipping).
fn isect_line_rect(mut x1: f64, mut y1: f64, mut x2: f64, mut y2: f64, rect: RectF) -> bool {
    const LEFT: u32 = 1 << 0;
    const RIGHT: u32 = 1 << 1;
    const TOP: u32 = 1 << 2;
    const BOTTOM: u32 = 1 << 3;
    let (left, right, top, bottom) = (rect.left(), rect.right(), rect.top(), rect.bottom());
    let code = |x: f64, y: f64| {
        (u32::from(x < left) * LEFT)
            | (u32::from(x > right) * RIGHT)
            | (u32::from(y < top) * TOP)
            | (u32::from(y > bottom) * BOTTOM)
    };
    let mut p1 = code(x1, y1);
    let mut p2 = code(x2, y2);
    if p1 & p2 != 0 {
        // Completely outside.
        return false;
    }
    if p1 | p2 != 0 {
        let dx = x2 - x1;
        let dy = y2 - y1;

        // Clip x coordinates.
        if x1 < left {
            y1 += dy / dx * (left - x1);
            x1 = left;
        } else if x1 > right {
            y1 -= dy / dx * (x1 - right);
            x1 = right;
        }
        if x2 < left {
            y2 += dy / dx * (left - x2);
            x2 = left;
        } else if x2 > right {
            y2 -= dy / dx * (x2 - right);
            x2 = right;
        }

        p1 = (u32::from(y1 < top) * TOP) | (u32::from(y1 > bottom) * BOTTOM);
        p2 = (u32::from(y2 < top) * TOP) | (u32::from(y2 > bottom) * BOTTOM);
        if p1 & p2 != 0 {
            return false;
        }

        // Clip y coordinates (only the resulting x coordinates are needed).
        if y1 < top {
            x1 += dx / dy * (top - y1);
        } else if y1 > bottom {
            x1 -= dx / dy * (y1 - bottom);
        }
        if y2 < top {
            x2 += dx / dy * (top - y2);
        } else if y2 > bottom {
            x2 -= dx / dy * (y2 - bottom);
        }

        p1 = (u32::from(x1 < left) * LEFT) | (u32::from(x1 > right) * RIGHT);
        p2 = (u32::from(x2 < left) * LEFT) | (u32::from(x2 > right) * RIGHT);
        if p1 & p2 != 0 {
            return false;
        }
        return true;
    }
    false
}

/// `pointOnEdge()` of qpainterpath.cpp.
fn point_on_edge(rect: RectF, p: PointF) -> bool {
    ((p.0 == rect.left() || p.0 == rect.right()) && (p.1 >= rect.top() && p.1 <= rect.bottom()))
        || ((p.1 == rect.top() || p.1 == rect.bottom())
            && (p.0 >= rect.left() && p.0 <= rect.right()))
}

/// `QIntersectionFinder::linesIntersect()`
fn lines_intersect(a: &Segment, b: &Segment) -> bool {
    let (p1, p2, q1, q2) = (a.p1, a.p2, b.p1, b.p2);
    if compare_points(p1, p2) || compare_points(q1, q2) {
        return false;
    }
    let p1_equals_q1 = compare_points(p1, q1);
    let p2_equals_q2 = compare_points(p2, q2);
    if p1_equals_q1 && p2_equals_q2 {
        return true;
    }
    let p1_equals_q2 = compare_points(p1, q2);
    let p2_equals_q1 = compare_points(p2, q1);
    if p1_equals_q2 && p2_equals_q1 {
        return true;
    }
    let p_delta = sub(p2, p1);
    let q_delta = sub(q2, q1);
    let par = p_delta.0 * q_delta.1 - p_delta.1 * q_delta.0;
    if q_fuzzy_is_null(par) {
        let normal = (-p_delta.1, p_delta.0);
        // Coinciding?
        if q_fuzzy_is_null(dot(normal, sub(q1, p1))) {
            let dp = dot(p_delta, p_delta);
            let tq1 = dot(p_delta, sub(q1, p1));
            let tq2 = dot(p_delta, sub(q2, p1));
            if (tq1 > 0.0 && tq1 < dp) || (tq2 > 0.0 && tq2 < dp) {
                return true;
            }
            let dq = dot(q_delta, q_delta);
            let tp1 = dot(q_delta, sub(p1, q1));
            let tp2 = dot(q_delta, sub(p2, q1));
            if (tp1 > 0.0 && tp1 < dq) || (tp2 > 0.0 && tp2 < dq) {
                return true;
            }
        }
        return false;
    }
    let inv_par = 1.0 / par;
    let tp = (q_delta.1 * (q1.0 - p1.0) - q_delta.0 * (q1.1 - p1.1)) * inv_par;
    if !(0.0..=1.0).contains(&tp) {
        return false;
    }
    let tq = (p_delta.1 * (q1.0 - p1.0) - p_delta.0 * (q1.1 - p1.1)) * inv_par;
    (0.0..=1.0).contains(&tq)
}

/// `QIntersectionFinder::hasIntersections()`
fn has_intersections(a: &[Segment], b: &[Segment]) -> bool {
    let Some(rb0) = b.first() else {
        return false;
    };
    if a.is_empty() {
        return false;
    }
    let (mut min_x, mut min_y) = (rb0.bounds.left(), rb0.bounds.top());
    let (mut max_x, mut max_y) = (rb0.bounds.right(), rb0.bounds.bottom());
    for s in &b[1..] {
        min_x = min_x.min(s.bounds.left());
        min_y = min_y.min(s.bounds.top());
        max_x = max_x.max(s.bounds.right());
        max_y = max_y.max(s.bounds.bottom());
    }
    for sa in a {
        let r1 = sa.bounds;
        if r1.left() > max_x || min_x > r1.right() || r1.top() > max_y || min_y > r1.bottom() {
            continue;
        }
        for sb in b {
            let r2 = sb.bounds;
            if r1.left() > r2.right()
                || r2.left() > r1.right()
                || r1.top() > r2.bottom()
                || r2.top() > r1.bottom()
            {
                continue;
            }
            if lines_intersect(sa, sb) {
                return true;
            }
        }
    }
    false
}

impl PainterPathPx {
    fn element_count(&self) -> usize {
        self.elements.len()
    }

    fn point_at(&self, i: usize) -> PointF {
        self.elements.get(i).map_or((0.0, 0.0), Element::point)
    }

    /// The segments of `QPathSegments::addPath()` (curves sampled coarsely).
    fn segments(&self) -> Vec<Segment> {
        let mut points: Vec<PointF> = Vec::new();
        let mut indices: Vec<(usize, usize)> = Vec::new();
        let mut has_move_to = false;
        let mut last_move_to = 0;
        let mut last = 0;
        let mut i = 0;
        while i < self.elements.len() {
            let e = self.elements[i];
            let mut current = points.len();
            let current_point = if e.kind == ElementType::CurveTo {
                self.point_at(i + 2)
            } else {
                e.point()
            };
            if i > 0 && compare_points(points[last_move_to], current_point) {
                current = last_move_to;
            } else {
                points.push(current_point);
            }
            match e.kind {
                ElementType::MoveTo => {
                    if has_move_to
                        && last != last_move_to
                        && !compare_points(points[last], points[last_move_to])
                    {
                        indices.push((last, last_move_to));
                    }
                    has_move_to = true;
                    last = current;
                    last_move_to = current;
                }
                ElementType::LineTo => {
                    indices.push((last, current));
                    last = current;
                }
                ElementType::CurveTo => {
                    let bezier = Bezier::from_points(
                        points[last],
                        e.point(),
                        self.point_at(i + 1),
                        self.point_at(i + 2),
                    );
                    if is_bezier_line(&bezier) {
                        indices.push((last, current));
                    } else {
                        let bounds = bezier.control_point_bounds();
                        // Threshold based on similar algorithm as in
                        // qtriangulatingstroker.cpp (float precision like Qt).
                        let size = bounds.width.max(bounds.height) * (2.0 * QT_PI / 6.0);
                        let threshold = (64f32.min(size as f32) as i32).max(3);
                        let one_over_threshold_minus_1 = 1.0 / f64::from(threshold - 1);
                        for t in 1..threshold - 1 {
                            let p = bezier.point_at(f64::from(t) * one_over_threshold_minus_1);
                            let index = points.len();
                            indices.push((last, index));
                            last = index;
                            points.push(p);
                        }
                        indices.push((last, current));
                    }
                    last = current;
                    i += 2;
                }
                ElementType::CurveToData => {}
            }
            i += 1;
        }
        if has_move_to
            && last != last_move_to
            && !compare_points(points[last], points[last_move_to])
        {
            indices.push((last, last_move_to));
        }
        indices
            .into_iter()
            .map(|(a, b)| {
                let (p1, p2) = (points[a], points[b]);
                let (x1, x2) = if p2.0 < p1.0 {
                    (p2.0, p1.0)
                } else {
                    (p1.0, p2.0)
                };
                let (y1, y2) = if p2.1 < p1.1 {
                    (p2.1, p1.1)
                } else {
                    (p1.1, p2.1)
                };
                Segment {
                    p1,
                    p2,
                    bounds: RectF {
                        x: x1,
                        y: y1,
                        width: x2 - x1,
                        height: y2 - y1,
                    },
                }
            })
            .collect()
    }

    /// `QPainterPath::contains(QPointF)`
    pub fn contains_point(&self, pt: (f64, f64)) -> bool {
        let Some(rect) = self.control_point_rect() else {
            return false;
        };
        if !rect_contains(rect, pt) {
            return false;
        }
        let mut winding = 0;
        let mut last_pt = (0.0, 0.0);
        let mut last_start = (0.0, 0.0);
        let mut i = 0;
        while i < self.elements.len() {
            let e = self.elements[i];
            match e.kind {
                ElementType::MoveTo => {
                    if i > 0 {
                        // Implicitly close all paths.
                        isect_line(last_pt, last_start, pt, &mut winding);
                    }
                    last_start = e.point();
                    last_pt = e.point();
                }
                ElementType::LineTo => {
                    isect_line(last_pt, e.point(), pt, &mut winding);
                    last_pt = e.point();
                }
                ElementType::CurveTo => {
                    let ep = self.point_at(i + 2);
                    let bezier = Bezier::from_points(last_pt, e.point(), self.point_at(i + 1), ep);
                    isect_curve(&bezier, pt, &mut winding, 0);
                    last_pt = ep;
                    i += 2;
                }
                ElementType::CurveToData => {}
            }
            i += 1;
        }
        // Implicitly close last subpath.
        if !fuzzy_point_eq(last_pt, last_start) {
            isect_line(last_pt, last_start, pt, &mut winding);
        }
        if self.winding_fill {
            winding != 0
        } else {
            winding % 2 != 0
        }
    }

    /// `qt_painterpath_check_crossing()`: whether any line or curve crosses
    /// the edges of `rect`.
    fn check_crossing(&self, rect: RectF) -> bool {
        #[derive(PartialEq)]
        #[allow(clippy::enum_variant_names)] // Upstream names.
        enum EdgeStatus {
            OnRect,
            InsideRect,
            OutsideRect,
        }
        let mut last_pt = (0.0, 0.0);
        let mut last_start = (0.0, 0.0);
        let mut edge_status = EdgeStatus::OnRect;
        let mut i = 0;
        while i < self.elements.len() {
            let e = self.elements[i];
            match e.kind {
                ElementType::MoveTo => {
                    if i > 0
                        && fuzzy_compare(last_pt.0, last_start.0)
                        && fuzzy_compare(last_pt.1, last_start.1)
                        && isect_line_rect(last_pt.0, last_pt.1, last_start.0, last_start.1, rect)
                    {
                        return true;
                    }
                    last_start = e.point();
                    last_pt = e.point();
                }
                ElementType::LineTo => {
                    if isect_line_rect(last_pt.0, last_pt.1, e.x, e.y, rect) {
                        return true;
                    }
                    last_pt = e.point();
                }
                ElementType::CurveTo => {
                    let cp2 = self.point_at(i + 1);
                    let ep = self.point_at(i + 2);
                    i += 2;
                    let bezier = Bezier::from_points(last_pt, e.point(), cp2, ep);
                    if isect_curve_horizontal(&bezier, rect.top(), rect.left(), rect.right(), 0)
                        || isect_curve_horizontal(
                            &bezier,
                            rect.bottom(),
                            rect.left(),
                            rect.right(),
                            0,
                        )
                        || isect_curve_vertical(&bezier, rect.left(), rect.top(), rect.bottom(), 0)
                        || isect_curve_vertical(&bezier, rect.right(), rect.top(), rect.bottom(), 0)
                    {
                        return true;
                    }
                    last_pt = ep;
                }
                ElementType::CurveToData => {}
            }
            // Handle crossing the edges of the rect at the end-points of
            // individual sub-paths. A point on the edge itself is considered
            // neither inside nor outside for this purpose.
            if !point_on_edge(rect, last_pt) {
                let contained = rect_contains(rect, last_pt);
                match edge_status {
                    EdgeStatus::OutsideRect if contained => return true,
                    EdgeStatus::InsideRect if !contained => return true,
                    EdgeStatus::OnRect => {
                        edge_status = if contained {
                            EdgeStatus::InsideRect
                        } else {
                            EdgeStatus::OutsideRect
                        };
                    }
                    _ => {}
                }
            } else if fuzzy_point_eq(last_pt, last_start) {
                edge_status = EdgeStatus::OnRect;
            }
            i += 1;
        }
        // Implicitly close last subpath.
        !fuzzy_point_eq(last_pt, last_start)
            && isect_line_rect(last_pt.0, last_pt.1, last_start.0, last_start.1, rect)
    }

    /// `QPainterPath::intersects(QRectF)`
    pub fn intersects_rect(&self, rect: RectF) -> bool {
        if self.element_count() == 1 && rect_contains(rect, self.point_at(0)) {
            return true;
        }
        let Some(cp) = self.control_point_rect() else {
            return false;
        };
        if rect.left().max(cp.left()) > rect.right().min(cp.right())
            || rect.top().max(cp.top()) > rect.bottom().min(cp.bottom())
        {
            return false;
        }
        // If any path element crosses the rect it's bound to be an
        // intersection.
        if self.check_crossing(rect) {
            return true;
        }
        let center = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        if self.contains_point(center) {
            return true;
        }
        // Check if the rectangle surrounds any subpath.
        self.subpath_starts().any(|p| rect_contains(rect, p))
    }

    /// `QPainterPath::contains(QRectF)`
    pub fn contains_rect(&self, rect: RectF) -> bool {
        // The path is empty or the control point rect doesn't enclose the
        // rectangle: we won't contain it.
        let Some(cp) = self.control_point_rect() else {
            return false;
        };
        if !rect_contains_rect(cp, rect) {
            return false;
        }
        let corners = [
            (rect.left(), rect.top()),
            (rect.right(), rect.top()),
            (rect.right(), rect.bottom()),
            (rect.left(), rect.bottom()),
        ];
        // If there are intersections, chances are that the rect is not
        // contained, except if we have winding rule, in which case it still
        // might.
        if self.check_crossing(rect) {
            if !self.winding_fill {
                return false;
            }
            // Do some vague sampling in the winding case. This is not precise
            // but it should mostly be good enough.
            if corners.iter().any(|c| !self.contains_point(*c)) {
                return false;
            }
        }
        // If there exists a point inside that is not part of this path it's
        // not contained.
        let center = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        if !self.contains_point(center) {
            return false;
        }
        // If there are any subpaths inside this rectangle we need to check
        // if they are holes or not.
        let mut i = 0;
        while i < self.elements.len() {
            let e = self.elements[i];
            if e.kind == ElementType::MoveTo && rect_contains(rect, e.point()) {
                if !self.winding_fill {
                    return false;
                }
                let mut stop = false;
                while !stop && i < self.elements.len() {
                    let el = self.elements[i];
                    match el.kind {
                        ElementType::MoveTo => stop = true,
                        ElementType::LineTo => {
                            if !self.contains_point(el.point()) {
                                return false;
                            }
                        }
                        ElementType::CurveTo => {
                            if !self.contains_point(self.point_at(i + 2)) {
                                return false;
                            }
                            i += 2;
                        }
                        ElementType::CurveToData => {}
                    }
                    i += 1;
                }
                // Compensate for the last increment in the inner loop.
                i -= 1;
            }
            i += 1;
        }
        true
    }

    /// `QPainterPath::intersects(QPainterPath)`: whether the filled areas
    /// intersect (touching counts).
    pub fn intersects(&self, other: &Self) -> bool {
        if other.element_count() == 1 {
            return self.contains_point(other.point_at(0));
        }
        let (Some(r1), Some(r2)) = (self.control_point_rect(), other.control_point_rect()) else {
            return false;
        };
        // QPathClipper::intersect()
        if rects_disjoint(r1, r2) {
            return false;
        }
        match (self.is_rect(), other.is_rect()) {
            (true, true) => return true,
            (true, false) => return other.intersects_rect(r1),
            (false, true) => return self.intersects_rect(r2),
            (false, false) => {}
        }
        if has_intersections(&self.segments(), &other.segments()) {
            return true;
        }
        other
            .subpath_starts()
            .any(|p| rect_contains(r1, p) && self.contains_point(p))
            || self
                .subpath_starts()
                .any(|p| rect_contains(r2, p) && other.contains_point(p))
    }

    /// `QPainterPath::contains(QPainterPath)`: whether the filled area of
    /// `other` lies within this one (touching boundaries don't count).
    pub fn contains_path(&self, other: &Self) -> bool {
        if other.element_count() == 1 {
            return self.contains_point(other.point_at(0));
        }
        let (Some(r1), Some(r2)) = (self.control_point_rect(), other.control_point_rect()) else {
            return false;
        };
        // QPathClipper::contains()
        if rects_disjoint(r1, r2) {
            return false;
        }
        if other.is_rect() {
            return self.contains_rect(r2);
        }
        if has_intersections(&self.segments(), &other.segments()) {
            return false;
        }
        other
            .subpath_starts()
            .all(|p| rect_contains(r1, p) && self.contains_point(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn polygon(points: &[(f64, f64)]) -> PainterPathPx {
        PainterPathPx::from_polygons(&[points.to_vec()])
    }

    fn rect(x1: f64, y1: f64, x2: f64, y2: f64) -> PainterPathPx {
        polygon(&[(x1, y1), (x2, y1), (x2, y2), (x1, y2), (x1, y1)])
    }

    #[test]
    fn rects() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        assert!(a.is_rect());
        assert!(a.intersects(&rect(10.0, 0.0, 20.0, 10.0))); // Touching.
        assert!(!a.intersects(&rect(10.1, 0.0, 20.0, 10.0)));
        assert!(a.contains_point((0.0, 5.0))); // Left edge.
        assert!(!a.contains_point((10.0, 5.0))); // Right edge.
    }

    #[test]
    fn polygons() {
        let triangle = polygon(&[(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)]);
        assert!(!triangle.is_rect());
        assert!(triangle.intersects(&polygon(&[(4.0, 4.0), (9.0, 4.0), (4.0, 9.0)])));
        assert!(!triangle.intersects(&polygon(&[(6.0, 6.0), (9.0, 6.0), (6.0, 9.0)])));
        // Contained, touching, overlapping.
        assert!(triangle.contains_path(&polygon(&[(1.0, 1.0), (2.0, 1.0), (1.0, 2.0)])));
        assert!(!triangle.contains_path(&polygon(&[(0.0, 1.0), (2.0, 1.0), (1.0, 2.0)])));
        assert!(!triangle.contains_path(&polygon(&[(4.0, 4.0), (9.0, 4.0), (4.0, 9.0)])));
        // Rectangle inside the triangle.
        assert!(triangle.intersects(&rect(1.0, 1.0, 2.0, 2.0)));
        assert!(triangle.contains_path(&rect(1.0, 1.0, 2.0, 2.0)));
        assert!(!triangle.contains_path(&rect(1.0, 1.0, 6.0, 6.0)));
    }
}
