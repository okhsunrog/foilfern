//! Bounded history of contention between generated routes in one run.

use crate::grid::Grid;
use crate::search::STEP;

/// Bucket size in grid steps. Nearby paths through the same contested
/// passage should share the penalty without allocating a map per net.
const TILE: usize = 4;
const HIT_COST: u32 = 2 * STEP;
const MAX_HITS: u8 = 4;

#[derive(Debug)]
pub(crate) struct Congestion {
    grid: Grid,
    cols: usize,
    tiles_per_layer: usize,
    /// One tile plane per routing layer, plus one for via locations.
    hits: Vec<u8>,
    used_tiles: usize,
}

impl Congestion {
    pub fn new(grid: Grid, layers: usize) -> Self {
        let cols = grid.nx.div_ceil(TILE);
        let tiles_per_layer = cols * grid.ny.div_ceil(TILE);
        Self {
            grid,
            cols,
            tiles_per_layer,
            hits: vec![0; tiles_per_layer * (layers + 1)],
            used_tiles: 0,
        }
    }

    #[inline]
    pub fn tile(&self, layer: usize, cell: usize) -> usize {
        let (x, y) = (cell % self.grid.nx, cell / self.grid.nx);
        layer * self.tiles_per_layer + y / TILE * self.cols + x / TILE
    }

    pub fn via_tile(&self, cell: usize) -> usize {
        self.hits.len() - self.tiles_per_layer + self.tile(0, cell)
    }

    pub fn record(&mut self, mut tiles: Vec<usize>) {
        tiles.sort_unstable();
        tiles.dedup();
        for tile in tiles {
            let hits = &mut self.hits[tile];
            self.used_tiles += usize::from(*hits == 0);
            *hits = (*hits + 1).min(MAX_HITS);
        }
    }

    #[inline]
    pub fn cost(&self, tile: usize) -> u32 {
        u32::from(self.hits[tile]) * HIT_COST
    }

    pub fn used_tiles(&self) -> usize {
        self.used_tiles
    }
}
