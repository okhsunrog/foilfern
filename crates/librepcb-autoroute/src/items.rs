//! Copper items with their keepout distances, in an R*-tree for exact
//! clearance checks.
//!
//! An [`Item`] stores, instead of a clearance, the distances from its core
//! (see [`geometry`](crate::geometry)) that the center line of a trace and
//! the center of a via must keep:
//!
//! - `trace_keep`: required distance to the center line of a trace, minus
//!   the trace half width (item radius + clearance);
//! - `via_keep_other`: required distance to the center of a via of another
//!   net (item radius + clearance + via radius, or the drill-to-drill
//!   distance for holes);
//! - `via_keep_own`: the same for vias of the item's own net (drill-to-drill
//!   distance of vias, pads which must not carry vias).
//!
//! This makes all rules (copper, board edge, NPTH, drill clearances) one
//! uniform check, used both for rasterizing the grid and for the final
//! verification.

use rstar::{AABB, RTree, RTreeObject};

use crate::geometry::{Core, P};

/// Identifier of a committed route.
pub(crate) type RouteId = usize;

/// Owner value for "no net": blocks every net.
pub(crate) const NO_NET: u32 = u32::MAX;

/// A copper item, keepout, hole or board edge.
#[derive(Debug, Clone)]
pub(crate) struct Item {
    pub core: Core,
    /// Net index, or [`NO_NET`].
    pub net: u32,
    /// Bit mask of copper layer indices.
    pub layers: u64,
    pub trace_keep: Option<f64>,
    pub via_keep_other: Option<f64>,
    pub via_keep_own: Option<f64>,
    /// The route this item belongs to (`None` for static items).
    pub route: Option<RouteId>,
}

impl Item {
    fn max_keep(&self) -> f64 {
        [self.trace_keep, self.via_keep_other, self.via_keep_own]
            .into_iter()
            .flatten()
            .fold(0.0, f64::max)
    }
}

/// Entry of the R*-tree: index into the item list plus bounding box.
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    index: usize,
    envelope: AABB<[i64; 2]>,
}

impl RTreeObject for Entry {
    type Envelope = AABB<[i64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

/// A clearance violation found by a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Violation {
    /// With a static item (obstacle, terminal, hole, board edge), or outside
    /// the board.
    Static,
    /// With a route (which could be ripped up).
    Route(RouteId),
}

/// All items, spatially indexed.
#[derive(Debug, Default)]
pub(crate) struct Items {
    items: Vec<Item>,
    alive: Vec<bool>,
    tree: RTree<Entry>,
    max_keep: f64,
    outline: Vec<Vec<P>>,
}

impl Items {
    pub fn new(outline: Vec<Vec<P>>, static_items: Vec<Item>) -> Self {
        let entries = static_items
            .iter()
            .enumerate()
            .map(|(index, item)| Entry {
                index,
                envelope: envelope(&item.core),
            })
            .collect();
        let max_keep = static_items.iter().map(Item::max_keep).fold(0.0, f64::max);
        Self {
            alive: vec![true; static_items.len()],
            items: static_items,
            tree: RTree::bulk_load(entries),
            max_keep,
            outline,
        }
    }

    pub fn outline(&self) -> &[Vec<P>] {
        &self.outline
    }

    /// Iterates over the living items.
    pub fn iter(&self) -> impl Iterator<Item = &Item> {
        self.items
            .iter()
            .zip(&self.alive)
            .filter_map(|(item, alive)| alive.then_some(item))
    }

    /// Adds an item and returns its index.
    pub fn insert(&mut self, item: Item) -> usize {
        let index = self.items.len();
        self.tree.insert(Entry {
            index,
            envelope: envelope(&item.core),
        });
        self.max_keep = self.max_keep.max(item.max_keep());
        self.items.push(item);
        self.alive.push(true);
        index
    }

    /// Removes an item by index.
    pub fn remove(&mut self, index: usize) {
        if let Some(item) = self.items.get(index)
            && self.alive[index]
        {
            let entry = Entry {
                index,
                envelope: envelope(&item.core),
            };
            self.tree.remove(&entry);
            self.alive[index] = false;
        }
    }

    /// Whether a point is inside the board outline (even-odd over all
    /// outline polygons).
    pub fn inside_board(&self, p: P) -> bool {
        let mut inside = false;
        for poly in &self.outline {
            if crate::geometry::point_in_polygon(poly, p) {
                inside = !inside;
            }
        }
        inside
    }

    fn candidates(&self, lo: P, hi: P, extra: f64) -> impl Iterator<Item = &Item> {
        let e = (self.max_keep + extra).ceil() as i64 + 1;
        let env = AABB::from_corners([lo.x - e, lo.y - e], [hi.x + e, hi.y + e]);
        self.tree
            .locate_in_envelope_intersecting(env)
            .map(|entry| &self.items[entry.index])
    }

    /// Checks a trace segment of net `net` with half width `half_width` on
    /// copper layer index `layer`. Route violations are passed to
    /// `on_route`, which returns whether to continue checking; static
    /// violations stop the check.
    pub fn check_segment(
        &self,
        (a, b): (P, P),
        layer: usize,
        net: u32,
        half_width: f64,
        skip_route: Option<RouteId>,
        on_route: &mut dyn FnMut(RouteId) -> bool,
    ) -> Result<(), Violation> {
        if !self.inside_board(a) || !self.inside_board(b) {
            return Err(Violation::Static);
        }
        let bit = 1u64 << layer;
        let lo = P::new(a.x.min(b.x), a.y.min(b.y));
        let hi = P::new(a.x.max(b.x), a.y.max(b.y));
        for item in self.candidates(lo, hi, half_width) {
            if item.layers & bit == 0 || (item.net == net && net != NO_NET) {
                continue;
            }
            if item.route.is_some() && item.route == skip_route {
                continue;
            }
            let Some(keep) = item.trace_keep else {
                continue;
            };
            if item.core.distance_to_segment(a, b) < keep + half_width {
                match item.route {
                    None => return Err(Violation::Static),
                    Some(route) => {
                        if !on_route(route) {
                            return Err(Violation::Route(route));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Checks a via of net `net` at `p` (see [`check_segment`](Self::check_segment)).
    pub fn check_via(
        &self,
        p: P,
        net: u32,
        skip_route: Option<RouteId>,
        on_route: &mut dyn FnMut(RouteId) -> bool,
    ) -> Result<(), Violation> {
        if !self.inside_board(p) {
            return Err(Violation::Static);
        }
        for item in self.candidates(p, p, 0.0) {
            if item.route.is_some() && item.route == skip_route {
                continue;
            }
            let keep = if item.net == net && net != NO_NET {
                item.via_keep_own
            } else {
                item.via_keep_other
            };
            let Some(keep) = keep else {
                continue;
            };
            if item.core.distance_to_segment(p, p) < keep {
                match item.route {
                    None => return Err(Violation::Static),
                    Some(route) => {
                        if !on_route(route) {
                            return Err(Violation::Route(route));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn envelope(core: &Core) -> AABB<[i64; 2]> {
    let (lo, hi) = core.bounds();
    AABB::from_corners([lo.x, lo.y], [hi.x, hi.y])
}
