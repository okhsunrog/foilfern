//! Port of libs/librepcb/core/algorithm/airwiresbuilder.{h,cpp}, including
//! the (patched) `delaunay-triangulation` library upstream uses
//! (`libs/delaunay-triangulation`, Bowyer-Watson algorithm).
//!
//! The air wires are the minimum spanning forest (Kruskal) over the already
//! connected edges and the air wire candidate edges. Among several equally
//! long alternatives, the choice depends on the exact candidate edges and
//! their order after upstream's (unstable) `std::sort()`. Since the chosen
//! air wires end up in DRC "missing connection" messages, whose approvals
//! are stored in board files, everything is ported literally instead of
//! using a Delaunay/MST crate (which choose other wires among ties):
//!
//! - candidate edges: for 2 and 3 points all point pairs, otherwise a
//!   fallback chain through all points in insertion order plus all edges of
//!   all triangles of the Delaunay triangulation (with duplicates), in
//!   upstream's order;
//! - the triangulation with upstream's floating point semantics
//!   (single precision circumcircle test, bounding box and super triangle);
//! - the sort with [`clipper::std_sort`], a replica of libstdc++'s
//!   `std::sort()` (the reference platform);
//! - Kruskal's algorithm processing the edges from the end of the sorted
//!   list, including the orientation of the resulting air wires.
//!
//! The port was compared with the C++ code (compiled with GCC/libstdc++)
//! on a few thousand random inputs (grids, duplicates, collinear points,
//! connected edges): the air wires, their order and orientation were
//! identical. The triangulation is O(n²), like upstream.

use clipper::std_sort;

use crate::types::Point;

/// An air wire between two points, identified by the indices returned by
/// [`AirWiresBuilder::add_point()`].
pub type AirWire = (usize, usize);

/// Calculates the air wires which connect a set of points, taking already
/// existing connections into account.
#[derive(Debug, Clone, Default)]
pub struct AirWiresBuilder {
    points: Vec<Vertex>,
    edges: Vec<(usize, usize)>,
}

impl AirWiresBuilder {
    /// Creates an empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a point and returns its index.
    pub fn add_point(&mut self, point: Point) -> usize {
        let id = self.points.len();
        self.points.push(Vertex {
            x: point.x.to_nm() as f64,
            y: point.y.to_nm() as f64,
            id,
        });
        id
    }

    /// Adds an existing connection between two points.
    ///
    /// # Panics
    ///
    /// [`build_air_wires()`](Self::build_air_wires) panics if an index was
    /// not returned by [`add_point()`](Self::add_point).
    pub fn add_edge(&mut self, p1: usize, p2: usize) {
        debug_assert!(p1 < self.points.len() && p2 < self.points.len());
        self.edges.push((p1, p2));
    }

    /// Returns the air wires needed to connect all points with the shortest
    /// total length, in upstream's order and orientation.
    pub fn build_air_wires(&self) -> Vec<AirWire> {
        let points = &self.points;
        let mut edges: Vec<Edge> = self
            .edges
            .iter()
            .map(|&(p1, p2)| Edge {
                p1: points[p1],
                p2: points[p2],
                weight: -1.0,
            })
            .collect();
        let connected_edges = edges.len();

        // Determine additional edges between the points (candidates for air
        // wires).
        let candidate = |p1: Vertex, p2: Vertex| Edge {
            p1,
            p2,
            weight: -1.0,
        };
        match points.len() {
            0 | 1 => {}
            2 => edges.push(candidate(points[0], points[1])),
            3 => {
                // Upstream triangulates manually (more stable than the
                // triangulation library).
                edges.push(candidate(points[0], points[1]));
                edges.push(candidate(points[1], points[2]));
                edges.push(candidate(points[2], points[0]));
            }
            _ => {
                // Fallback edges, since the triangulation does not work well
                // for some inputs (e.g. collinear points).
                edges.extend(points.windows(2).map(|w| candidate(w[0], w[1])));
                edges.extend(triangulate(points));
            }
        }

        // Determine the weights of the new edges.
        for edge in &mut edges[connected_edges..] {
            edge.weight = edge.p1.dist2(&edge.p2);
        }

        kruskal_mst(points.len(), edges)
    }
}

/// Kruskal's algorithm like upstream `kruskalMst()` (adapted from
/// horizon/KiCad): the edges are sorted by descending weight and processed
/// from the end, connected edges (negative weight) first.
fn kruskal_mst(node_count: usize, mut edges: Vec<Edge>) -> Vec<AirWire> {
    // Upstream uses unsigned arithmetic (wraps for 0 nodes, which have no
    // edges anyway).
    let mut mst_expected_size = node_count.saturating_sub(1);
    let mut mst = Vec::new();
    let mut ratsnest_lines = false;

    // Tags for detecting cycles, and the nodes per tag (subtrees).
    let mut tags: Vec<usize> = (0..node_count).collect();
    let mut cycles: Vec<Vec<usize>> = (0..node_count).map(|i| vec![i]).collect();

    std_sort::sort_by(&mut edges, |a, b| a.weight > b.weight);

    while mst.len() < mst_expected_size {
        let Some(edge) = edges.pop() else {
            break;
        };
        let src_tag = tags[edge.p1.id];
        let trg_tag = tags[edge.p2.id];
        if src_tag != trg_tag {
            // Connected items (negative weight) come first; the remaining
            // edges are air wires.
            if !ratsnest_lines && edge.weight >= 0.0 {
                ratsnest_lines = true;
            }
            for &node in &cycles[trg_tag] {
                tags[node] = src_tag;
            }
            if ratsnest_lines {
                mst.push((edge.p1.id, edge.p2.id));
            } else {
                mst_expected_size -= 1;
            }
            let moved = std::mem::take(&mut cycles[trg_tag]);
            cycles[src_tag].extend(moved);
        }
    }
    mst
}

/// A point of the triangulation (upstream `delaunay::Vector2<qreal>`).
#[derive(Debug, Clone, Copy)]
struct Vertex {
    x: f64,
    y: f64,
    id: usize,
}

impl Vertex {
    /// Squared distance (`Vector2::dist2()`).
    fn dist2(&self, other: &Vertex) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx * dx + dy * dy
    }

    /// Equality of the coordinates (`operator==`, ignores the ID).
    fn same_position(&self, other: &Vertex) -> bool {
        self.x == other.x && self.y == other.y
    }
}

/// An edge (upstream `delaunay::Edge<qreal>`).
#[derive(Debug, Clone, Copy)]
struct Edge {
    p1: Vertex,
    p2: Vertex,
    weight: f64,
}

impl Edge {
    /// Undirected equality of the coordinates (`operator==`).
    fn same_as(&self, other: &Edge) -> bool {
        (self.p1.same_position(&other.p1) && self.p2.same_position(&other.p2))
            || (self.p1.same_position(&other.p2) && self.p2.same_position(&other.p1))
    }
}

/// A triangle (upstream `delaunay::Triangle<qreal>`).
#[derive(Debug, Clone, Copy)]
struct Triangle {
    p1: Vertex,
    p2: Vertex,
    p3: Vertex,
    is_bad: bool,
}

impl Triangle {
    fn new(p1: Vertex, p2: Vertex, p3: Vertex) -> Self {
        Self {
            p1,
            p2,
            p3,
            is_bad: false,
        }
    }

    /// The edges `(p1, p2)`, `(p2, p3)`, `(p3, p1)`.
    fn edges(&self) -> [Edge; 3] {
        let edge = |p1, p2| Edge {
            p1,
            p2,
            weight: -1.0,
        };
        [
            edge(self.p1, self.p2),
            edge(self.p2, self.p3),
            edge(self.p3, self.p1),
        ]
    }

    fn contains_vertex(&self, v: &Vertex) -> bool {
        self.p1.same_position(v) || self.p2.same_position(v) || self.p3.same_position(v)
    }

    /// Upstream `circumCircleContains()`: the calculation mixes double
    /// precision expressions with `float` variables and `sqrtf()`; the
    /// conversions are reproduced exactly since they decide about the
    /// triangulation of (nearly) co-circular points.
    fn circum_circle_contains(&self, v: &Vertex) -> bool {
        let (p1, p2, p3) = (self.p1, self.p2, self.p3);
        let ab = ((p1.x * p1.x) + (p1.y * p1.y)) as f32 as f64;
        let cd = ((p2.x * p2.x) + (p2.y * p2.y)) as f32 as f64;
        let ef = ((p3.x * p3.x) + (p3.y * p3.y)) as f32 as f64;

        let circum_x = ((ab * (p3.y - p2.y) + cd * (p1.y - p3.y) + ef * (p2.y - p1.y))
            / (p1.x * (p3.y - p2.y) + p2.x * (p1.y - p3.y) + p3.x * (p2.y - p1.y))
            / 2.0) as f32 as f64;
        let circum_y = ((ab * (p3.x - p2.x) + cd * (p1.x - p3.x) + ef * (p2.x - p1.x))
            / (p1.y * (p3.x - p2.x) + p2.y * (p1.x - p3.x) + p3.y * (p2.x - p1.x))
            / 2.0) as f32 as f64;
        let circum_radius = ((((p1.x - circum_x) * (p1.x - circum_x))
            + ((p1.y - circum_y) * (p1.y - circum_y))) as f32)
            .sqrt();
        let dist = ((((v.x - circum_x) * (v.x - circum_x)) + ((v.y - circum_y) * (v.y - circum_y)))
            as f32)
            .sqrt();
        dist <= circum_radius
    }
}

/// Upstream `delaunay::Delaunay<qreal>::triangulate()` followed by
/// `getEdges()`: returns the three edges of every resulting triangle (edges
/// shared by two triangles are returned twice). `vertices` must not be
/// empty.
fn triangulate(vertices: &[Vertex]) -> Vec<Edge> {
    // Determine the super triangle (upstream uses `float` here).
    let mut min_x = vertices[0].x as f32;
    let mut min_y = vertices[0].y as f32;
    let mut max_x = min_x;
    let mut max_y = min_y;
    for v in vertices {
        if v.x < f64::from(min_x) {
            min_x = v.x as f32;
        }
        if v.y < f64::from(min_y) {
            min_y = v.y as f32;
        }
        if v.x > f64::from(max_x) {
            max_x = v.x as f32;
        }
        if v.y > f64::from(max_y) {
            max_y = v.y as f32;
        }
    }
    let dx = max_x - min_x;
    let dy = max_y - min_y;
    let delta_max = if dx < dy { dy } else { dx }; // std::max(dx, dy)
    let mid_x = (min_x + max_x) / 2.0;
    let mid_y = (min_y + max_y) / 2.0;
    let super_vertex = |x: f32, y: f32| Vertex {
        x: f64::from(x),
        y: f64::from(y),
        id: 0,
    };
    let s1 = super_vertex(mid_x - 20.0 * delta_max, mid_y - delta_max);
    let s2 = super_vertex(mid_x, mid_y + 20.0 * delta_max);
    let s3 = super_vertex(mid_x + 20.0 * delta_max, mid_y - delta_max);

    let mut triangles = vec![Triangle::new(s1, s2, s3)];
    for p in vertices {
        let mut polygon: Vec<(Edge, bool)> = Vec::new();
        for t in &mut triangles {
            if t.circum_circle_contains(p) {
                t.is_bad = true;
                polygon.extend(t.edges().map(|e| (e, false)));
            }
        }
        triangles.retain(|t| !t.is_bad);

        for i in 0..polygon.len() {
            for j in 0..polygon.len() {
                if i != j && polygon[i].0.same_as(&polygon[j].0) {
                    polygon[i].1 = true;
                    polygon[j].1 = true;
                }
            }
        }
        polygon.retain(|(_, bad)| !bad);

        triangles.extend(polygon.iter().map(|(e, _)| Triangle::new(e.p1, e.p2, *p)));
    }

    triangles
        .retain(|t| !(t.contains_vertex(&s1) || t.contains_vertex(&s2) || t.contains_vertex(&s3)));
    triangles.iter().flat_map(Triangle::edges).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_with_duplicates() {
        // 4x4 grid (co-circular points everywhere) with duplicated corner.
        // Expected result (incl. order and orientation) captured from the
        // upstream C++ code (triangulation library + `kruskalMst()`, built
        // with GCC/libstdc++). Note that upstream connects the duplicate
        // point to a neighbor instead of the point at the same position.
        let mut builder = AirWiresBuilder::new();
        for y in 0..4 {
            for x in 0..4 {
                builder.add_point(Point::from_nm(x * 1000, y * 1000));
            }
        }
        builder.add_point(Point::from_nm(0, 0));
        builder.add_edge(0, 1);
        let expected = [
            (9, 10),
            (5, 9),
            (10, 6),
            (8, 9),
            (4, 8),
            (6, 7),
            (2, 6),
            (7, 3),
            (2, 1),
            (14, 15),
            (13, 14),
            (12, 13),
            (10, 11),
            (16, 4),
            (10, 14),
        ];
        assert_eq!(builder.build_air_wires(), expected);
    }

    #[test]
    fn repeated_calls() {
        let mut builder = AirWiresBuilder::new();
        builder.add_point(Point::from_nm(0, 0));
        builder.add_point(Point::from_nm(10, 0));
        builder.add_point(Point::from_nm(0, 10));
        builder.add_point(Point::from_nm(10, 10));
        let first = builder.build_air_wires();
        assert_eq!(first.len(), 3);
        assert_eq!(first, builder.build_air_wires());
    }
}
