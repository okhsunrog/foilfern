//! The routing grid and its occupancy maps.
//!
//! A map stores one `u32` per grid node (and routing layer): [`FREE`],
//! [`BLOCKED`] (for all nets) or `net + 1` (only this net may use the node).
//! Two items of different nets covering a node make it [`BLOCKED`].
//!
//! Items are rasterized *inflated*: a trace map for traces of half width `h`
//! marks every node whose center is closer than `trace_keep + h` to an item
//! core, enlarged by the margin `sqrt(d² + pitch²/2) - d`. The margin makes
//! a straight or diagonal move between two free nodes free as a whole: the
//! distance from a convex core along a segment of length `L` dips at most
//! to `sqrt(D² - L²/4)` between two endpoints at distance `D`.
//!
//! Rows are rasterized by computing the covered x interval analytically
//! (capsule = two circles plus a band, polygon = even-odd crossings), so the
//! cost is proportional to the covered rows, not to the bounding box area.

use crate::geometry::Core;
use crate::geometry::{P, capsule_row_interval, polygon_edges, polygon_row_crossings};
use crate::items::{Item, NO_NET};

/// Node usable by every net.
pub(crate) const FREE: u32 = 0;
/// Node usable by no net.
pub(crate) const BLOCKED: u32 = u32::MAX;

/// Map value for items of net index `net`.
pub(crate) fn owner_of(net: u32) -> u32 {
    if net == NO_NET { BLOCKED } else { net + 1 }
}

/// Whether a node with map value `value` is usable by `owner`.
#[inline]
pub(crate) fn usable(value: u32, owner: u32) -> bool {
    value == FREE || value == owner
}

fn merge(cell: &mut u32, owner: u32) {
    if *cell == FREE {
        *cell = owner;
    } else if *cell != owner {
        *cell = BLOCKED;
    }
}

/// Geometry of the grid: node `(i, j)` is at `(x0 + i * pitch, y0 + j * pitch)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Grid {
    pub x0: i64,
    pub y0: i64,
    pub pitch: i64,
    pub nx: usize,
    pub ny: usize,
}

impl Grid {
    /// Creates a grid covering the bounding box `lo`..`hi`, aligned to
    /// multiples of the pitch.
    pub fn new(lo: P, hi: P, pitch: i64) -> Self {
        let x0 = lo.x.div_euclid(pitch) * pitch;
        let y0 = lo.y.div_euclid(pitch) * pitch;
        let nx = ((hi.x - x0) / pitch + 1).max(1) as usize;
        let ny = ((hi.y - y0) / pitch + 1).max(1) as usize;
        Self {
            x0,
            y0,
            pitch,
            nx,
            ny,
        }
    }

    pub fn cells(&self) -> usize {
        self.nx * self.ny
    }

    /// Position of the node with cell index `c`.
    pub fn point(&self, c: usize) -> P {
        P::new(
            self.x0 + (c % self.nx) as i64 * self.pitch,
            self.y0 + (c / self.nx) as i64 * self.pitch,
        )
    }

    /// Cell index of a point exactly on a node.
    pub fn cell_at(&self, p: P) -> Option<usize> {
        let (dx, dy) = (p.x - self.x0, p.y - self.y0);
        if dx < 0 || dy < 0 || dx % self.pitch != 0 || dy % self.pitch != 0 {
            return None;
        }
        let (i, j) = ((dx / self.pitch) as usize, (dy / self.pitch) as usize);
        (i < self.nx && j < self.ny).then_some(j * self.nx + i)
    }

    /// Range of node indices whose coordinate lies in `[lo, hi]` along an
    /// axis starting at `origin` with `n` nodes (widened by 1 nm).
    fn index_range(&self, origin: i64, n: usize, lo: f64, hi: f64) -> Option<(usize, usize)> {
        let p = self.pitch as f64;
        let i0 = ((lo - 1.0 - origin as f64) / p).ceil().max(0.0);
        let i1 = ((hi + 1.0 - origin as f64) / p).floor().min(n as f64 - 1.0);
        (i0 <= i1).then_some((i0 as usize, i1 as usize))
    }

    fn mark_row(&self, layer_map: &mut [u32], j: usize, lo: f64, hi: f64, owner: u32) {
        if let Some((i0, i1)) = self.index_range(self.x0, self.nx, lo, hi) {
            let row = &mut layer_map[j * self.nx..(j + 1) * self.nx];
            row[i0..=i1].iter_mut().for_each(|c| merge(c, owner));
        }
    }

    /// Marks all nodes within distance `d` of `core` in a single-layer map.
    pub fn rasterize(&self, layer_map: &mut [u32], core: &Core, d: f64, owner: u32) {
        let (lo, hi) = core.bounds();
        let Some((j0, j1)) = self.index_range(self.y0, self.ny, lo.y as f64 - d, hi.y as f64 + d)
        else {
            return;
        };
        for j in j0..=j1 {
            let y = (self.y0 + j as i64 * self.pitch) as f64;
            match core {
                Core::Point(a) => {
                    if let Some((l, h)) = capsule_row_interval(*a, *a, d, y) {
                        self.mark_row(layer_map, j, l, h, owner);
                    }
                }
                Core::Segment(a, b) => {
                    if let Some((l, h)) = capsule_row_interval(*a, *b, d, y) {
                        self.mark_row(layer_map, j, l, h, owner);
                    }
                }
                Core::Polygon(v) => {
                    let xs = polygon_row_crossings([v.as_slice()], y);
                    for pair in xs.as_chunks::<2>().0 {
                        self.mark_row(layer_map, j, pair[0], pair[1], owner);
                    }
                    for (a, b) in polygon_edges(v) {
                        if let Some((l, h)) = capsule_row_interval(a, b, d, y) {
                            self.mark_row(layer_map, j, l, h, owner);
                        }
                    }
                }
            }
        }
    }

    /// Marks all nodes outside the board outline as blocked.
    pub fn block_outside(&self, layer_map: &mut [u32], outline: &[Vec<P>]) {
        for j in 0..self.ny {
            let y = (self.y0 + j as i64 * self.pitch) as f64;
            let xs = polygon_row_crossings(outline.iter().map(Vec::as_slice), y);
            let mut inside = vec![false; self.nx];
            for pair in xs.as_chunks::<2>().0 {
                // Shrunk by 1 nm: nodes on the edge count as outside (the
                // board clearance blocks them anyway).
                if let Some((i0, i1)) =
                    self.index_range(self.x0, self.nx, pair[0] + 2.0, pair[1] - 2.0)
                {
                    inside[i0..=i1].iter_mut().for_each(|c| *c = true);
                }
            }
            let row = &mut layer_map[j * self.nx..(j + 1) * self.nx];
            for (cell, inside) in row.iter_mut().zip(inside) {
                if !inside {
                    *cell = BLOCKED;
                }
            }
        }
    }

    /// Distance at which items are rasterized into a trace map (see module
    /// docs).
    pub fn trace_distance(&self, keep: f64, half_width: f64) -> f64 {
        let d = keep + half_width;
        let p = self.pitch as f64;
        (d * d + p * p / 2.0).sqrt() + 2.0
    }
}

/// Occupancy of the grid: static items (obstacles, terminals, holes, board)
/// and routes, per trace width and for vias.
#[derive(Debug, Clone)]
pub(crate) struct Layered {
    pub fixed: Vec<u32>,
    pub routes: Vec<u32>,
}

impl Layered {
    pub fn new(len: usize) -> Self {
        Self {
            fixed: vec![FREE; len],
            routes: vec![FREE; len],
        }
    }
}

/// Rasterizes an item into a trace map with `layers` routing layers.
/// `layer_bits[l]` is the copper layer bit of routing layer `l`.
pub(crate) fn rasterize_trace_item(
    grid: &Grid,
    map: &mut [u32],
    layer_bits: &[u64],
    item: &Item,
    half_width: f64,
) {
    let Some(keep) = item.trace_keep else {
        return;
    };
    let d = grid.trace_distance(keep, half_width);
    let owner = owner_of(item.net);
    for (l, bit) in layer_bits.iter().enumerate() {
        if item.layers & bit != 0 {
            let cells = grid.cells();
            grid.rasterize(&mut map[l * cells..(l + 1) * cells], &item.core, d, owner);
        }
    }
}

/// Rasterizes an item into the via map.
pub(crate) fn rasterize_via_item(grid: &Grid, map: &mut [u32], item: &Item) {
    if let Some(keep) = item.via_keep_other {
        grid.rasterize(map, &item.core, keep + 2.0, owner_of(item.net));
    }
    if let Some(keep) = item.via_keep_own {
        grid.rasterize(map, &item.core, keep + 2.0, BLOCKED);
    }
}
