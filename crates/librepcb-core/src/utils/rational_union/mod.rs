//! Exact DRC boolean geometry replacing the copper union in upstream
//! libs/librepcb/core/project/board/drc/boardclipperpathgenerator.cpp.
//!
//! Original implementation: existing Clipper/i_overlay/polyclip kernels quantize
//! constructed vertices (polyclip also narrows the coordinate range). This
//! implementation retains `dashu_ratio::RBig` vertices between unions, with a separate
//! explicit conversion to the integer grid. It does not replace file/export
//! geometry or claim upstream numeric equivalence.
//!
//! The arrangement is built by splitting every intersecting/overlapping edge,
//! then sweeping the resulting noncrossing edges with per-operand winding counts.
//! Each operand keeps its own fill rule; their filled areas are ORed together.
//! Bounding-box rejection is conservative. No epsilon or floating point is used.

mod arrangement;
mod bounds;
mod lines;
mod sweep;

use std::collections::{BTreeMap, VecDeque};

use clipper::{IntPoint, PolyFillType};
use dashu_ratio::RBig;

/// The coordinate range accepted by Clipper 1.
const MAX_COORD: i64 = 0x3fff_ffff_ffff_ffff;

/// Exact point in nanometers. Coordinates may remain fractional between unions.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    pub x: RBig,
    pub y: RBig,
}

impl Point {
    pub fn new(x: RBig, y: RBig) -> Self {
        Self { x, y }
    }

    /// Converts a display point to the nanometer grid, ties away from zero.
    pub fn rounded(&self) -> Result<IntPoint, Error> {
        let round = |v: &RBig| -> Result<i64, Error> {
            let half = RBig::ONE / RBig::from(2);
            let n = if v < &RBig::ZERO {
                -(-v + &half).floor()
            } else {
                (v + &half).floor()
            };
            i64::try_from(n).map_err(|_| Error::CoordinateOutOfRange)
        };
        Ok(IntPoint::new(round(&self.x)?, round(&self.y)?))
    }

    pub fn from_nm(x: i64, y: i64) -> Self {
        Self::new(RBig::from(x), RBig::from(y))
    }
}

/// One independently filled object. Opposite winding in *different* operands
/// cannot cancel; holes within an operand follow that operand's fill rule.
#[derive(Debug, Clone)]
pub struct Operand {
    pub paths: Vec<Vec<Point>>,
    pub fill: PolyFillType,
}

impl Operand {
    pub fn from_integer(paths: &[Vec<IntPoint>], fill: PolyFillType) -> Self {
        Self {
            paths: paths
                .iter()
                .map(|path| path.iter().map(|p| Point::from_nm(p.x, p.y)).collect())
                .collect(),
            fill,
        }
    }
}

/// Diagnostics for the union, independent of wall-clock timings.
#[derive(Debug, Clone, Default)]
pub struct Statistics {
    pub input_segments: usize,
    pub candidate_pairs: usize,
    pub intersections: usize,
    pub atomic_edges: usize,
    pub boundary_edges: usize,
}

/// Exact oriented boundary: filled area is to the left of every edge.
/// Rings have canonical start vertices and order; holes have clockwise winding.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Region {
    paths: Vec<Vec<Point>>,
    bounds: Option<(Point, Point)>,
    path_bounds: Vec<(Point, Point)>,
}

impl Region {
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    pub fn from_integer(paths: &[Vec<IntPoint>], fill: PolyFillType) -> Result<Self, Error> {
        Ok(union(&[Operand::from_integer(paths, fill)])?.0)
    }

    pub fn intersection(&self, other: &Self) -> Result<Self, Error> {
        if !self.bounds_overlap(other) {
            return Ok(Self::default());
        }
        // Both bounds exist after the overlap test.
        let ((a, b), (c, d)) = self
            .bounds
            .as_ref()
            .zip(other.bounds.as_ref())
            .ok_or(Error::InvalidTopology)?;
        let min = Point::new(a.x.clone().max(c.x.clone()), a.y.clone().max(c.y.clone()));
        let max = Point::new(b.x.clone().min(d.x.clone()), b.y.clone().min(d.y.clone()));
        Ok(combine(
            &[
                self.operand_within(&min, &max),
                other.operand_within(&min, &max),
            ],
            Operation::Intersection,
        )?
        .0)
    }

    pub fn difference(&self, other: &Self) -> Result<Self, Error> {
        if !self.bounds_overlap(other) {
            return Ok(self.clone());
        }
        let (min, max) = self.bounds.as_ref().ok_or(Error::InvalidTopology)?;
        Ok(combine(
            &[self.operand(), other.operand_within(min, max)],
            Operation::Difference,
        )?
        .0)
    }

    /// Conservative integer envelope for broad-phase queries. Rounding is only
    /// outward here; exact contours still decide every geometric predicate.
    pub(crate) fn envelope(&self) -> Result<Option<rstar::AABB<[i128; 2]>>, Error> {
        self.bounds
            .as_ref()
            .map(|(min, max)| {
                let floor =
                    |v: &RBig| i128::try_from(v.floor()).map_err(|_| Error::CoordinateOutOfRange);
                let ceil =
                    |v: &RBig| i128::try_from(v.ceil()).map_err(|_| Error::CoordinateOutOfRange);
                Ok(rstar::AABB::from_corners(
                    [floor(&min.x)?, floor(&min.y)?],
                    [ceil(&max.x)?, ceil(&max.y)?],
                ))
            })
            .transpose()
    }

    /// Intersects a small query with copper on any layer. Clip each layer to the
    /// query bounds before combining, keeping enclosing contours and hole winding.
    pub(crate) fn intersection_with_union<'a>(
        &self,
        regions: impl Iterator<Item = &'a Self>,
    ) -> Result<Self, Error> {
        let Some((min, max)) = &self.bounds else {
            return Ok(Self::default());
        };
        let operands: Vec<_> = regions.map(|r| r.operand_within(min, max)).collect();
        self.intersection(&union(&operands)?.0)
    }

    /// Finds missing copper on any required layer within this small query.
    /// Empty input and empty layers retain the n-ary intersection semantics.
    pub(crate) fn difference_from_intersection<'a>(
        &self,
        regions: impl Iterator<Item = &'a Self>,
    ) -> Result<Self, Error> {
        let Some((min, max)) = &self.bounds else {
            return Ok(Self::default());
        };
        let operands: Vec<_> = regions.map(|r| r.operand_within(min, max)).collect();
        self.difference(&intersection(&operands)?)
    }

    fn bounds_overlap(&self, other: &Self) -> bool {
        match (&self.bounds, &other.bounds) {
            (Some((a, b)), Some((c, d))) => a.x < d.x && c.x < b.x && a.y < d.y && c.y < b.y,
            _ => false,
        }
    }

    pub fn paths(&self) -> &[Vec<Point>] {
        &self.paths
    }

    /// Reuses the exact output as an operand without any intermediate rounding.
    pub fn operand(&self) -> Operand {
        Operand {
            paths: self.paths.clone(),
            fill: PolyFillType::NonZero,
        }
    }

    /// Lossy integer coordinates for displaying an already classified violation.
    /// Never feed this output back into a geometric predicate: small gaps and
    /// positive-area regions may collapse on the integer grid.
    pub fn rounded_paths(&self) -> Result<Vec<Vec<IntPoint>>, Error> {
        self.paths
            .iter()
            .map(|path| path.iter().map(Point::rounded).collect())
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Exact copper union coordinate exceeds the Clipper input range.")]
    CoordinateOutOfRange,
    #[error("Exact copper union produced inconsistent boundary topology.")]
    InvalidTopology,
}

static_assertions::assert_impl_all!(Point: Send, Sync);
static_assertions::assert_impl_all!(Region: Send, Sync);
static_assertions::assert_impl_all!(Operand: Send, Sync);

#[derive(Clone, Copy)]
enum Operation {
    Union,
    Intersection,
    Difference,
}

impl Operation {
    fn filled(self, count: usize, first: bool, operands: usize) -> bool {
        match self {
            Self::Union => count != 0,
            Self::Intersection => operands != 0 && count == operands,
            Self::Difference => first && count == 1,
        }
    }
}

pub fn union(operands: &[Operand]) -> Result<(Region, Statistics), Error> {
    combine(operands, Operation::Union)
}

/// Intersection of every operand; an empty list or empty operand yields empty.
pub fn intersection(operands: &[Operand]) -> Result<Region, Error> {
    Ok(combine(operands, Operation::Intersection)?.0)
}

fn combine(operands: &[Operand], operation: Operation) -> Result<(Region, Statistics), Error> {
    let mut stats = Statistics::default();
    let (points, edges) = arrangement::build(operands, &mut stats)?;
    let boundary = sweep::boundary(&points, &edges, operands, operation)?;
    stats.boundary_edges = boundary.len();
    let paths = rings(&points, &boundary)?;
    let path_bounds: Vec<_> = paths
        .iter()
        .map(|path| {
            // rings() rejects paths with fewer than three vertices.
            let first = &path[0];
            path.iter()
                .skip(1)
                .fold((first.clone(), first.clone()), |(a, b), p| {
                    (
                        Point::new(a.x.min(p.x.clone()), a.y.min(p.y.clone())),
                        Point::new(b.x.max(p.x.clone()), b.y.max(p.y.clone())),
                    )
                })
        })
        .collect();
    let bounds = path_bounds.iter().cloned().reduce(|(a, b), (c, d)| {
        (
            Point::new(a.x.min(c.x), a.y.min(c.y)),
            Point::new(b.x.max(d.x), b.y.max(d.y)),
        )
    });
    Ok((
        Region {
            paths,
            bounds,
            path_bounds,
        },
        stats,
    ))
}

#[derive(Debug)]
struct Edge {
    a: usize,
    b: usize,
    deltas: Vec<(usize, i64)>,
}

fn cross(ax: &RBig, ay: &RBig, bx: &RBig, by: &RBig) -> RBig {
    ax * by - ay * bx
}

fn direction(a: &Point, b: &Point) -> (RBig, RBig) {
    (&b.x - &a.x, &b.y - &a.y)
}

fn angular_cmp(a: &(RBig, RBig), b: &(RBig, RBig)) -> std::cmp::Ordering {
    let upper = |v: &(RBig, RBig)| v.1 > RBig::ZERO || (v.1 == RBig::ZERO && v.0 >= RBig::ZERO);
    upper(b)
        .cmp(&upper(a))
        .then_with(|| cross(&b.0, &b.1, &a.0, &a.1).cmp(&RBig::ZERO))
}

fn rings(points: &[Point], edges: &[(usize, usize)]) -> Result<Vec<Vec<Point>>, Error> {
    let mut outgoing: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, &(a, _)) in edges.iter().enumerate() {
        outgoing.entry(a).or_default().push(i);
    }
    for (&v, choices) in &mut outgoing {
        choices.sort_by(|&a, &b| {
            angular_cmp(
                &direction(&points[v], &points[edges[a].1]),
                &direction(&points[v], &points[edges[b].1]),
            )
        });
    }
    let mut next = Vec::with_capacity(edges.len());
    for &(a, b) in edges {
        let choices = outgoing.get(&b).ok_or(Error::InvalidTopology)?;
        let reverse = direction(&points[b], &points[a]);
        let insertion = choices.partition_point(|&i| {
            angular_cmp(&direction(&points[b], &points[edges[i].1]), &reverse).is_lt()
        });
        // A nonempty outgoing list exists for every boundary endpoint.
        let index = if insertion == 0 {
            choices.len() - 1
        } else {
            insertion - 1
        };
        next.push(choices[index]);
    }
    let mut visited = vec![false; edges.len()];
    let mut result = Vec::new();
    for start in 0..edges.len() {
        if visited[start] {
            continue;
        }
        let mut path = Vec::new();
        let mut edge = start;
        loop {
            if visited[edge] {
                if edge == start {
                    break;
                }
                return Err(Error::InvalidTopology);
            }
            visited[edge] = true;
            path.push(points[edges[edge].0].clone());
            edge = next[edge];
        }
        // Remove collinear split vertices, including the wraparound seam. Exact
        // predicates make canonical output independent of how operands were split.
        remove_collinear(&mut path)?;
        // Nonempty by the preceding size check.
        let first = (0..path.len())
            .min_by_key(|&i| &path[i])
            .ok_or(Error::InvalidTopology)?;
        path.rotate_left(first);
        result.push(path);
    }
    result.sort();
    Ok(result)
}

/// Each removal rechecks only its two neighbors. The queue has at most 3n
/// entries, including the cyclic seam, and vertices are compacted once.
fn remove_collinear(path: &mut Vec<Point>) -> Result<(), Error> {
    let n = path.len();
    if n < 3 {
        return Err(Error::InvalidTopology);
    }
    let mut prev: Vec<_> = (0..n).map(|i| (i + n - 1) % n).collect();
    let mut next: Vec<_> = (0..n).map(|i| (i + 1) % n).collect();
    let mut alive = vec![true; n];
    let mut pending: VecDeque<_> = (0..n).collect();
    let mut remaining = n;
    while let Some(i) = pending.pop_front() {
        if !alive[i] {
            continue;
        }
        let (p, q) = (prev[i], next[i]);
        let a = direction(&path[p], &path[i]);
        let b = direction(&path[i], &path[q]);
        if cross(&a.0, &a.1, &b.0, &b.1) != RBig::ZERO {
            continue;
        }
        remaining -= 1;
        if remaining < 3 {
            return Err(Error::InvalidTopology);
        }
        alive[i] = false;
        next[p] = q;
        prev[q] = p;
        pending.extend([p, q]);
    }
    let mut i = 0;
    path.retain(|_| {
        let keep = alive[i];
        i += 1;
        keep
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collinear_cleanup_handles_long_chains_seams_and_degenerate_rings() {
        let corners: Vec<_> = [(0, 0), (1000, 0), (1000, 1000), (0, 1000)]
            .into_iter()
            .map(|(x, y)| Point::from_nm(x, y))
            .collect();
        let mut split = Vec::new();
        for (a, b) in corners.iter().zip(corners.iter().cycle().skip(1)).take(4) {
            for t in 0..1000 {
                let fraction = RBig::from(t) / RBig::from(1000);
                split.push(Point::new(
                    &a.x + (&b.x - &a.x) * &fraction,
                    &a.y + (&b.y - &a.y) * &fraction,
                ));
            }
        }
        for reversed in [false, true] {
            for shift in [0, 1, 999, 1999, 3999] {
                let mut path = split.clone();
                path.rotate_left(shift);
                if reversed {
                    path.reverse();
                }
                remove_collinear(&mut path).unwrap();
                assert_eq!(path.len(), 4);
                for p in &path {
                    assert!(corners.contains(p));
                }
                let a = direction(&path[0], &path[1]);
                let b = direction(&path[1], &path[2]);
                assert_eq!(cross(&a.0, &a.1, &b.0, &b.1) < RBig::ZERO, reversed);
            }
        }
        for n in 0..5 {
            let mut line = (0..n).map(|x| Point::from_nm(x, 0)).collect();
            assert!(remove_collinear(&mut line).is_err());
        }
    }

    #[test]
    fn integer_envelope_rounds_fractional_extrema_outward() {
        let third = RBig::ONE / RBig::from(3);
        let region = union(&[Operand {
            paths: vec![vec![
                Point::new(-&third, -&third),
                Point::new(third.clone(), -&third),
                Point::new(third.clone(), third.clone()),
                Point::new(-&third, third),
            ]],
            fill: PolyFillType::NonZero,
        }])
        .unwrap()
        .0;
        let envelope = region.envelope().unwrap().unwrap();
        assert_eq!(envelope.lower(), [-1, -1]);
        assert_eq!(envelope.upper(), [1, 1]);
    }
}
