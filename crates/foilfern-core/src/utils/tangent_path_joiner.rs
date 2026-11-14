//! Port of libs/librepcb/core/utils/tangentpathjoiner.{h,cpp}.
//!
//! Hand-written: this is a LibrePCB specific heuristic (e.g. used to import
//! DXF outlines), no crate implements it.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::{Duration, Instant};

use crate::geometry::{Path, Vertex};
use crate::types::Point;

/// Result of [`join()`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinResult {
    /// The joined paths.
    pub paths: Vec<Path>,
    /// Whether the search was aborted due to the timeout (the result is then
    /// valid but not optimal).
    pub timed_out: bool,
}

/// Joins tangent paths (polylines) together:
///
/// - Invalid paths (less than 2 vertices) are removed.
/// - Already closed paths are returned as-is.
/// - Joined, closed paths are searched, starting with the longest path.
/// - Then joined, open paths are searched, starting with the longest path.
/// - Remaining (non tangent) paths are returned as-is.
///
/// If there are many possible solutions (many paths at the same coordinate),
/// the search can take a lot of time. If `timeout` is given, the search is
/// aborted after that time and a non-optimal (but still valid) result is
/// returned.
pub fn join(mut paths: Vec<Path>, timeout: Option<Duration>) -> JoinResult {
    let mut result = Vec::new();

    // Return closed paths as-is and skip invalid paths.
    for i in (0..paths.len()).rev() {
        if paths[i].is_closed() {
            result.push(paths.remove(i));
        } else if paths[i].vertices().len() < 2 {
            paths.remove(i);
        }
    }

    // Find all unambiguous path pairs which can be joined.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum VertexIndex {
        First,
        Last,
    }
    let first_pos = |p: &Path| p.vertices()[0].pos;
    let last_pos = |p: &Path| p.vertices()[p.vertices().len() - 1].pos;
    let mut join_points: BTreeMap<Point, BTreeMap<usize, VertexIndex>> = BTreeMap::new();
    for (i, path) in paths.iter().enumerate() {
        join_points
            .entry(first_pos(path))
            .or_default()
            .insert(i, VertexIndex::First);
        join_points
            .entry(last_pos(path))
            .or_default()
            .insert(i, VertexIndex::Last);
    }

    // Now join these pairs. Joining only modifies the maps of existing keys,
    // so iterating over a snapshot of the keys is equivalent to upstream.
    let mut obsolete_paths: Vec<usize> = Vec::new();
    let keys: Vec<Point> = join_points.keys().copied().collect();
    for key in keys {
        let entry = &join_points[&key];
        if entry.len() != 2 {
            continue;
        }
        let (&i1, &p1) = entry.first_key_value().expect("map has two entries");
        let (&i2, &p2) = entry.last_key_value().expect("map has two entries");
        let mut vertices: Vec<Vertex>;
        match (p1, p2) {
            (VertexIndex::Last, VertexIndex::First) => {
                vertices = paths[i1].vertices().to_vec();
                vertices.pop();
                vertices.extend_from_slice(paths[i2].vertices());
                let last = vertices[vertices.len() - 1].pos;
                join_points.entry(last).or_default().remove(&i2);
                join_points
                    .entry(last)
                    .or_default()
                    .insert(i1, VertexIndex::Last);
            }
            (VertexIndex::Last, VertexIndex::Last) => {
                vertices = paths[i1].vertices().to_vec();
                vertices.pop();
                vertices.extend_from_slice(paths[i2].reversed().vertices());
                let last = vertices[vertices.len() - 1].pos;
                join_points.entry(last).or_default().remove(&i2);
                join_points
                    .entry(last)
                    .or_default()
                    .insert(i1, VertexIndex::Last);
            }
            (VertexIndex::First, VertexIndex::Last) => {
                vertices = paths[i2].vertices().to_vec();
                vertices.pop();
                vertices.extend_from_slice(paths[i1].vertices());
                let first = vertices[0].pos;
                let last = vertices[vertices.len() - 1].pos;
                join_points.entry(first).or_default().remove(&i2);
                join_points
                    .entry(first)
                    .or_default()
                    .insert(i1, VertexIndex::First);
                join_points
                    .entry(last)
                    .or_default()
                    .insert(i1, VertexIndex::Last);
            }
            (VertexIndex::First, VertexIndex::First) => {
                vertices = paths[i1].reversed().into_vertices();
                vertices.pop();
                vertices.extend_from_slice(paths[i2].vertices());
                let first = vertices[0].pos;
                let last = vertices[vertices.len() - 1].pos;
                join_points.entry(last).or_default().remove(&i2);
                join_points
                    .entry(last)
                    .or_default()
                    .insert(i1, VertexIndex::Last);
                join_points
                    .entry(first)
                    .or_default()
                    .insert(i1, VertexIndex::First);
            }
        }
        paths[i1] = Path::new(vertices);
        debug_assert!(!obsolete_paths.contains(&i2));
        obsolete_paths.push(i2);
    }
    obsolete_paths.sort_unstable_by(|a, b| b.cmp(a));
    for i in obsolete_paths {
        paths.remove(i);
    }

    // Add closed paths to the result and remove them from the input.
    for i in (0..paths.len()).rev() {
        if paths[i].is_closed() {
            result.push(paths.remove(i));
        }
    }

    // Find all paths and sort by relevance.
    let mut search = Search {
        paths: &paths,
        start: Instant::now(),
        timeout,
        timed_out: false,
        found: Vec::new(),
    };
    search.find_all_paths(&Candidate::default());
    let timed_out = search.timed_out;
    let mut found = search.found;
    for candidate in &mut found {
        candidate.length_or_area = candidate.calc_length_or_area(&paths);
    }
    stable_sort_by_less(&mut found, |r1, r2| {
        // Prio 1: Closed paths.
        if r1.is_closed() != r2.is_closed() {
            return r1.is_closed();
        }
        // Prio 2: Long open paths or closed paths with a large area, to get
        // the outest most polygons instead of polygons inside e.g. a symbol
        // body. Use a tolerance to avoid sub-ULP floating-point differences.
        let (l1, l2) = (r1.length_or_area, r2.length_or_area);
        if (l1 - l2).abs() > 50e-9 {
            return l1 > l2;
        }
        // Prio 3: Paths consisting of many joints.
        let (c1, c2) = (r1.segments.len(), r2.segments.len());
        if c1 != c2 {
            return c1 > c2;
        }
        // Prio 4: Lower, non-reversed indices.
        for (s1, s2) in r1.segments.iter().zip(&r2.segments) {
            if s1.reverse != s2.reverse {
                return s2.reverse;
            }
            if s1.index != s2.index {
                return s1.index < s2.index;
            }
        }
        false
    });

    // Add found paths to result.
    let mut consumed: HashSet<usize> = HashSet::new();
    for f in &found {
        if f.indices.is_disjoint(&consumed) {
            result.push(f.build_path(&paths));
            consumed.extend(f.indices.iter().copied());
        }
    }

    JoinResult {
        paths: result,
        timed_out,
    }
}

#[derive(Debug, Clone, Copy)]
struct Segment {
    index: usize,
    reverse: bool,
}

/// A (partially) joined path (upstream `TangentPathJoiner::Result`).
#[derive(Debug, Clone, Default)]
struct Candidate {
    segments: Vec<Segment>,
    indices: HashSet<usize>,
    junctions: BTreeSet<Point>,
    start_pos: Point,
    end_pos: Point,
    length_or_area: f64,
}

impl Candidate {
    fn is_closed(&self) -> bool {
        !self.segments.is_empty() && (self.start_pos == self.end_pos)
    }

    fn calc_length_or_area(&self, paths: &[Path]) -> f64 {
        let path = self.build_path(paths);
        if path.is_closed() {
            path.calc_area_of_straight_segments()
        } else {
            path.total_straight_length().to_mm()
        }
    }

    fn sub(&self, index: usize, reverse: bool, start: Point, end: Point) -> Self {
        let mut r = self.clone();
        r.segments.push(Segment { index, reverse });
        r.indices.insert(index);
        r.junctions.insert(end);
        if self.segments.is_empty() {
            r.start_pos = start;
        }
        r.end_pos = end;
        r
    }

    fn build_path(&self, paths: &[Path]) -> Path {
        let mut vertices: Vec<Vertex> = Vec::new();
        for segment in &self.segments {
            vertices.pop();
            let p = &paths[segment.index];
            if segment.reverse {
                vertices.extend_from_slice(p.reversed().vertices());
            } else {
                vertices.extend_from_slice(p.vertices());
            }
        }
        Path::new(vertices)
    }
}

struct Search<'a> {
    paths: &'a [Path],
    start: Instant,
    timeout: Option<Duration>,
    timed_out: bool,
    found: Vec<Candidate>,
}

impl Search<'_> {
    fn find_all_paths(&mut self, prefix: &Candidate) {
        for i in 0..self.paths.len() {
            if self.timeout.is_some_and(|t| self.start.elapsed() > t) {
                self.timed_out = true;
                break;
            }
            if !prefix.indices.contains(&i) {
                for reverse in [false, true] {
                    if let Some(r) = self.join(prefix, i, reverse) {
                        let closed = r.is_closed();
                        self.found.push(r.clone());
                        if !closed {
                            self.find_all_paths(&r);
                        }
                    }
                }
            }
        }
    }

    fn join(&self, prefix: &Candidate, index: usize, reverse: bool) -> Option<Candidate> {
        let vertices = self.paths[index].vertices();
        let (first, last) = (vertices[0].pos, vertices[vertices.len() - 1].pos);
        let (start, end) = if reverse {
            (last, first)
        } else {
            (first, last)
        };
        if (prefix.segments.is_empty() || (start == prefix.end_pos))
            && !prefix.junctions.contains(&end)
        {
            Some(prefix.sub(index, reverse, start, end))
        } else {
            None
        }
    }
}

/// Stable merge sort with a "less than" predicate (like `std::stable_sort()`,
/// never panics on inconsistent comparisons).
fn stable_sort_by_less<T: Clone>(v: &mut [T], mut less: impl FnMut(&T, &T) -> bool) {
    fn merge_sort<T: Clone>(v: &mut [T], less: &mut impl FnMut(&T, &T) -> bool) {
        let n = v.len();
        if n <= 1 {
            return;
        }
        let mid = n / 2;
        merge_sort(&mut v[..mid], less);
        merge_sort(&mut v[mid..], less);
        let left = v[..mid].to_vec();
        let right = v[mid..].to_vec();
        let (mut i, mut j) = (0, 0);
        for slot in v.iter_mut() {
            // Take from the right only if strictly less (stability).
            if j < right.len() && (i >= left.len() || less(&right[j], &left[i])) {
                *slot = right[j].clone();
                j += 1;
            } else {
                *slot = left[i].clone();
                i += 1;
            }
        }
    }
    merge_sort(v, &mut less);
}
