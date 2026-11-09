//! Port of the `ClipperBase` and `Clipper` classes of clipper.cpp (the
//! Vatti clipping algorithm).
//!
//! Function and variable names follow upstream (in snake case) to ease
//! comparison with the original. Raw pointers are replaced by indices:
//! edges index into `Clipper::edges` (one contiguous range per added path),
//! output points into `Clipper::pts`, output records into
//! `Clipper::poly_outs` (upstream `m_PolyOuts`, whose pointer identities are
//! the positions in that list).

use std::collections::BinaryHeap;

use crate::poly_tree::{NodeData, NodeId, PolyTree};
use crate::{
    ClipType, Error, IntPoint, IntRect, Path, Paths, PolyFillType, PolyType, Result, round,
    std_sort,
};

const HORIZONTAL: f64 = -1.0E+40;
/// Edge not currently 'owning' a solution.
const UNASSIGNED: i32 = -1;
/// Edge that would otherwise close a path.
const SKIP: i32 = -2;

const LO_RANGE: i64 = 0x3FFF_FFFF;
const HI_RANGE: i64 = 0x3FFF_FFFF_FFFF_FFFF;

/// Marker for a removed edge (upstream sets `Prev` to null).
const NIL: usize = usize::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EdgeSide {
    /// Zero-initialized (upstream `memset`), neither left nor right.
    Unset,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    RightToLeft,
    LeftToRight,
}

#[derive(Debug, Clone)]
struct TEdge {
    bot: IntPoint,
    /// Current position (updated for every new scanbeam).
    curr: IntPoint,
    top: IntPoint,
    dx: f64,
    poly_typ: PolyType,
    /// Side only refers to current side of solution poly.
    side: EdgeSide,
    /// 1 or -1 depending on winding direction (0 for open paths).
    wind_delta: i32,
    wind_cnt: i32,
    /// Winding count of the opposite polytype.
    wind_cnt2: i32,
    out_idx: i32,
    next: usize,
    prev: usize,
    next_in_lml: Option<usize>,
    next_in_ael: Option<usize>,
    prev_in_ael: Option<usize>,
    next_in_sel: Option<usize>,
    prev_in_sel: Option<usize>,
}

impl TEdge {
    /// Upstream `InitEdge()` (after `memset(0)`).
    fn new(next: usize, prev: usize, pt: IntPoint) -> Self {
        Self {
            bot: IntPoint::default(),
            curr: pt,
            top: IntPoint::default(),
            dx: 0.0,
            poly_typ: PolyType::Subject,
            side: EdgeSide::Unset,
            wind_delta: 0,
            wind_cnt: 0,
            wind_cnt2: 0,
            out_idx: UNASSIGNED,
            next,
            prev,
            next_in_lml: None,
            next_in_ael: None,
            prev_in_ael: None,
            next_in_sel: None,
            prev_in_sel: None,
        }
    }

    fn is_horizontal(&self) -> bool {
        self.dx == HORIZONTAL
    }

    fn set_dx(&mut self) {
        let dy = self.top.y.wrapping_sub(self.bot.y);
        if dy == 0 {
            self.dx = HORIZONTAL;
        } else {
            self.dx = self.top.x.wrapping_sub(self.bot.x) as f64 / dy as f64;
        }
    }

    fn reverse_horizontal(&mut self) {
        std::mem::swap(&mut self.top.x, &mut self.bot.x);
    }
}

#[derive(Debug, Clone, Copy)]
struct IntersectNode {
    edge1: usize,
    edge2: usize,
    pt: IntPoint,
}

#[derive(Debug, Clone, Copy)]
struct LocalMinimum {
    y: i64,
    left_bound: Option<usize>,
    right_bound: Option<usize>,
}

/// A path in the clipping solution.
#[derive(Debug, Clone)]
struct OutRec {
    idx: usize,
    is_hole: bool,
    is_open: bool,
    first_left: Option<usize>,
    poly_nd: Option<usize>,
    pts: Option<usize>,
    bottom_pt: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
struct OutPt {
    idx: usize,
    pt: IntPoint,
    next: usize,
    prev: usize,
}

#[derive(Debug, Clone, Copy)]
struct Join {
    out_pt1: usize,
    /// `None` for ghost joins.
    out_pt2: Option<usize>,
    off_pt: IntPoint,
}

/// Status of `execute_internal()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Succeeded,
    /// Nothing to do (upstream returns `false` without an error).
    Empty,
    Failed,
}

/// Internal error (upstream exception thrown during execution, which is
/// caught by `ExecuteInternal()`).
struct Failure;

type Flow<T = ()> = std::result::Result<T, Failure>;

/// The polygon clipper (upstream `Clipper`, including `ClipperBase`).
///
/// Add subject and clip paths with [`add_path()`](Self::add_path) /
/// [`add_paths()`](Self::add_paths), then run a boolean operation with
/// [`execute()`](Self::execute) or [`execute_tree()`](Self::execute_tree).
/// The paths are kept, so multiple operations can be executed on the same
/// input.
#[derive(Debug, Clone)]
pub struct Clipper {
    // ClipperBase
    minima_list: Vec<LocalMinimum>,
    current_lm: usize,
    use_full_range: bool,
    edges: Vec<TEdge>,
    preserve_collinear: bool,
    has_open_paths: bool,
    poly_outs: Vec<OutRec>,
    pts: Vec<OutPt>,
    active_edges: Option<usize>,
    scanbeam: BinaryHeap<i64>,
    // Clipper
    joins: Vec<Join>,
    ghost_joins: Vec<Join>,
    intersect_list: Vec<IntersectNode>,
    clip_type: ClipType,
    maxima: Vec<i64>,
    sorted_edges: Option<usize>,
    clip_fill_type: PolyFillType,
    subj_fill_type: PolyFillType,
    reverse_output: bool,
    using_poly_tree: bool,
    strict_simple: bool,
}

impl Default for Clipper {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Free helper functions
// ---------------------------------------------------------------------------

/// Upstream `SlopesEqual(pt1, pt2, pt3)`. Always evaluated with 128 bit
/// integers, which gives the same result as upstream's 64 bit math (used
/// when all coordinates are within `LO_RANGE`, where it cannot overflow).
fn slopes_equal3(pt1: IntPoint, pt2: IntPoint, pt3: IntPoint) -> bool {
    slopes_equal4(pt1, pt2, pt2, pt3)
}

/// Upstream `SlopesEqual(pt1, pt2, pt3, pt4)`.
fn slopes_equal4(pt1: IntPoint, pt2: IntPoint, pt3: IntPoint, pt4: IntPoint) -> bool {
    let a = i128::from(pt1.y.wrapping_sub(pt2.y)) * i128::from(pt3.x.wrapping_sub(pt4.x));
    let b = i128::from(pt1.x.wrapping_sub(pt2.x)) * i128::from(pt3.y.wrapping_sub(pt4.y));
    a == b
}

/// Upstream `SlopesEqual(e1, e2)`.
fn slopes_equal_edges(e1: &TEdge, e2: &TEdge) -> bool {
    let a =
        i128::from(e1.top.y.wrapping_sub(e1.bot.y)) * i128::from(e2.top.x.wrapping_sub(e2.bot.x));
    let b =
        i128::from(e1.top.x.wrapping_sub(e1.bot.x)) * i128::from(e2.top.y.wrapping_sub(e2.bot.y));
    a == b
}

fn get_dx(pt1: IntPoint, pt2: IntPoint) -> f64 {
    if pt1.y == pt2.y {
        HORIZONTAL
    } else {
        pt2.x.wrapping_sub(pt1.x) as f64 / pt2.y.wrapping_sub(pt1.y) as f64
    }
}

fn top_x(edge: &TEdge, current_y: i64) -> i64 {
    if current_y == edge.top.y {
        edge.top.x
    } else {
        edge.bot
            .x
            .wrapping_add(round(edge.dx * current_y.wrapping_sub(edge.bot.y) as f64))
    }
}

fn intersect_point(edge1: &TEdge, edge2: &TEdge) -> IntPoint {
    let mut ip = IntPoint::default();
    if edge1.dx == edge2.dx {
        ip.y = edge1.curr.y;
        ip.x = top_x(edge1, ip.y);
        return ip;
    } else if edge1.dx == 0.0 {
        ip.x = edge1.bot.x;
        if edge2.is_horizontal() {
            ip.y = edge2.bot.y;
        } else {
            let b2 = edge2.bot.y as f64 - (edge2.bot.x as f64 / edge2.dx);
            ip.y = round(ip.x as f64 / edge2.dx + b2);
        }
    } else if edge2.dx == 0.0 {
        ip.x = edge2.bot.x;
        if edge1.is_horizontal() {
            ip.y = edge1.bot.y;
        } else {
            let b1 = edge1.bot.y as f64 - (edge1.bot.x as f64 / edge1.dx);
            ip.y = round(ip.x as f64 / edge1.dx + b1);
        }
    } else {
        let b1 = edge1.bot.x as f64 - edge1.bot.y as f64 * edge1.dx;
        let b2 = edge2.bot.x as f64 - edge2.bot.y as f64 * edge2.dx;
        let q = (b2 - b1) / (edge1.dx - edge2.dx);
        ip.y = round(q);
        if edge1.dx.abs() < edge2.dx.abs() {
            ip.x = round(edge1.dx * q + b1);
        } else {
            ip.x = round(edge2.dx * q + b2);
        }
    }

    if ip.y < edge1.top.y || ip.y < edge2.top.y {
        if edge1.top.y > edge2.top.y {
            ip.y = edge1.top.y;
        } else {
            ip.y = edge2.top.y;
        }
        if edge1.dx.abs() < edge2.dx.abs() {
            ip.x = top_x(edge1, ip.y);
        } else {
            ip.x = top_x(edge2, ip.y);
        }
    }
    // Finally, don't allow 'ip' to be BELOW curr.y (ie bottom of scanbeam).
    if ip.y > edge1.curr.y {
        ip.y = edge1.curr.y;
        // Use the more vertical edge to derive X.
        if edge1.dx.abs() > edge2.dx.abs() {
            ip.x = top_x(edge2, ip.y);
        } else {
            ip.x = top_x(edge1, ip.y);
        }
    }
    ip
}

fn pt2_is_between_pt1_and_pt3(pt1: IntPoint, pt2: IntPoint, pt3: IntPoint) -> bool {
    if pt1 == pt3 || pt1 == pt2 || pt3 == pt2 {
        false
    } else if pt1.x != pt3.x {
        (pt2.x > pt1.x) == (pt2.x < pt3.x)
    } else {
        (pt2.y > pt1.y) == (pt2.y < pt3.y)
    }
}

fn horz_segments_overlap(mut seg1a: i64, mut seg1b: i64, mut seg2a: i64, mut seg2b: i64) -> bool {
    if seg1a > seg1b {
        std::mem::swap(&mut seg1a, &mut seg1b);
    }
    if seg2a > seg2b {
        std::mem::swap(&mut seg2a, &mut seg2b);
    }
    (seg1a < seg2b) && (seg2a < seg1b)
}

fn range_test(pt: IntPoint, use_full_range: &mut bool) -> Result<()> {
    if *use_full_range {
        if pt.x > HI_RANGE
            || pt.y > HI_RANGE
            || pt.x.wrapping_neg() > HI_RANGE
            || pt.y.wrapping_neg() > HI_RANGE
        {
            return Err(Error::CoordinateOutOfRange);
        }
    } else if pt.x > LO_RANGE
        || pt.y > LO_RANGE
        || pt.x.wrapping_neg() > LO_RANGE
        || pt.y.wrapping_neg() > LO_RANGE
    {
        *use_full_range = true;
        return range_test(pt, use_full_range);
    }
    Ok(())
}

fn get_overlap(a1: i64, a2: i64, b1: i64, b2: i64) -> Option<(i64, i64)> {
    let (left, right) = if a1 < a2 {
        if b1 < b2 {
            (a1.max(b1), a2.min(b2))
        } else {
            (a1.max(b2), a2.min(b1))
        }
    } else if b1 < b2 {
        (a2.max(b1), a1.min(b2))
    } else {
        (a2.max(b2), a1.min(b1))
    };
    (left < right).then_some((left, right))
}

fn e2_inserts_before_e1(e1: &TEdge, e2: &TEdge) -> bool {
    if e2.curr.x == e1.curr.x {
        if e2.top.y > e1.top.y {
            e2.top.x < top_x(e1, e2.top.y)
        } else {
            e1.top.x > top_x(e2, e1.top.y)
        }
    } else {
        e2.curr.x < e1.curr.x
    }
}

// ---------------------------------------------------------------------------
// Output point (OutPt ring) helpers
// ---------------------------------------------------------------------------

/// Upstream `Area(const OutPt*)`. Note the integer addition before the
/// conversion to double, unlike [`crate::area()`].
fn area_op(pts: &[OutPt], op: Option<usize>) -> f64 {
    let Some(start) = op else {
        return 0.0;
    };
    let mut a = 0.0;
    let mut op = start;
    loop {
        let prev = &pts[pts[op].prev];
        let cur = &pts[op];
        a += prev.pt.x.wrapping_add(cur.pt.x) as f64 * prev.pt.y.wrapping_sub(cur.pt.y) as f64;
        op = cur.next;
        if op == start {
            break;
        }
    }
    a * 0.5
}

/// Upstream `PointInPolygon(const IntPoint&, OutPt*)`.
fn point_in_polygon_op(pt: IntPoint, pts: &[OutPt], mut op: usize) -> i32 {
    // Returns 0 if false, +1 if true, -1 if pt ON polygon boundary.
    let mut result = 0;
    let start_op = op;
    loop {
        let cur = pts[op].pt;
        let next = pts[pts[op].next].pt;
        if next.y == pt.y
            && (next.x == pt.x || (cur.y == pt.y && ((next.x > pt.x) == (cur.x < pt.x))))
        {
            return -1;
        }
        if (cur.y < pt.y) != (next.y < pt.y) {
            if cur.x >= pt.x {
                if next.x > pt.x {
                    result = 1 - result;
                } else {
                    let d = cur.x.wrapping_sub(pt.x) as f64 * next.y.wrapping_sub(pt.y) as f64
                        - next.x.wrapping_sub(pt.x) as f64 * cur.y.wrapping_sub(pt.y) as f64;
                    if d == 0.0 {
                        return -1;
                    }
                    if (d > 0.0) == (next.y > cur.y) {
                        result = 1 - result;
                    }
                }
            } else if next.x > pt.x {
                let d = cur.x.wrapping_sub(pt.x) as f64 * next.y.wrapping_sub(pt.y) as f64
                    - next.x.wrapping_sub(pt.x) as f64 * cur.y.wrapping_sub(pt.y) as f64;
                if d == 0.0 {
                    return -1;
                }
                if (d > 0.0) == (next.y > cur.y) {
                    result = 1 - result;
                }
            }
        }
        op = pts[op].next;
        if start_op == op {
            break;
        }
    }
    result
}

fn poly2_contains_poly1(pts: &[OutPt], out_pt1: usize, out_pt2: usize) -> bool {
    let mut op = out_pt1;
    loop {
        // PointInPolygon returns 0 if false, +1 if true, -1 if pt on polygon.
        let res = point_in_polygon_op(pts[op].pt, pts, out_pt2);
        if res >= 0 {
            return res > 0;
        }
        op = pts[op].next;
        if op == out_pt1 {
            break;
        }
    }
    true
}

fn reverse_poly_pt_links(pts: &mut [OutPt], pp: Option<usize>) {
    let Some(pp) = pp else {
        return;
    };
    let mut pp1 = pp;
    loop {
        let pt = &mut pts[pp1];
        std::mem::swap(&mut pt.next, &mut pt.prev);
        pp1 = pt.prev;
        if pp1 == pp {
            break;
        }
    }
}

fn first_is_bottom_pt(pts: &[OutPt], btm_pt1: usize, btm_pt2: usize) -> bool {
    let mut p = pts[btm_pt1].prev;
    while pts[p].pt == pts[btm_pt1].pt && p != btm_pt1 {
        p = pts[p].prev;
    }
    let dx1p = get_dx(pts[btm_pt1].pt, pts[p].pt).abs();
    p = pts[btm_pt1].next;
    while pts[p].pt == pts[btm_pt1].pt && p != btm_pt1 {
        p = pts[p].next;
    }
    let dx1n = get_dx(pts[btm_pt1].pt, pts[p].pt).abs();

    p = pts[btm_pt2].prev;
    while pts[p].pt == pts[btm_pt2].pt && p != btm_pt2 {
        p = pts[p].prev;
    }
    let dx2p = get_dx(pts[btm_pt2].pt, pts[p].pt).abs();
    p = pts[btm_pt2].next;
    while pts[p].pt == pts[btm_pt2].pt && p != btm_pt2 {
        p = pts[p].next;
    }
    let dx2n = get_dx(pts[btm_pt2].pt, pts[p].pt).abs();

    if max_f64(dx1p, dx1n) == max_f64(dx2p, dx2n) && min_f64(dx1p, dx1n) == min_f64(dx2p, dx2n) {
        area_op(pts, Some(btm_pt1)) > 0.0 // If otherwise identical use orientation.
    } else {
        (dx1p >= dx2p && dx1p >= dx2n) || (dx1n >= dx2p && dx1n >= dx2n)
    }
}

/// `std::max()` for doubles (returns `a` unless `a < b`).
fn max_f64(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}

/// `std::min()` for doubles (returns `a` unless `b < a`).
fn min_f64(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

fn get_bottom_pt(pts: &[OutPt], mut pp: usize) -> usize {
    let mut dups: Option<usize> = None;
    let mut p = pts[pp].next;
    while p != pp {
        if pts[p].pt.y > pts[pp].pt.y {
            pp = p;
            dups = None;
        } else if pts[p].pt.y == pts[pp].pt.y && pts[p].pt.x <= pts[pp].pt.x {
            if pts[p].pt.x < pts[pp].pt.x {
                dups = None;
                pp = p;
            } else if pts[p].next != pp && pts[p].prev != pp {
                dups = Some(p);
            }
        }
        p = pts[p].next;
    }
    if let Some(mut d) = dups {
        // There appears to be at least 2 vertices at BottomPt so...
        while d != p {
            if !first_is_bottom_pt(pts, p, d) {
                pp = d;
            }
            d = pts[d].next;
            while pts[d].pt != pts[pp].pt {
                d = pts[d].next;
            }
        }
    }
    pp
}

fn point_count(pts: &[OutPt], start: Option<usize>) -> usize {
    let Some(start) = start else {
        return 0;
    };
    let mut result = 0;
    let mut p = start;
    loop {
        result += 1;
        p = pts[p].next;
        if p == start {
            break;
        }
    }
    result
}

fn dup_out_pt(pts: &mut Vec<OutPt>, out_pt: usize, insert_after: bool) -> usize {
    let result = pts.len();
    let op = pts[out_pt];
    if insert_after {
        pts.push(OutPt {
            idx: op.idx,
            pt: op.pt,
            next: op.next,
            prev: out_pt,
        });
        pts[op.next].prev = result;
        pts[out_pt].next = result;
    } else {
        pts.push(OutPt {
            idx: op.idx,
            pt: op.pt,
            next: out_pt,
            prev: op.prev,
        });
        pts[op.prev].next = result;
        pts[out_pt].prev = result;
    }
    result
}

fn join_horz(
    pts: &mut Vec<OutPt>,
    mut op1: usize,
    op1b: usize,
    mut op2: usize,
    op2b: usize,
    pt: IntPoint,
    discard_left: bool,
) -> bool {
    let dir1 = if pts[op1].pt.x > pts[op1b].pt.x {
        Direction::RightToLeft
    } else {
        Direction::LeftToRight
    };
    let dir2 = if pts[op2].pt.x > pts[op2b].pt.x {
        Direction::RightToLeft
    } else {
        Direction::LeftToRight
    };
    if dir1 == dir2 {
        return false;
    }

    // When DiscardLeft, we want Op1b to be on the Left of Op1, otherwise we
    // want Op1b to be on the Right. (And likewise with Op2 and Op2b.)
    // So, to facilitate this while inserting Op1b and Op2b ...
    // when DiscardLeft, make sure we're AT or RIGHT of Pt before adding Op1b,
    // otherwise make sure we're AT or LEFT of Pt. (Likewise with Op2b.)
    let mut op1b;
    if dir1 == Direction::LeftToRight {
        while pts[pts[op1].next].pt.x <= pt.x
            && pts[pts[op1].next].pt.x >= pts[op1].pt.x
            && pts[pts[op1].next].pt.y == pt.y
        {
            op1 = pts[op1].next;
        }
        if discard_left && pts[op1].pt.x != pt.x {
            op1 = pts[op1].next;
        }
        op1b = dup_out_pt(pts, op1, !discard_left);
        if pts[op1b].pt != pt {
            op1 = op1b;
            pts[op1].pt = pt;
            op1b = dup_out_pt(pts, op1, !discard_left);
        }
    } else {
        while pts[pts[op1].next].pt.x >= pt.x
            && pts[pts[op1].next].pt.x <= pts[op1].pt.x
            && pts[pts[op1].next].pt.y == pt.y
        {
            op1 = pts[op1].next;
        }
        if !discard_left && pts[op1].pt.x != pt.x {
            op1 = pts[op1].next;
        }
        op1b = dup_out_pt(pts, op1, discard_left);
        if pts[op1b].pt != pt {
            op1 = op1b;
            pts[op1].pt = pt;
            op1b = dup_out_pt(pts, op1, discard_left);
        }
    }

    let mut op2b;
    if dir2 == Direction::LeftToRight {
        while pts[pts[op2].next].pt.x <= pt.x
            && pts[pts[op2].next].pt.x >= pts[op2].pt.x
            && pts[pts[op2].next].pt.y == pt.y
        {
            op2 = pts[op2].next;
        }
        if discard_left && pts[op2].pt.x != pt.x {
            op2 = pts[op2].next;
        }
        op2b = dup_out_pt(pts, op2, !discard_left);
        if pts[op2b].pt != pt {
            op2 = op2b;
            pts[op2].pt = pt;
            op2b = dup_out_pt(pts, op2, !discard_left);
        }
    } else {
        while pts[pts[op2].next].pt.x >= pt.x
            && pts[pts[op2].next].pt.x <= pts[op2].pt.x
            && pts[pts[op2].next].pt.y == pt.y
        {
            op2 = pts[op2].next;
        }
        if !discard_left && pts[op2].pt.x != pt.x {
            op2 = pts[op2].next;
        }
        op2b = dup_out_pt(pts, op2, discard_left);
        if pts[op2b].pt != pt {
            op2 = op2b;
            pts[op2].pt = pt;
            op2b = dup_out_pt(pts, op2, discard_left);
        }
    }

    if (dir1 == Direction::LeftToRight) == discard_left {
        pts[op1].prev = op2;
        pts[op2].next = op1;
        pts[op1b].next = op2b;
        pts[op2b].prev = op1b;
    } else {
        pts[op1].next = op2;
        pts[op2].prev = op1;
        pts[op1b].prev = op2b;
        pts[op2b].next = op1b;
    }
    true
}

// ---------------------------------------------------------------------------
// ClipperBase
// ---------------------------------------------------------------------------

impl Clipper {
    /// Creates a clipper without paths (upstream `Clipper(0)`).
    pub fn new() -> Self {
        Self {
            minima_list: Vec::new(),
            current_lm: 0,
            use_full_range: false,
            edges: Vec::new(),
            preserve_collinear: false,
            has_open_paths: false,
            poly_outs: Vec::new(),
            pts: Vec::new(),
            active_edges: None,
            scanbeam: BinaryHeap::new(),
            joins: Vec::new(),
            ghost_joins: Vec::new(),
            intersect_list: Vec::new(),
            clip_type: ClipType::Intersection,
            maxima: Vec::new(),
            sorted_edges: None,
            clip_fill_type: PolyFillType::EvenOdd,
            subj_fill_type: PolyFillType::EvenOdd,
            reverse_output: false,
            using_poly_tree: false,
            strict_simple: false,
        }
    }

    /// Returns whether the orientation of the solution is reversed.
    pub fn reverse_solution(&self) -> bool {
        self.reverse_output
    }

    /// Sets whether the orientation of the solution is reversed (outlines
    /// clockwise instead of counter-clockwise).
    pub fn set_reverse_solution(&mut self, value: bool) {
        self.reverse_output = value;
    }

    /// Returns whether the solution is made strictly simple.
    pub fn strictly_simple(&self) -> bool {
        self.strict_simple
    }

    /// Sets whether the solution is made strictly simple (no touching
    /// vertices).
    pub fn set_strictly_simple(&mut self, value: bool) {
        self.strict_simple = value;
    }

    /// Returns whether collinear vertices are preserved.
    pub fn preserve_collinear(&self) -> bool {
        self.preserve_collinear
    }

    /// Sets whether collinear vertices are preserved.
    pub fn set_preserve_collinear(&mut self, value: bool) {
        self.preserve_collinear = value;
    }

    fn next(&self, e: usize) -> usize {
        self.edges[e].next
    }

    fn prev(&self, e: usize) -> usize {
        self.edges[e].prev
    }

    fn is_horz(&self, e: usize) -> bool {
        self.edges[e].is_horizontal()
    }

    /// Upstream `RemoveEdge()`: removes `e` from the double linked list (but
    /// not from memory) and returns the next edge.
    fn remove_edge(&mut self, e: usize) -> usize {
        let (prev, next) = (self.edges[e].prev, self.edges[e].next);
        self.edges[prev].next = next;
        self.edges[next].prev = prev;
        self.edges[e].prev = NIL; // Flag as removed.
        next
    }

    fn init_edge2(&mut self, e: usize, pt: PolyType) {
        let next_curr = self.edges[self.edges[e].next].curr;
        let edge = &mut self.edges[e];
        if edge.curr.y >= next_curr.y {
            edge.bot = edge.curr;
            edge.top = next_curr;
        } else {
            edge.top = edge.curr;
            edge.bot = next_curr;
        }
        edge.set_dx();
        edge.poly_typ = pt;
    }

    fn find_next_loc_min(&self, mut e: usize) -> usize {
        loop {
            while self.edges[e].bot != self.edges[self.prev(e)].bot
                || self.edges[e].curr == self.edges[e].top
            {
                e = self.next(e);
            }
            if !self.is_horz(e) && !self.is_horz(self.prev(e)) {
                break;
            }
            while self.is_horz(self.prev(e)) {
                e = self.prev(e);
            }
            let e2 = e;
            while self.is_horz(e) {
                e = self.next(e);
            }
            if self.edges[e].top.y == self.edges[self.prev(e)].bot.y {
                continue; // Ie just an intermediate horz.
            }
            if self.edges[self.prev(e2)].bot.x < self.edges[e].bot.x {
                e = e2;
            }
            break;
        }
        e
    }

    fn process_bound(&mut self, mut e: usize, next_is_forward: bool) -> usize {
        let mut result = e;

        if self.edges[e].out_idx == SKIP {
            // If edges still remain in the current bound beyond the skip edge
            // then create another LocMin and call ProcessBound once more.
            if next_is_forward {
                while self.edges[e].top.y == self.edges[self.next(e)].bot.y {
                    e = self.next(e);
                }
                // Don't include top horizontals when parsing a bound a second
                // time, they will be contained in the opposite bound.
                while e != result && self.is_horz(e) {
                    e = self.prev(e);
                }
            } else {
                while self.edges[e].top.y == self.edges[self.prev(e)].bot.y {
                    e = self.prev(e);
                }
                while e != result && self.is_horz(e) {
                    e = self.next(e);
                }
            }

            if e == result {
                result = if next_is_forward {
                    self.next(e)
                } else {
                    self.prev(e)
                };
            } else {
                // There are more edges in the bound beyond result starting
                // with E.
                e = if next_is_forward {
                    self.next(result)
                } else {
                    self.prev(result)
                };
                let loc_min = LocalMinimum {
                    y: self.edges[e].bot.y,
                    left_bound: None,
                    right_bound: Some(e),
                };
                self.edges[e].wind_delta = 0;
                result = self.process_bound(e, next_is_forward);
                self.minima_list.push(loc_min);
            }
            return result;
        }

        if self.is_horz(e) {
            // We need to be careful with open paths because this may not be a
            // true local minima (ie E may be following a skip edge).
            // Also, consecutive horz. edges may start heading left before
            // going right.
            let e_start = if next_is_forward {
                self.prev(e)
            } else {
                self.next(e)
            };
            if self.is_horz(e_start) {
                // Ie an adjoining horizontal skip edge.
                if self.edges[e_start].bot.x != self.edges[e].bot.x
                    && self.edges[e_start].top.x != self.edges[e].bot.x
                {
                    self.edges[e].reverse_horizontal();
                }
            } else if self.edges[e_start].bot.x != self.edges[e].bot.x {
                self.edges[e].reverse_horizontal();
            }
        }

        let e_start = e;
        if next_is_forward {
            while self.edges[result].top.y == self.edges[self.next(result)].bot.y
                && self.edges[self.next(result)].out_idx != SKIP
            {
                result = self.next(result);
            }
            if self.is_horz(result) && self.edges[self.next(result)].out_idx != SKIP {
                // Nb: at the top of a bound, horizontals are added to the
                // bound only when the preceding edge attaches to the
                // horizontal's left vertex unless a Skip edge is encountered
                // when that becomes the top divide.
                let mut horz = result;
                while self.is_horz(self.prev(horz)) {
                    horz = self.prev(horz);
                }
                if self.edges[self.prev(horz)].top.x > self.edges[self.next(result)].top.x {
                    result = self.prev(horz);
                }
            }
            while e != result {
                self.edges[e].next_in_lml = Some(self.next(e));
                if self.is_horz(e)
                    && e != e_start
                    && self.edges[e].bot.x != self.edges[self.prev(e)].top.x
                {
                    self.edges[e].reverse_horizontal();
                }
                e = self.next(e);
            }
            if self.is_horz(e)
                && e != e_start
                && self.edges[e].bot.x != self.edges[self.prev(e)].top.x
            {
                self.edges[e].reverse_horizontal();
            }
            result = self.next(result); // Move to the edge just beyond current bound.
        } else {
            while self.edges[result].top.y == self.edges[self.prev(result)].bot.y
                && self.edges[self.prev(result)].out_idx != SKIP
            {
                result = self.prev(result);
            }
            if self.is_horz(result) && self.edges[self.prev(result)].out_idx != SKIP {
                let mut horz = result;
                while self.is_horz(self.next(horz)) {
                    horz = self.next(horz);
                }
                if self.edges[self.next(horz)].top.x == self.edges[self.prev(result)].top.x
                    || self.edges[self.next(horz)].top.x > self.edges[self.prev(result)].top.x
                {
                    result = self.next(horz);
                }
            }

            while e != result {
                self.edges[e].next_in_lml = Some(self.prev(e));
                if self.is_horz(e)
                    && e != e_start
                    && self.edges[e].bot.x != self.edges[self.next(e)].top.x
                {
                    self.edges[e].reverse_horizontal();
                }
                e = self.prev(e);
            }
            if self.is_horz(e)
                && e != e_start
                && self.edges[e].bot.x != self.edges[self.next(e)].top.x
            {
                self.edges[e].reverse_horizontal();
            }
            result = self.prev(result); // Move to the edge just beyond current bound.
        }

        result
    }

    /// Adds a path (upstream `AddPath()`).
    ///
    /// Returns `Ok(false)` if the path was ignored because it is degenerate
    /// (e.g. less than 3 distinct vertices for closed paths). Open paths
    /// (`closed = false`) must be subject paths.
    pub fn add_path(&mut self, pg: &[IntPoint], poly_typ: PolyType, closed: bool) -> Result<bool> {
        if !closed && poly_typ == PolyType::Clip {
            return Err(Error::OpenPathMustBeSubject);
        }

        let mut high_i = pg.len() as isize - 1;
        if closed {
            while high_i > 0 && pg[high_i as usize] == pg[0] {
                high_i -= 1;
            }
        }
        while high_i > 0 && pg[high_i as usize] == pg[high_i as usize - 1] {
            high_i -= 1;
        }
        if (closed && high_i < 2) || (!closed && high_i < 1) {
            return Ok(false);
        }
        let high_i = high_i as usize;

        // Create a new edge array.
        let base = self.edges.len();
        let mut is_flat = true;

        // 1. Basic (first) edge initialization.
        let range_ok = range_test(pg[0], &mut self.use_full_range)
            .and_then(|()| range_test(pg[high_i], &mut self.use_full_range))
            .and_then(|()| {
                (1..high_i)
                    .rev()
                    .try_for_each(|i| range_test(pg[i], &mut self.use_full_range))
            });
        range_ok?; // Range test fails.
        for (i, pt) in pg.iter().enumerate().take(high_i + 1) {
            let next = if i == high_i { base } else { base + i + 1 };
            let prev = if i == 0 { base + high_i } else { base + i - 1 };
            self.edges.push(TEdge::new(next, prev, *pt));
        }
        let mut e_start = base;

        // 2. Remove duplicate vertices, and (when closed) collinear edges.
        let mut e = e_start;
        let mut e_loop_stop = e_start;
        loop {
            // Nb: allows matching start and end points when not Closed.
            if self.edges[e].curr == self.edges[self.next(e)].curr
                && (closed || self.next(e) != e_start)
            {
                if e == self.next(e) {
                    break;
                }
                if e == e_start {
                    e_start = self.next(e);
                }
                e = self.remove_edge(e);
                e_loop_stop = e;
                continue;
            }
            if self.prev(e) == self.next(e) {
                break; // Only two vertices.
            } else if closed
                && slopes_equal3(
                    self.edges[self.prev(e)].curr,
                    self.edges[e].curr,
                    self.edges[self.next(e)].curr,
                )
                && (!self.preserve_collinear
                    || !pt2_is_between_pt1_and_pt3(
                        self.edges[self.prev(e)].curr,
                        self.edges[e].curr,
                        self.edges[self.next(e)].curr,
                    ))
            {
                // Collinear edges are allowed for open paths but in closed
                // paths the default is to merge adjacent collinear edges into
                // a single edge. However, if the PreserveCollinear property is
                // enabled, only overlapping collinear edges (ie spikes) will
                // be removed from closed paths.
                if e == e_start {
                    e_start = self.next(e);
                }
                e = self.remove_edge(e);
                e = self.prev(e);
                e_loop_stop = e;
                continue;
            }
            e = self.next(e);
            if e == e_loop_stop || (!closed && self.next(e) == e_start) {
                break;
            }
        }

        if (!closed && e == self.next(e)) || (closed && self.prev(e) == self.next(e)) {
            self.edges.truncate(base);
            return Ok(false);
        }

        if !closed {
            self.has_open_paths = true;
            let p = self.prev(e_start);
            self.edges[p].out_idx = SKIP;
        }

        // 3. Do second stage of edge initialization.
        e = e_start;
        loop {
            self.init_edge2(e, poly_typ);
            e = self.next(e);
            if is_flat && self.edges[e].curr.y != self.edges[e_start].curr.y {
                is_flat = false;
            }
            if e == e_start {
                break;
            }
        }

        // 4. Finally, add edge bounds to LocalMinima list.

        // Totally flat paths must be handled differently when adding them to
        // LocalMinima list to avoid endless loops etc.
        if is_flat {
            if closed {
                self.edges.truncate(base);
                return Ok(false);
            }
            let p = self.prev(e);
            self.edges[p].out_idx = SKIP;
            let loc_min = LocalMinimum {
                y: self.edges[e].bot.y,
                left_bound: None,
                right_bound: Some(e),
            };
            self.edges[e].side = EdgeSide::Right;
            self.edges[e].wind_delta = 0;
            loop {
                if self.edges[e].bot.x != self.edges[self.prev(e)].top.x {
                    self.edges[e].reverse_horizontal();
                }
                if self.edges[self.next(e)].out_idx == SKIP {
                    break;
                }
                self.edges[e].next_in_lml = Some(self.next(e));
                e = self.next(e);
            }
            self.minima_list.push(loc_min);
            return Ok(true);
        }

        let mut e_min: Option<usize> = None;

        // Workaround to avoid an endless loop in the while loop below when
        // open paths have matching start and end points.
        let p = self.prev(e);
        if self.edges[p].bot == self.edges[p].top {
            e = self.next(e);
        }

        loop {
            e = self.find_next_loc_min(e);
            if Some(e) == e_min {
                break;
            } else if e_min.is_none() {
                e_min = Some(e);
            }

            // E and E.Prev now share a local minima (left aligned if
            // horizontal). Compare their slopes to find which starts which
            // bound.
            let (left, right, left_bound_is_forward) =
                if self.edges[e].dx < self.edges[self.prev(e)].dx {
                    (self.prev(e), e, false) // Q.nextInLML = Q.prev
                } else {
                    (e, self.prev(e), true) // Q.nextInLML = Q.next
                };
            let mut loc_min = LocalMinimum {
                y: self.edges[e].bot.y,
                left_bound: Some(left),
                right_bound: Some(right),
            };

            if !closed {
                self.edges[left].wind_delta = 0;
            } else if self.next(left) == right {
                self.edges[left].wind_delta = -1;
            } else {
                self.edges[left].wind_delta = 1;
            }
            self.edges[right].wind_delta = -self.edges[left].wind_delta;

            e = self.process_bound(left, left_bound_is_forward);
            if self.edges[e].out_idx == SKIP {
                e = self.process_bound(e, left_bound_is_forward);
            }

            let mut e2 = self.process_bound(right, !left_bound_is_forward);
            if self.edges[e2].out_idx == SKIP {
                e2 = self.process_bound(e2, !left_bound_is_forward);
            }

            if self.edges[left].out_idx == SKIP {
                loc_min.left_bound = None;
            } else if self.edges[right].out_idx == SKIP {
                loc_min.right_bound = None;
            }
            self.minima_list.push(loc_min);
            if !left_bound_is_forward {
                e = e2;
            }
        }
        Ok(true)
    }

    /// Adds multiple paths (upstream `AddPaths()`). Returns whether at least
    /// one path was added.
    pub fn add_paths(&mut self, ppg: &[Path], poly_typ: PolyType, closed: bool) -> Result<bool> {
        let mut result = false;
        for pg in ppg {
            if self.add_path(pg, poly_typ, closed)? {
                result = true;
            }
        }
        Ok(result)
    }

    /// Removes all paths (upstream `Clear()`).
    pub fn clear(&mut self) {
        self.minima_list.clear();
        self.current_lm = 0;
        self.edges.clear();
        self.use_full_range = false;
        self.has_open_paths = false;
    }

    fn reset(&mut self) {
        self.current_lm = 0;
        if self.minima_list.is_empty() {
            return; // Ie nothing to process.
        }
        std_sort::sort_by(&mut self.minima_list, |lm1, lm2| lm2.y < lm1.y);

        self.scanbeam.clear();
        // Reset all edges.
        for i in 0..self.minima_list.len() {
            let lm = self.minima_list[i];
            self.scanbeam.push(lm.y);
            if let Some(e) = lm.left_bound {
                let edge = &mut self.edges[e];
                edge.curr = edge.bot;
                edge.side = EdgeSide::Left;
                edge.out_idx = UNASSIGNED;
            }
            if let Some(e) = lm.right_bound {
                let edge = &mut self.edges[e];
                edge.curr = edge.bot;
                edge.side = EdgeSide::Right;
                edge.out_idx = UNASSIGNED;
            }
        }
        self.active_edges = None;
        self.current_lm = 0;
    }

    fn pop_local_minima(&mut self, y: i64) -> Option<LocalMinimum> {
        let lm = *self.minima_list.get(self.current_lm)?;
        if lm.y != y {
            return None;
        }
        self.current_lm += 1;
        Some(lm)
    }

    fn local_minima_pending(&self) -> bool {
        self.current_lm < self.minima_list.len()
    }

    /// Returns the bounding rectangle of all added paths (upstream
    /// `GetBounds()`, which only considers closed paths correctly).
    pub fn bounds(&self) -> IntRect {
        let mut result = IntRect::default();
        let Some(first) = self.minima_list.first() else {
            return result;
        };
        let Some(first_left) = first.left_bound else {
            return result; // Upstream would crash (open paths).
        };
        result.left = self.edges[first_left].bot.x;
        result.top = self.edges[first_left].bot.y;
        result.right = self.edges[first_left].bot.x;
        result.bottom = self.edges[first_left].bot.y;
        for lm in &self.minima_list {
            let Some(left) = lm.left_bound else {
                continue;
            };
            // Todo (upstream) - needs fixing for open paths.
            result.bottom = result.bottom.max(self.edges[left].bot.y);
            let mut e = left;
            loop {
                let bottom_e = e;
                while let Some(n) = self.edges[e].next_in_lml {
                    let x = self.edges[e].bot.x;
                    if x < result.left {
                        result.left = x;
                    }
                    if x > result.right {
                        result.right = x;
                    }
                    e = n;
                }
                let edge = &self.edges[e];
                result.left = result.left.min(edge.bot.x);
                result.right = result.right.max(edge.bot.x);
                result.left = result.left.min(edge.top.x);
                result.right = result.right.max(edge.top.x);
                result.top = result.top.min(edge.top.y);
                match lm.right_bound {
                    Some(r) if bottom_e == left => e = r,
                    _ => break,
                }
            }
        }
        result
    }

    fn pop_scanbeam(&mut self) -> Option<i64> {
        let y = self.scanbeam.pop()?;
        while self.scanbeam.peek() == Some(&y) {
            self.scanbeam.pop(); // Pop duplicates.
        }
        Some(y)
    }

    fn dispose_all_out_recs(&mut self) {
        self.poly_outs.clear();
        self.pts.clear();
    }

    fn delete_from_ael(&mut self, e: usize) {
        let ael_prev = self.edges[e].prev_in_ael;
        let ael_next = self.edges[e].next_in_ael;
        if ael_prev.is_none() && ael_next.is_none() && Some(e) != self.active_edges {
            return; // Already deleted.
        }
        match ael_prev {
            Some(p) => self.edges[p].next_in_ael = ael_next,
            None => self.active_edges = ael_next,
        }
        if let Some(n) = ael_next {
            self.edges[n].prev_in_ael = ael_prev;
        }
        self.edges[e].next_in_ael = None;
        self.edges[e].prev_in_ael = None;
    }

    fn create_out_rec(&mut self) -> usize {
        let idx = self.poly_outs.len();
        self.poly_outs.push(OutRec {
            idx,
            is_hole: false,
            is_open: false,
            first_left: None,
            poly_nd: None,
            pts: None,
            bottom_pt: None,
        });
        idx
    }

    fn swap_positions_in_ael(&mut self, edge1: usize, edge2: usize) {
        // Check that one or other edge hasn't already been removed from AEL.
        if self.edges[edge1].next_in_ael == self.edges[edge1].prev_in_ael
            || self.edges[edge2].next_in_ael == self.edges[edge2].prev_in_ael
        {
            return;
        }

        if self.edges[edge1].next_in_ael == Some(edge2) {
            let next = self.edges[edge2].next_in_ael;
            if let Some(n) = next {
                self.edges[n].prev_in_ael = Some(edge1);
            }
            let prev = self.edges[edge1].prev_in_ael;
            if let Some(p) = prev {
                self.edges[p].next_in_ael = Some(edge2);
            }
            self.edges[edge2].prev_in_ael = prev;
            self.edges[edge2].next_in_ael = Some(edge1);
            self.edges[edge1].prev_in_ael = Some(edge2);
            self.edges[edge1].next_in_ael = next;
        } else if self.edges[edge2].next_in_ael == Some(edge1) {
            let next = self.edges[edge1].next_in_ael;
            if let Some(n) = next {
                self.edges[n].prev_in_ael = Some(edge2);
            }
            let prev = self.edges[edge2].prev_in_ael;
            if let Some(p) = prev {
                self.edges[p].next_in_ael = Some(edge1);
            }
            self.edges[edge1].prev_in_ael = prev;
            self.edges[edge1].next_in_ael = Some(edge2);
            self.edges[edge2].prev_in_ael = Some(edge1);
            self.edges[edge2].next_in_ael = next;
        } else {
            let next = self.edges[edge1].next_in_ael;
            let prev = self.edges[edge1].prev_in_ael;
            self.edges[edge1].next_in_ael = self.edges[edge2].next_in_ael;
            if let Some(n) = self.edges[edge1].next_in_ael {
                self.edges[n].prev_in_ael = Some(edge1);
            }
            self.edges[edge1].prev_in_ael = self.edges[edge2].prev_in_ael;
            if let Some(p) = self.edges[edge1].prev_in_ael {
                self.edges[p].next_in_ael = Some(edge1);
            }
            self.edges[edge2].next_in_ael = next;
            if let Some(n) = self.edges[edge2].next_in_ael {
                self.edges[n].prev_in_ael = Some(edge2);
            }
            self.edges[edge2].prev_in_ael = prev;
            if let Some(p) = self.edges[edge2].prev_in_ael {
                self.edges[p].next_in_ael = Some(edge2);
            }
        }

        if self.edges[edge1].prev_in_ael.is_none() {
            self.active_edges = Some(edge1);
        } else if self.edges[edge2].prev_in_ael.is_none() {
            self.active_edges = Some(edge2);
        }
    }

    fn update_edge_into_ael(&mut self, e: usize) -> Flow<usize> {
        let Some(nlml) = self.edges[e].next_in_lml else {
            return Err(Failure); // "UpdateEdgeIntoAEL: invalid call"
        };

        self.edges[nlml].out_idx = self.edges[e].out_idx;
        let ael_prev = self.edges[e].prev_in_ael;
        let ael_next = self.edges[e].next_in_ael;
        match ael_prev {
            Some(p) => self.edges[p].next_in_ael = Some(nlml),
            None => self.active_edges = Some(nlml),
        }
        if let Some(n) = ael_next {
            self.edges[n].prev_in_ael = Some(nlml);
        }
        let (side, wd, wc, wc2) = {
            let edge = &self.edges[e];
            (edge.side, edge.wind_delta, edge.wind_cnt, edge.wind_cnt2)
        };
        let edge = &mut self.edges[nlml];
        edge.side = side;
        edge.wind_delta = wd;
        edge.wind_cnt = wc;
        edge.wind_cnt2 = wc2;
        edge.curr = edge.bot;
        edge.prev_in_ael = ael_prev;
        edge.next_in_ael = ael_next;
        if !edge.is_horizontal() {
            let top_y = edge.top.y;
            self.scanbeam.push(top_y);
        }
        Ok(nlml)
    }
}

// ---------------------------------------------------------------------------
// Clipper
// ---------------------------------------------------------------------------

impl Clipper {
    /// Performs the boolean operation and returns the resulting polygons
    /// (upstream `Execute(ClipType, Paths&, PolyFillType, PolyFillType)`).
    ///
    /// Outlines are oriented counter-clockwise (positive area), holes
    /// clockwise (unless [reversed](Self::set_reverse_solution)). Fails if
    /// open paths were added (use [`execute_tree()`](Self::execute_tree)
    /// instead) or if the algorithm failed.
    pub fn execute(
        &mut self,
        clip_type: ClipType,
        subj_fill_type: PolyFillType,
        clip_fill_type: PolyFillType,
    ) -> Result<Paths> {
        if self.has_open_paths {
            return Err(Error::PolyTreeNeededForOpenPaths);
        }
        self.subj_fill_type = subj_fill_type;
        self.clip_fill_type = clip_fill_type;
        self.clip_type = clip_type;
        self.using_poly_tree = false;
        let status = self.execute_internal();
        let mut solution = Paths::new();
        if status == Status::Succeeded {
            self.build_result(&mut solution);
        }
        self.dispose_all_out_recs();
        match status {
            Status::Failed => Err(Error::ExecutionFailed),
            _ => Ok(solution),
        }
    }

    /// Performs the boolean operation and returns the result as tree
    /// (upstream `Execute(ClipType, PolyTree&, PolyFillType,
    /// PolyFillType)`). Supports open subject paths.
    pub fn execute_tree(
        &mut self,
        clip_type: ClipType,
        subj_fill_type: PolyFillType,
        clip_fill_type: PolyFillType,
    ) -> Result<PolyTree> {
        self.subj_fill_type = subj_fill_type;
        self.clip_fill_type = clip_fill_type;
        self.clip_type = clip_type;
        self.using_poly_tree = true;
        let status = self.execute_internal();
        let mut tree = PolyTree::new();
        if status == Status::Succeeded {
            self.build_result2(&mut tree);
        }
        self.dispose_all_out_recs();
        match status {
            Status::Failed => Err(Error::ExecutionFailed),
            _ => Ok(tree),
        }
    }

    fn fix_hole_linkage(&mut self, outrec: usize) {
        // Skip OutRecs that (a) contain outermost polygons or (b) already
        // have the correct owner/child linkage.
        let Some(fl) = self.poly_outs[outrec].first_left else {
            return;
        };
        let is_hole = self.poly_outs[outrec].is_hole;
        if is_hole != self.poly_outs[fl].is_hole && self.poly_outs[fl].pts.is_some() {
            return;
        }

        let mut orfl = Some(fl);
        while let Some(o) = orfl {
            if self.poly_outs[o].is_hole == is_hole || self.poly_outs[o].pts.is_none() {
                orfl = self.poly_outs[o].first_left;
            } else {
                break;
            }
        }
        self.poly_outs[outrec].first_left = orfl;
    }

    fn execute_internal(&mut self) -> Status {
        // Note: Upstream keeps stale intersect nodes after a failed
        // execution, which would be undefined behavior when executing again.
        self.intersect_list.clear();
        self.reset();
        self.maxima.clear();
        self.sorted_edges = None;

        let Some(bot_y) = self.pop_scanbeam() else {
            return Status::Empty;
        };
        let succeeded = matches!(self.execute_loop(bot_y), Ok(true));

        if succeeded {
            // Fix orientations.
            for i in 0..self.poly_outs.len() {
                let outrec = &self.poly_outs[i];
                if outrec.pts.is_none() || outrec.is_open {
                    continue;
                }
                if (outrec.is_hole ^ self.reverse_output) == (area_op(&self.pts, outrec.pts) > 0.0)
                {
                    reverse_poly_pt_links(&mut self.pts, outrec.pts);
                }
            }

            if !self.joins.is_empty() {
                self.join_common_edges();
            }

            // Unfortunately FixupOutPolygon() must be done after
            // JoinCommonEdges().
            for i in 0..self.poly_outs.len() {
                if self.poly_outs[i].pts.is_none() {
                    continue;
                }
                if self.poly_outs[i].is_open {
                    self.fixup_out_polyline(i);
                } else {
                    self.fixup_out_polygon(i);
                }
            }

            if self.strict_simple {
                self.do_simple_polygons();
            }
        }

        self.joins.clear();
        self.ghost_joins.clear();
        if succeeded {
            Status::Succeeded
        } else {
            Status::Failed
        }
    }

    /// The scanbeam loop of `ExecuteInternal()`. Returns `Ok(false)` if the
    /// intersections could not be processed, `Err` if an exception would
    /// have been thrown.
    fn execute_loop(&mut self, bot_y: i64) -> Flow<bool> {
        self.insert_local_minima_into_ael(bot_y)?;
        let mut top_y = bot_y;
        loop {
            let popped = self.pop_scanbeam();
            if let Some(y) = popped {
                top_y = y;
            } else if !self.local_minima_pending() {
                break;
            }
            self.process_horizontals()?;
            self.ghost_joins.clear();
            if !self.process_intersections(top_y)? {
                return Ok(false);
            }
            self.process_edges_at_top_of_scanbeam(top_y)?;
            self.insert_local_minima_into_ael(top_y)?;
        }
        Ok(true)
    }

    fn set_winding_count(&mut self, edge: usize) {
        let mut e = self.edges[edge].prev_in_ael;
        // Find the edge of the same polytype that immediately preceeds 'edge'
        // in AEL.
        while let Some(ei) = e {
            if self.edges[ei].poly_typ != self.edges[edge].poly_typ
                || self.edges[ei].wind_delta == 0
            {
                e = self.edges[ei].prev_in_ael;
            } else {
                break;
            }
        }
        let edge_wd = self.edges[edge].wind_delta;
        let mut e = match e {
            None => {
                if edge_wd == 0 {
                    let pft = if self.edges[edge].poly_typ == PolyType::Subject {
                        self.subj_fill_type
                    } else {
                        self.clip_fill_type
                    };
                    self.edges[edge].wind_cnt = if pft == PolyFillType::Negative { -1 } else { 1 };
                } else {
                    self.edges[edge].wind_cnt = edge_wd;
                }
                self.edges[edge].wind_cnt2 = 0;
                self.active_edges // Ie get ready to calc WindCnt2.
            }
            Some(ei) if edge_wd == 0 && self.clip_type != ClipType::Union => {
                self.edges[edge].wind_cnt = 1;
                self.edges[edge].wind_cnt2 = self.edges[ei].wind_cnt2;
                self.edges[ei].next_in_ael // Ie get ready to calc WindCnt2.
            }
            Some(ei) if self.is_even_odd_fill_type(edge) => {
                // EvenOdd filling.
                if edge_wd == 0 {
                    // Are we inside a subj polygon?
                    let mut inside = true;
                    let mut e2 = self.edges[ei].prev_in_ael;
                    while let Some(e2i) = e2 {
                        if self.edges[e2i].poly_typ == self.edges[ei].poly_typ
                            && self.edges[e2i].wind_delta != 0
                        {
                            inside = !inside;
                        }
                        e2 = self.edges[e2i].prev_in_ael;
                    }
                    self.edges[edge].wind_cnt = if inside { 0 } else { 1 };
                } else {
                    self.edges[edge].wind_cnt = edge_wd;
                }
                self.edges[edge].wind_cnt2 = self.edges[ei].wind_cnt2;
                self.edges[ei].next_in_ael // Ie get ready to calc WindCnt2.
            }
            Some(ei) => {
                // NonZero, Positive or Negative filling.
                let e_wc = self.edges[ei].wind_cnt;
                let e_wd = self.edges[ei].wind_delta;
                let wc = if e_wc * e_wd < 0 {
                    // Prev edge is 'decreasing' WindCount (WC) toward zero so
                    // we're outside the previous polygon.
                    if e_wc.abs() > 1 {
                        // Outside prev poly but still inside another. When
                        // reversing direction of prev poly use the same WC.
                        if e_wd * edge_wd < 0 {
                            e_wc
                        } else {
                            // Otherwise continue to 'decrease' WC.
                            e_wc + edge_wd
                        }
                    } else {
                        // Now outside all polys of same polytype so set own WC.
                        if edge_wd == 0 { 1 } else { edge_wd }
                    }
                } else {
                    // Prev edge is 'increasing' WindCount (WC) away from zero
                    // so we're inside the previous polygon.
                    if edge_wd == 0 {
                        if e_wc < 0 { e_wc - 1 } else { e_wc + 1 }
                    } else if e_wd * edge_wd < 0 {
                        // If wind direction is reversing prev then use same WC.
                        e_wc
                    } else {
                        // Otherwise add to WC.
                        e_wc + edge_wd
                    }
                };
                self.edges[edge].wind_cnt = wc;
                self.edges[edge].wind_cnt2 = self.edges[ei].wind_cnt2;
                self.edges[ei].next_in_ael // Ie get ready to calc WindCnt2.
            }
        };

        // Update WindCnt2.
        if self.is_even_odd_alt_fill_type(edge) {
            // EvenOdd filling.
            while let Some(ei) = e.filter(|&ei| ei != edge) {
                if self.edges[ei].wind_delta != 0 {
                    let wc2 = self.edges[edge].wind_cnt2;
                    self.edges[edge].wind_cnt2 = if wc2 == 0 { 1 } else { 0 };
                }
                e = self.edges[ei].next_in_ael;
            }
        } else {
            // NonZero, Positive or Negative filling.
            while let Some(ei) = e.filter(|&ei| ei != edge) {
                self.edges[edge].wind_cnt2 += self.edges[ei].wind_delta;
                e = self.edges[ei].next_in_ael;
            }
        }
    }

    fn is_even_odd_fill_type(&self, edge: usize) -> bool {
        if self.edges[edge].poly_typ == PolyType::Subject {
            self.subj_fill_type == PolyFillType::EvenOdd
        } else {
            self.clip_fill_type == PolyFillType::EvenOdd
        }
    }

    fn is_even_odd_alt_fill_type(&self, edge: usize) -> bool {
        if self.edges[edge].poly_typ == PolyType::Subject {
            self.clip_fill_type == PolyFillType::EvenOdd
        } else {
            self.subj_fill_type == PolyFillType::EvenOdd
        }
    }

    fn is_contributing(&self, edge: usize) -> bool {
        let edge = &self.edges[edge];
        let (pft, pft2) = if edge.poly_typ == PolyType::Subject {
            (self.subj_fill_type, self.clip_fill_type)
        } else {
            (self.clip_fill_type, self.subj_fill_type)
        };

        match pft {
            PolyFillType::EvenOdd => {
                // Return false if a subj line has been flagged as inside a
                // subj polygon.
                if edge.wind_delta == 0 && edge.wind_cnt != 1 {
                    return false;
                }
            }
            PolyFillType::NonZero => {
                if edge.wind_cnt.abs() != 1 {
                    return false;
                }
            }
            PolyFillType::Positive => {
                if edge.wind_cnt != 1 {
                    return false;
                }
            }
            PolyFillType::Negative => {
                if edge.wind_cnt != -1 {
                    return false;
                }
            }
        }

        let wc2 = edge.wind_cnt2;
        let outside = || match pft2 {
            PolyFillType::EvenOdd | PolyFillType::NonZero => wc2 == 0,
            PolyFillType::Positive => wc2 <= 0,
            PolyFillType::Negative => wc2 >= 0,
        };
        let inside = || match pft2 {
            PolyFillType::EvenOdd | PolyFillType::NonZero => wc2 != 0,
            PolyFillType::Positive => wc2 > 0,
            PolyFillType::Negative => wc2 < 0,
        };
        match self.clip_type {
            ClipType::Intersection => inside(),
            ClipType::Union => outside(),
            ClipType::Difference => {
                if edge.poly_typ == PolyType::Subject {
                    outside()
                } else {
                    inside()
                }
            }
            ClipType::Xor => {
                if edge.wind_delta == 0 {
                    // XOr always contributing unless open.
                    outside()
                } else {
                    true
                }
            }
        }
    }

    fn add_local_min_poly(&mut self, e1: usize, e2: usize, pt: IntPoint) -> usize {
        let (result, e, other) = if self.is_horz(e2) || self.edges[e1].dx > self.edges[e2].dx {
            let result = self.add_out_pt(e1, pt);
            self.edges[e2].out_idx = self.edges[e1].out_idx;
            self.edges[e1].side = EdgeSide::Left;
            self.edges[e2].side = EdgeSide::Right;
            (result, e1, e2)
        } else {
            let result = self.add_out_pt(e2, pt);
            self.edges[e1].out_idx = self.edges[e2].out_idx;
            self.edges[e1].side = EdgeSide::Right;
            self.edges[e2].side = EdgeSide::Left;
            (result, e2, e1)
        };
        let prev_e = if self.edges[e].prev_in_ael == Some(other) {
            self.edges[other].prev_in_ael
        } else {
            self.edges[e].prev_in_ael
        };

        if let Some(prev_e) = prev_e
            && self.edges[prev_e].out_idx >= 0
            && self.edges[prev_e].top.y < pt.y
            && self.edges[e].top.y < pt.y
        {
            let x_prev = top_x(&self.edges[prev_e], pt.y);
            let x_e = top_x(&self.edges[e], pt.y);
            if x_prev == x_e
                && self.edges[e].wind_delta != 0
                && self.edges[prev_e].wind_delta != 0
                && slopes_equal4(
                    IntPoint::new(x_prev, pt.y),
                    self.edges[prev_e].top,
                    IntPoint::new(x_e, pt.y),
                    self.edges[e].top,
                )
            {
                let out_pt = self.add_out_pt(prev_e, pt);
                let off = self.edges[e].top;
                self.add_join(result, out_pt, off);
            }
        }
        result
    }

    fn add_local_max_poly(&mut self, e1: usize, e2: usize, pt: IntPoint) {
        self.add_out_pt(e1, pt);
        if self.edges[e2].wind_delta == 0 {
            self.add_out_pt(e2, pt);
        }
        if self.edges[e1].out_idx == self.edges[e2].out_idx {
            self.edges[e1].out_idx = UNASSIGNED;
            self.edges[e2].out_idx = UNASSIGNED;
        } else if self.edges[e1].out_idx < self.edges[e2].out_idx {
            self.append_polygon(e1, e2);
        } else {
            self.append_polygon(e2, e1);
        }
    }

    fn add_edge_to_sel(&mut self, edge: usize) {
        // SEL pointers in PEdge are reused to build a list of horizontal
        // edges. However, we don't need to worry about order with horizontal
        // edge processing.
        match self.sorted_edges {
            None => {
                self.sorted_edges = Some(edge);
                self.edges[edge].prev_in_sel = None;
                self.edges[edge].next_in_sel = None;
            }
            Some(s) => {
                self.edges[edge].next_in_sel = Some(s);
                self.edges[edge].prev_in_sel = None;
                self.edges[s].prev_in_sel = Some(edge);
                self.sorted_edges = Some(edge);
            }
        }
    }

    fn pop_edge_from_sel(&mut self) -> Option<usize> {
        let edge = self.sorted_edges?;
        self.delete_from_sel(edge);
        Some(edge)
    }

    fn copy_ael_to_sel(&mut self) {
        let mut e = self.active_edges;
        self.sorted_edges = e;
        while let Some(ei) = e {
            self.edges[ei].prev_in_sel = self.edges[ei].prev_in_ael;
            self.edges[ei].next_in_sel = self.edges[ei].next_in_ael;
            e = self.edges[ei].next_in_ael;
        }
    }

    fn add_join(&mut self, op1: usize, op2: usize, off_pt: IntPoint) {
        self.joins.push(Join {
            out_pt1: op1,
            out_pt2: Some(op2),
            off_pt,
        });
    }

    fn add_ghost_join(&mut self, op: usize, off_pt: IntPoint) {
        self.ghost_joins.push(Join {
            out_pt1: op,
            out_pt2: None,
            off_pt,
        });
    }

    fn insert_local_minima_into_ael(&mut self, bot_y: i64) -> Flow {
        while let Some(lm) = self.pop_local_minima(bot_y) {
            let lb = lm.left_bound;
            let rb = lm.right_bound;

            let mut op1: Option<usize> = None;
            match (lb, rb) {
                (None, Some(rb)) => {
                    // Nb: don't insert LB into either AEL or SEL.
                    self.insert_edge_into_ael(rb, None);
                    self.set_winding_count(rb);
                    if self.is_contributing(rb) {
                        let bot = self.edges[rb].bot;
                        op1 = Some(self.add_out_pt(rb, bot));
                    }
                }
                (Some(lb), None) => {
                    self.insert_edge_into_ael(lb, None);
                    self.set_winding_count(lb);
                    if self.is_contributing(lb) {
                        let bot = self.edges[lb].bot;
                        op1 = Some(self.add_out_pt(lb, bot));
                    }
                    self.scanbeam.push(self.edges[lb].top.y);
                }
                (Some(lb), Some(rb)) => {
                    self.insert_edge_into_ael(lb, None);
                    self.insert_edge_into_ael(rb, Some(lb));
                    self.set_winding_count(lb);
                    self.edges[rb].wind_cnt = self.edges[lb].wind_cnt;
                    self.edges[rb].wind_cnt2 = self.edges[lb].wind_cnt2;
                    if self.is_contributing(lb) {
                        let bot = self.edges[lb].bot;
                        op1 = Some(self.add_local_min_poly(lb, rb, bot));
                    }
                    self.scanbeam.push(self.edges[lb].top.y);
                }
                (None, None) => {}
            }

            if let Some(rb) = rb {
                if self.is_horz(rb) {
                    self.add_edge_to_sel(rb);
                    if let Some(n) = self.edges[rb].next_in_lml {
                        self.scanbeam.push(self.edges[n].top.y);
                    }
                } else {
                    self.scanbeam.push(self.edges[rb].top.y);
                }
            }

            let (Some(lb), Some(rb)) = (lb, rb) else {
                continue;
            };

            // If any output polygons share an edge, they'll need joining
            // later.
            if let Some(op1) = op1
                && self.is_horz(rb)
                && !self.ghost_joins.is_empty()
                && self.edges[rb].wind_delta != 0
            {
                for i in 0..self.ghost_joins.len() {
                    let jr = self.ghost_joins[i];
                    // If the horizontal Rb and a 'ghost' horizontal overlap,
                    // then convert the 'ghost' join to a real join ready for
                    // later.
                    if horz_segments_overlap(
                        self.pts[jr.out_pt1].pt.x,
                        jr.off_pt.x,
                        self.edges[rb].bot.x,
                        self.edges[rb].top.x,
                    ) {
                        self.add_join(jr.out_pt1, op1, jr.off_pt);
                    }
                }
            }

            if self.edges[lb].out_idx >= 0
                && let Some(lb_prev) = self.edges[lb].prev_in_ael
                && self.edges[lb_prev].curr.x == self.edges[lb].bot.x
                && self.edges[lb_prev].out_idx >= 0
                && slopes_equal4(
                    self.edges[lb_prev].bot,
                    self.edges[lb_prev].top,
                    self.edges[lb].curr,
                    self.edges[lb].top,
                )
                && self.edges[lb].wind_delta != 0
                && self.edges[lb_prev].wind_delta != 0
            {
                let bot = self.edges[lb].bot;
                let op2 = self.add_out_pt(lb_prev, bot);
                let top = self.edges[lb].top;
                // Note: op1 is always set here (lb is contributing).
                if let Some(op1) = op1 {
                    self.add_join(op1, op2, top);
                }
            }

            if self.edges[lb].next_in_ael != Some(rb) {
                if self.edges[rb].out_idx >= 0
                    && let Some(rb_prev) = self.edges[rb].prev_in_ael
                    && self.edges[rb_prev].out_idx >= 0
                    && slopes_equal4(
                        self.edges[rb_prev].curr,
                        self.edges[rb_prev].top,
                        self.edges[rb].curr,
                        self.edges[rb].top,
                    )
                    && self.edges[rb].wind_delta != 0
                    && self.edges[rb_prev].wind_delta != 0
                {
                    let bot = self.edges[rb].bot;
                    let op2 = self.add_out_pt(rb_prev, bot);
                    let top = self.edges[rb].top;
                    if let Some(op1) = op1 {
                        self.add_join(op1, op2, top);
                    }
                }

                let mut e = self.edges[lb].next_in_ael;
                while let Some(ei) = e.filter(|&ei| ei != rb) {
                    // Nb: For calculating winding counts etc, IntersectEdges()
                    // assumes that param1 will be to the Right of param2
                    // ABOVE the intersection.
                    let pt = self.edges[lb].curr;
                    self.intersect_edges(rb, ei, pt); // Order important here.
                    e = self.edges[ei].next_in_ael;
                }
            }
        }
        Ok(())
    }

    fn delete_from_sel(&mut self, e: usize) {
        let sel_prev = self.edges[e].prev_in_sel;
        let sel_next = self.edges[e].next_in_sel;
        if sel_prev.is_none() && sel_next.is_none() && Some(e) != self.sorted_edges {
            return; // Already deleted.
        }
        match sel_prev {
            Some(p) => self.edges[p].next_in_sel = sel_next,
            None => self.sorted_edges = sel_next,
        }
        if let Some(n) = sel_next {
            self.edges[n].prev_in_sel = sel_prev;
        }
        self.edges[e].next_in_sel = None;
        self.edges[e].prev_in_sel = None;
    }

    fn intersect_edges(&mut self, e1: usize, e2: usize, pt: IntPoint) {
        let e1_contributing = self.edges[e1].out_idx >= 0;
        let e2_contributing = self.edges[e2].out_idx >= 0;

        // If either edge is on an OPEN path...
        let (wd1, wd2) = (self.edges[e1].wind_delta, self.edges[e2].wind_delta);
        if wd1 == 0 || wd2 == 0 {
            let (typ1, typ2) = (self.edges[e1].poly_typ, self.edges[e2].poly_typ);
            let (wc1, wc2) = (self.edges[e1].wind_cnt, self.edges[e2].wind_cnt);
            let (wc2_1, wc2_2) = (self.edges[e1].wind_cnt2, self.edges[e2].wind_cnt2);
            // Ignore subject-subject open path intersections UNLESS they are
            // both open paths, AND they are both 'contributing maximas'.
            if wd1 == 0 && wd2 == 0 {
                return;
            } else if typ1 == typ2 && wd1 != wd2 && self.clip_type == ClipType::Union {
                // If intersecting a subj line with a subj poly.
                if wd1 == 0 {
                    if e2_contributing {
                        self.add_out_pt(e1, pt);
                        if e1_contributing {
                            self.edges[e1].out_idx = UNASSIGNED;
                        }
                    }
                } else if e1_contributing {
                    self.add_out_pt(e2, pt);
                    if e2_contributing {
                        self.edges[e2].out_idx = UNASSIGNED;
                    }
                }
            } else if typ1 != typ2 {
                // Toggle subj open path OutIdx on/off when Abs(clip.WndCnt)
                // == 1.
                if wd1 == 0 && wc2.abs() == 1 && (self.clip_type != ClipType::Union || wc2_2 == 0) {
                    self.add_out_pt(e1, pt);
                    if e1_contributing {
                        self.edges[e1].out_idx = UNASSIGNED;
                    }
                } else if wd2 == 0
                    && wc1.abs() == 1
                    && (self.clip_type != ClipType::Union || wc2_1 == 0)
                {
                    self.add_out_pt(e2, pt);
                    if e2_contributing {
                        self.edges[e2].out_idx = UNASSIGNED;
                    }
                }
            }
            return;
        }

        // Update winding counts. Assumes that e1 will be to the Right of e2
        // ABOVE the intersection.
        if self.edges[e1].poly_typ == self.edges[e2].poly_typ {
            if self.is_even_odd_fill_type(e1) {
                let old_e1_wind_cnt = self.edges[e1].wind_cnt;
                self.edges[e1].wind_cnt = self.edges[e2].wind_cnt;
                self.edges[e2].wind_cnt = old_e1_wind_cnt;
            } else {
                let e1_wd = self.edges[e1].wind_delta;
                let e2_wd = self.edges[e2].wind_delta;
                let ed1 = &mut self.edges[e1];
                if ed1.wind_cnt + e2_wd == 0 {
                    ed1.wind_cnt = -ed1.wind_cnt;
                } else {
                    ed1.wind_cnt += e2_wd;
                }
                let ed2 = &mut self.edges[e2];
                if ed2.wind_cnt - e1_wd == 0 {
                    ed2.wind_cnt = -ed2.wind_cnt;
                } else {
                    ed2.wind_cnt -= e1_wd;
                }
            }
        } else {
            if !self.is_even_odd_fill_type(e2) {
                self.edges[e1].wind_cnt2 += self.edges[e2].wind_delta;
            } else {
                let wc2 = self.edges[e1].wind_cnt2;
                self.edges[e1].wind_cnt2 = if wc2 == 0 { 1 } else { 0 };
            }
            if !self.is_even_odd_fill_type(e1) {
                self.edges[e2].wind_cnt2 -= self.edges[e1].wind_delta;
            } else {
                let wc2 = self.edges[e2].wind_cnt2;
                self.edges[e2].wind_cnt2 = if wc2 == 0 { 1 } else { 0 };
            }
        }

        let fill_types = |typ: PolyType| {
            if typ == PolyType::Subject {
                (self.subj_fill_type, self.clip_fill_type)
            } else {
                (self.clip_fill_type, self.subj_fill_type)
            }
        };
        let (e1_fill_type, e1_fill_type2) = fill_types(self.edges[e1].poly_typ);
        let (e2_fill_type, e2_fill_type2) = fill_types(self.edges[e2].poly_typ);

        let wc = |ft: PolyFillType, cnt: i32| -> i64 {
            match ft {
                PolyFillType::Positive => i64::from(cnt),
                PolyFillType::Negative => -i64::from(cnt),
                _ => i64::from(cnt).abs(),
            }
        };
        let e1_wc = wc(e1_fill_type, self.edges[e1].wind_cnt);
        let e2_wc = wc(e2_fill_type, self.edges[e2].wind_cnt);

        if e1_contributing && e2_contributing {
            if (e1_wc != 0 && e1_wc != 1)
                || (e2_wc != 0 && e2_wc != 1)
                || (self.edges[e1].poly_typ != self.edges[e2].poly_typ
                    && self.clip_type != ClipType::Xor)
            {
                self.add_local_max_poly(e1, e2, pt);
            } else {
                self.add_out_pt(e1, pt);
                self.add_out_pt(e2, pt);
                self.swap_sides(e1, e2);
                self.swap_poly_indexes(e1, e2);
            }
        } else if e1_contributing {
            if e2_wc == 0 || e2_wc == 1 {
                self.add_out_pt(e1, pt);
                self.swap_sides(e1, e2);
                self.swap_poly_indexes(e1, e2);
            }
        } else if e2_contributing {
            if e1_wc == 0 || e1_wc == 1 {
                self.add_out_pt(e2, pt);
                self.swap_sides(e1, e2);
                self.swap_poly_indexes(e1, e2);
            }
        } else if (e1_wc == 0 || e1_wc == 1) && (e2_wc == 0 || e2_wc == 1) {
            // Neither edge is currently contributing.
            let e1_wc2 = wc(e1_fill_type2, self.edges[e1].wind_cnt2);
            let e2_wc2 = wc(e2_fill_type2, self.edges[e2].wind_cnt2);

            if self.edges[e1].poly_typ != self.edges[e2].poly_typ {
                self.add_local_min_poly(e1, e2, pt);
            } else if e1_wc == 1 && e2_wc == 1 {
                match self.clip_type {
                    ClipType::Intersection => {
                        if e1_wc2 > 0 && e2_wc2 > 0 {
                            self.add_local_min_poly(e1, e2, pt);
                        }
                    }
                    ClipType::Union => {
                        if e1_wc2 <= 0 && e2_wc2 <= 0 {
                            self.add_local_min_poly(e1, e2, pt);
                        }
                    }
                    ClipType::Difference => {
                        let typ = self.edges[e1].poly_typ;
                        if ((typ == PolyType::Clip) && (e1_wc2 > 0) && (e2_wc2 > 0))
                            || ((typ == PolyType::Subject) && (e1_wc2 <= 0) && (e2_wc2 <= 0))
                        {
                            self.add_local_min_poly(e1, e2, pt);
                        }
                    }
                    ClipType::Xor => {
                        self.add_local_min_poly(e1, e2, pt);
                    }
                }
            } else {
                self.swap_sides(e1, e2);
            }
        }
    }

    fn swap_sides(&mut self, e1: usize, e2: usize) {
        let side = self.edges[e1].side;
        self.edges[e1].side = self.edges[e2].side;
        self.edges[e2].side = side;
    }

    fn swap_poly_indexes(&mut self, e1: usize, e2: usize) {
        let out_idx = self.edges[e1].out_idx;
        self.edges[e1].out_idx = self.edges[e2].out_idx;
        self.edges[e2].out_idx = out_idx;
    }

    fn set_hole_state(&mut self, e: usize, outrec: usize) {
        let mut e2 = self.edges[e].prev_in_ael;
        let mut e_tmp: Option<usize> = None;
        while let Some(e2i) = e2 {
            if self.edges[e2i].out_idx >= 0 && self.edges[e2i].wind_delta != 0 {
                match e_tmp {
                    None => e_tmp = Some(e2i),
                    Some(t) if self.edges[t].out_idx == self.edges[e2i].out_idx => e_tmp = None,
                    Some(_) => {}
                }
            }
            e2 = self.edges[e2i].prev_in_ael;
        }
        match e_tmp {
            None => {
                self.poly_outs[outrec].first_left = None;
                self.poly_outs[outrec].is_hole = false;
            }
            Some(t) => {
                let fl = self.edges[t].out_idx as usize;
                self.poly_outs[outrec].first_left = Some(fl);
                self.poly_outs[outrec].is_hole = !self.poly_outs[fl].is_hole;
            }
        }
    }

    // Keeps the branch structure of upstream.
    #[allow(clippy::if_same_then_else)]
    fn get_lowermost_rec(&mut self, out_rec1: usize, out_rec2: usize) -> usize {
        // Work out which polygon fragment has the correct hole state.
        for r in [out_rec1, out_rec2] {
            if self.poly_outs[r].bottom_pt.is_none()
                && let Some(p) = self.poly_outs[r].pts
            {
                self.poly_outs[r].bottom_pt = Some(get_bottom_pt(&self.pts, p));
            }
        }
        let (Some(out_pt1), Some(out_pt2)) = (
            self.poly_outs[out_rec1].bottom_pt,
            self.poly_outs[out_rec2].bottom_pt,
        ) else {
            return out_rec2; // Unreachable: both records have points.
        };
        let (p1, p2) = (self.pts[out_pt1].pt, self.pts[out_pt2].pt);
        if p1.y > p2.y {
            out_rec1
        } else if p1.y < p2.y {
            out_rec2
        } else if p1.x < p2.x {
            out_rec1
        } else if p1.x > p2.x {
            out_rec2
        } else if self.pts[out_pt1].next == out_pt1 {
            out_rec2
        } else if self.pts[out_pt2].next == out_pt2 {
            out_rec1
        } else if first_is_bottom_pt(&self.pts, out_pt1, out_pt2) {
            out_rec1
        } else {
            out_rec2
        }
    }

    fn out_rec1_right_of_out_rec2(&self, mut out_rec1: usize, out_rec2: usize) -> bool {
        loop {
            match self.poly_outs[out_rec1].first_left {
                Some(fl) => {
                    out_rec1 = fl;
                    if out_rec1 == out_rec2 {
                        return true;
                    }
                }
                None => return false,
            }
        }
    }

    fn get_out_rec(&self, idx: usize) -> usize {
        let mut outrec = idx;
        while outrec != self.poly_outs[outrec].idx {
            outrec = self.poly_outs[outrec].idx;
        }
        outrec
    }

    fn append_polygon(&mut self, e1: usize, e2: usize) {
        // Get the start and ends of both output polygons.
        let out_rec1 = self.edges[e1].out_idx as usize;
        let out_rec2 = self.edges[e2].out_idx as usize;

        let hole_state_rec = if self.out_rec1_right_of_out_rec2(out_rec1, out_rec2) {
            out_rec2
        } else if self.out_rec1_right_of_out_rec2(out_rec2, out_rec1) {
            out_rec1
        } else {
            self.get_lowermost_rec(out_rec1, out_rec2)
        };

        // Get the start and ends of both output polygons and join e2 poly
        // onto e1 poly and delete pointers to e2.
        let (Some(p1_lft), Some(p2_lft)) =
            (self.poly_outs[out_rec1].pts, self.poly_outs[out_rec2].pts)
        else {
            return; // Unreachable: both records have points.
        };
        let p1_rt = self.pts[p1_lft].prev;
        let p2_rt = self.pts[p2_lft].prev;

        // Join e2 poly onto e1 poly and delete pointers to e2.
        if self.edges[e1].side == EdgeSide::Left {
            if self.edges[e2].side == EdgeSide::Left {
                // z y x a b c
                reverse_poly_pt_links(&mut self.pts, Some(p2_lft));
                self.pts[p2_lft].next = p1_lft;
                self.pts[p1_lft].prev = p2_lft;
                self.pts[p1_rt].next = p2_rt;
                self.pts[p2_rt].prev = p1_rt;
                self.poly_outs[out_rec1].pts = Some(p2_rt);
            } else {
                // x y z a b c
                self.pts[p2_rt].next = p1_lft;
                self.pts[p1_lft].prev = p2_rt;
                self.pts[p2_lft].prev = p1_rt;
                self.pts[p1_rt].next = p2_lft;
                self.poly_outs[out_rec1].pts = Some(p2_lft);
            }
        } else if self.edges[e2].side == EdgeSide::Right {
            // a b c z y x
            reverse_poly_pt_links(&mut self.pts, Some(p2_lft));
            self.pts[p1_rt].next = p2_rt;
            self.pts[p2_rt].prev = p1_rt;
            self.pts[p2_lft].next = p1_lft;
            self.pts[p1_lft].prev = p2_lft;
        } else {
            // a b c x y z
            self.pts[p1_rt].next = p2_lft;
            self.pts[p2_lft].prev = p1_rt;
            self.pts[p1_lft].prev = p2_rt;
            self.pts[p2_rt].next = p1_lft;
        }

        self.poly_outs[out_rec1].bottom_pt = None;
        if hole_state_rec == out_rec2 {
            if self.poly_outs[out_rec2].first_left != Some(out_rec1) {
                self.poly_outs[out_rec1].first_left = self.poly_outs[out_rec2].first_left;
            }
            self.poly_outs[out_rec1].is_hole = self.poly_outs[out_rec2].is_hole;
        }
        self.poly_outs[out_rec2].pts = None;
        self.poly_outs[out_rec2].bottom_pt = None;
        self.poly_outs[out_rec2].first_left = Some(out_rec1);

        let ok_idx = self.edges[e1].out_idx;
        let obsolete_idx = self.edges[e2].out_idx;

        self.edges[e1].out_idx = UNASSIGNED; // Nb: safe because we only get here via AddLocalMaxPoly.
        self.edges[e2].out_idx = UNASSIGNED;

        let mut e = self.active_edges;
        while let Some(ei) = e {
            if self.edges[ei].out_idx == obsolete_idx {
                self.edges[ei].out_idx = ok_idx;
                self.edges[ei].side = self.edges[e1].side;
                break;
            }
            e = self.edges[ei].next_in_ael;
        }

        self.poly_outs[out_rec2].idx = self.poly_outs[out_rec1].idx;
    }

    fn add_out_pt(&mut self, e: usize, pt: IntPoint) -> usize {
        if self.edges[e].out_idx < 0 {
            let out_rec = self.create_out_rec();
            self.poly_outs[out_rec].is_open = self.edges[e].wind_delta == 0;
            let new_op = self.pts.len();
            self.pts.push(OutPt {
                idx: out_rec,
                pt,
                next: new_op,
                prev: new_op,
            });
            self.poly_outs[out_rec].pts = Some(new_op);
            if !self.poly_outs[out_rec].is_open {
                self.set_hole_state(e, out_rec);
            }
            self.edges[e].out_idx = out_rec as i32;
            new_op
        } else {
            let out_rec = self.edges[e].out_idx as usize;
            // OutRec.Pts is the 'Left-most' point & OutRec.Pts.Prev is the
            // 'Right-most'.
            let Some(op) = self.poly_outs[out_rec].pts else {
                // Unreachable: an edge only owns records with points.
                return 0;
            };

            let to_front = self.edges[e].side == EdgeSide::Left;
            if to_front && pt == self.pts[op].pt {
                return op;
            } else if !to_front && pt == self.pts[self.pts[op].prev].pt {
                return self.pts[op].prev;
            }

            let new_op = self.pts.len();
            let op_prev = self.pts[op].prev;
            self.pts.push(OutPt {
                idx: self.poly_outs[out_rec].idx,
                pt,
                next: op,
                prev: op_prev,
            });
            self.pts[op_prev].next = new_op;
            self.pts[op].prev = new_op;
            if to_front {
                self.poly_outs[out_rec].pts = Some(new_op);
            }
            new_op
        }
    }

    fn get_last_out_pt(&self, e: usize) -> usize {
        let out_rec = &self.poly_outs[self.edges[e].out_idx as usize];
        // Note: An edge only owns records with points.
        let pts = out_rec.pts.unwrap_or_default();
        if self.edges[e].side == EdgeSide::Left {
            pts
        } else {
            self.pts[pts].prev
        }
    }

    fn process_horizontals(&mut self) -> Flow {
        while let Some(horz_edge) = self.pop_edge_from_sel() {
            self.process_horizontal(horz_edge)?;
        }
        Ok(())
    }

    fn get_maxima_pair(&self, e: usize) -> Option<usize> {
        let (next, prev) = (self.next(e), self.prev(e));
        if self.edges[next].top == self.edges[e].top && self.edges[next].next_in_lml.is_none() {
            Some(next)
        } else if self.edges[prev].top == self.edges[e].top
            && self.edges[prev].next_in_lml.is_none()
        {
            Some(prev)
        } else {
            None
        }
    }

    fn get_maxima_pair_ex(&self, e: usize) -> Option<usize> {
        // As GetMaximaPair() but returns 0 if MaxPair isn't in AEL (unless
        // it's horizontal).
        let result = self.get_maxima_pair(e)?;
        let r = &self.edges[result];
        if r.out_idx == SKIP || (r.next_in_ael == r.prev_in_ael && !r.is_horizontal()) {
            return None;
        }
        Some(result)
    }

    fn swap_positions_in_sel(&mut self, edge1: usize, edge2: usize) {
        if self.edges[edge1].next_in_sel.is_none() && self.edges[edge1].prev_in_sel.is_none() {
            return;
        }
        if self.edges[edge2].next_in_sel.is_none() && self.edges[edge2].prev_in_sel.is_none() {
            return;
        }

        if self.edges[edge1].next_in_sel == Some(edge2) {
            let next = self.edges[edge2].next_in_sel;
            if let Some(n) = next {
                self.edges[n].prev_in_sel = Some(edge1);
            }
            let prev = self.edges[edge1].prev_in_sel;
            if let Some(p) = prev {
                self.edges[p].next_in_sel = Some(edge2);
            }
            self.edges[edge2].prev_in_sel = prev;
            self.edges[edge2].next_in_sel = Some(edge1);
            self.edges[edge1].prev_in_sel = Some(edge2);
            self.edges[edge1].next_in_sel = next;
        } else if self.edges[edge2].next_in_sel == Some(edge1) {
            let next = self.edges[edge1].next_in_sel;
            if let Some(n) = next {
                self.edges[n].prev_in_sel = Some(edge2);
            }
            let prev = self.edges[edge2].prev_in_sel;
            if let Some(p) = prev {
                self.edges[p].next_in_sel = Some(edge1);
            }
            self.edges[edge1].prev_in_sel = prev;
            self.edges[edge1].next_in_sel = Some(edge2);
            self.edges[edge2].prev_in_sel = Some(edge1);
            self.edges[edge2].next_in_sel = next;
        } else {
            let next = self.edges[edge1].next_in_sel;
            let prev = self.edges[edge1].prev_in_sel;
            self.edges[edge1].next_in_sel = self.edges[edge2].next_in_sel;
            if let Some(n) = self.edges[edge1].next_in_sel {
                self.edges[n].prev_in_sel = Some(edge1);
            }
            self.edges[edge1].prev_in_sel = self.edges[edge2].prev_in_sel;
            if let Some(p) = self.edges[edge1].prev_in_sel {
                self.edges[p].next_in_sel = Some(edge1);
            }
            self.edges[edge2].next_in_sel = next;
            if let Some(n) = self.edges[edge2].next_in_sel {
                self.edges[n].prev_in_sel = Some(edge2);
            }
            self.edges[edge2].prev_in_sel = prev;
            if let Some(p) = self.edges[edge2].prev_in_sel {
                self.edges[p].next_in_sel = Some(edge2);
            }
        }

        if self.edges[edge1].prev_in_sel.is_none() {
            self.sorted_edges = Some(edge1);
        } else if self.edges[edge2].prev_in_sel.is_none() {
            self.sorted_edges = Some(edge2);
        }
    }

    fn get_next_in_ael(&self, e: usize, dir: Direction) -> Option<usize> {
        if dir == Direction::LeftToRight {
            self.edges[e].next_in_ael
        } else {
            self.edges[e].prev_in_ael
        }
    }

    fn get_horz_direction(&self, horz_edge: usize) -> (Direction, i64, i64) {
        let e = &self.edges[horz_edge];
        if e.bot.x < e.top.x {
            (Direction::LeftToRight, e.bot.x, e.top.x)
        } else {
            (Direction::RightToLeft, e.top.x, e.bot.x)
        }
    }

    // Notes: Horizontal edges (HEs) at scanline intersections (ie at the Top
    // or Bottom of a scanbeam) are processed as if layered. The order in
    // which HEs are processed doesn't matter. HEs intersect with other HE
    // Bot.Xs only [#] (or they could intersect with Top.Xs only, ie EITHER
    // Bot.Xs OR Top.Xs), and with other non-horizontal edges [*]. Once these
    // intersections are processed, intermediate HEs then 'promote' the Edge
    // above (NextInLML) into the AEL. These 'promoted' edges may in turn
    // intersect [%] with other HEs.
    fn process_horizontal(&mut self, mut horz_edge: usize) -> Flow {
        let is_open = self.edges[horz_edge].wind_delta == 0;

        let (mut dir, mut horz_left, mut horz_right) = self.get_horz_direction(horz_edge);

        let mut e_last_horz = horz_edge;
        let mut e_max_pair: Option<usize> = None;
        while let Some(n) = self.edges[e_last_horz]
            .next_in_lml
            .filter(|&n| self.is_horz(n))
        {
            e_last_horz = n;
        }
        if self.edges[e_last_horz].next_in_lml.is_none() {
            e_max_pair = self.get_maxima_pair(e_last_horz);
        }

        // Iterators into `maxima`: `max_it` is a forward index, `max_rit`
        // the number of elements already passed from the back.
        let maxima_len = self.maxima.len();
        let mut max_it = 0usize;
        let mut max_rit = 0usize;
        if maxima_len > 0 {
            // Get the first maxima in range (X).
            if dir == Direction::LeftToRight {
                while max_it < maxima_len && self.maxima[max_it] <= self.edges[horz_edge].bot.x {
                    max_it += 1;
                }
                if max_it < maxima_len && self.maxima[max_it] >= self.edges[e_last_horz].top.x {
                    max_it = maxima_len;
                }
            } else {
                while max_rit < maxima_len
                    && self.maxima[maxima_len - 1 - max_rit] > self.edges[horz_edge].bot.x
                {
                    max_rit += 1;
                }
                if max_rit < maxima_len
                    && self.maxima[maxima_len - 1 - max_rit] <= self.edges[e_last_horz].top.x
                {
                    max_rit = maxima_len;
                }
            }
        }

        let mut op1: Option<usize> = None;

        loop {
            // Loop through consec. horizontal edges.
            let is_last_horz = horz_edge == e_last_horz;
            let mut e = self.get_next_in_ael(horz_edge, dir);
            while let Some(ei) = e {
                // This code block inserts extra coords into horizontal edges
                // (in output polygons) whereever maxima touch these horizontal
                // edges. This helps 'simplifying' polygons (ie if the Simplify
                // property is set).
                if maxima_len > 0 {
                    if dir == Direction::LeftToRight {
                        while max_it < maxima_len && self.maxima[max_it] < self.edges[ei].curr.x {
                            if self.edges[horz_edge].out_idx >= 0 && !is_open {
                                let pt =
                                    IntPoint::new(self.maxima[max_it], self.edges[horz_edge].bot.y);
                                self.add_out_pt(horz_edge, pt);
                            }
                            max_it += 1;
                        }
                    } else {
                        while max_rit < maxima_len
                            && self.maxima[maxima_len - 1 - max_rit] > self.edges[ei].curr.x
                        {
                            if self.edges[horz_edge].out_idx >= 0 && !is_open {
                                let pt = IntPoint::new(
                                    self.maxima[maxima_len - 1 - max_rit],
                                    self.edges[horz_edge].bot.y,
                                );
                                self.add_out_pt(horz_edge, pt);
                            }
                            max_rit += 1;
                        }
                    }
                }

                if (dir == Direction::LeftToRight && self.edges[ei].curr.x > horz_right)
                    || (dir == Direction::RightToLeft && self.edges[ei].curr.x < horz_left)
                {
                    break;
                }

                // Also break if we've got to the end of an intermediate
                // horizontal edge. Nb: Smaller Dx's are to the right of larger
                // Dx's ABOVE the horizontal.
                if self.edges[ei].curr.x == self.edges[horz_edge].top.x
                    && let Some(n) = self.edges[horz_edge].next_in_lml
                    && self.edges[ei].dx < self.edges[n].dx
                {
                    break;
                }

                if self.edges[horz_edge].out_idx >= 0 && !is_open {
                    // Note: may be done multiple times.
                    let curr = self.edges[ei].curr;
                    let o1 = self.add_out_pt(horz_edge, curr);
                    op1 = Some(o1);
                    let mut e_next_horz = self.sorted_edges;
                    while let Some(nh) = e_next_horz {
                        if self.edges[nh].out_idx >= 0
                            && horz_segments_overlap(
                                self.edges[horz_edge].bot.x,
                                self.edges[horz_edge].top.x,
                                self.edges[nh].bot.x,
                                self.edges[nh].top.x,
                            )
                        {
                            let op2 = self.get_last_out_pt(nh);
                            let top = self.edges[nh].top;
                            self.add_join(op2, o1, top);
                        }
                        e_next_horz = self.edges[nh].next_in_sel;
                    }
                    let bot = self.edges[horz_edge].bot;
                    self.add_ghost_join(o1, bot);
                }

                // OK, so far we're still in range of the horizontal Edge but
                // make sure we're at the last of consec. horizontals when
                // matching with eMaxPair.
                if Some(ei) == e_max_pair && is_last_horz {
                    if self.edges[horz_edge].out_idx >= 0 {
                        let top = self.edges[horz_edge].top;
                        self.add_local_max_poly(horz_edge, ei, top);
                    }
                    self.delete_from_ael(horz_edge);
                    self.delete_from_ael(ei);
                    return Ok(());
                }

                let pt = IntPoint::new(self.edges[ei].curr.x, self.edges[horz_edge].curr.y);
                if dir == Direction::LeftToRight {
                    self.intersect_edges(horz_edge, ei, pt);
                } else {
                    self.intersect_edges(ei, horz_edge, pt);
                }
                let e_next = self.get_next_in_ael(ei, dir);
                self.swap_positions_in_ael(horz_edge, ei);
                e = e_next;
            }

            // Break out of loop if HorzEdge.NextInLML is not also horizontal.
            match self.edges[horz_edge].next_in_lml {
                Some(n) if self.is_horz(n) => {}
                _ => break,
            }

            horz_edge = self.update_edge_into_ael(horz_edge)?;
            if self.edges[horz_edge].out_idx >= 0 {
                let bot = self.edges[horz_edge].bot;
                self.add_out_pt(horz_edge, bot);
            }
            (dir, horz_left, horz_right) = self.get_horz_direction(horz_edge);
        }

        if self.edges[horz_edge].out_idx >= 0 && op1.is_none() {
            let o1 = self.get_last_out_pt(horz_edge);
            let mut e_next_horz = self.sorted_edges;
            while let Some(nh) = e_next_horz {
                if self.edges[nh].out_idx >= 0
                    && horz_segments_overlap(
                        self.edges[horz_edge].bot.x,
                        self.edges[horz_edge].top.x,
                        self.edges[nh].bot.x,
                        self.edges[nh].top.x,
                    )
                {
                    let op2 = self.get_last_out_pt(nh);
                    let top = self.edges[nh].top;
                    self.add_join(op2, o1, top);
                }
                e_next_horz = self.edges[nh].next_in_sel;
            }
            let top = self.edges[horz_edge].top;
            self.add_ghost_join(o1, top);
        }

        if self.edges[horz_edge].next_in_lml.is_some() {
            if self.edges[horz_edge].out_idx >= 0 {
                let top = self.edges[horz_edge].top;
                let o1 = self.add_out_pt(horz_edge, top);
                horz_edge = self.update_edge_into_ael(horz_edge)?;
                if self.edges[horz_edge].wind_delta == 0 {
                    return Ok(());
                }
                // Nb: HorzEdge is no longer horizontal here.
                let e_prev = self.edges[horz_edge].prev_in_ael;
                let e_next = self.edges[horz_edge].next_in_ael;
                let h = self.edges[horz_edge].clone();
                if let Some(ep) = e_prev
                    && self.edges[ep].curr.x == h.bot.x
                    && self.edges[ep].curr.y == h.bot.y
                    && self.edges[ep].wind_delta != 0
                    && self.edges[ep].out_idx >= 0
                    && self.edges[ep].curr.y > self.edges[ep].top.y
                    && slopes_equal_edges(&h, &self.edges[ep])
                {
                    let op2 = self.add_out_pt(ep, h.bot);
                    self.add_join(o1, op2, h.top);
                } else if let Some(en) = e_next
                    && self.edges[en].curr.x == h.bot.x
                    && self.edges[en].curr.y == h.bot.y
                    && self.edges[en].wind_delta != 0
                    && self.edges[en].out_idx >= 0
                    && self.edges[en].curr.y > self.edges[en].top.y
                    && slopes_equal_edges(&h, &self.edges[en])
                {
                    let op2 = self.add_out_pt(en, h.bot);
                    self.add_join(o1, op2, h.top);
                }
            } else {
                self.update_edge_into_ael(horz_edge)?;
            }
        } else {
            if self.edges[horz_edge].out_idx >= 0 {
                let top = self.edges[horz_edge].top;
                self.add_out_pt(horz_edge, top);
            }
            self.delete_from_ael(horz_edge);
        }
        Ok(())
    }

    fn process_intersections(&mut self, top_y: i64) -> Flow<bool> {
        if self.active_edges.is_none() {
            return Ok(true);
        }
        self.build_intersect_list(top_y);
        let il_size = self.intersect_list.len();
        if il_size == 0 {
            return Ok(true);
        }
        if il_size == 1 || self.fixup_intersection_order() {
            self.process_intersect_list();
        } else {
            return Ok(false);
        }
        self.sorted_edges = None;
        Ok(true)
    }

    fn build_intersect_list(&mut self, top_y: i64) {
        let Some(active) = self.active_edges else {
            return;
        };

        // Prepare for sorting.
        let mut e = Some(active);
        self.sorted_edges = e;
        while let Some(ei) = e {
            let edge = &mut self.edges[ei];
            edge.prev_in_sel = edge.prev_in_ael;
            edge.next_in_sel = edge.next_in_ael;
            edge.curr.x = top_x(edge, top_y);
            e = edge.next_in_ael;
        }

        // Bubblesort.
        loop {
            let mut is_modified = false;
            let Some(mut e) = self.sorted_edges else {
                break;
            };
            while let Some(e_next) = self.edges[e].next_in_sel {
                if self.edges[e].curr.x > self.edges[e_next].curr.x {
                    let mut pt = intersect_point(&self.edges[e], &self.edges[e_next]);
                    if pt.y < top_y {
                        pt = IntPoint::new(top_x(&self.edges[e], top_y), top_y);
                    }
                    self.intersect_list.push(IntersectNode {
                        edge1: e,
                        edge2: e_next,
                        pt,
                    });
                    self.swap_positions_in_sel(e, e_next);
                    is_modified = true;
                } else {
                    e = e_next;
                }
            }
            match self.edges[e].prev_in_sel {
                Some(p) => self.edges[p].next_in_sel = None,
                None => break,
            }
            if !is_modified {
                break;
            }
        }
        self.sorted_edges = None; // Important.
    }

    fn process_intersect_list(&mut self) {
        for i in 0..self.intersect_list.len() {
            let node = self.intersect_list[i];
            self.intersect_edges(node.edge1, node.edge2, node.pt);
            self.swap_positions_in_ael(node.edge1, node.edge2);
        }
        self.intersect_list.clear();
    }

    fn edges_adjacent(&self, inode: &IntersectNode) -> bool {
        self.edges[inode.edge1].next_in_sel == Some(inode.edge2)
            || self.edges[inode.edge1].prev_in_sel == Some(inode.edge2)
    }

    fn fixup_intersection_order(&mut self) -> bool {
        // Pre-condition: intersections are sorted Bottom-most first. Now it's
        // crucial that intersections are made only between adjacent edges,
        // so to ensure this the order of intersections may need adjusting.
        self.copy_ael_to_sel();
        std_sort::sort_by(&mut self.intersect_list, |node1, node2| {
            node2.pt.y < node1.pt.y
        });
        let cnt = self.intersect_list.len();
        for i in 0..cnt {
            if !self.edges_adjacent(&self.intersect_list[i]) {
                let mut j = i + 1;
                while j < cnt && !self.edges_adjacent(&self.intersect_list[j]) {
                    j += 1;
                }
                if j == cnt {
                    return false;
                }
                self.intersect_list.swap(i, j);
            }
            let node = self.intersect_list[i];
            self.swap_positions_in_sel(node.edge1, node.edge2);
        }
        true
    }

    fn do_maxima(&mut self, e: usize) -> Flow {
        let Some(e_max_pair) = self.get_maxima_pair_ex(e) else {
            if self.edges[e].out_idx >= 0 {
                let top = self.edges[e].top;
                self.add_out_pt(e, top);
            }
            self.delete_from_ael(e);
            return Ok(());
        };

        let mut e_next = self.edges[e].next_in_ael;
        while let Some(en) = e_next.filter(|&en| en != e_max_pair) {
            let top = self.edges[e].top;
            self.intersect_edges(e, en, top);
            self.swap_positions_in_ael(e, en);
            e_next = self.edges[e].next_in_ael;
        }

        if self.edges[e].out_idx == UNASSIGNED && self.edges[e_max_pair].out_idx == UNASSIGNED {
            self.delete_from_ael(e);
            self.delete_from_ael(e_max_pair);
        } else if self.edges[e].out_idx >= 0 && self.edges[e_max_pair].out_idx >= 0 {
            let top = self.edges[e].top;
            self.add_local_max_poly(e, e_max_pair, top);
            self.delete_from_ael(e);
            self.delete_from_ael(e_max_pair);
        } else if self.edges[e].wind_delta == 0 {
            let top = self.edges[e].top;
            if self.edges[e].out_idx >= 0 {
                self.add_out_pt(e, top);
                self.edges[e].out_idx = UNASSIGNED;
            }
            self.delete_from_ael(e);

            if self.edges[e_max_pair].out_idx >= 0 {
                self.add_out_pt(e_max_pair, top);
                self.edges[e_max_pair].out_idx = UNASSIGNED;
            }
            self.delete_from_ael(e_max_pair);
        } else {
            return Err(Failure); // "DoMaxima error"
        }
        Ok(())
    }

    fn is_maxima(&self, e: usize, y: i64) -> bool {
        self.edges[e].top.y == y && self.edges[e].next_in_lml.is_none()
    }

    fn is_intermediate(&self, e: usize, y: i64) -> bool {
        self.edges[e].top.y == y && self.edges[e].next_in_lml.is_some()
    }

    fn process_edges_at_top_of_scanbeam(&mut self, top_y: i64) -> Flow {
        let mut e = self.active_edges;
        while let Some(mut ei) = e {
            // 1. Process maxima, treating them as if they're 'bent'
            //    horizontal edges, but exclude maxima with horizontal edges.
            //    Nb: e can't be a horizontal.
            let mut is_maxima_edge = self.is_maxima(ei, top_y);

            if is_maxima_edge {
                let e_max_pair = self.get_maxima_pair_ex(ei);
                is_maxima_edge = e_max_pair.is_none_or(|p| !self.is_horz(p));
            }

            if is_maxima_edge {
                if self.strict_simple {
                    self.maxima.push(self.edges[ei].top.x);
                }
                let e_prev = self.edges[ei].prev_in_ael;
                self.do_maxima(ei)?;
                e = match e_prev {
                    None => self.active_edges,
                    Some(p) => self.edges[p].next_in_ael,
                };
            } else {
                // 2. Promote horizontal edges, otherwise update Curr.X and
                //    Curr.Y.
                if self.is_intermediate(ei, top_y)
                    && self.edges[ei].next_in_lml.is_some_and(|n| self.is_horz(n))
                {
                    ei = self.update_edge_into_ael(ei)?;
                    if self.edges[ei].out_idx >= 0 {
                        let bot = self.edges[ei].bot;
                        self.add_out_pt(ei, bot);
                    }
                    self.add_edge_to_sel(ei);
                } else {
                    let x = top_x(&self.edges[ei], top_y);
                    self.edges[ei].curr.x = x;
                    self.edges[ei].curr.y = top_y;
                }

                // When StrictlySimple and 'e' is being touched by another
                // edge, then make sure both edges have a vertex here.
                if self.strict_simple {
                    let e_prev = self.edges[ei].prev_in_ael;
                    if self.edges[ei].out_idx >= 0
                        && self.edges[ei].wind_delta != 0
                        && let Some(ep) = e_prev
                        && self.edges[ep].out_idx >= 0
                        && self.edges[ep].curr.x == self.edges[ei].curr.x
                        && self.edges[ep].wind_delta != 0
                    {
                        let pt = self.edges[ei].curr;
                        let op = self.add_out_pt(ep, pt);
                        let op2 = self.add_out_pt(ei, pt);
                        self.add_join(op, op2, pt); // StrictlySimple (type-3) join.
                    }
                }

                e = self.edges[ei].next_in_ael;
            }
        }

        // 3. Process horizontals at the Top of the scanbeam.
        self.maxima.sort_unstable();
        self.process_horizontals()?;
        self.maxima.clear();

        // 4. Promote intermediate vertices.
        let mut e = self.active_edges;
        while let Some(mut ei) = e {
            if self.is_intermediate(ei, top_y) {
                let mut op: Option<usize> = None;
                if self.edges[ei].out_idx >= 0 {
                    let top = self.edges[ei].top;
                    op = Some(self.add_out_pt(ei, top));
                }
                ei = self.update_edge_into_ael(ei)?;

                // If output polygons share an edge, they'll need joining
                // later.
                let e_prev = self.edges[ei].prev_in_ael;
                let e_next = self.edges[ei].next_in_ael;
                let cur = self.edges[ei].clone();
                if let Some(ep) = e_prev
                    && self.edges[ep].curr.x == cur.bot.x
                    && self.edges[ep].curr.y == cur.bot.y
                    && let Some(op) = op
                    && self.edges[ep].out_idx >= 0
                    && self.edges[ep].curr.y > self.edges[ep].top.y
                    && slopes_equal4(cur.curr, cur.top, self.edges[ep].curr, self.edges[ep].top)
                    && cur.wind_delta != 0
                    && self.edges[ep].wind_delta != 0
                {
                    let op2 = self.add_out_pt(ep, cur.bot);
                    self.add_join(op, op2, cur.top);
                } else if let Some(en) = e_next
                    && self.edges[en].curr.x == cur.bot.x
                    && self.edges[en].curr.y == cur.bot.y
                    && let Some(op) = op
                    && self.edges[en].out_idx >= 0
                    && self.edges[en].curr.y > self.edges[en].top.y
                    && slopes_equal4(cur.curr, cur.top, self.edges[en].curr, self.edges[en].top)
                    && cur.wind_delta != 0
                    && self.edges[en].wind_delta != 0
                {
                    let op2 = self.add_out_pt(en, cur.bot);
                    self.add_join(op, op2, cur.top);
                }
            }
            e = self.edges[ei].next_in_ael;
        }
        Ok(())
    }

    fn fixup_out_polyline(&mut self, outrec: usize) {
        let Some(mut pp) = self.poly_outs[outrec].pts else {
            return;
        };
        let mut last_pp = self.pts[pp].prev;
        while pp != last_pp {
            pp = self.pts[pp].next;
            if self.pts[pp].pt == self.pts[self.pts[pp].prev].pt {
                if pp == last_pp {
                    last_pp = self.pts[pp].prev;
                }
                let tmp_pp = self.pts[pp].prev;
                let pp_next = self.pts[pp].next;
                self.pts[tmp_pp].next = pp_next;
                self.pts[pp_next].prev = tmp_pp;
                pp = tmp_pp;
            }
        }

        if pp == self.pts[pp].prev {
            self.poly_outs[outrec].pts = None;
        }
    }

    fn fixup_out_polygon(&mut self, outrec: usize) {
        // FixupOutPolygon() - removes duplicate points and simplifies
        // consecutive parallel edges by removing the middle vertex.
        let mut last_ok: Option<usize> = None;
        self.poly_outs[outrec].bottom_pt = None;
        let Some(mut pp) = self.poly_outs[outrec].pts else {
            return;
        };
        let preserve_col = self.preserve_collinear || self.strict_simple;

        loop {
            let (prev, next) = (self.pts[pp].prev, self.pts[pp].next);
            if prev == pp || prev == next {
                self.poly_outs[outrec].pts = None;
                return;
            }

            // Test for duplicate points and collinear edges.
            let (pt_prev, pt, pt_next) = (self.pts[prev].pt, self.pts[pp].pt, self.pts[next].pt);
            if pt == pt_next
                || pt == pt_prev
                || (slopes_equal3(pt_prev, pt, pt_next)
                    && (!preserve_col || !pt2_is_between_pt1_and_pt3(pt_prev, pt, pt_next)))
            {
                last_ok = None;
                self.pts[prev].next = next;
                self.pts[next].prev = prev;
                pp = prev;
            } else if Some(pp) == last_ok {
                break;
            } else {
                if last_ok.is_none() {
                    last_ok = Some(pp);
                }
                pp = next;
            }
        }
        self.poly_outs[outrec].pts = Some(pp);
    }

    fn build_result(&self, polys: &mut Paths) {
        polys.reserve(self.poly_outs.len());
        for outrec in &self.poly_outs {
            let Some(pts) = outrec.pts else {
                continue;
            };
            let mut p = self.pts[pts].prev;
            let cnt = point_count(&self.pts, Some(p));
            if cnt < 2 {
                continue;
            }
            let mut pg = Path::with_capacity(cnt);
            for _ in 0..cnt {
                pg.push(self.pts[p].pt);
                p = self.pts[p].prev;
            }
            polys.push(pg);
        }
    }

    fn build_result2(&mut self, polytree: &mut PolyTree) {
        polytree.clear();
        polytree.nodes.reserve(self.poly_outs.len());
        // Add each output polygon/contour to polytree.
        for i in 0..self.poly_outs.len() {
            let cnt = point_count(&self.pts, self.poly_outs[i].pts);
            let is_open = self.poly_outs[i].is_open;
            if (is_open && cnt < 2) || (!is_open && cnt < 3) {
                continue;
            }
            self.fix_hole_linkage(i);
            let mut contour = Path::with_capacity(cnt);
            if let Some(pts) = self.poly_outs[i].pts {
                let mut op = self.pts[pts].prev;
                for _ in 0..cnt {
                    contour.push(self.pts[op].pt);
                    op = self.pts[op].prev;
                }
            }
            // Nb: polytree takes ownership of all the PolyNodes.
            self.poly_outs[i].poly_nd = Some(polytree.nodes.len());
            polytree.nodes.push(NodeData {
                contour,
                ..NodeData::default()
            });
        }

        // Fixup PolyNode links etc.
        polytree.root.children.reserve(self.poly_outs.len());
        for i in 0..self.poly_outs.len() {
            let outrec = &self.poly_outs[i];
            let Some(nd) = outrec.poly_nd else {
                continue;
            };
            if outrec.is_open {
                polytree.nodes[nd].is_open = true;
                polytree.add_child(NodeId::Root, nd);
            } else if let Some(parent_nd) =
                outrec.first_left.and_then(|fl| self.poly_outs[fl].poly_nd)
            {
                polytree.add_child(NodeId::Node(parent_nd), nd);
            } else {
                polytree.add_child(NodeId::Root, nd);
            }
        }
    }

    fn insert_edge_into_ael(&mut self, edge: usize, start_edge: Option<usize>) {
        let Some(active) = self.active_edges else {
            self.edges[edge].prev_in_ael = None;
            self.edges[edge].next_in_ael = None;
            self.active_edges = Some(edge);
            return;
        };
        if start_edge.is_none() && e2_inserts_before_e1(&self.edges[active], &self.edges[edge]) {
            self.edges[edge].prev_in_ael = None;
            self.edges[edge].next_in_ael = Some(active);
            self.edges[active].prev_in_ael = Some(edge);
            self.active_edges = Some(edge);
        } else {
            let mut start_edge = start_edge.unwrap_or(active);
            while let Some(n) = self.edges[start_edge].next_in_ael
                && !e2_inserts_before_e1(&self.edges[n], &self.edges[edge])
            {
                start_edge = n;
            }
            let next = self.edges[start_edge].next_in_ael;
            self.edges[edge].next_in_ael = next;
            if let Some(n) = next {
                self.edges[n].prev_in_ael = Some(edge);
            }
            self.edges[edge].prev_in_ael = Some(start_edge);
            self.edges[start_edge].next_in_ael = Some(edge);
        }
    }

    fn join_points(&mut self, j: usize, out_rec1: usize, out_rec2: usize) -> bool {
        let join = self.joins[j];
        let mut op1 = join.out_pt1;
        let Some(mut op2) = join.out_pt2 else {
            return false; // Unreachable: only real joins are processed.
        };
        let off_pt = join.off_pt;

        // There are 3 kinds of joins for output polygons...
        // 1. Horizontal joins where Join.OutPt1 & Join.OutPt2 are vertices
        //    anywhere along (horizontal) collinear edges (& Join.OffPt is on
        //    the same horizontal).
        // 2. Non-horizontal joins where Join.OutPt1 & Join.OutPt2 are at the
        //    same location at the Bottom of the overlapping segment (&
        //    Join.OffPt is above).
        // 3. StrictSimple joins where edges touch but are not collinear and
        //    where Join.OutPt1, Join.OutPt2 & Join.OffPt all share the same
        //    point.
        let is_horizontal = self.pts[op1].pt.y == off_pt.y;

        if is_horizontal && off_pt == self.pts[op1].pt && off_pt == self.pts[op2].pt {
            // Strictly Simple join.
            if out_rec1 != out_rec2 {
                return false;
            }
            let mut op1b = self.pts[op1].next;
            while op1b != op1 && self.pts[op1b].pt == off_pt {
                op1b = self.pts[op1b].next;
            }
            let reverse1 = self.pts[op1b].pt.y > off_pt.y;
            let mut op2b = self.pts[op2].next;
            while op2b != op2 && self.pts[op2b].pt == off_pt {
                op2b = self.pts[op2b].next;
            }
            let reverse2 = self.pts[op2b].pt.y > off_pt.y;
            if reverse1 == reverse2 {
                return false;
            }
            self.link_join(j, op1, op2, reverse1);
            true
        } else if is_horizontal {
            // Treat horizontal joins differently to non-horizontal joins
            // since with them we're not yet sure where the overlapping is.
            // OutPt1.Pt & OutPt2.Pt may be anywhere along the horizontal edge.
            let mut op1b = op1;
            while self.pts[self.pts[op1].prev].pt.y == self.pts[op1].pt.y
                && self.pts[op1].prev != op1b
                && self.pts[op1].prev != op2
            {
                op1 = self.pts[op1].prev;
            }
            while self.pts[self.pts[op1b].next].pt.y == self.pts[op1b].pt.y
                && self.pts[op1b].next != op1
                && self.pts[op1b].next != op2
            {
                op1b = self.pts[op1b].next;
            }
            if self.pts[op1b].next == op1 || self.pts[op1b].next == op2 {
                return false; // A flat 'polygon'.
            }

            let mut op2b = op2;
            while self.pts[self.pts[op2].prev].pt.y == self.pts[op2].pt.y
                && self.pts[op2].prev != op2b
                && self.pts[op2].prev != op1b
            {
                op2 = self.pts[op2].prev;
            }
            while self.pts[self.pts[op2b].next].pt.y == self.pts[op2b].pt.y
                && self.pts[op2b].next != op2
                && self.pts[op2b].next != op1
            {
                op2b = self.pts[op2b].next;
            }
            if self.pts[op2b].next == op2 || self.pts[op2b].next == op1 {
                return false; // A flat 'polygon'.
            }

            // Op1 --> Op1b & Op2 --> Op2b are the extremites of the
            // horizontal edges.
            let (p1, p1b, p2, p2b) = (
                self.pts[op1].pt,
                self.pts[op1b].pt,
                self.pts[op2].pt,
                self.pts[op2b].pt,
            );
            let Some((left, right)) = get_overlap(p1.x, p1b.x, p2.x, p2b.x) else {
                return false;
            };

            // DiscardLeftSide: when overlapping edges are joined, a spike will
            // created which needs to be cleaned up. However, we don't want
            // Op1 or Op2 caught up on the discard Side as either may still be
            // needed for other joins.
            let (pt, discard_left_side) = if p1.x >= left && p1.x <= right {
                (p1, p1.x > p1b.x)
            } else if p2.x >= left && p2.x <= right {
                (p2, p2.x > p2b.x)
            } else if p1b.x >= left && p1b.x <= right {
                (p1b, p1b.x > p1.x)
            } else {
                (p2b, p2b.x > p2.x)
            };
            self.joins[j].out_pt1 = op1;
            self.joins[j].out_pt2 = Some(op2);
            join_horz(&mut self.pts, op1, op1b, op2, op2b, pt, discard_left_side)
        } else {
            // Nb: For non-horizontal joins...
            //  1. Jr.OutPt1.Pt.Y == Jr.OutPt2.Pt.Y
            //  2. Jr.OutPt1.Pt > Jr.OffPt.Y

            // Make sure the polygons are correctly oriented.
            let mut op1b = self.pts[op1].next;
            while self.pts[op1b].pt == self.pts[op1].pt && op1b != op1 {
                op1b = self.pts[op1b].next;
            }
            let reverse1 = self.pts[op1b].pt.y > self.pts[op1].pt.y
                || !slopes_equal3(self.pts[op1].pt, self.pts[op1b].pt, off_pt);
            if reverse1 {
                op1b = self.pts[op1].prev;
                while self.pts[op1b].pt == self.pts[op1].pt && op1b != op1 {
                    op1b = self.pts[op1b].prev;
                }
                if self.pts[op1b].pt.y > self.pts[op1].pt.y
                    || !slopes_equal3(self.pts[op1].pt, self.pts[op1b].pt, off_pt)
                {
                    return false;
                }
            }
            let mut op2b = self.pts[op2].next;
            while self.pts[op2b].pt == self.pts[op2].pt && op2b != op2 {
                op2b = self.pts[op2b].next;
            }
            let reverse2 = self.pts[op2b].pt.y > self.pts[op2].pt.y
                || !slopes_equal3(self.pts[op2].pt, self.pts[op2b].pt, off_pt);
            if reverse2 {
                op2b = self.pts[op2].prev;
                while self.pts[op2b].pt == self.pts[op2].pt && op2b != op2 {
                    op2b = self.pts[op2b].prev;
                }
                if self.pts[op2b].pt.y > self.pts[op2].pt.y
                    || !slopes_equal3(self.pts[op2].pt, self.pts[op2b].pt, off_pt)
                {
                    return false;
                }
            }

            if op1b == op1
                || op2b == op2
                || op1b == op2b
                || (out_rec1 == out_rec2 && reverse1 == reverse2)
            {
                return false;
            }

            self.link_join(j, op1, op2, reverse1);
            true
        }
    }

    /// The common tail of the strictly simple and the non-horizontal join
    /// cases of `JoinPoints()`.
    fn link_join(&mut self, j: usize, op1: usize, op2: usize, reverse1: bool) {
        let pts = &mut self.pts;
        let op1b;
        if reverse1 {
            op1b = dup_out_pt(pts, op1, false);
            let op2b = dup_out_pt(pts, op2, true);
            pts[op1].prev = op2;
            pts[op2].next = op1;
            pts[op1b].next = op2b;
            pts[op2b].prev = op1b;
        } else {
            op1b = dup_out_pt(pts, op1, true);
            let op2b = dup_out_pt(pts, op2, false);
            pts[op1].next = op2;
            pts[op2].prev = op1;
            pts[op1b].prev = op2b;
            pts[op2b].next = op1b;
        }
        self.joins[j].out_pt1 = op1;
        self.joins[j].out_pt2 = Some(op1b);
    }

    fn parse_first_left(&self, mut first_left: Option<usize>) -> Option<usize> {
        while let Some(fl) = first_left {
            if self.poly_outs[fl].pts.is_some() {
                break;
            }
            first_left = self.poly_outs[fl].first_left;
        }
        first_left
    }

    fn fixup_first_lefts1(&mut self, old_out_rec: usize, new_out_rec: usize) {
        // Tests if NewOutRec contains the polygon before reassigning
        // FirstLeft.
        for i in 0..self.poly_outs.len() {
            let first_left = self.parse_first_left(self.poly_outs[i].first_left);
            if let Some(pts) = self.poly_outs[i].pts
                && first_left == Some(old_out_rec)
                && let Some(new_pts) = self.poly_outs[new_out_rec].pts
                && poly2_contains_poly1(&self.pts, pts, new_pts)
            {
                self.poly_outs[i].first_left = Some(new_out_rec);
            }
        }
    }

    fn fixup_first_lefts2(&mut self, inner_out_rec: usize, outer_out_rec: usize) {
        // A polygon has split into two such that one is now the inner of the
        // other. It's possible that these polygons now wrap around other
        // polygons, so check every polygon that's also contained by
        // OuterOutRec's FirstLeft container (including 0) to see if they've
        // become inner to the new inner polygon.
        let orfl = self.poly_outs[outer_out_rec].first_left;
        for i in 0..self.poly_outs.len() {
            let Some(pts) = self.poly_outs[i].pts else {
                continue;
            };
            if i == outer_out_rec || i == inner_out_rec {
                continue;
            }
            let first_left = self.parse_first_left(self.poly_outs[i].first_left);
            if first_left != orfl
                && first_left != Some(inner_out_rec)
                && first_left != Some(outer_out_rec)
            {
                continue;
            }
            let inner_pts = self.poly_outs[inner_out_rec].pts;
            let outer_pts = self.poly_outs[outer_out_rec].pts;
            if inner_pts.is_some_and(|p| poly2_contains_poly1(&self.pts, pts, p)) {
                self.poly_outs[i].first_left = Some(inner_out_rec);
            } else if outer_pts.is_some_and(|p| poly2_contains_poly1(&self.pts, pts, p)) {
                self.poly_outs[i].first_left = Some(outer_out_rec);
            } else if self.poly_outs[i].first_left == Some(inner_out_rec)
                || self.poly_outs[i].first_left == Some(outer_out_rec)
            {
                self.poly_outs[i].first_left = orfl;
            }
        }
    }

    fn fixup_first_lefts3(&mut self, old_out_rec: usize, new_out_rec: usize) {
        // Reassigns FirstLeft WITHOUT testing if NewOutRec contains the
        // polygon.
        for i in 0..self.poly_outs.len() {
            let first_left = self.parse_first_left(self.poly_outs[i].first_left);
            if self.poly_outs[i].pts.is_some() && first_left == Some(old_out_rec) {
                self.poly_outs[i].first_left = Some(new_out_rec);
            }
        }
    }

    fn update_out_pt_idxs(&mut self, outrec: usize) {
        let Some(start) = self.poly_outs[outrec].pts else {
            return;
        };
        let idx = self.poly_outs[outrec].idx;
        let mut op = start;
        loop {
            self.pts[op].idx = idx;
            op = self.pts[op].prev;
            if op == start {
                break;
            }
        }
    }

    fn join_common_edges(&mut self) {
        for i in 0..self.joins.len() {
            let join = self.joins[i];
            let Some(out_pt2) = join.out_pt2 else {
                continue; // Unreachable: only real joins are in the list.
            };

            let out_rec1 = self.get_out_rec(self.pts[join.out_pt1].idx);
            let mut out_rec2 = self.get_out_rec(self.pts[out_pt2].idx);

            if self.poly_outs[out_rec1].pts.is_none() || self.poly_outs[out_rec2].pts.is_none() {
                continue;
            }
            if self.poly_outs[out_rec1].is_open || self.poly_outs[out_rec2].is_open {
                continue;
            }

            // Get the polygon fragment with the correct hole state
            // (FirstLeft) before calling JoinPoints().
            let hole_state_rec = if out_rec1 == out_rec2 {
                out_rec1
            } else if self.out_rec1_right_of_out_rec2(out_rec1, out_rec2) {
                out_rec2
            } else if self.out_rec1_right_of_out_rec2(out_rec2, out_rec1) {
                out_rec1
            } else {
                self.get_lowermost_rec(out_rec1, out_rec2)
            };

            if !self.join_points(i, out_rec1, out_rec2) {
                continue;
            }
            let join = self.joins[i];

            if out_rec1 == out_rec2 {
                // Instead of joining two polygons, we've just created a new
                // one by splitting one polygon into two.
                self.poly_outs[out_rec1].pts = Some(join.out_pt1);
                self.poly_outs[out_rec1].bottom_pt = None;
                out_rec2 = self.create_out_rec();
                self.poly_outs[out_rec2].pts = join.out_pt2;

                // Update all OutRec2.Pts Idx's.
                self.update_out_pt_idxs(out_rec2);

                let (Some(pts1), Some(pts2)) =
                    (self.poly_outs[out_rec1].pts, self.poly_outs[out_rec2].pts)
                else {
                    continue; // Unreachable.
                };
                if poly2_contains_poly1(&self.pts, pts2, pts1) {
                    // OutRec1 contains OutRec2.
                    self.poly_outs[out_rec2].is_hole = !self.poly_outs[out_rec1].is_hole;
                    self.poly_outs[out_rec2].first_left = Some(out_rec1);

                    if self.using_poly_tree {
                        self.fixup_first_lefts2(out_rec2, out_rec1);
                    }

                    let rec2 = &self.poly_outs[out_rec2];
                    if (rec2.is_hole ^ self.reverse_output) == (area_op(&self.pts, rec2.pts) > 0.0)
                    {
                        reverse_poly_pt_links(&mut self.pts, rec2.pts);
                    }
                } else if poly2_contains_poly1(&self.pts, pts1, pts2) {
                    // OutRec2 contains OutRec1.
                    self.poly_outs[out_rec2].is_hole = self.poly_outs[out_rec1].is_hole;
                    self.poly_outs[out_rec1].is_hole = !self.poly_outs[out_rec2].is_hole;
                    self.poly_outs[out_rec2].first_left = self.poly_outs[out_rec1].first_left;
                    self.poly_outs[out_rec1].first_left = Some(out_rec2);

                    if self.using_poly_tree {
                        self.fixup_first_lefts2(out_rec1, out_rec2);
                    }

                    let rec1 = &self.poly_outs[out_rec1];
                    if (rec1.is_hole ^ self.reverse_output) == (area_op(&self.pts, rec1.pts) > 0.0)
                    {
                        reverse_poly_pt_links(&mut self.pts, rec1.pts);
                    }
                } else {
                    // The 2 polygons are completely separate.
                    self.poly_outs[out_rec2].is_hole = self.poly_outs[out_rec1].is_hole;
                    self.poly_outs[out_rec2].first_left = self.poly_outs[out_rec1].first_left;

                    // Fixup FirstLeft pointers that may need reassigning to
                    // OutRec2.
                    if self.using_poly_tree {
                        self.fixup_first_lefts1(out_rec1, out_rec2);
                    }
                }
            } else {
                // Joined 2 polygons together.
                self.poly_outs[out_rec2].pts = None;
                self.poly_outs[out_rec2].bottom_pt = None;
                self.poly_outs[out_rec2].idx = self.poly_outs[out_rec1].idx;

                self.poly_outs[out_rec1].is_hole = self.poly_outs[hole_state_rec].is_hole;
                if hole_state_rec == out_rec2 {
                    self.poly_outs[out_rec1].first_left = self.poly_outs[out_rec2].first_left;
                }
                self.poly_outs[out_rec2].first_left = Some(out_rec1);

                if self.using_poly_tree {
                    self.fixup_first_lefts3(out_rec2, out_rec1);
                }
            }
        }
    }

    fn do_simple_polygons(&mut self) {
        let mut i = 0;
        while i < self.poly_outs.len() {
            let outrec = i;
            i += 1;
            let Some(mut op) = self.poly_outs[outrec].pts else {
                continue;
            };
            if self.poly_outs[outrec].is_open {
                continue;
            }
            loop {
                // For each Pt in Polygon until duplicate found do...
                let mut op2 = self.pts[op].next;
                while Some(op2) != self.poly_outs[outrec].pts {
                    if self.pts[op].pt == self.pts[op2].pt
                        && self.pts[op2].next != op
                        && self.pts[op2].prev != op
                    {
                        // Split the polygon into two.
                        let op3 = self.pts[op].prev;
                        let op4 = self.pts[op2].prev;
                        self.pts[op].prev = op4;
                        self.pts[op4].next = op;
                        self.pts[op2].prev = op3;
                        self.pts[op3].next = op2;

                        self.poly_outs[outrec].pts = Some(op);
                        let outrec2 = self.create_out_rec();
                        self.poly_outs[outrec2].pts = Some(op2);
                        self.update_out_pt_idxs(outrec2);
                        if poly2_contains_poly1(&self.pts, op2, op) {
                            // OutRec2 is contained by OutRec1.
                            self.poly_outs[outrec2].is_hole = !self.poly_outs[outrec].is_hole;
                            self.poly_outs[outrec2].first_left = Some(outrec);
                            if self.using_poly_tree {
                                self.fixup_first_lefts2(outrec2, outrec);
                            }
                        } else if poly2_contains_poly1(&self.pts, op, op2) {
                            // OutRec1 is contained by OutRec2.
                            self.poly_outs[outrec2].is_hole = self.poly_outs[outrec].is_hole;
                            self.poly_outs[outrec].is_hole = !self.poly_outs[outrec2].is_hole;
                            self.poly_outs[outrec2].first_left = self.poly_outs[outrec].first_left;
                            self.poly_outs[outrec].first_left = Some(outrec2);
                            if self.using_poly_tree {
                                self.fixup_first_lefts2(outrec, outrec2);
                            }
                        } else {
                            // The 2 polygons are separate.
                            self.poly_outs[outrec2].is_hole = self.poly_outs[outrec].is_hole;
                            self.poly_outs[outrec2].first_left = self.poly_outs[outrec].first_left;
                            if self.using_poly_tree {
                                self.fixup_first_lefts1(outrec, outrec2);
                            }
                        }
                        op2 = op; // Ie get ready for the Next iteration.
                    }
                    op2 = self.pts[op2].next;
                }
                op = self.pts[op].next;
                if Some(op) == self.poly_outs[outrec].pts {
                    break;
                }
            }
        }
    }
}
