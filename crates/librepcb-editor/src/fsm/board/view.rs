//! The view a board editor FSM works on (the graphics scene and view parts
//! of upstream `BoardEditorFsmAdapter`) and the hit testing of the board
//! editor states (port of `BoardEditorState::findItemsAtPos()` in
//! libs/librepcb/editor/project/board/fsm/boardeditorstate.{h,cpp}).
//!
//! The application implements [`BoardView`] on its board scene
//! (`librepcb_scene::BoardScene`: grab areas are the scene items, hidden
//! layers are not hit); the priorities of upstream (vias, THT pads, holes,
//! then per side junctions, traces, planes/zones, footprints, pads,
//! polygons/texts; near matches and grid matches after exact ones) are
//! applied here from the model. Junctions are not scene items: their grab
//! area (a circle of the widest trace at them, upstream `BGI_NetPoint`) is
//! computed here.

use std::collections::BTreeMap;

use bitflags::bitflags;
use librepcb_core::geometry::TraceAnchor;
use librepcb_core::project::board::Board;
use librepcb_core::project::{
    BoardId, ComponentInstanceId, NetSegmentId, NetSignalId, PlaneId, Project,
};
use librepcb_core::types::{Layer, Length, Point, PositiveLength, Uuid};
use librepcb_core::utils::toolbox;

/// A selectable item of a board (upstream: the graphics items of
/// `BoardGraphicsScene`). Net segment elements are addressed with their
/// segment; since segments are split and combined by many commands, use
/// [`BoardItemRef::resolved()`] to find the current segment of an element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BoardItemRef {
    /// A device (its footprint).
    Device(ComponentInstanceId),
    /// A footprint pad of a device.
    FootprintPad(ComponentInstanceId, Uuid),
    /// A standalone pad of a net segment.
    Pad(NetSegmentId, Uuid),
    /// A via.
    Via(NetSegmentId, Uuid),
    /// A junction (upstream net point).
    Junction(NetSegmentId, Uuid),
    /// A trace (upstream net line).
    Trace(NetSegmentId, Uuid),
    /// A plane.
    Plane(PlaneId),
    /// A zone of the board.
    Zone(Uuid),
    /// A polygon of the board.
    Polygon(Uuid),
    /// A stroke text of the board.
    StrokeText(Uuid),
    /// A stroke text of a device.
    DeviceStrokeText(ComponentInstanceId, Uuid),
    /// A hole of the board.
    Hole(Uuid),
}

impl BoardItemRef {
    /// Returns the reference with the current net segment of a net segment
    /// element (searched by element UUID if it moved to another segment),
    /// and a device stroke text for a stroke text UUID which is not a board
    /// text. `None` if the item does not exist (anymore).
    pub fn resolved(self, board: &Board) -> Option<Self> {
        use BoardItemRef as R;
        let find = |seg: NetSegmentId,
                    has: &dyn Fn(&librepcb_core::project::board::BoardNetSegment) -> bool|
         -> Option<NetSegmentId> {
            if board.net_segment(seg).is_some_and(has) {
                return Some(seg);
            }
            board
                .net_segments()
                .values()
                .find(|s| has(s))
                .map(|s| s.id())
        };
        match self {
            R::Device(c) => board.device(c).map(|_| self),
            R::FootprintPad(c, _) => board.device(c).map(|_| self),
            R::Pad(s, u) => find(s, &|seg| seg.pads().contains_key(&u)).map(|s| R::Pad(s, u)),
            R::Via(s, u) => find(s, &|seg| seg.vias().contains_key(&u)).map(|s| R::Via(s, u)),
            R::Junction(s, u) => {
                find(s, &|seg| seg.junctions().contains_key(&u)).map(|s| R::Junction(s, u))
            }
            R::Trace(s, u) => find(s, &|seg| seg.traces().contains_key(&u)).map(|s| R::Trace(s, u)),
            R::Plane(p) => board.plane(p).map(|_| self),
            R::Zone(u) => board.zones().contains_key(&u).then_some(self),
            R::Polygon(u) => board.polygons().contains_key(&u).then_some(self),
            R::StrokeText(u) => {
                if board.stroke_texts().contains_key(&u) {
                    Some(self)
                } else {
                    board
                        .devices()
                        .values()
                        .find(|d| d.stroke_texts().contains_key(&u))
                        .map(|d| R::DeviceStrokeText(d.component(), u))
                }
            }
            R::DeviceStrokeText(c, u) => board
                .device(c)
                .filter(|d| d.stroke_texts().contains_key(&u))
                .map(|_| self),
            R::Hole(u) => board.holes().contains_key(&u).then_some(self),
        }
    }

    /// The net segment of a net segment element.
    pub fn segment(self) -> Option<NetSegmentId> {
        match self {
            Self::Pad(s, _) | Self::Via(s, _) | Self::Junction(s, _) | Self::Trace(s, _) => Some(s),
            _ => None,
        }
    }
}

/// What the FSM needs from the view: hit testing on the displayed board
/// scene (upstream `BoardGraphicsScene` and `fsmCalcPosWithTolerance()`),
/// implemented by the application on its `librepcb_scene::BoardScene`
/// (mapping `BoardObject`s to [`BoardItemRef`]s; air wires are not
/// selectable) and by tests on a headless scene.
///
/// Grab areas are the shapes of the scene items (upstream
/// `QGraphicsItem::shape()`); items on hidden layers are not hit. The
/// order of the returned items does not matter. The application updates
/// its scene from the change journal after each FSM call.
pub trait BoardView {
    /// All items whose grab area is at most `tolerance` away from `pos`
    /// (`tolerance == 0`: exactly under `pos`). Junctions need not be
    /// reported.
    fn items_at(&self, pos: Point, tolerance: Length) -> Vec<BoardItemRef>;

    /// All items touching the rectangle spanned by `p1` and `p2`.
    fn items_in_rect(&self, p1: Point, p2: Point) -> Vec<BoardItemRef>;

    /// The hit tolerance: upstream uses 5 screen pixels, so this is the
    /// world length of 5 pixels at the current zoom level (see
    /// [`tolerance_for_pixel_size()`](crate::fsm::tolerance_for_pixel_size)).
    fn tolerance(&self) -> Length;

    /// Whether a board layer is visible (junctions on hidden layers are not
    /// hit).
    fn is_layer_visible(&self, layer: Layer) -> bool;

    /// Whether the board is viewed from the bottom (swaps the hit priority
    /// of the board sides).
    fn is_flipped(&self) -> bool {
        false
    }

    /// The current cursor position in world coordinates, if known (used
    /// when a tool starts without pointer event, e.g. paste or add via;
    /// upstream `QCursor::pos()`).
    fn cursor_pos(&self) -> Option<Point> {
        None
    }
}

bitflags! {
    /// What [`find_items_at_pos()`] looks for (upstream
    /// `BoardEditorState::FindFlag`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct FindFlags: u32 {
        /// Vias.
        const VIAS = 1 << 0;
        /// Junctions.
        const JUNCTIONS = 1 << 1;
        /// Traces.
        const TRACES = 1 << 2;
        /// Devices.
        const DEVICES = 1 << 3;
        /// Footprint pads.
        const FOOTPRINT_PADS = 1 << 4;
        /// Standalone pads.
        const BOARD_PADS = 1 << 5;
        /// Planes.
        const PLANES = 1 << 6;
        /// Zones.
        const ZONES = 1 << 7;
        /// Polygons.
        const POLYGONS = 1 << 8;
        /// Stroke texts.
        const STROKE_TEXTS = 1 << 9;
        /// Holes.
        const HOLES = 1 << 10;
        /// All items.
        const ALL = Self::VIAS.bits() | Self::JUNCTIONS.bits() | Self::TRACES.bits()
            | Self::DEVICES.bits() | Self::FOOTPRINT_PADS.bits() | Self::BOARD_PADS.bits()
            | Self::PLANES.bits() | Self::ZONES.bits() | Self::POLYGONS.bits()
            | Self::STROKE_TEXTS.bits() | Self::HOLES.bits();
        /// Accept items near (not under) the position.
        const ACCEPT_NEAR_MATCH = 1 << 11;
        /// Accept items at the position mapped to the grid.
        const ACCEPT_NEXT_GRID_MATCH = 1 << 12;
        /// Return devices instead of their footprint pads.
        const DEVICES_OF_PADS = 1 << 20;
    }
}

/// Filters of [`find_items_at_pos()`].
#[derive(Debug, Clone, Default)]
pub struct FindFilter<'a> {
    /// Only items on this copper layer (vias spanning it, pads on it).
    pub layer: Option<Layer>,
    /// Only net items of this net (`Some(None)`: without net).
    pub net: Option<Option<NetSignalId>>,
    /// Items to ignore.
    pub except: &'a [BoardItemRef],
}

/// What is known about a hit candidate.
struct Candidate {
    item: BoardItemRef,
    /// Item which is checked for the hit (a footprint pad for a device of
    /// `DEVICES_OF_PADS`).
    check: BoardItemRef,
    nearest: Point,
    priority: i64,
    large: bool,
}

/// The hit test sets of one position.
struct Hits {
    exact: Vec<BoardItemRef>,
    near: Vec<BoardItemRef>,
    near_large: Vec<BoardItemRef>,
    grid: Vec<BoardItemRef>,
}

/// Returns the position of an anchor.
pub(crate) fn anchor_position(
    p: &Project,
    board: &Board,
    seg: NetSegmentId,
    a: TraceAnchor,
) -> Option<Point> {
    let seg = board.net_segment(seg)?;
    board.anchor_position(seg, a, p.library(), p.circuit())
}

/// The grab radius of a junction (half the widest trace at it; upstream
/// `BGI_NetPoint` shape). `None` if no trace is attached.
fn junction_radius(board: &Board, seg: NetSegmentId, junction: Uuid) -> Option<Length> {
    board
        .net_segment(seg)?
        .traces_at(TraceAnchor::Junction(junction))
        .map(|t| *t.width() / 2)
        .max()
}

/// Returns the items at `pos`, the best match first (port of upstream
/// `BoardEditorState::findItemsAtPos()`).
pub fn find_items_at_pos(
    project: &Project,
    board: BoardId,
    view: &dyn BoardView,
    pos: Point,
    flags: FindFlags,
    filter: &FindFilter<'_>,
) -> Vec<BoardItemRef> {
    let Some(b) = project.board(board) else {
        return Vec::new();
    };
    let grid = b.settings().grid_interval;
    let pos_on_grid = pos.mapped_to_grid(grid);
    let tolerance = view.tolerance();
    let large_tolerance = Length::new((tolerance.to_nm() as f64 * 1.5).round() as i64);
    let mut hits = Hits {
        exact: normalize(b, view.items_at(pos, Length::ZERO)),
        near: normalize(b, view.items_at(pos, tolerance)),
        near_large: normalize(b, view.items_at(pos, large_tolerance)),
        grid: if pos_on_grid != pos {
            normalize(b, view.items_at(pos_on_grid, Length::ZERO))
        } else {
            Vec::new()
        },
    };
    // Junctions are hit tested from the model.
    if flags.contains(FindFlags::JUNCTIONS) {
        for seg in b.net_segments().values() {
            for j in seg.junctions().values() {
                let Some(layer) = seg.junction_layer(&j.uuid()) else {
                    continue;
                };
                if !view.is_layer_visible(layer) {
                    continue;
                }
                let Some(radius) = junction_radius(b, seg.id(), j.uuid()) else {
                    continue;
                };
                let r = BoardItemRef::Junction(seg.id(), j.uuid());
                let d = *(j.position() - pos).length();
                if d <= radius {
                    hits.exact.push(r);
                }
                if d <= radius + tolerance {
                    hits.near.push(r);
                }
                if d <= radius + large_tolerance {
                    hits.near_large.push(r);
                }
                if pos_on_grid != pos && *(j.position() - pos_on_grid).length() <= radius {
                    hits.grid.push(r);
                }
            }
        }
    }
    let flipped = view.is_flipped();
    let layer_priority = |layer: Layer| -> i64 {
        if layer.is_top() {
            if flipped { 300 } else { 100 }
        } else if layer.is_inner() {
            200
        } else if layer.is_bottom() {
            if flipped { 100 } else { 300 }
        } else {
            0
        }
    };
    let net_ok = |net: Option<NetSignalId>| filter.net.is_none_or(|n| n == net);
    let mut candidates: BTreeMap<BoardItemRef, Candidate> = BTreeMap::new();
    let all: Vec<BoardItemRef> = hits
        .exact
        .iter()
        .chain(&hits.near)
        .chain(&hits.near_large)
        .chain(&hits.grid)
        .copied()
        .collect();
    for check in all {
        if candidates.values().any(|c| c.check == check) {
            continue;
        }
        let candidate = match check {
            BoardItemRef::Hole(u) if flags.contains(FindFlags::HOLES) => {
                b.holes().get(&u).map(|h| Candidate {
                    item: check,
                    check,
                    nearest: h.path().first().pos,
                    priority: 5,
                    large: false,
                })
            }
            BoardItemRef::Via(s, u) if flags.contains(FindFlags::VIAS) => {
                b.net_segment(s).and_then(|seg| {
                    let via = seg.vias().get(&u)?;
                    (net_ok(seg.net()) && filter.layer.is_none_or(|l| via.is_on_layer(l))).then(
                        || Candidate {
                            item: check,
                            check,
                            nearest: via.position(),
                            priority: 0,
                            large: false,
                        },
                    )
                })
            }
            BoardItemRef::Junction(s, u) if flags.contains(FindFlags::JUNCTIONS) => {
                b.net_segment(s).and_then(|seg| {
                    let j = seg.junctions().get(&u)?;
                    let layer = seg.junction_layer(&u);
                    (net_ok(seg.net()) && filter.layer.is_none_or(|l| Some(l) == layer)).then(
                        || Candidate {
                            item: check,
                            check,
                            nearest: j.position(),
                            priority: 10 + layer.map_or(0, layer_priority),
                            large: false,
                        },
                    )
                })
            }
            BoardItemRef::Trace(s, u) if flags.contains(FindFlags::TRACES) => {
                b.net_segment(s).and_then(|seg| {
                    let t = seg.traces().get(&u)?;
                    if !net_ok(seg.net()) || filter.layer.is_some_and(|l| l != t.layer()) {
                        return None;
                    }
                    let p1 = anchor_position(project, b, s, t.p1())?;
                    let p2 = anchor_position(project, b, s, t.p2())?;
                    Some(Candidate {
                        item: check,
                        check,
                        nearest: toolbox::nearest_point_on_line(pos_on_grid, p1, p2),
                        priority: 20 + layer_priority(t.layer()),
                        large: false,
                    })
                })
            }
            BoardItemRef::Plane(id) if flags.contains(FindFlags::PLANES) => {
                b.plane(id).and_then(|plane| {
                    (net_ok(plane.net()) && filter.layer.is_none_or(|l| l == plane.layer())).then(
                        || Candidate {
                            item: check,
                            check,
                            nearest: plane.outline().calc_nearest_point_between_vertices(pos),
                            priority: 30 + layer_priority(plane.layer()),
                            large: true,
                        },
                    )
                })
            }
            BoardItemRef::Zone(u) if flags.contains(FindFlags::ZONES) => {
                b.zones().get(&u).and_then(|zone| {
                    filter
                        .layer
                        .is_none_or(|l| zone.layers().contains(&l))
                        .then(|| Candidate {
                            item: check,
                            check,
                            nearest: zone.outline().calc_nearest_point_between_vertices(pos),
                            priority: 30
                                + zone
                                    .layers()
                                    .iter()
                                    .next()
                                    .map_or(0, |l| layer_priority(*l)),
                            large: true,
                        })
                })
            }
            BoardItemRef::Device(c) if flags.contains(FindFlags::DEVICES) => {
                b.device(c).map(|d| Candidate {
                    item: check,
                    check,
                    nearest: d.position(),
                    priority: 40 + if d.mirrored() { 300 } else { 100 },
                    large: false,
                })
            }
            BoardItemRef::FootprintPad(c, u) if flags.contains(FindFlags::FOOTPRINT_PADS) => {
                b.device(c).and_then(|d| {
                    let pad = d.pad(&u, project.library(), project.circuit()).ok()??;
                    if !net_ok(pad.net()) || filter.layer.is_some_and(|l| !pad.is_on_layer(l)) {
                        return None;
                    }
                    let item = if flags.contains(FindFlags::DEVICES_OF_PADS) {
                        BoardItemRef::Device(c)
                    } else {
                        check
                    };
                    Some(Candidate {
                        item,
                        check,
                        nearest: pad.position(),
                        priority: if pad.properties().is_tht() {
                            1
                        } else {
                            50 + if pad.mirrored() { 300 } else { 100 }
                        },
                        large: false,
                    })
                })
            }
            BoardItemRef::Pad(s, u) if flags.contains(FindFlags::BOARD_PADS) => {
                b.net_segment(s).and_then(|seg| {
                    let pad = seg.pads().get(&u)?;
                    if !net_ok(seg.net()) || filter.layer.is_some_and(|l| !pad.pad().is_on_layer(l))
                    {
                        return None;
                    }
                    let bottom = pad.pad().component_side()
                        == librepcb_core::geometry::ComponentSide::Bottom;
                    Some(Candidate {
                        item: check,
                        check,
                        nearest: pad.pad().position(),
                        priority: if pad.pad().is_tht() {
                            1
                        } else {
                            50 + if bottom { 300 } else { 100 }
                        },
                        large: false,
                    })
                })
            }
            BoardItemRef::Polygon(u) if flags.contains(FindFlags::POLYGONS) => {
                b.polygons().get(&u).map(|poly| Candidate {
                    item: check,
                    check,
                    nearest: poly.path().calc_nearest_point_between_vertices(pos),
                    priority: 60 + layer_priority(poly.layer()),
                    large: true,
                })
            }
            BoardItemRef::StrokeText(u) if flags.contains(FindFlags::STROKE_TEXTS) => {
                b.stroke_texts().get(&u).map(|t| Candidate {
                    item: check,
                    check,
                    nearest: t.position(),
                    priority: 60 + layer_priority(t.layer()),
                    large: false,
                })
            }
            BoardItemRef::DeviceStrokeText(c, u) if flags.contains(FindFlags::STROKE_TEXTS) => b
                .device(c)
                .and_then(|d| d.stroke_texts().get(&u))
                .map(|t| Candidate {
                    item: check,
                    check,
                    nearest: t.position(),
                    priority: 60 + layer_priority(t.layer()),
                    large: false,
                }),
            _ => None,
        };
        if let Some(c) = candidate
            && !filter.except.contains(&c.item)
        {
            candidates.entry(c.check).or_insert(c);
        }
    }

    // Classify like upstream `processItem()`.
    let accept_near =
        flags.intersects(FindFlags::ACCEPT_NEAR_MATCH | FindFlags::ACCEPT_NEXT_GRID_MATCH);
    let accept_grid = flags.contains(FindFlags::ACCEPT_NEXT_GRID_MATCH) && pos_on_grid != pos;
    let mut result: Vec<((i64, i64), BoardItemRef)> = Vec::new();
    for c in candidates.values() {
        let distance = (*(c.nearest - pos).length()).to_px().round() as i64;
        let key = if hits.exact.contains(&c.check) {
            (c.priority, distance)
        } else if accept_near
            && (if c.large {
                &hits.near_large
            } else {
                &hits.near
            })
            .contains(&c.check)
        {
            (c.priority + 1000, distance)
        } else if accept_grid && hits.grid.contains(&c.check) {
            (distance + 2000, c.priority)
        } else {
            continue;
        };
        result.push((key, c.item));
    }
    result.sort_by_key(|(key, _)| *key);
    let mut items: Vec<BoardItemRef> = Vec::new();
    for (_, item) in result {
        if !items.contains(&item) {
            items.push(item);
        }
    }
    items
}

/// Resolves device texts reported as board texts and removes duplicates.
fn normalize(board: &Board, items: Vec<BoardItemRef>) -> Vec<BoardItemRef> {
    let mut out: Vec<BoardItemRef> = Vec::with_capacity(items.len());
    for item in items {
        let item = match item {
            BoardItemRef::StrokeText(_) => item.resolved(board).unwrap_or(item),
            _ => item,
        };
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

/// A grid interval as length.
pub(crate) fn grid_interval(project: &Project, board: BoardId) -> PositiveLength {
    project
        .board(board)
        .map(|b| b.settings().grid_interval)
        .unwrap_or_else(|| {
            // Invariant: 2.54 mm is a valid positive length.
            PositiveLength::new(Length::new(2_540_000)).expect("positive")
        })
}
