//! Geometric centerline coverage against freshly built reference pours.

use std::collections::{BTreeMap, BTreeSet};

use clipper::PolyFillType;

use super::BoardExportResult;
use super::plane_fragments_builder::max_arc_tolerance;
use crate::project::error::{EntityKind, Error};
use crate::project::{BoardId, NetSignalId, Project};
use crate::types::{Layer, Point, Uuid};
use crate::utils::clipper_helpers as ch;

#[derive(Debug)]
pub struct ReferenceSelection {
    pub signal_nets: BTreeSet<NetSignalId>,
    pub reference_net: NetSignalId,
    pub reference_layer: Layer,
}

#[derive(Debug, Default)]
pub struct ReferenceNetCoverage {
    pub checked_traces: usize,
    pub checked_mm: f64,
    pub unchecked_layers: BTreeMap<Layer, usize>,
    pub gaps: Vec<ReferenceGap>,
}

#[derive(Debug)]
pub struct ReferenceCoverage {
    pub connected_fragments: usize,
    pub nets: BTreeMap<NetSignalId, ReferenceNetCoverage>,
}

#[derive(Debug, Clone)]
pub struct ReferenceGap {
    pub trace: Uuid,
    pub layer: Layer,
    pub from: Point,
    pub to: Point,
}

impl ReferenceGap {
    pub fn length_mm(&self) -> f64 {
        Line::new(self.from, self.to).length
    }

    /// Parts of this gap that are absent from the other snapshot's gaps on
    /// the same signal. Compare by board geometry so replacing or splitting
    /// a trace does not turn an unchanged gap into a new one.
    pub fn difference(&self, other: &[ReferenceGap]) -> Vec<Self> {
        let line = Line::new(self.from, self.to);
        let overlap = other
            .iter()
            .filter(|gap| gap.layer == self.layer)
            .filter_map(|gap| {
                let a = line.collinear_parameter(gap.from)?;
                let b = line.collinear_parameter(gap.to)?;
                Some(ordered(a, b))
            });
        line.uncovered(overlap)
            .map(|(from, to)| Self { from, to, ..*self })
            .collect()
    }
}

impl Project {
    pub fn reference_coverage(
        &self,
        board: BoardId,
        selection: &ReferenceSelection,
    ) -> BoardExportResult<ReferenceCoverage> {
        let model = self.board(board).ok_or(Error::NotFound {
            kind: EntityKind::Board,
            uuid: board.0,
        })?;
        let mut layers: Vec<_> = model.copper_layers().into_iter().collect();
        layers.sort_by_key(|layer| layer.copper_number());
        let adjacent: BTreeSet<_> = layers
            .windows(2)
            .filter(|pair| pair.contains(&selection.reference_layer))
            .flatten()
            .copied()
            .filter(|layer| *layer != selection.reference_layer)
            .collect();
        let mut nets: BTreeMap<_, ReferenceNetCoverage> = selection
            .signal_nets
            .iter()
            .map(|net| (*net, ReferenceNetCoverage::default()))
            .collect();
        let paths = match self.plane_job(board, None)? {
            Some(job) => {
                job.reference_fragments(selection.reference_layer, selection.reference_net)?
            }
            None => Vec::new(),
        };
        // Plane fragments are linearized by the fill engine. Preserve their
        // cut-ins for holes and use NonZero to count overlapping pours once.
        let copper = ch::paths_to_clipper(&paths, max_arc_tolerance());
        for segment in model.net_segments().values() {
            let Some(net) = segment.net().and_then(|net| nets.get_mut(&net)) else {
                continue;
            };
            for trace in segment.traces().values() {
                let layer = trace.layer();
                if !adjacent.contains(&layer) {
                    *net.unchecked_layers.entry(layer).or_default() += 1;
                    continue;
                }
                let position = |anchor| {
                    model
                        .anchor_position(segment, anchor, self.library(), self.circuit())
                        .ok_or(super::BoardExportError::Logic(
                            "Trace has an unresolved anchor",
                        ))
                };
                let start = position(trace.p1())?;
                let end = position(trace.p2())?;
                if start == end {
                    continue;
                }
                // Native serialization can reverse the two anchors. Report
                // intervals in board coordinate order in either representation.
                let (start, end) = if (start.x, start.y) <= (end.x, end.y) {
                    (start, end)
                } else {
                    (end, start)
                };
                let line = Line::new(start, end);
                net.checked_traces += 1;
                net.checked_mm += line.length;
                let mut covered = Vec::new();
                if !copper.is_empty() {
                    let subject = vec![ch::point_to_clipper(start), ch::point_to_clipper(end)];
                    let clipped = ch::intersect_to_tree(
                        &[subject],
                        &copper,
                        PolyFillType::NonZero,
                        PolyFillType::NonZero,
                        false,
                    )?;
                    for path in clipper::open_paths_from_poly_tree(&clipped) {
                        if let (Some(first), Some(last)) = (path.first(), path.last()) {
                            covered.push(ordered(
                                line.parameter(ch::point_from_clipper(*first)),
                                line.parameter(ch::point_from_clipper(*last)),
                            ));
                        }
                    }
                }
                net.gaps
                    .extend(line.uncovered(covered).map(|(from, to)| ReferenceGap {
                        trace: trace.uuid(),
                        layer,
                        from,
                        to,
                    }));
            }
        }
        Ok(ReferenceCoverage {
            connected_fragments: paths.len(),
            nets,
        })
    }
}

struct Line {
    from: Point,
    to: Point,
    dx: f64,
    dy: f64,
    length: f64,
}

impl Line {
    fn new(from: Point, to: Point) -> Self {
        let dx = (to.x - from.x).to_mm();
        let dy = (to.y - from.y).to_mm();
        Self {
            from,
            to,
            dx,
            dy,
            length: dx.hypot(dy),
        }
    }

    fn parameter(&self, point: Point) -> f64 {
        let x = (point.x - self.from.x).to_mm();
        let y = (point.y - self.from.y).to_mm();
        (x * self.dx + y * self.dy) / (self.length * self.length)
    }

    fn collinear_parameter(&self, point: Point) -> Option<f64> {
        let x = (point.x - self.from.x).to_mm();
        let y = (point.y - self.from.y).to_mm();
        // Allow rounding to integer nm at the two independently clipped ends.
        let distance = (x * self.dy - y * self.dx).abs() / self.length;
        (distance <= 0.000_002).then(|| self.parameter(point))
    }

    fn point(&self, t: f64) -> Point {
        if t == 0.0 {
            return self.from;
        }
        if t == 1.0 {
            return self.to;
        }
        Point::from_nm(
            self.from.x.to_nm() + (self.dx * 1e6 * t).round() as i64,
            self.from.y.to_nm() + (self.dy * 1e6 * t).round() as i64,
        )
    }

    fn uncovered(
        &self,
        intervals: impl IntoIterator<Item = (f64, f64)>,
    ) -> impl Iterator<Item = (Point, Point)> + '_ {
        let mut intervals: Vec<_> = intervals
            .into_iter()
            .map(|(a, b)| (a.clamp(0.0, 1.0), b.clamp(0.0, 1.0)))
            .filter(|(a, b)| a < b)
            .collect();
        intervals.sort_by(|(a, _), (b, _)| a.total_cmp(b));
        let mut covered_until: f64 = 0.0;
        intervals
            .into_iter()
            .chain([(1.0, 1.0)])
            .filter_map(move |(a, b)| {
                let from = self.point(covered_until);
                let to = self.point(a);
                let gap = (covered_until < a && from != to).then_some((from, to));
                covered_until = covered_until.max(b);
                gap
            })
    }
}

fn ordered(a: f64, b: f64) -> (f64, f64) {
    (a.min(b), a.max(b))
}
