//! Port of `QPainterPathStroker::createStroke()` of Qt 6 (`qstroker.cpp`,
//! `qbezier.cpp`, `qline.cpp`, LGPL-3.0/GPL) for the default `QPen` (square
//! caps, bevel joins, no dashes).

use super::{
    Bezier, Element, ElementType, PainterPath, PainterPathPx, fuzzy_compare, fuzzy_point_eq,
};

type PointF = (f64, f64);

/// `QT_PATH_KAPPA` / `KAPPA` of qbezier.cpp.
const KAPPA: f64 = 0.5522847498;

/// `qFuzzyIsNull(double)`
fn fuzzy_is_null(d: f64) -> bool {
    d.abs() <= 0.000000000001
}

/// `QLineF`
#[derive(Debug, Clone, Copy, Default)]
struct LineF {
    p1: PointF,
    p2: PointF,
}

impl LineF {
    fn new(p1: PointF, p2: PointF) -> Self {
        Self { p1, p2 }
    }

    fn dx(&self) -> f64 {
        self.p2.0 - self.p1.0
    }

    fn dy(&self) -> f64 {
        self.p2.1 - self.p1.1
    }

    fn is_null(&self) -> bool {
        fuzzy_point_eq(self.p1, self.p2)
    }

    fn length(&self) -> f64 {
        libm::hypot(self.dx(), self.dy())
    }

    fn normal_vector(&self) -> Self {
        Self::new(self.p1, (self.p1.0 + self.dy(), self.p1.1 - self.dx()))
    }

    fn unit_vector(&self) -> Self {
        let (x, y) = (self.dx(), self.dy());
        let len = libm::hypot(x, y);
        Self::new(self.p1, (self.p1.0 + x / len, self.p1.1 + y / len))
    }

    fn translate(&mut self, dx: f64, dy: f64) {
        self.p1 = (self.p1.0 + dx, self.p1.1 + dy);
        self.p2 = (self.p2.0 + dx, self.p2.1 + dy);
    }

    fn set_length(&mut self, len: f64) {
        let old_length = self.length();
        if old_length > 0.0 {
            self.p2 = (
                self.p1.0 + len * (self.dx() / old_length),
                self.p1.1 + len * (self.dy() / old_length),
            );
        }
    }

    fn angle(&self) -> f64 {
        let theta = libm::atan2(-self.dy(), self.dx()) * (180.0 / std::f64::consts::PI);
        let theta_normalized = if theta < 0.0 { theta + 360.0 } else { theta };
        if fuzzy_compare(theta_normalized, 360.0) {
            0.0
        } else {
            theta_normalized
        }
    }

    fn angle_to(&self, l: &Self) -> f64 {
        if self.is_null() || l.is_null() {
            return 0.0;
        }
        let delta = l.angle() - self.angle();
        let delta_normalized = if delta < 0.0 { delta + 360.0 } else { delta };
        if fuzzy_compare(delta, 360.0) {
            0.0
        } else {
            delta_normalized
        }
    }

    /// `intersects()`: returns whether the segments intersect ("bounded").
    fn bounded_intersection(&self, l: &Self) -> bool {
        let a = (self.p2.0 - self.p1.0, self.p2.1 - self.p1.1);
        let b = (l.p1.0 - l.p2.0, l.p1.1 - l.p2.1);
        let c = (self.p1.0 - l.p1.0, self.p1.1 - l.p1.1);
        let denominator = a.1 * b.0 - a.0 * b.1;
        if denominator == 0.0 || !denominator.is_finite() {
            return false;
        }
        let reciprocal = 1.0 / denominator;
        let na = (b.1 * c.0 - b.0 * c.1) * reciprocal;
        if !(0.0..=1.0).contains(&na) {
            return false;
        }
        let nb = (a.0 * c.1 - a.1 * c.0) * reciprocal;
        (0.0..=1.0).contains(&nb)
    }
}

impl Bezier {
    fn pt1(&self) -> PointF {
        (self.x1, self.y1)
    }

    fn pt2(&self) -> PointF {
        (self.x2, self.y2)
    }

    fn pt3(&self) -> PointF {
        (self.x3, self.y3)
    }

    fn pt4(&self) -> PointF {
        (self.x4, self.y4)
    }

    /// `QBezier::normalVector()`
    fn normal_vector(&self, t: f64) -> PointF {
        let m_t = 1. - t;
        let a = m_t * m_t;
        let b = t * m_t;
        let c = t * t;
        (
            (self.y2 - self.y1) * a + (self.y3 - self.y2) * b + (self.y4 - self.y3) * c,
            -(self.x2 - self.x1) * a - (self.x3 - self.x2) * b - (self.x4 - self.x3) * c,
        )
    }

    /// `QBezier::startTangent()`
    fn start_tangent(&self) -> LineF {
        let mut tangent = LineF::new(self.pt1(), self.pt2());
        if tangent.is_null() {
            tangent = LineF::new(self.pt1(), self.pt3());
        }
        if tangent.is_null() {
            tangent = LineF::new(self.pt1(), self.pt4());
        }
        tangent
    }

    /// `QBezier::shifted()`: the offset curve (at most `max_segments`
    /// curves).
    fn shifted(&self, max_segments: usize, offset: f64, mut threshold: f32) -> Vec<Bezier> {
        let mut result = Vec::new();
        if fuzzy_compare(self.x1, self.x2)
            && fuzzy_compare(self.x1, self.x3)
            && fuzzy_compare(self.x1, self.x4)
            && fuzzy_compare(self.y1, self.y2)
            && fuzzy_compare(self.y1, self.y3)
            && fuzzy_compare(self.y1, self.y4)
        {
            return result;
        }
        let max_segments = max_segments - 1;
        let mut stack: Vec<Bezier>;
        'redo: loop {
            stack = vec![*self];
            result.clear();
            while let Some(b) = stack.last().copied() {
                let stack_segments = stack.len();
                if stack_segments == 10 || result.len() + stack_segments == max_segments {
                    threshold *= 1.5;
                    if threshold > 2.0 {
                        break 'redo;
                    }
                    continue 'redo;
                }
                match shift(&b, offset, f64::from(threshold)) {
                    ShiftResult::Discard => {
                        stack.pop();
                    }
                    ShiftResult::Ok(shifted) => {
                        result.push(shifted);
                        stack.pop();
                    }
                    ShiftResult::Circle if max_segments - result.len() >= 2 => {
                        // Add semi circle.
                        if let Some(circle) = add_circle(&b, offset) {
                            result.extend(circle);
                        }
                        stack.pop();
                    }
                    _ => {
                        let (first, second) = b.split();
                        stack.pop();
                        stack.push(second);
                        stack.push(first);
                    }
                }
            }
            return result;
        }
        // Give up.
        while let Some(b) = stack.pop() {
            match shift(&b, offset, f64::from(threshold)) {
                ShiftResult::Ok(shifted) | ShiftResult::Split(shifted) => result.push(shifted),
                _ => {}
            }
        }
        result
    }
}

enum ShiftResult {
    Ok(Bezier),
    Discard,
    Split(Bezier),
    Circle,
}

/// `good_offset()` of qbezier.cpp.
fn good_offset(b1: &Bezier, b2: &Bezier, offset: f64, threshold: f64) -> bool {
    let o2 = offset * offset;
    let max_dist_line = threshold * offset * offset;
    let max_dist_normal = threshold * offset;
    let divisions = 4;
    let spacing = 1.0 / f64::from(divisions);
    let mut t = spacing;
    for _ in 1..divisions {
        let p1 = b1.point_at(t);
        let p2 = b2.point_at(t);
        let d = (p1.0 - p2.0) * (p1.0 - p2.0) + (p1.1 - p2.1) * (p1.1 - p2.1);
        if (d - o2).abs() > max_dist_line {
            return false;
        }
        let normal_point = b1.normal_vector(t);
        let l = normal_point.0.abs() + normal_point.1.abs();
        if l != 0.0 {
            let d = (normal_point.0 * (p1.1 - p2.1) - normal_point.1 * (p1.0 - p2.0)).abs() / l;
            if d > max_dist_normal {
                return false;
            }
        }
        t += spacing;
    }
    true
}

/// `shift()` of qbezier.cpp.
fn shift(orig: &Bezier, offset: f64, threshold: f64) -> ShiftResult {
    let p1_p2_equal = fuzzy_compare(orig.x1, orig.x2) && fuzzy_compare(orig.y1, orig.y2);
    let p2_p3_equal = fuzzy_compare(orig.x2, orig.x3) && fuzzy_compare(orig.y2, orig.y3);
    let p3_p4_equal = fuzzy_compare(orig.x3, orig.x4) && fuzzy_compare(orig.y3, orig.y4);

    let mut points: Vec<PointF> = vec![orig.pt1()];
    let mut map = [0usize; 4];
    if !p1_p2_equal {
        points.push(orig.pt2());
    }
    map[1] = points.len() - 1;
    if !p2_p3_equal {
        points.push(orig.pt3());
    }
    map[2] = points.len() - 1;
    if !p3_p4_equal {
        points.push(orig.pt4());
    }
    map[3] = points.len() - 1;
    let np = points.len();
    if np == 1 {
        return ShiftResult::Discard;
    }

    let b = orig.control_point_bounds();
    if np == 4 && b.width < 0.1 * offset && b.height < 0.1 * offset {
        let l = (orig.x1 - orig.x2) * (orig.x1 - orig.x2)
            + (orig.y1 - orig.y2) * (orig.y1 - orig.y2) * (orig.x3 - orig.x4) * (orig.x3 - orig.x4)
            + (orig.y3 - orig.y4) * (orig.y3 - orig.y4);
        let dot =
            (orig.x1 - orig.x2) * (orig.x3 - orig.x4) + (orig.y1 - orig.y2) * (orig.y3 - orig.y4);
        if dot < 0.0 && dot * dot < 0.8 * l {
            // The points are close and reverse direction. Approximate the
            // whole thing by a semi circle.
            return ShiftResult::Circle;
        }
    }

    let unit_normal = |d: PointF| {
        let line = LineF::new((0.0, 0.0), d);
        line.normal_vector().unit_vector().p2
    };
    let first = (points[1].0 - points[0].0, points[1].1 - points[0].1);
    if LineF::new((0.0, 0.0), first).length() == 0.0 {
        return ShiftResult::Discard;
    }
    let mut prev_normal = unit_normal(first);
    let mut shifted = [(0.0, 0.0); 4];
    shifted[0] = (
        points[0].0 + offset * prev_normal.0,
        points[0].1 + offset * prev_normal.1,
    );
    for i in 1..np - 1 {
        let next = (points[i + 1].0 - points[i].0, points[i + 1].1 - points[i].1);
        let next_normal = unit_normal(next);
        let normal_sum = (prev_normal.0 + next_normal.0, prev_normal.1 + next_normal.1);
        let r = 1.0 + prev_normal.0 * next_normal.0 + prev_normal.1 * next_normal.1;
        shifted[i] = if fuzzy_is_null(r) {
            (
                points[i].0 + offset * prev_normal.0,
                points[i].1 + offset * prev_normal.1,
            )
        } else {
            let k = offset / r;
            (
                points[i].0 + k * normal_sum.0,
                points[i].1 + k * normal_sum.1,
            )
        };
        prev_normal = next_normal;
    }
    shifted[np - 1] = (
        points[np - 1].0 + offset * prev_normal.0,
        points[np - 1].1 + offset * prev_normal.1,
    );
    let result = Bezier::from_points(
        shifted[map[0]],
        shifted[map[1]],
        shifted[map[2]],
        shifted[map[3]],
    );
    if np > 2 && !good_offset(orig, &result, offset, threshold) {
        return ShiftResult::Split(result);
    }
    ShiftResult::Ok(result)
}

/// `addCircle()` of qbezier.cpp.
fn add_circle(b: &Bezier, offset: f64) -> Option<[Bezier; 2]> {
    let mut normals = [(0.0, 0.0); 3];
    normals[0] = (b.y2 - b.y1, b.x1 - b.x2);
    let dist = (normals[0].0 * normals[0].0 + normals[0].1 * normals[0].1).sqrt();
    if fuzzy_is_null(dist) {
        return None;
    }
    normals[0] = (normals[0].0 / dist, normals[0].1 / dist);
    normals[2] = (b.y4 - b.y3, b.x3 - b.x4);
    let dist = (normals[2].0 * normals[2].0 + normals[2].1 * normals[2].1).sqrt();
    if fuzzy_is_null(dist) {
        return None;
    }
    normals[2] = (normals[2].0 / dist, normals[2].1 / dist);
    normals[1] = (b.x1 - b.x2 - b.x3 + b.x4, b.y1 - b.y2 - b.y3 + b.y4);
    let len = -(normals[1].0 * normals[1].0 + normals[1].1 * normals[1].1).sqrt();
    normals[1] = (normals[1].0 / len, normals[1].1 / len);

    let mut angles = [0.0; 2];
    let mut sign = 1.0;
    for i in 0..2 {
        let cos_a =
            (normals[i].0 * normals[i + 1].0 + normals[i].1 * normals[i + 1].1).clamp(-1.0, 1.0);
        angles[i] = libm::acos(cos_a) * std::f64::consts::FRAC_1_PI;
    }
    if angles[0] + angles[1] > 1.0 {
        // More than 180 degrees.
        normals[1] = (-normals[1].0, -normals[1].1);
        angles[0] = 1.0 - angles[0];
        angles[1] = 1.0 - angles[1];
        sign = -1.0;
    }

    let circle = [
        (b.x1 + normals[0].0 * offset, b.y1 + normals[0].1 * offset),
        (
            0.5 * (b.x1 + b.x4) + normals[1].0 * offset,
            0.5 * (b.y1 + b.y4) + normals[1].1 * offset,
        ),
        (b.x4 + normals[2].0 * offset, b.y4 + normals[2].1 * offset),
    ];
    let segment = |i: usize| {
        let kappa = 2.0 * KAPPA * sign * offset * angles[i];
        Bezier::from_points(
            circle[i],
            (
                circle[i].0 - normals[i].1 * kappa,
                circle[i].1 + normals[i].0 * kappa,
            ),
            (
                circle[i + 1].0 + normals[i + 1].1 * kappa,
                circle[i + 1].1 - normals[i + 1].0 * kappa,
            ),
            circle[i + 1],
        )
    };
    Some([segment(0), segment(1)])
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum JoinMode {
    /// Bevel join (`Qt::BevelJoin`).
    Flat,
    /// Square cap (`Qt::SquareCap`).
    Square,
}

/// `QStroker` with its output `QPainterPath`.
struct Stroker {
    stroke_width: f64,
    curve_threshold: f64,
    back1: PointF,
    back2: PointF,
    out: PainterPath,
    touched: bool,
}

impl Stroker {
    fn emit_move_to(&mut self, p: PointF) {
        self.back2 = self.back1;
        self.back1 = p;
        self.out.move_to(p);
        self.touched = true;
    }

    fn emit_line_to(&mut self, p: PointF) {
        self.back2 = self.back1;
        self.back1 = p;
        self.out.line_to(p);
        self.touched = true;
    }

    fn emit_cubic_to(&mut self, c1: PointF, c2: PointF, e: PointF) {
        if c2 == e {
            if c1 == e {
                self.back2 = self.back1;
            } else {
                self.back2 = c1;
            }
        } else {
            self.back2 = c2;
        }
        self.back1 = e;
        self.out.cubic_to(c1, c2, e);
        self.touched = true;
    }

    /// `QStroker::joinPoints()` (bevel joins and square caps only).
    fn join_points(&mut self, focal: PointF, next_line: LineF, join: JoinMode) {
        // Points connected already, don't join.
        if fuzzy_compare(self.back1.0, next_line.p1.0)
            && fuzzy_compare(self.back1.1, next_line.p1.1)
        {
            return;
        }
        let prev_line = LineF::new(self.back2, self.back1);
        match join {
            JoinMode::Flat => {
                let short_cut = LineF::new(prev_line.p2, next_line.p1);
                let angle = short_cut.angle_to(&prev_line);
                if prev_line.bounded_intersection(&next_line)
                    || (angle > 90.0 && !fuzzy_compare(angle, 90.0))
                {
                    self.emit_line_to(focal);
                    self.emit_line_to(next_line.p1);
                    return;
                }
                self.emit_line_to(next_line.p1);
            }
            JoinMode::Square => {
                let offset = self.stroke_width / 2.0;
                let mut l1 = prev_line;
                let dp = prev_line.dx() * next_line.dx() + prev_line.dy() * next_line.dy();
                if dp > 0.0 {
                    // Same direction, means that prevLine is from a bezier
                    // that has been "reversed" by shifting.
                    l1 = LineF::new(prev_line.p2, prev_line.p1);
                } else {
                    l1.translate(l1.dx(), l1.dy());
                }
                l1.set_length(offset);
                let mut l2 = LineF::new(next_line.p2, next_line.p1);
                l2.translate(l2.dx(), l2.dy());
                l2.set_length(offset);
                self.emit_line_to(l1.p2);
                self.emit_line_to(l2.p2);
                self.emit_line_to(l2.p1);
            }
        }
    }

    /// `qt_stroke_side()`: strokes one side of the subpath `elements`,
    /// returns whether the subpath is closed.
    fn stroke_side(
        &mut self,
        elements: &[Element],
        cap_first: bool,
        start_tangent: &mut LineF,
    ) -> bool {
        const MAX_OFFSET: usize = 16;
        let start = elements[0].point();
        let mut prev = start;
        let mut first = true;
        let offset = self.stroke_width / 2.0;
        let mut i = 1;
        while i < elements.len() {
            let e = elements[i];
            match e.kind {
                ElementType::LineTo => {
                    let mut line = LineF::new(prev, e.point());
                    if !fuzzy_point_eq(line.p1, line.p2) {
                        let mut normal = line.normal_vector();
                        normal.set_length(offset);
                        line.translate(normal.dx(), normal.dy());
                        // If we are starting a new subpath, move to correct
                        // starting point.
                        if first {
                            if cap_first {
                                self.join_points(prev, line, JoinMode::Square);
                            } else {
                                self.emit_move_to(line.p1);
                            }
                            *start_tangent = line;
                            first = false;
                        } else {
                            self.join_points(prev, line, JoinMode::Flat);
                        }
                        // Add the stroke for this line.
                        self.emit_line_to(line.p2);
                        prev = e.point();
                    }
                    i += 1;
                }
                ElementType::CurveTo => {
                    let cp2 = elements.get(i + 1).map_or((0.0, 0.0), Element::point);
                    let ep = elements.get(i + 2).map_or((0.0, 0.0), Element::point);
                    i += 3;
                    let bezier = Bezier::from_points(prev, e.point(), cp2, ep);
                    let offset_curves =
                        bezier.shifted(MAX_OFFSET, offset, self.curve_threshold as f32);
                    if let Some(first_curve) = offset_curves.first() {
                        // If we are starting a new subpath, move to correct
                        // starting point.
                        let mut tangent = bezier.start_tangent();
                        tangent.translate(first_curve.x1 - bezier.x1, first_curve.y1 - bezier.y1);
                        if first {
                            if cap_first {
                                self.join_points(prev, tangent, JoinMode::Square);
                            } else {
                                self.emit_move_to(first_curve.pt1());
                            }
                            *start_tangent = tangent;
                            first = false;
                        } else {
                            self.join_points(prev, tangent, JoinMode::Flat);
                        }
                        // Add these beziers.
                        for curve in &offset_curves {
                            self.emit_cubic_to(curve.pt2(), curve.pt3(), curve.pt4());
                        }
                    }
                    prev = ep;
                }
                _ => i += 1,
            }
        }
        if fuzzy_compare(start.0, prev.0) && fuzzy_compare(start.1, prev.1) {
            // Closed subpath, join first and last point (don't join empty
            // subpaths).
            if !first {
                self.join_points(prev, *start_tangent, JoinMode::Flat);
            }
            true
        } else {
            false
        }
    }

    /// `QStroker::processCurrentSubpath()`
    fn process_subpath(&mut self, elements: &[Element]) {
        let backward = reversed(elements);
        let mut fw_start_tangent = LineF::default();
        let mut bw_start_tangent = LineF::default();
        let fw_closed = self.stroke_side(elements, false, &mut fw_start_tangent);
        let bw_closed = self.stroke_side(&backward, !fw_closed, &mut bw_start_tangent);
        if !bw_closed && !fw_start_tangent.is_null() {
            self.join_points(elements[0].point(), fw_start_tangent, JoinMode::Square);
        }
    }
}

/// `QSubpathBackwardIterator`: the subpath in reverse order.
fn reversed(elements: &[Element]) -> Vec<Element> {
    let n = elements.len();
    let mut result = Vec::with_capacity(n);
    for pos in (0..n).rev() {
        let mut ce = elements[pos];
        if pos == n - 1 {
            ce.kind = ElementType::MoveTo;
        } else {
            let pe = elements[pos + 1];
            ce.kind = match pe.kind {
                ElementType::LineTo => ElementType::LineTo,
                ElementType::CurveToData => {
                    if ce.kind == ElementType::CurveTo {
                        ElementType::CurveToData
                    } else {
                        ElementType::CurveTo
                    }
                }
                ElementType::CurveTo => ElementType::CurveToData,
                ElementType::MoveTo => ce.kind,
            };
        }
        result.push(ce);
    }
    result
}

impl PainterPathPx {
    /// Returns the outline of the stroked path with the given width (in
    /// pixels), like `QPainterPathStroker::createStroke()` configured by
    /// upstream `Toolbox::shapeFromPath()` for a default `QPen` (square
    /// caps, bevel joins), with winding fill.
    pub fn stroked(&self, width: f64) -> Self {
        let width = if width <= 0.0 { 1.0 } else { width };
        let mut stroker = Stroker {
            stroke_width: width,
            curve_threshold: 0.25,
            back1: (0.0, 0.0),
            back2: (0.0, 0.0),
            out: PainterPath {
                elements: vec![Element {
                    x: 0.0,
                    y: 0.0,
                    kind: ElementType::MoveTo,
                }],
            },
            touched: false,
        };
        // QStrokerOps::strokePath(): collect subpaths, process them on the
        // next move-to and at the end.
        let mut current: Vec<Element> = Vec::new();
        for e in &self.elements {
            if e.kind == ElementType::MoveTo {
                if current.len() > 1 {
                    stroker.process_subpath(&current);
                }
                current.clear();
            }
            current.push(*e);
        }
        if current.len() > 1 {
            stroker.process_subpath(&current);
        }
        let elements = if stroker.touched && stroker.out.elements.len() > 1 {
            stroker.out.elements
        } else {
            Vec::new()
        };
        Self {
            elements,
            winding_fill: true,
        }
    }
}
