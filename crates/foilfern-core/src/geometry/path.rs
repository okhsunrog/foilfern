//! Port of libs/librepcb/core/geometry/path.{h,cpp} (and
//! libs/librepcb/rust-core/src/types/vertex_vec.rs).
//!
//! Differences to upstream:
//! - Overloaded static constructors got distinct names:
//!   `obround(p1, p2, width)` is [`Path::obround_line()`].
//! - `toQPainterPathPx()` is not ported (UI specific). The bounding
//!   rectangle computation which the stroke font needs is emulated in
//!   [`utils::painter_path`](crate::utils::painter_path).

use std::ops::Deref;

use super::{Error, Vertex};
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::{Angle, Length, Orientation, Point, PositiveLength, UnsignedLength};
use crate::utils::toolbox;

/// A list of vertices connected by straight lines or circular arcs.
///
/// A path is closed if it has at least two vertices and the first and last
/// position are equal. Unlike polygons, paths have no width or layer.
///
/// The ordering compares the vertices lexicographically (e.g. for canonical
/// order in files).
///
/// Serde: an array of vertices.
#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(transparent)]
pub struct Path {
    vertices: Vec<Vertex>,
}

impl Path {
    /// Creates a path from vertices.
    pub fn new(vertices: Vec<Vertex>) -> Self {
        Self { vertices }
    }

    /// Returns whether the path is closed (at least two vertices, first and
    /// last position equal).
    pub fn is_closed(&self) -> bool {
        match (self.vertices.first(), self.vertices.last()) {
            (Some(first), Some(last)) if self.vertices.len() >= 2 => first.pos == last.pos,
            _ => false,
        }
    }

    /// Returns whether any segment is an arc (the angle of the last vertex
    /// is not relevant).
    pub fn is_curved(&self) -> bool {
        let n = self.vertices.len().saturating_sub(1);
        self.vertices[..n].iter().any(|v| v.angle != Angle::DEG0)
    }

    /// Returns whether all vertices are at the same position (or there are
    /// less than two vertices).
    pub fn is_zero_length(&self) -> bool {
        match self.vertices.split_first() {
            Some((first, rest)) if !rest.is_empty() => rest.iter().all(|v| v.pos == first.pos),
            _ => true,
        }
    }

    /// Returns whether the path does not enclose any area.
    pub fn is_zero_area(&self) -> bool {
        if !self.is_closed() || self.is_zero_length() {
            return true;
        }
        if self.is_curved() {
            self.vertices.len() < 3
        } else {
            self.vertices.len() < 4
        }
    }

    /// Returns whether all vertices are on the given grid.
    pub fn is_on_grid(&self, grid_interval: PositiveLength) -> bool {
        self.vertices
            .iter()
            .all(|v| v.pos.is_on_grid(grid_interval))
    }

    /// Returns the vertices.
    pub fn vertices(&self) -> &[Vertex] {
        &self.vertices
    }

    /// Returns the vertices for modification.
    pub fn vertices_mut(&mut self) -> &mut Vec<Vertex> {
        &mut self.vertices
    }

    /// Consumes the path and returns its vertices.
    pub fn into_vertices(self) -> Vec<Vertex> {
        self.vertices
    }

    /// Returns the total length of all segments, treating arcs as straight
    /// lines.
    pub fn total_straight_length(&self) -> UnsignedLength {
        self.vertices
            .windows(2)
            .fold(UnsignedLength::ZERO, |sum, w| {
                sum + (w[1].pos - w[0].pos).length()
            })
    }

    /// Returns the area in mm² enclosed by the path, treating arcs as
    /// straight lines (the path is implicitly closed).
    pub fn calc_area_of_straight_segments(&self) -> f64 {
        // https://en.wikipedia.org/wiki/Shoelace_formula
        let n = if self.is_closed() {
            self.vertices.len() - 1
        } else {
            self.vertices.len()
        };
        let mut area = 0.0;
        let mut j = n.wrapping_sub(1);
        for i in 0..n {
            let (pjx, pjy) = self.vertices[j].pos.to_mm();
            let (pix, piy) = self.vertices[i].pos.to_mm();
            area += (pjx + pix) * (pjy - piy);
            j = i;
        }
        (area / 2.0).abs()
    }

    /// Returns the point on the path (arcs treated as straight lines) which
    /// is nearest to `p`, or the origin if the path is empty.
    pub fn calc_nearest_point_between_vertices(&self, p: Point) -> Point {
        let Some(first) = self.vertices.first() else {
            return Point::ORIGIN;
        };
        let mut nearest = first.pos;
        for w in self.vertices.windows(2) {
            let tmp = toolbox::nearest_point_on_line(p, w[0].pos, w[1].pos);
            if (tmp - p).length() < (nearest - p).length() {
                nearest = tmp;
            }
        }
        nearest
    }

    /// Returns the outlines of the path stroked with the given width: one
    /// (arc) obround per segment, or a circle for a single vertex.
    pub fn to_outline_strokes(&self, width: PositiveLength) -> Vec<Path> {
        if let [single] = self.vertices.as_slice() {
            return vec![Self::circle(width).translated(single.pos)];
        }
        self.vertices
            .windows(2)
            .map(|w| {
                if w[0].angle == Angle::DEG0 {
                    Self::obround_line(w[0].pos, w[1].pos, width)
                } else {
                    Self::arc_obround(w[0].pos, w[1].pos, w[0].angle, width)
                }
            })
            .collect()
    }

    /// Returns the path as SVG path data in millimeters (Y axis inverted),
    /// e.g. `"M 1 -1.234567 L 0 0"`.
    pub fn to_svg_path_mm(&self) -> String {
        fn format_length(value: Length) -> String {
            let s = value.to_mm_string();
            match s.strip_suffix(".0") {
                Some(stripped) => stripped.to_owned(),
                None => s,
            }
        }

        let mut s = String::new();
        if let Some(first) = self.vertices.first() {
            s.push_str(&format!(
                "M {} {} ",
                format_length(first.pos.x),
                format_length(-first.pos.y)
            ));
        }
        for w in self.vertices.windows(2) {
            let (v0, v1) = (w[0], w[1]);
            if let Some(radius) = toolbox::arc_radius(v0.pos, v1.pos, v0.angle) {
                s.push_str(&format!(
                    "A {r} {r} 0 {} {} {} {} ",
                    u8::from(v0.angle.abs() >= Angle::DEG180),
                    u8::from(v0.angle < Angle::DEG0),
                    format_length(v1.pos.x),
                    format_length(-v1.pos.y),
                    r = format_length(radius.abs()),
                ));
            } else {
                s.push_str(&format!(
                    "L {} {} ",
                    format_length(v1.pos.x),
                    format_length(-v1.pos.y)
                ));
            }
        }
        s.pop();
        s
    }

    /// Returns a copy with all positions modified by `f`.
    fn map_positions(&self, f: impl Fn(Point) -> Point) -> Self {
        self.vertices
            .iter()
            .map(|v| Vertex::new(f(v.pos), v.angle))
            .collect()
    }

    /// Returns a copy translated by `offset`.
    pub fn translated(&self, offset: Point) -> Self {
        self.map_positions(|p| p + offset)
    }

    /// Returns a copy with all vertices mapped to the grid.
    pub fn mapped_to_grid(&self, grid_interval: PositiveLength) -> Self {
        self.map_positions(|p| p.mapped_to_grid(grid_interval))
    }

    /// Returns a copy rotated by `angle` around `center`.
    pub fn rotated(&self, angle: Angle, center: Point) -> Self {
        self.map_positions(|p| p.rotated(angle, center))
    }

    /// Returns a copy mirrored around `center` (arc angles are inverted).
    pub fn mirrored(&self, orientation: Orientation, center: Point) -> Self {
        self.vertices
            .iter()
            .map(|v| Vertex::new(v.pos.mirrored(orientation, center), -v.angle))
            .collect()
    }

    /// Returns a copy with reversed vertex order (arc angles are moved to the
    /// right vertex and inverted).
    pub fn reversed(&self) -> Self {
        (0..self.vertices.len())
            .rev()
            .map(|i| {
                let angle = i
                    .checked_sub(1)
                    .map_or(Angle::DEG0, |prev| self.vertices[prev].angle);
                Vertex::new(self.vertices[i].pos, -angle)
            })
            .collect()
    }

    /// Returns a copy with all arcs replaced by straight line segments with
    /// the given maximum deviation, see [`flat_arc()`](Self::flat_arc).
    pub fn flattened_arcs(&self, max_tolerance: PositiveLength) -> Self {
        let mut vertices = self.vertices.clone();
        if let Some(last) = vertices.last_mut() {
            last.angle = Angle::DEG0;
        }
        for i in (0..vertices.len().saturating_sub(1)).rev() {
            let (v0, v1) = (vertices[i], vertices[i + 1]);
            if v0.angle != Angle::DEG0 {
                let arc = Self::flat_arc(v0.pos, v1.pos, v0.angle, max_tolerance);
                vertices.splice(i..i + 2, arc.vertices);
            }
        }
        Self::new(vertices)
    }

    /// Appends a vertex.
    pub fn add_vertex(&mut self, pos: Point, angle: Angle) {
        self.vertices.push(Vertex::new(pos, angle));
    }

    /// Inserts a vertex at `index` (clamped to the vertex count).
    pub fn insert_vertex(&mut self, index: usize, pos: Point, angle: Angle) {
        let index = index.min(self.vertices.len());
        self.vertices.insert(index, Vertex::new(pos, angle));
    }

    /// Removes consecutive vertices at the same position (keeping the later
    /// one, i.e. its angle). Returns whether the path was modified.
    pub fn clean(&mut self) -> bool {
        let mut modified = false;
        for i in (1..self.vertices.len()).rev() {
            if self.vertices[i - 1].pos == self.vertices[i].pos {
                self.vertices.remove(i - 1);
                modified = true;
            }
        }
        modified
    }

    /// Closes the path by appending the first position if needed. Returns
    /// whether the path was modified.
    pub fn close(&mut self) -> bool {
        if !self.is_closed() && (self.vertices.len() > 1) {
            let first = self.vertices[0].pos;
            self.add_vertex(first, Angle::DEG0);
            true
        } else {
            false
        }
    }

    /// Opens a closed path (with more than two vertices) by removing the
    /// last vertex. Returns whether the path was modified.
    pub fn open(&mut self) -> bool {
        if (self.vertices.len() > 2) && self.is_closed() {
            self.vertices.pop();
            true
        } else {
            false
        }
    }

    /// Creates a line from `p1` to `p2` with the given arc angle.
    pub fn line(p1: Point, p2: Point, angle: Angle) -> Self {
        Self::new(vec![Vertex::new(p1, angle), Vertex::at(p2)])
    }

    /// Creates a circle around the origin.
    pub fn circle(diameter: PositiveLength) -> Self {
        Self::obround(diameter, diameter)
    }

    /// Creates a ring (outer circle with a cut-in inner circle) around the
    /// origin, or an empty path if the inner diameter is not smaller than
    /// the outer diameter.
    pub fn donut(outer_diameter: PositiveLength, inner_diameter: PositiveLength) -> Self {
        let mut p = Self::default();
        let ro = outer_diameter / 2;
        let ri = inner_diameter / 2;
        if ro > ri {
            let z = Length::ZERO;
            p.add_vertex(Point::new(z, ro), -Angle::DEG180);
            p.add_vertex(Point::new(z, -ro), Angle::DEG0);
            p.add_vertex(Point::new(z, -ri), Angle::DEG180);
            p.add_vertex(Point::new(z, ri), Angle::DEG180);
            p.add_vertex(Point::new(z, -ri), Angle::DEG0);
            p.add_vertex(Point::new(z, -ro), -Angle::DEG180);
            p.add_vertex(Point::new(z, ro), Angle::DEG0);
        }
        p
    }

    /// Creates an obround (or circle) centered at the origin.
    pub fn obround(width: PositiveLength, height: PositiveLength) -> Self {
        let mut p = Self::default();
        let rx = width / 2;
        let ry = height / 2;
        let z = Length::ZERO;
        if width > height {
            p.add_vertex(Point::new(ry - rx, ry), Angle::DEG0);
            p.add_vertex(Point::new(rx - ry, ry), -Angle::DEG180);
            p.add_vertex(Point::new(rx - ry, -ry), Angle::DEG0);
            p.add_vertex(Point::new(ry - rx, -ry), -Angle::DEG180);
            p.add_vertex(Point::new(ry - rx, ry), Angle::DEG0);
        } else if width < height {
            p.add_vertex(Point::new(rx, ry - rx), Angle::DEG0);
            p.add_vertex(Point::new(rx, rx - ry), -Angle::DEG180);
            p.add_vertex(Point::new(-rx, rx - ry), Angle::DEG0);
            p.add_vertex(Point::new(-rx, ry - rx), -Angle::DEG180);
            p.add_vertex(Point::new(rx, ry - rx), Angle::DEG0);
        } else {
            p.add_vertex(Point::new(rx, z), -Angle::DEG180);
            p.add_vertex(Point::new(-rx, z), -Angle::DEG180);
            p.add_vertex(Point::new(rx, z), Angle::DEG0);
        }
        p
    }

    /// Creates an obround around the line from `p1` to `p2` (upstream
    /// `obround(p1, p2, width)`).
    pub fn obround_line(p1: Point, p2: Point, width: PositiveLength) -> Self {
        let diff = p2 - p1;
        // atan2() is always within [-π..π], i.e. a valid angle.
        let angle =
            Angle::from_rad(libm::atan2(diff.y.to_mm(), diff.x.to_mm())).unwrap_or_default();
        Self::obround(diff.length() + width, width)
            .rotated(angle, Point::ORIGIN)
            .translated((p1 + p2) / 2)
    }

    /// Creates an obround around the arc from `p1` to `p2`.
    pub fn arc_obround(p1: Point, p2: Point, angle: Angle, width: PositiveLength) -> Self {
        if p1 == p2 {
            return Self::circle(width).translated(p1);
        }
        let Some(center) = toolbox::arc_center(p1, p2, angle) else {
            // Seems to be a straight segment.
            return Self::obround_line(p1, p2, width);
        };
        let delta1 = p1 - center;
        let delta2 = p2 - center;
        // atan2() is always within [-π..π], i.e. a valid angle.
        let angle1 =
            Angle::from_rad(libm::atan2(delta1.y.to_px(), delta1.x.to_px())).unwrap_or_default();
        let angle2 =
            Angle::from_rad(libm::atan2(delta2.y.to_px(), delta2.x.to_px())).unwrap_or_default();
        let radius = delta1.length();
        let inner_radius = *radius - (width / 2);
        let outer_radius = *radius + (width / 2);
        let at =
            |r: Length, a: Angle| center + Point::new(r, Length::ZERO).rotated(a, Point::ORIGIN);
        let p1_inner = at(inner_radius, angle1);
        let p1_outer = at(outer_radius, angle1);
        let p2_inner = at(inner_radius, angle2);
        let p2_outer = at(outer_radius, angle2);
        let half_circle = if angle < Angle::DEG0 {
            Angle::DEG180
        } else {
            -Angle::DEG180
        };
        let mut p = Self::default();
        p.add_vertex(p1_inner, angle);
        p.add_vertex(p2_inner, half_circle);
        p.add_vertex(p2_outer, -angle);
        p.add_vertex(p1_outer, half_circle);
        p.add_vertex(p1_inner, Angle::DEG0);
        p
    }

    /// Creates the outline of a line (or arc) with flat caps, or `None` if
    /// the line has zero length.
    pub fn flat_cap_line(
        p1: Point,
        p2: Point,
        angle: Angle,
        width: PositiveLength,
    ) -> Option<Self> {
        // Handle arc.
        if let Some(center) = toolbox::arc_center(p1, p2, angle) {
            let a1 = toolbox::angle_between_points(center, p1);
            let a2 = toolbox::angle_between_points(center, p2);
            let vector = Point::new(width / 2, Length::ZERO);
            let o = Point::ORIGIN;
            let p11 = p1 + vector.rotated(a1, o);
            let p12 = p1 + vector.rotated(a1 + Angle::DEG180, o);
            let p21 = p2 + vector.rotated(a2, o);
            let p22 = p2 + vector.rotated(a2 + Angle::DEG180, o);
            let mut p = Self::default();
            p.add_vertex(p12, angle);
            p.add_vertex(p22, Angle::DEG0);
            p.add_vertex(p21, -angle);
            p.add_vertex(p11, Angle::DEG0);
            p.close();
            p.clean();
            return Some(p);
        }

        // Handle straight line.
        let length = PositiveLength::new(*(p2 - p1).length()).ok()?;
        let center = (p1 + p2) / 2;
        Some(
            Self::centered_rect(length, width, UnsignedLength::ZERO)
                .translated(center)
                .rotated(toolbox::angle_between_points(p1, p2), center),
        )
    }

    /// Creates a closed rectangle with corners `p1` and `p2`.
    pub fn rect(p1: Point, p2: Point) -> Self {
        let mut p = Self::default();
        p.add_vertex(Point::new(p1.x, p1.y), Angle::DEG0);
        p.add_vertex(Point::new(p2.x, p1.y), Angle::DEG0);
        p.add_vertex(Point::new(p2.x, p2.y), Angle::DEG0);
        p.add_vertex(Point::new(p1.x, p2.y), Angle::DEG0);
        p.add_vertex(Point::new(p1.x, p1.y), Angle::DEG0);
        p
    }

    /// Creates a closed rectangle centered at the origin, optionally with
    /// rounded corners (an obround if the radius is too large).
    pub fn centered_rect(
        width: PositiveLength,
        height: PositiveLength,
        corner_radius: UnsignedLength,
    ) -> Self {
        let mut p = Self::default();
        let rx = width / 2;
        let ry = height / 2;
        let r = *corner_radius;
        if corner_radius == UnsignedLength::ZERO {
            // Regular rectangle without rounded corners.
            p.add_vertex(Point::new(-rx, ry), Angle::DEG0);
            p.add_vertex(Point::new(rx, ry), Angle::DEG0);
            p.add_vertex(Point::new(rx, -ry), Angle::DEG0);
            p.add_vertex(Point::new(-rx, -ry), Angle::DEG0);
        } else if r >= rx.min(ry) {
            // Corner radius is too large for the given size, it's actually an
            // obround.
            return Self::obround(width, height);
        } else {
            // Rectangle with rounded corners.
            p.add_vertex(Point::new(-rx + r, ry), Angle::DEG0);
            p.add_vertex(Point::new(rx - r, ry), -Angle::DEG90);
            p.add_vertex(Point::new(rx, ry - r), Angle::DEG0);
            p.add_vertex(Point::new(rx, -ry + r), -Angle::DEG90);
            p.add_vertex(Point::new(rx - r, -ry), Angle::DEG0);
            p.add_vertex(Point::new(-rx + r, -ry), -Angle::DEG90);
            p.add_vertex(Point::new(-rx, -ry + r), Angle::DEG0);
            p.add_vertex(Point::new(-rx, ry - r), -Angle::DEG90);
        }
        p.close();
        p
    }

    /// Creates a closed rectangle centered at the origin with the given
    /// corners chamfered.
    pub fn chamfered_rect(
        width: PositiveLength,
        height: PositiveLength,
        chamfer_size: UnsignedLength,
        top_left: bool,
        top_right: bool,
        bottom_left: bool,
        bottom_right: bool,
    ) -> Self {
        let mut p = Self::default();
        let (w2, h2) = (width / 2, height / 2);
        let c = (*chamfer_size).min(w2).min(h2);
        let cx = w2 - c;
        let cy = h2 - c;
        let mut add = |x: Length, y: Length| p.add_vertex(Point::new(x, y), Angle::DEG0);
        if top_left {
            add(-w2, cy);
            add(-cx, h2);
        } else {
            add(-w2, h2);
        }
        if top_right {
            add(cx, h2);
            add(w2, cy);
        } else {
            add(w2, h2);
        }
        if bottom_right {
            add(w2, -cy);
            add(cx, -h2);
        } else {
            add(w2, -h2);
        }
        if bottom_left {
            add(-cx, -h2);
            add(-w2, -cy);
        } else {
            add(-w2, -h2);
        }
        p.close();
        p.clean();
        p
    }

    /// Creates a closed trapezoid centered at the origin (`dw`/`dh` are the
    /// width/height differences, clipped to the size).
    pub fn trapezoid(
        width: PositiveLength,
        height: PositiveLength,
        dw: Length,
        dh: Length,
    ) -> Self {
        let mut p = Self::default();
        let dw = if dw >= Length::ZERO {
            dw.min(*width)
        } else {
            dw.max(-width)
        };
        let dh = if dh >= Length::ZERO {
            dh.min(*height)
        } else {
            dh.max(-height)
        };
        let xt = (width / 2) + (dw / 2);
        let xb = (width / 2) - (dw / 2);
        let yl = (height / 2) - (dh / 2);
        let yr = (height / 2) + (dh / 2);
        p.add_vertex(Point::new(-xt, yl), Angle::DEG0);
        p.add_vertex(Point::new(xt, yr), Angle::DEG0);
        p.add_vertex(Point::new(xb, -yr), Angle::DEG0);
        p.add_vertex(Point::new(-xb, -yl), Angle::DEG0);
        p.close();
        p.clean();
        p
    }

    /// Creates a closed octagon centered at the origin, optionally with
    /// rounded corners (an obround if the radius is too large).
    pub fn octagon(
        width: PositiveLength,
        height: PositiveLength,
        corner_radius: UnsignedLength,
    ) -> Self {
        let mut p = Self::default();
        let rx = width / 2;
        let ry = height / 2;
        let r = *corner_radius;
        // Lengths computed from lengths in range are always in range.
        let from_mm = |mm: f64| Length::from_mm(mm).unwrap_or_default();
        let inner_chamfer = from_mm((rx - r).min(ry - r).to_mm() * (2.0 - 2f64.sqrt())) + r;
        let add = |p: &mut Self, x: Length, y: Length, angle: Angle| {
            p.add_vertex(Point::new(x, y), angle);
        };
        let a0 = Angle::DEG0;
        let a45 = Angle::DEG45;
        if corner_radius == UnsignedLength::ZERO {
            // Regular polygon without rounded corners.
            add(&mut p, rx, ry - inner_chamfer, a0);
            add(&mut p, rx - inner_chamfer, ry, a0);
            add(&mut p, inner_chamfer - rx, ry, a0);
            add(&mut p, -rx, ry - inner_chamfer, a0);
            add(&mut p, -rx, inner_chamfer - ry, a0);
            add(&mut p, inner_chamfer - rx, -ry, a0);
            add(&mut p, rx - inner_chamfer, -ry, a0);
            add(&mut p, rx, inner_chamfer - ry, a0);
        } else if inner_chamfer >= rx.min(ry) {
            // Corner radius is too large for the given size, it's actually an
            // obround.
            return Self::obround(width, height);
        } else {
            // Octagon with rounded corners.
            let chamfer_offset = from_mm(r.to_mm() * (1.0 - (1.0 / 2f64.sqrt())));
            let outer_chamfer = inner_chamfer - r + chamfer_offset;
            debug_assert!(chamfer_offset >= Length::ZERO);
            debug_assert!(chamfer_offset <= outer_chamfer);
            debug_assert!(outer_chamfer <= inner_chamfer);
            let (ic, oc, co) = (inner_chamfer, outer_chamfer, chamfer_offset);
            add(&mut p, rx, ry - ic, a45);
            add(&mut p, rx - co, ry - oc, a0);
            add(&mut p, rx - oc, ry - co, a45);
            add(&mut p, rx - ic, ry, a0);
            add(&mut p, ic - rx, ry, a45);
            add(&mut p, oc - rx, ry - co, a0);
            add(&mut p, co - rx, ry - oc, a45);
            add(&mut p, -rx, ry - ic, a0);
            add(&mut p, -rx, ic - ry, a45);
            add(&mut p, co - rx, oc - ry, a0);
            add(&mut p, oc - rx, co - ry, a45);
            add(&mut p, ic - rx, -ry, a0);
            add(&mut p, rx - ic, -ry, a45);
            add(&mut p, rx - oc, co - ry, a0);
            add(&mut p, rx - co, oc - ry, a45);
            add(&mut p, rx, ic - ry, a0);
        }
        p.close();
        p
    }

    /// Approximates the arc from `p1` to `p2` by straight line segments
    /// with the given maximum deviation (or returns a straight line if the
    /// input is not an arc or too small).
    pub fn flat_arc(p1: Point, p2: Point, angle: Angle, max_tolerance: PositiveLength) -> Self {
        if let Some(center) = toolbox::arc_center(p1, p2, angle) {
            let radius_abs = (p1 - center).length();
            if radius_abs > (max_tolerance / 2) {
                // Calculate how many lines we need to create.
                let radius_abs_nm = radius_abs.to_nm() as f64;
                // qBound(0, tolerance, radius / 4)
                let y = (radius_abs_nm / 4.0)
                    .min(max_tolerance.to_nm() as f64)
                    .max(0.0);
                let steps_per_rad =
                    (0.5 / libm::acos(1.0 - y / radius_abs_nm)).min(radius_abs_nm / 2.0);
                // Truncating conversion like `qCeil()` (saturating instead of UB).
                let steps = (steps_per_rad * angle.abs().to_rad()).ceil() as i32;

                // Create line segments.
                let mut p = Self::default();
                p.add_vertex(p1, Angle::DEG0);
                let angle_delta = f64::from(angle.to_micro_deg()) / f64::from(steps);
                for i in 1..steps {
                    // Truncating conversion like the implicit upstream
                    // conversion to `qint32`.
                    let a = Angle::new((angle_delta * f64::from(i)) as i32);
                    p.add_vertex(p1.rotated(a, center), Angle::DEG0);
                }
                p.add_vertex(p2, Angle::DEG0);
                return p;
            }
        }

        // By default, return a straight line segment.
        Self::line(p1, p2, Angle::DEG0)
    }
}

impl From<Vec<Vertex>> for Path {
    fn from(vertices: Vec<Vertex>) -> Self {
        Self::new(vertices)
    }
}

impl FromIterator<Vertex> for Path {
    fn from_iter<I: IntoIterator<Item = Vertex>>(iter: I) -> Self {
        Self::new(iter.into_iter().collect())
    }
}

impl SerializeObject for Path {
    /// Appends one `(vertex ...)` child per vertex, each preceded by a line
    /// break, followed by a final line break.
    fn serialize(&self, root: &mut List) {
        for vertex in &self.vertices {
            root.ensure_line_break();
            vertex.serialize(root.append_list("vertex"));
        }
        root.ensure_line_break();
    }
}

impl DeserializeObject for Path {
    /// Loads all `(vertex ...)` children of `node`.
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        node.children_named("vertex")
            .map(Vertex::deserialize)
            .collect::<serialization::Result<Vec<_>>>()
            .map(Self::new)
    }
}

/// Generates a validated [`Path`] wrapper.
macro_rules! constrained_path {
    ($(#[$meta:meta])* $name:ident, $check:expr, $err:expr) => {
        $(#[$meta])*
        ///
        /// Serde: serialized like [`Path`], the constraint is checked when
        /// deserializing.
        #[derive(
            Debug,
            Clone,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            serde::Serialize,
            serde::Deserialize,
        )]
        #[serde(try_from = "Path", into = "Path")]
        pub struct $name(Path);

        impl $name {
            /// Creates the value, or returns an error if `path` violates the
            /// constraint.
            pub fn new(path: Path) -> Result<Self, Error> {
                let check: fn(&Path) -> bool = $check;
                if check(&path) { Ok(Self(path)) } else { Err($err) }
            }

            /// Returns the wrapped path.
            pub fn get(&self) -> &Path {
                &self.0
            }

            /// Consumes the wrapper and returns the path.
            pub fn into_inner(self) -> Path {
                self.0
            }
        }

        impl Deref for $name {
            type Target = Path;
            fn deref(&self) -> &Path {
                &self.0
            }
        }

        impl TryFrom<Path> for $name {
            type Error = Error;
            fn try_from(path: Path) -> Result<Self, Error> {
                Self::new(path)
            }
        }

        impl From<$name> for Path {
            fn from(value: $name) -> Path {
                value.0
            }
        }

        impl PartialEq<Path> for $name {
            fn eq(&self, other: &Path) -> bool {
                self.0 == *other
            }
        }
    };
}

constrained_path!(
    /// A [`Path`] which is guaranteed to contain at least one vertex.
    NonEmptyPath,
    |p| !p.vertices.is_empty(),
    Error::EmptyPath
);

constrained_path!(
    /// A [`Path`] which is guaranteed to be closed, to have at least 4
    /// vertices and to contain only straight segments.
    StraightAreaPath,
    |p| (p.vertices.len() >= 4) && p.is_closed() && !p.is_curved(),
    Error::NotStraightArea
);

impl NonEmptyPath {
    /// Creates a path consisting of a single vertex (upstream
    /// `makeNonEmptyPath()`).
    pub fn from_point(pos: Point) -> Self {
        Self(Path::new(vec![Vertex::at(pos)]))
    }

    /// Returns the first vertex.
    pub fn first(&self) -> Vertex {
        // The constraint guarantees at least one vertex.
        self.0.vertices[0]
    }
}

impl DeserializeObject for NonEmptyPath {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self::new(Path::deserialize(node)?)?)
    }
}
