//! Port of `SchematicEditorState::findItemsAtPos()`
//! (libs/librepcb/editor/project/schematic/fsm/schematiceditorstate.cpp):
//! the items under the cursor, ordered by priority.
//!
//! The grab areas come from the view ([`SchematicView::items_at()`], asked
//! with the exact position, the near tolerance and the grid distance),
//! the priorities and distances from the model. Net and bus junctions are
//! tested on the model with upstream's 1.2 mm square grab area (upstream:
//! `SGI_NetPoint`/`SGI_BusJunction` bounding rect), visible or not.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::geometry::NetLineAnchor;
use librepcb_core::project::schematic::Schematic;
use librepcb_core::types::{Length, Point};
use librepcb_core::utils::toolbox;

use super::selection::item_exists;
use super::{Cx, SchematicItem, SchematicView};

/// Half size of the junction grab area (upstream `SGI_NetPoint`).
const JUNCTION_GRAB_RADIUS: Length = Length::new(600_000);
/// Maximum distance of the symbol origin for the higher symbol priority
/// (upstream issue #1319).
const SYMBOL_ORIGIN_DISTANCE: Length = Length::new(700_000);

/// Which items to find and how (upstream `FindFlag`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FindFlags {
    pub net_points: bool,
    pub net_lines: bool,
    pub net_labels: bool,
    pub bus_junctions: bool,
    pub bus_lines: bool,
    pub bus_labels: bool,
    pub symbols: bool,
    pub symbol_pins: bool,
    pub polygons: bool,
    pub texts: bool,
    pub images: bool,
    /// Also items near the cursor (tolerance).
    pub accept_near_match: bool,
    /// Also items within the distance to the next grid point.
    pub accept_nearest_within_grid: bool,
}

impl FindFlags {
    /// All item kinds (upstream `FindFlag::All`), exact matches only.
    pub const ALL: Self = Self {
        net_points: true,
        net_lines: true,
        net_labels: true,
        bus_junctions: true,
        bus_lines: true,
        bus_labels: true,
        symbols: true,
        symbol_pins: true,
        polygons: true,
        texts: true,
        images: true,
        accept_near_match: false,
        accept_nearest_within_grid: false,
    };

    /// No item kinds.
    pub const NONE: Self = Self {
        net_points: false,
        net_lines: false,
        net_labels: false,
        bus_junctions: false,
        bus_lines: false,
        bus_labels: false,
        symbols: false,
        symbol_pins: false,
        polygons: false,
        texts: false,
        images: false,
        accept_near_match: false,
        accept_nearest_within_grid: false,
    };

    pub fn near(mut self) -> Self {
        self.accept_near_match = true;
        self
    }

    pub fn within_grid(mut self) -> Self {
        self.accept_nearest_within_grid = true;
        self
    }
}

/// Distance of a point to an axis aligned square around `center`.
fn distance_to_square(p: Point, center: Point, half: Length) -> Length {
    let dx = ((p.x - center.x).abs() - half).max(Length::ZERO);
    let dy = ((p.y - center.y).abs() - half).max(Length::ZERO);
    Point::new(dx, dy).length().get()
}

fn px(l: Length) -> i64 {
    l.to_px().round() as i64
}

/// Finds the items at `pos`, highest priority first (0 = highest).
pub(crate) fn find_items_at(
    cx: &Cx<'_, '_>,
    pos: Point,
    flags: FindFlags,
    except: &[SchematicItem],
) -> Vec<SchematicItem> {
    let Some(s) = cx.sch() else {
        return Vec::new();
    };
    let view: &dyn SchematicView = cx.ctx.view;
    let grid = cx.grid();
    let tolerance = view.tolerance();
    let pos_on_grid = pos.mapped_to_grid(grid);
    let grid_distance =
        (pos_on_grid != pos).then(|| (pos - pos_on_grid).length().get() + grid.get() / 100);

    // Grab area tests from the view.
    let to_set = |v: Vec<SchematicItem>| -> BTreeSet<SchematicItem> { v.into_iter().collect() };
    let exact = to_set(view.items_at(pos, Length::ZERO));
    let near = to_set(view.items_at(pos, tolerance));
    let near_large = to_set(view.items_at(pos, tolerance * 2));
    let in_grid = grid_distance.map(|d| to_set(view.items_at(pos, d)));
    let mut candidates: BTreeSet<SchematicItem> = exact
        .iter()
        .chain(&near_large)
        .chain(in_grid.iter().flatten())
        .copied()
        .collect();
    // Junctions are tested on the model.
    let max_distance = grid_distance
        .unwrap_or(Length::ZERO)
        .max(tolerance * 2)
        .max(Length::ZERO);
    let mut junction_hits: BTreeMap<SchematicItem, Length> = BTreeMap::new();
    if flags.bus_junctions {
        for (seg_id, seg) in s.bus_segments() {
            for j in seg.junctions().values() {
                let d = distance_to_square(pos, j.position(), JUNCTION_GRAB_RADIUS);
                if d <= max_distance {
                    let item = SchematicItem::BusJunction(*seg_id, j.uuid());
                    candidates.insert(item);
                    junction_hits.insert(item, d);
                }
            }
        }
    }
    if flags.net_points {
        for (seg_id, seg) in s.net_segments() {
            for j in seg.junctions().values() {
                let d = distance_to_square(pos, j.position(), JUNCTION_GRAB_RADIUS);
                if d <= max_distance {
                    let item = SchematicItem::NetPoint(*seg_id, j.uuid());
                    candidates.insert(item);
                    junction_hits.insert(item, d);
                }
            }
        }
    }

    let hit =
        |item: &SchematicItem, set: &BTreeSet<SchematicItem>, limit: Length| match junction_hits
            .get(item)
        {
            Some(d) => *d <= limit,
            None => set.contains(item),
        };

    let mut result: Vec<((i64, i64), SchematicItem)> = Vec::new();
    // Upstream `processItem()`.
    // `item` is the item whose grab area is tested, `returned` the item
    // returned (upstream: a locked symbol text returns its symbol).
    let mut process = |item: SchematicItem,
                       returned: SchematicItem,
                       nearest: Point,
                       priority: i64,
                       large: bool,
                       max_distance: Option<Length>|
     -> bool {
        if except.contains(&returned) {
            return false;
        }
        let distance = (nearest - pos).length().get();
        if max_distance.is_some_and(|m| distance > m) {
            return false;
        }
        let distance_px = px(distance);
        if hit(&item, &exact, Length::ZERO) {
            result.push(((priority, distance_px), returned));
            return true;
        }
        let (near_set, near_tol) = if large {
            (&near_large, tolerance * 2)
        } else {
            (&near, tolerance)
        };
        if (flags.accept_near_match || flags.accept_nearest_within_grid)
            && hit(&item, near_set, near_tol)
        {
            result.push(((priority + 1000, distance_px), returned));
            return true;
        }
        if flags.accept_nearest_within_grid
            && let (Some(set), Some(d)) = (&in_grid, grid_distance)
            && hit(&item, set, d)
        {
            // Swapped order like upstream!
            result.push(((distance_px + 2000, priority), returned));
            return true;
        }
        false
    };

    for item in candidates {
        if !item_exists(&item, s) {
            continue;
        }
        match item {
            SchematicItem::NetPoint(seg_id, j) if flags.net_points => {
                let seg = &s.net_segments()[&seg_id];
                let position = seg.junctions()[&j].position();
                let visible = seg.is_visible_junction(NetLineAnchor::Junction(j));
                process(
                    item,
                    item,
                    position,
                    if visible { 0 } else { 10 },
                    false,
                    None,
                );
            }
            SchematicItem::NetLine(seg_id, l) if flags.net_lines => {
                let seg = &s.net_segments()[&seg_id];
                let line = &seg.lines()[&l];
                let p = |a| s.net_line_anchor_position(seg_id, a, cx.project().view());
                if let (Some(p1), Some(p2)) = (p(line.p1()), p(line.p2())) {
                    let nearest = toolbox::nearest_point_on_line(pos_on_grid, p1, p2);
                    process(item, item, nearest, 20, true, None);
                }
            }
            SchematicItem::NetLabel(seg_id, l) if flags.net_labels => {
                let label = &s.net_segments()[&seg_id].labels()[&l];
                process(item, item, label.position(), 30, false, None);
            }
            SchematicItem::BusJunction(seg_id, j) if flags.bus_junctions => {
                if let Some(junction) = s.bus_segments()[&seg_id].junctions().get(&j) {
                    process(item, item, junction.position(), 15, false, None);
                }
            }
            SchematicItem::BusLine(seg_id, l) if flags.bus_lines => {
                let seg = &s.bus_segments()[&seg_id];
                let line = &seg.lines()[&l];
                if let (Some(p1), Some(p2)) = (
                    seg.junction_position(line.p1()),
                    seg.junction_position(line.p2()),
                ) {
                    let nearest = toolbox::nearest_point_on_line(pos_on_grid, p1, p2);
                    process(item, item, nearest, 34, true, None);
                }
            }
            SchematicItem::BusLabel(seg_id, l) if flags.bus_labels => {
                if let Some(label) = s.bus_segments()[&seg_id].labels().get(&l) {
                    process(item, item, label.position(), 36, false, None);
                }
            }
            SchematicItem::SymbolPin(symbol, pin) if flags.symbol_pins => {
                if let Some(position) = pin_position(cx, s, symbol, pin) {
                    process(item, item, position, 40, false, None);
                }
            }
            SchematicItem::Symbol(id) if flags.symbols => {
                let position = s.symbols()[&id].position();
                if !process(
                    item,
                    item,
                    position,
                    50,
                    false,
                    Some(SYMBOL_ORIGIN_DISTANCE),
                ) {
                    process(item, item, position, 70, false, None);
                }
            }
            SchematicItem::Polygon(id) if flags.polygons => {
                let nearest = s.polygons()[&id]
                    .path()
                    .calc_nearest_point_between_vertices(pos);
                process(item, item, nearest, 80, true, None);
            }
            SchematicItem::Text(id) if flags.texts => {
                let text = &s.texts()[&id];
                if !text.locked() || cx.settings.ignore_locks {
                    process(item, item, text.position(), 60, false, None);
                }
            }
            SchematicItem::SymbolText(symbol, id) => {
                let text = &s.symbols()[&symbol].texts()[&id];
                if flags.texts && (!text.locked() || cx.settings.ignore_locks) {
                    process(item, item, text.position(), 60, false, None);
                } else if flags.symbols {
                    // A locked text is part of the symbol's grab area.
                    process(
                        item,
                        SchematicItem::Symbol(symbol),
                        text.position(),
                        70,
                        false,
                        None,
                    );
                }
            }
            SchematicItem::Image(id) if flags.images => {
                process(item, item, s.images()[&id].position(), 90, false, None);
            }
            _ => {}
        }
    }
    // Stable sort: equal priorities keep the (deterministic) item order.
    result.sort_by_key(|(prio, _)| *prio);
    let mut seen = BTreeSet::new();
    result
        .into_iter()
        .map(|(_, item)| item)
        .filter(|item| seen.insert(*item))
        .collect()
}

/// Returns the position of a symbol pin.
pub(crate) fn pin_position(
    cx: &Cx<'_, '_>,
    s: &Schematic,
    symbol: librepcb_core::project::SymbolId,
    pin: librepcb_core::types::Uuid,
) -> Option<Point> {
    s.symbols()
        .get(&symbol)?
        .pin(cx.project().view(), pin)
        .ok()
        .flatten()
        .map(|v| v.position())
}
