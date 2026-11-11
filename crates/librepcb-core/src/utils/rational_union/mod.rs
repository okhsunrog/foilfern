//! Experimental exact alternative to the copper union in upstream
//! libs/librepcb/core/project/board/drc/boardclipperpathgenerator.cpp.
//!
//! Original implementation: existing Clipper/i_overlay/polyclip kernels quantize
//! constructed vertices (polyclip also narrows the coordinate range). This
//! experiment retains `dashu_ratio::RBig` vertices between unions, with a separate
//! explicit conversion to the integer grid. It does not replace file/export
//! geometry or claim upstream numeric equivalence.
//!
//! The arrangement is built by splitting every intersecting/overlapping edge,
//! then sweeping the resulting noncrossing edges with per-operand winding counts.
//! Each operand keeps its own fill rule; their filled areas are ORed together.
//! Bounding-box rejection is conservative. No epsilon or floating point is used.

mod arrangement;
mod sweep;

use std::collections::BTreeMap;

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

/// Diagnostics for the experimental union, independent of wall-clock timings.
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    paths: Vec<Vec<Point>>,
}

impl Region {
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

    /// Rounds each vertex to integer nanometers, with half ties away from zero.
    /// This is a lossy comparison adapter, not a topology-preserving snap rounder.
    /// It can collapse small features or change contacts. Do not use this output
    /// to authorize manufacturing, routing acceptance or approval cleanup.
    pub fn rounded_paths(&self) -> Result<Vec<Vec<IntPoint>>, Error> {
        let round = |v: &RBig| -> Result<i64, Error> {
            let half = RBig::ONE / RBig::from(2);
            let n = if v < &RBig::ZERO {
                -(-v + &half).floor()
            } else {
                (v + &half).floor()
            };
            i64::try_from(n).map_err(|_| Error::CoordinateOutOfRange)
        };
        self.paths
            .iter()
            .map(|path| {
                path.iter()
                    .map(|p| Ok(IntPoint::new(round(&p.x)?, round(&p.y)?)))
                    .collect()
            })
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

pub fn union(operands: &[Operand]) -> Result<(Region, Statistics), Error> {
    let mut stats = Statistics::default();
    let (points, edges) = arrangement::build(operands, &mut stats)?;
    let boundary = sweep::boundary(&points, &edges, operands)?;
    stats.boundary_edges = boundary.len();
    let paths = rings(&points, &boundary)?;
    Ok((Region { paths }, stats))
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
        loop {
            if path.len() < 3 {
                return Err(Error::InvalidTopology);
            }
            let redundant = (0..path.len()).find(|&i| {
                let prev = (i + path.len() - 1) % path.len();
                let next = (i + 1) % path.len();
                let a = direction(&path[prev], &path[i]);
                let b = direction(&path[i], &path[next]);
                cross(&a.0, &a.1, &b.0, &b.1) == RBig::ZERO
            });
            match redundant {
                Some(i) => {
                    path.remove(i);
                }
                None => break,
            }
        }
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
