//! The trace tool: port of
//! libs/librepcb/editor/project/board/fsm/boardeditorstate_drawtrace.{h,cpp}.
//!
//! While positioning, the undo group "Draw Board Trace" contains the setup
//! (a split trace, a new segment, the start junction) followed by the live
//! preview: junction P1 at the wire mode's middle point, junction P2 (or a
//! via when changing the layer) at the target, and the traces
//! start → P1 → P2. A click connects P2 to the vias, pads, junctions and
//! traces of the net at the target (combining net segments), commits the
//! group and continues from P2 (or finishes if it was connected).

use librepcb_core::geometry::{Junction, Trace, TraceAnchor, Via};
use librepcb_core::project::board::{BoardNetSegment, BoardSegmentElements, BoardSegmentItems};
use librepcb_core::project::{
    BoardMutation, BoardNetSegmentRef, Mutation, NetSegmentId, NetSignalId,
};
use librepcb_core::types::{Layer, Length, Point, PositiveLength, UnsignedLength, Uuid};
use librepcb_core::utils::toolbox;
use librepcb_i18n::tr;

use super::add_via::ViaSettings;
use super::context::Cx;
use super::output::{BoardToolData, ToolNet, ToolSetting, WireMode};
use super::view::{BoardItemRef, FindFilter, FindFlags, anchor_position};
use super::{BoardFsmInput, State};
use crate::commands::{EditBoardSettings, EditNetClass};
use crate::error::{Error, Result};
use crate::fsm::{CursorShape, Key, KeyEvent};

/// The elements of the trace being positioned.
#[derive(Debug, Clone, Copy)]
struct Positioning {
    segment: NetSegmentId,
    net: Option<NetSignalId>,
    fixed: TraceAnchor,
    p1: Uuid,
    line1: Uuid,
    p2: Uuid,
    line2: Uuid,
    via: Uuid,
    via_shown: bool,
    layer: Layer,
    /// Group length after the setup (start of the preview).
    mark: usize,
}

/// The trace tool (upstream `BoardEditorState_DrawTrace`).
#[derive(Debug)]
pub(super) struct DrawTraceState {
    wire_mode: WireMode,
    layer: Layer,
    add_via: bool,
    via_layer: Option<Layer>,
    via: ViaSettings,
    width: Option<PositiveLength>,
    auto_width: bool,
    snap: bool,
    cursor: Point,
    target: Point,
    pos: Option<Positioning>,
}

fn brd(m: BoardMutation) -> Mutation {
    Mutation::Board(m)
}

fn text() -> String {
    tr!(
        "librepcb::editor::BoardEditorState_DrawTrace",
        "Draw Board Trace"
    )
}

/// Upstream `calcMiddlePointPos()`.
pub(super) fn middle_point(p1: Point, p2: Point, mode: WireMode) -> Point {
    let delta = p2 - p1;
    let xs: i64 = if delta.x >= Length::ZERO { 1 } else { -1 };
    let ys: i64 = if delta.y >= Length::ZERO { 1 } else { -1 };
    let (dx, dy) = (delta.x.abs(), delta.y.abs());
    match mode {
        WireMode::HV => Point::new(p2.x, p1.y),
        WireMode::VH => Point::new(p1.x, p2.y),
        WireMode::Deg9045 => {
            if dx >= dy {
                Point::new(p2.x - dy * xs, p1.y)
            } else {
                Point::new(p1.x, p2.y - dx * ys)
            }
        }
        WireMode::Deg4590 => {
            if dx >= dy {
                Point::new(p1.x + dy * xs, p2.y)
            } else {
                Point::new(p2.x, p1.y + dx * ys)
            }
        }
        WireMode::Straight => p1,
    }
}

/// The segment of a board item.
fn segment_of(cx: &Cx<'_, '_>, item: BoardItemRef) -> Option<NetSegmentId> {
    item.resolved(cx.board().ok()?)?.segment()
}

/// Upstream `combineAnchors()`: removes the junction of `a` and `b`
/// (preferring `a`) and attaches its traces to the other anchor. Returns
/// the remaining anchor.
fn combine_anchors(
    cx: &mut Cx<'_, '_>,
    segment: NetSegmentId,
    a: TraceAnchor,
    b: TraceAnchor,
) -> Result<TraceAnchor> {
    let (junction, other) = match (a, b) {
        (TraceAnchor::Junction(j), o) | (o, TraceAnchor::Junction(j)) => (j, o),
        _ => {
            return Err(Error::InvalidArgument(
                "No junction to be combined with.".into(),
            ));
        }
    };
    let board = cx.board_id();
    let seg = cx
        .board()?
        .net_segment(segment)
        .ok_or_else(|| Error::not_found("Net segment", segment))?;
    let j = TraceAnchor::Junction(junction);
    let mut add = BoardSegmentElements::default();
    let mut remove = BoardSegmentItems {
        junctions: vec![junction],
        ..Default::default()
    };
    for t in seg.traces_at(j) {
        let x = if t.p1() == j { t.p2() } else { t.p1() };
        if x != other {
            add.traces.push(Trace::new(
                Uuid::new_random(),
                t.layer(),
                t.width(),
                other,
                x,
            ));
        }
        remove.traces.push(t.uuid());
    }
    let r = BoardNetSegmentRef { board, segment };
    let mut muts = Vec::new();
    if !add.is_empty() {
        muts.push(brd(BoardMutation::AddNetSegmentElements {
            segment: r,
            elements: add,
        }));
    }
    muts.push(brd(BoardMutation::RemoveNetSegmentElements {
        segment: r,
        elements: remove,
    }));
    cx.apply(text(), muts)?;
    Ok(other)
}

/// Upstream `CmdCombineBoardNetSegments`: moves all elements of `old` into
/// `new`, replacing the junction `old_junction` by `new_anchor`.
fn combine_segments(
    cx: &mut Cx<'_, '_>,
    old: NetSegmentId,
    old_junction: Uuid,
    new: NetSegmentId,
    new_anchor: TraceAnchor,
) -> Result<()> {
    let board = cx.board_id();
    let seg = cx
        .board()?
        .net_segment(old)
        .ok_or_else(|| Error::not_found("Net segment", old))?
        .clone();
    let j = TraceAnchor::Junction(old_junction);
    let map = |a: TraceAnchor| if a == j { new_anchor } else { a };
    let mut elements = BoardSegmentElements {
        pads: seg.pads().values().cloned().collect(),
        vias: seg.vias().values().cloned().collect(),
        junctions: seg
            .junctions()
            .values()
            .filter(|x| x.uuid() != old_junction)
            .cloned()
            .collect(),
        traces: Vec::new(),
    };
    for t in seg.traces().values() {
        let (a1, a2) = (map(t.p1()), map(t.p2()));
        if a1 != a2 {
            let mut t = t.clone();
            t.set_anchors(a1, a2);
            elements.traces.push(t);
        }
    }
    cx.apply(
        text(),
        vec![
            brd(BoardMutation::RemoveNetSegment(BoardNetSegmentRef {
                board,
                segment: old,
            })),
            brd(BoardMutation::AddNetSegmentElements {
                segment: BoardNetSegmentRef {
                    board,
                    segment: new,
                },
                elements,
            }),
        ],
    )
}

impl DrawTraceState {
    pub fn new(default_width: Option<PositiveLength>) -> Self {
        Self {
            wire_mode: WireMode::HV,
            layer: Layer::TOP_COPPER,
            add_via: false,
            via_layer: None,
            via: ViaSettings::default(),
            width: default_width,
            // upstream client setting "board_editor/draw_trace/width/auto"
            auto_width: true,
            snap: true,
            cursor: Point::ORIGIN,
            target: Point::ORIGIN,
            pos: None,
        }
    }

    fn width(&self, cx: &Cx<'_, '_>) -> PositiveLength {
        self.width.unwrap_or_else(|| {
            cx.board()
                .map(|b| b.design_rules().default_trace_width())
                .unwrap_or_else(|_| PositiveLength::new(Length::new(500_000)).expect("positive"))
        })
    }

    fn net(&self) -> Option<NetSignalId> {
        self.pos.and_then(|p| p.net)
    }

    /// Upstream `startPositioning()`.
    fn start_positioning(
        &mut self,
        cx: &mut Cx<'_, '_>,
        pos: Point,
        auto_width_from_existing: bool,
        fixed: Option<BoardItemRef>,
    ) -> bool {
        self.target = self.cursor.mapped_to_grid(cx.grid());
        if let Err(e) = cx.begin(text()) {
            cx.error(e);
            return false;
        }
        self.add_via = false;
        self.via_layer = None;
        match self.start_inner(cx, pos, auto_width_from_existing, fixed) {
            Ok(()) => true,
            Err(e) => {
                cx.error(e);
                self.abort_positioning(cx);
                false
            }
        }
    }

    fn start_inner(
        &mut self,
        cx: &mut Cx<'_, '_>,
        pos: Point,
        auto_width_from_existing: bool,
        fixed: Option<BoardItemRef>,
    ) -> Result<()> {
        let grid = cx.grid();
        let pos_on_grid = pos.mapped_to_grid(grid);
        let board_id = cx.board_id();
        let copper = cx.copper_layers();
        let mut layer = self.layer;
        if !copper.contains(&layer) {
            return Err(Error::InvalidArgument(tr!(
                "librepcb::editor::BoardEditorState_DrawTrace",
                "Invalid layer selected."
            )));
        }
        let pad_not_connected = |device: bool| {
            if device {
                Error::PadNotConnected
            } else {
                Error::InvalidArgument(
                    "Sorry, this operation is not implemented yet! :-/\nPlease let us know if you need this feature so we can prioritize it. As a workaround, you can assign the pad to a net while adding it.".into(),
                )
            }
        };
        let is_fixed = fixed.is_some();
        let start = match fixed {
            Some(f) => Some(f),
            None => cx.find_item(
                pos,
                FindFlags::VIAS
                    | FindFlags::JUNCTIONS
                    | FindFlags::TRACES
                    | FindFlags::BOARD_PADS
                    | FindFlags::FOOTPRINT_PADS
                    | FindFlags::ACCEPT_NEXT_GRID_MATCH,
                &FindFilter::default(),
            ),
        };
        let start = match (start, cx.board()) {
            (Some(s), Ok(b)) => s.resolved(b),
            _ => None,
        };
        let mut segment: Option<NetSegmentId> = None;
        let mut net: Option<Option<NetSignalId>> = None;
        let mut anchor: Option<TraceAnchor> = None;
        {
            let p = cx.project();
            let board = cx.board()?;
            match start {
                Some(BoardItemRef::Junction(s, j)) => {
                    segment = Some(s);
                    anchor = Some(TraceAnchor::Junction(j));
                    if let Some(l) = board.net_segment(s).and_then(|seg| seg.junction_layer(&j))
                        && copper.contains(&l)
                    {
                        layer = l;
                    }
                }
                Some(BoardItemRef::Via(s, v)) => {
                    segment = Some(s);
                    anchor = Some(TraceAnchor::Via(v));
                    if !is_fixed
                        && let Some(via) = board.net_segment(s).and_then(|seg| seg.vias().get(&v))
                        && !via.is_on_layer(layer)
                        && copper.contains(&via.start_layer())
                    {
                        layer = via.start_layer();
                    }
                }
                Some(BoardItemRef::FootprintPad(c, u)) => {
                    let device = board
                        .device(c)
                        .ok_or_else(|| Error::not_found("Device", c))?;
                    let view = device
                        .pad(&u, p.library(), p.circuit())?
                        .ok_or_else(|| Error::not_found("Pad", u))?;
                    let n = view.net().ok_or_else(|| pad_not_connected(true))?;
                    net = Some(Some(n));
                    segment = board.footprint_pad_segment(c, u);
                    anchor = Some(view.trace_anchor());
                    if is_fixed {
                        if !view.is_on_layer(layer) && copper.contains(&view.solder_layer()) {
                            layer = view.solder_layer();
                        }
                    } else if !view.properties().is_tht() {
                        layer = view.solder_layer();
                    }
                }
                Some(BoardItemRef::Pad(s, u)) => {
                    let seg = board
                        .net_segment(s)
                        .ok_or_else(|| Error::not_found("Net segment", s))?;
                    let pad = seg
                        .pads()
                        .get(&u)
                        .ok_or_else(|| Error::not_found("Pad", u))?;
                    let n = seg.net().ok_or_else(|| pad_not_connected(false))?;
                    net = Some(Some(n));
                    segment = Some(s);
                    anchor = Some(TraceAnchor::Pad(u));
                    let smt = pad.pad().smt_layer();
                    if (!pad.pad().is_tht() || (is_fixed && !pad.pad().is_on_layer(layer)))
                        && copper.contains(&smt)
                    {
                        layer = smt;
                    }
                }
                Some(BoardItemRef::Trace(s, t)) => {
                    // Split the trace (upstream `CmdBoardSplitNetLine`).
                    let seg = board
                        .net_segment(s)
                        .ok_or_else(|| Error::not_found("Net segment", s))?;
                    let trace = seg
                        .traces()
                        .get(&t)
                        .ok_or_else(|| Error::not_found("Trace", t))?
                        .clone();
                    let a = anchor_position(p, board, s, trace.p1()).unwrap_or(pos_on_grid);
                    let b = anchor_position(p, board, s, trace.p2()).unwrap_or(pos_on_grid);
                    let split = toolbox::nearest_point_on_line(pos_on_grid, a, b);
                    layer = trace.layer();
                    segment = Some(s);
                    let junction = Junction::new(Uuid::new_random(), split);
                    let j = TraceAnchor::Junction(junction.uuid());
                    anchor = Some(j);
                    let r = BoardNetSegmentRef {
                        board: board_id,
                        segment: s,
                    };
                    let muts = vec![
                        brd(BoardMutation::AddNetSegmentElements {
                            segment: r,
                            elements: BoardSegmentElements {
                                junctions: vec![junction],
                                traces: vec![
                                    Trace::new(
                                        Uuid::new_random(),
                                        trace.layer(),
                                        trace.width(),
                                        j,
                                        trace.p1(),
                                    ),
                                    Trace::new(
                                        Uuid::new_random(),
                                        trace.layer(),
                                        trace.width(),
                                        j,
                                        trace.p2(),
                                    ),
                                ],
                                ..Default::default()
                            },
                        }),
                        brd(BoardMutation::RemoveNetSegmentElements {
                            segment: r,
                            elements: BoardSegmentItems {
                                traces: vec![t],
                                ..Default::default()
                            },
                        }),
                    ];
                    cx.apply(text(), muts)?;
                }
                _ => {}
            }
        }
        // The net of an existing segment.
        let net = match (net, segment) {
            (Some(n), _) => n,
            (None, Some(s)) => cx.board()?.net_segment(s).and_then(|seg| seg.net()),
            (None, None) => None,
        };
        // Create a new segment if none found, with the start junction if
        // there is no anchor.
        let segment = match segment {
            Some(s) => s,
            None => {
                let mut seg = BoardNetSegment::new(Uuid::new_random(), net);
                if anchor.is_none() {
                    let junction = Junction::new(Uuid::new_random(), pos_on_grid);
                    anchor = Some(TraceAnchor::Junction(junction.uuid()));
                    seg.insert_junction(junction);
                }
                let id = seg.id();
                cx.apply(
                    text(),
                    vec![brd(BoardMutation::AddNetSegment {
                        board: board_id,
                        segment: seg,
                    })],
                )?;
                id
            }
        };
        let anchor = match anchor {
            Some(a) => a,
            None => {
                let junction = Junction::new(Uuid::new_random(), pos_on_grid);
                let a = TraceAnchor::Junction(junction.uuid());
                cx.apply(
                    text(),
                    vec![brd(BoardMutation::AddNetSegmentElements {
                        segment: BoardNetSegmentRef {
                            board: board_id,
                            segment,
                        },
                        elements: BoardSegmentElements {
                            junctions: vec![junction],
                            ..Default::default()
                        },
                    })],
                )?;
                a
            }
        };
        self.layer = layer;
        // Automatic width.
        if self.auto_width {
            let board = cx.board()?;
            let mut width = board.design_rules().default_trace_width();
            let median = board.net_segment(segment).and_then(|seg| {
                let mut widths: Vec<PositiveLength> =
                    seg.traces_at(anchor).map(Trace::width).collect();
                widths.sort();
                widths.get(widths.len() / 2).copied()
            });
            if auto_width_from_existing && let Some(m) = median {
                width = m;
            } else if let Some(w) = net
                .and_then(|n| cx.project().circuit().net_signal(n))
                .and_then(|n| cx.project().circuit().net_class(n.net_class()))
                .and_then(|c| c.default_trace_width())
            {
                width = w;
            }
            self.width = Some(width);
        }
        self.pos = Some(Positioning {
            segment,
            net,
            fixed: anchor,
            p1: Uuid::new_random(),
            line1: Uuid::new_random(),
            p2: Uuid::new_random(),
            line2: Uuid::new_random(),
            via: Uuid::new_random(),
            via_shown: false,
            layer,
            mark: cx.group_len(),
        });
        self.update_positions(cx)?;
        cx.highlight_nets(net);
        Ok(())
    }

    /// The items of the preview (excluded from hit tests).
    fn preview_items(&self) -> Vec<BoardItemRef> {
        match self.pos {
            Some(p) => vec![
                BoardItemRef::Via(p.segment, p.via),
                BoardItemRef::Junction(p.segment, p.p1),
                BoardItemRef::Junction(p.segment, p.p2),
                BoardItemRef::Trace(p.segment, p.line1),
                BoardItemRef::Trace(p.segment, p.line2),
            ],
            None => Vec::new(),
        }
    }

    /// Upstream `updateNetpointPositions()` and `showVia()`.
    fn update_positions(&mut self, cx: &mut Cx<'_, '_>) -> Result<()> {
        let Some(mut p) = self.pos else {
            return Ok(());
        };
        self.target = self.cursor.mapped_to_grid(cx.grid());
        let mut on_via = false;
        let except = self.preview_items();
        if self.snap {
            let item = cx.find_item(
                self.cursor,
                FindFlags::VIAS
                    | FindFlags::JUNCTIONS
                    | FindFlags::TRACES
                    | FindFlags::BOARD_PADS
                    | FindFlags::FOOTPRINT_PADS
                    | FindFlags::ACCEPT_NEXT_GRID_MATCH,
                &FindFilter {
                    layer: Some(p.layer),
                    net: Some(p.net),
                    except: &except,
                },
            );
            let project = cx.project();
            let board = cx.board()?;
            match item {
                Some(BoardItemRef::Via(s, v)) => {
                    if let Some(via) = board.net_segment(s).and_then(|seg| seg.vias().get(&v)) {
                        self.target = via.position();
                        on_via = true;
                    }
                }
                Some(BoardItemRef::FootprintPad(c, u)) => {
                    if let Some(pad) = board.device(c).and_then(|d| {
                        d.pad(&u, project.library(), project.circuit())
                            .ok()
                            .flatten()
                    }) {
                        self.target = pad.position();
                        on_via = pad.properties().is_tht();
                    }
                }
                Some(BoardItemRef::Pad(s, u)) => {
                    if let Some(pad) = board.net_segment(s).and_then(|seg| seg.pads().get(&u)) {
                        self.target = pad.pad().position();
                        on_via = pad.pad().is_tht();
                    }
                }
                Some(BoardItemRef::Junction(s, j)) => {
                    if let Some(j) = board.net_segment(s).and_then(|seg| seg.junctions().get(&j)) {
                        self.target = j.position();
                    }
                }
                Some(BoardItemRef::Trace(s, t)) => {
                    if let Some(trace) = board.net_segment(s).and_then(|seg| seg.traces().get(&t))
                        && let (Some(a), Some(b)) = (
                            anchor_position(project, board, s, trace.p1()),
                            anchor_position(project, board, s, trace.p2()),
                        )
                    {
                        self.target = toolbox::nearest_point_on_line(self.target, a, b);
                    }
                }
                _ => {}
            }
        }
        let board_id = cx.board_id();
        let fixed_pos = {
            let board = cx.board()?;
            anchor_position(cx.project(), board, p.segment, p.fixed).unwrap_or(self.target)
        };
        let middle = middle_point(fixed_pos, self.target, self.wire_mode);
        p.via_shown = self.add_via && !on_via;
        let width = self.width(cx);
        let p1 = TraceAnchor::Junction(p.p1);
        let mut elements = BoardSegmentElements {
            junctions: vec![Junction::new(p.p1, middle)],
            traces: vec![Trace::new(p.line1, p.layer, width, p.fixed, p1)],
            ..Default::default()
        };
        if p.via_shown {
            let net = p.net;
            let mut via: Via = self.via.via(p.via, self.target)?;
            // Keep the via properties (drill/size) of the tool.
            let _ = net;
            via.set_position(self.target);
            elements.vias.push(via);
            elements.traces.push(Trace::new(
                p.line2,
                p.layer,
                width,
                p1,
                TraceAnchor::Via(p.via),
            ));
        } else {
            elements.junctions.push(Junction::new(p.p2, self.target));
            elements.traces.push(Trace::new(
                p.line2,
                p.layer,
                width,
                p1,
                TraceAnchor::Junction(p.p2),
            ));
        }
        self.pos = Some(p);
        cx.preview(
            p.mark,
            vec![brd(BoardMutation::AddNetSegmentElements {
                segment: BoardNetSegmentRef {
                    board: board_id,
                    segment: p.segment,
                },
                elements,
            })],
        )
    }

    /// Upstream `addNextNetPoint()`.
    fn add_next_point(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        let Some(p) = self.pos else {
            return false;
        };
        // Abort if no via is added and no line was drawn.
        let fixed_pos = cx
            .board()
            .ok()
            .and_then(|b| anchor_position(cx.project(), b, p.segment, p.fixed));
        if !p.via_shown && fixed_pos == Some(self.target) {
            self.abort_positioning_impl(cx, true);
            return false;
        }
        let result = self.connect_target(cx, p);
        let finish = match result {
            Ok(finish) => finish,
            Err(e) => {
                cx.error(e);
                self.abort_positioning(cx);
                return false;
            }
        };
        if let Err(e) = cx.commit() {
            cx.error(e);
            self.abort_positioning(cx);
            return false;
        }
        let next = finish.1;
        self.pos = None;
        cx.highlight_nets([]);
        match (finish.0, next) {
            (false, Some(next)) => {
                let target = self.target;
                self.start_positioning(cx, target, true, Some(next))
            }
            _ => {
                // Upstream `abortPositioning(true, true)`: simplify the
                // segment of the finished trace.
                let segment = cx.board().ok().and_then(|b| {
                    b.net_segments()
                        .iter()
                        .find(|(_, s)| s.traces().contains_key(&p.line1))
                        .map(|(id, _)| *id)
                });
                if let Some(segment) = segment {
                    simplify_segments(cx, [segment]);
                }
                true
            }
        }
    }

    /// Connects the end of the preview to the anchors at the target;
    /// returns whether the trace is finished and the next start anchor.
    fn connect_target(
        &mut self,
        cx: &mut Cx<'_, '_>,
        p: Positioning,
    ) -> Result<(bool, Option<BoardItemRef>)> {
        let target = self.target;
        let except = self.preview_items();
        let items = cx.find_items(
            target,
            FindFlags::VIAS | FindFlags::JUNCTIONS | FindFlags::TRACES,
            &FindFilter {
                layer: if self.add_via { None } else { Some(p.layer) },
                net: Some(p.net),
                except: &except,
            },
        );
        let mut others: Vec<(TraceAnchor, Option<NetSegmentId>)> = Vec::new();
        let at_target = |cx: &Cx<'_, '_>, item: BoardItemRef| -> bool {
            let Ok(board) = cx.board() else {
                return false;
            };
            let pos = match item {
                BoardItemRef::Via(s, u) => board
                    .net_segment(s)
                    .and_then(|seg| seg.vias().get(&u))
                    .map(|v| v.position()),
                BoardItemRef::Junction(s, u) => board
                    .net_segment(s)
                    .and_then(|seg| seg.junctions().get(&u))
                    .map(|j| j.position()),
                BoardItemRef::Pad(s, u) => board
                    .net_segment(s)
                    .and_then(|seg| seg.pads().get(&u))
                    .map(|pad| pad.pad().position()),
                BoardItemRef::FootprintPad(c, u) => board.device(c).and_then(|d| {
                    d.pad(&u, cx.project().library(), cx.project().circuit())
                        .ok()
                        .flatten()
                        .map(|pad| pad.position())
                }),
                _ => None,
            };
            pos == Some(target)
        };
        if p.via_shown && self.via_layer.is_some() {
            // Only the combination with one via can be handled correctly.
            if let Some(l) = self.via_layer {
                self.layer = l;
            }
        } else {
            for item in &items {
                if let BoardItemRef::Via(s, u) = *item
                    && (self.snap || at_target(cx, *item))
                {
                    others.push((TraceAnchor::Via(u), Some(s)));
                    if self.add_via
                        && let Some(l) = self.via_layer
                    {
                        self.layer = l;
                    }
                }
            }
            let pad = cx.find_item(
                target,
                FindFlags::BOARD_PADS
                    | FindFlags::FOOTPRINT_PADS
                    | FindFlags::ACCEPT_NEXT_GRID_MATCH,
                &FindFilter {
                    layer: Some(p.layer),
                    net: Some(p.net),
                    except: &except,
                },
            );
            if let Some(pad) = pad
                && (self.snap || at_target(cx, pad))
            {
                let board = cx.board()?;
                let project = cx.project();
                match pad {
                    BoardItemRef::FootprintPad(c, u) => {
                        let view = board.device(c).and_then(|d| {
                            d.pad(&u, project.library(), project.circuit())
                                .ok()
                                .flatten()
                        });
                        if let Some(view) = view {
                            let tht = view.properties().is_tht();
                            others.push((view.trace_anchor(), board.footprint_pad_segment(c, u)));
                            if self.add_via
                                && tht
                                && let Some(l) = self.via_layer
                            {
                                self.layer = l;
                            }
                        }
                    }
                    BoardItemRef::Pad(s, u) => {
                        let tht = board
                            .net_segment(s)
                            .and_then(|seg| seg.pads().get(&u))
                            .is_some_and(|pad| pad.pad().is_tht());
                        others.push((TraceAnchor::Pad(u), Some(s)));
                        if self.add_via
                            && tht
                            && let Some(l) = self.via_layer
                        {
                            self.layer = l;
                        }
                    }
                    _ => {}
                }
            }
        }
        for item in &items {
            if let BoardItemRef::Junction(s, u) = *item
                && (self.snap || at_target(cx, *item))
            {
                others.push((TraceAnchor::Junction(u), Some(s)));
            }
        }
        // Split traces at the target.
        for item in &items {
            let BoardItemRef::Trace(s, t) = *item else {
                continue;
            };
            let (trace, s) = {
                let board = cx.board()?;
                let Some(s) = BoardItemRef::Trace(s, t)
                    .resolved(board)
                    .and_then(|i| i.segment())
                else {
                    continue;
                };
                let Some(trace) = board.net_segment(s).and_then(|seg| seg.traces().get(&t)) else {
                    continue;
                };
                (trace.clone(), s)
            };
            if others
                .iter()
                .any(|(a, _)| *a == trace.p1() || *a == trace.p2())
            {
                continue;
            }
            let junction = Junction::new(Uuid::new_random(), target);
            let j = TraceAnchor::Junction(junction.uuid());
            let r = BoardNetSegmentRef {
                board: cx.board_id(),
                segment: s,
            };
            cx.apply(
                text(),
                vec![
                    brd(BoardMutation::AddNetSegmentElements {
                        segment: r,
                        elements: BoardSegmentElements {
                            junctions: vec![junction],
                            traces: vec![
                                Trace::new(
                                    Uuid::new_random(),
                                    trace.layer(),
                                    trace.width(),
                                    j,
                                    trace.p1(),
                                ),
                                Trace::new(
                                    Uuid::new_random(),
                                    trace.layer(),
                                    trace.width(),
                                    j,
                                    trace.p2(),
                                ),
                            ],
                            ..Default::default()
                        },
                    }),
                    brd(BoardMutation::RemoveNetSegmentElements {
                        segment: r,
                        elements: BoardSegmentItems {
                            traces: vec![t],
                            ..Default::default()
                        },
                    }),
                ],
            )?;
            others.push((j, Some(s)));
        }

        let mut current = p.segment;
        let mut combining = if p.via_shown {
            TraceAnchor::Via(p.via)
        } else {
            TraceAnchor::Junction(p.p2)
        };
        // Remove P1 if it is at the start or end position.
        let (middle, fixed_pos, end_pos) = {
            let board = cx.board()?;
            let project = cx.project();
            let middle = anchor_position(project, board, current, TraceAnchor::Junction(p.p1));
            let fixed = anchor_position(project, board, current, p.fixed);
            let end = match others.first() {
                Some((a, Some(s))) => anchor_position(project, board, *s, *a),
                Some((TraceAnchor::FootprintPad { device, pad }, None)) => board
                    .device(librepcb_core::project::ComponentInstanceId(*device))
                    .and_then(|d| {
                        d.pad(pad, project.library(), project.circuit())
                            .ok()
                            .flatten()
                    })
                    .map(|v| v.position()),
                _ => Some(target),
            };
            (middle, fixed, end)
        };
        if middle.is_some() && (middle == fixed_pos || middle == end_pos) {
            combining = combine_anchors(cx, current, TraceAnchor::Junction(p.p1), combining)?;
        }
        let finish = !others.is_empty() && !self.add_via;
        for (other, other_seg) in others.clone() {
            // Skip anchors which no longer exist (combined already).
            let other_seg = match other {
                TraceAnchor::FootprintPad { device, pad } => {
                    // The segment of the pad's traces, if any.
                    cx.board()?.footprint_pad_segment(
                        librepcb_core::project::ComponentInstanceId(device),
                        pad,
                    )
                }
                TraceAnchor::Junction(u) => {
                    other_seg.and_then(|s| segment_of(cx, BoardItemRef::Junction(s, u)))
                }
                TraceAnchor::Via(u) => {
                    other_seg.and_then(|s| segment_of(cx, BoardItemRef::Via(s, u)))
                }
                TraceAnchor::Pad(u) => {
                    other_seg.and_then(|s| segment_of(cx, BoardItemRef::Pad(s, u)))
                }
            };
            let exists = match other {
                TraceAnchor::FootprintPad { .. } => true,
                _ => other_seg.is_some(),
            };
            if !exists {
                continue;
            }
            match other_seg {
                // Same segment, or a pad without traces: combine within
                // the current segment.
                Some(s) if s == current => {
                    combining = combine_anchors(cx, current, combining, other)?;
                }
                None => {
                    combining = combine_anchors(cx, current, combining, other)?;
                }
                Some(s) => {
                    if let TraceAnchor::Junction(j) = combining {
                        combine_segments(cx, current, j, s, other)?;
                        current = s;
                        combining = other;
                    } else if let TraceAnchor::Junction(j) = other {
                        combine_segments(cx, s, j, current, combining)?;
                    }
                }
            }
        }
        if p.via_shown {
            // Connect the other junctions at the via position.
            let junctions: Vec<Uuid> = cx
                .find_items(
                    target,
                    FindFlags::JUNCTIONS,
                    &FindFilter {
                        net: Some(p.net),
                        ..Default::default()
                    },
                )
                .into_iter()
                .filter_map(|i| match i {
                    BoardItemRef::Junction(s, u) if s == current => Some(u),
                    _ => None,
                })
                .collect();
            for j in junctions {
                if segment_of(cx, BoardItemRef::Junction(current, j)) == Some(current) {
                    combine_anchors(
                        cx,
                        current,
                        TraceAnchor::Via(p.via),
                        TraceAnchor::Junction(j),
                    )?;
                }
            }
        }
        // The next start anchor.
        let next = if p.via_shown {
            Some(BoardItemRef::Via(current, p.via))
        } else if segment_of(cx, BoardItemRef::Junction(current, p.p2)).is_some() {
            Some(BoardItemRef::Junction(current, p.p2))
        } else {
            match combining {
                TraceAnchor::Junction(u) => Some(BoardItemRef::Junction(current, u)),
                TraceAnchor::Via(u) => Some(BoardItemRef::Via(current, u)),
                TraceAnchor::Pad(u) => Some(BoardItemRef::Pad(current, u)),
                TraceAnchor::FootprintPad { device, pad } => Some(BoardItemRef::FootprintPad(
                    librepcb_core::project::ComponentInstanceId(device),
                    pad,
                )),
            }
        };
        Ok((finish, next))
    }

    /// Upstream `abortPositioning()` (without the segment simplification,
    /// which is not ported).
    fn abort_positioning(&mut self, cx: &mut Cx<'_, '_>) {
        self.abort_positioning_impl(cx, false);
    }

    /// Upstream `abortPositioning(showErrMsgBox, simplifySegment)`: with
    /// `simplify`, the current net segment is simplified afterwards
    /// (`CmdSimplifyBoardNetSegments` as a separate undo step).
    fn abort_positioning_impl(&mut self, cx: &mut Cx<'_, '_>, simplify: bool) {
        let segment = self.pos.map(|p| p.segment);
        cx.highlight_nets([]);
        self.pos = None;
        self.add_via = false;
        self.via_layer = None;
        if cx.is_group_active() {
            cx.abort();
        }
        if simplify && let Some(segment) = segment {
            simplify_segments(cx, [segment]);
        }
    }

    /// Upstream `setLayer()`.
    fn set_layer(&mut self, cx: &mut Cx<'_, '_>, layer: Layer) {
        if !cx.copper_layers().contains(&layer) {
            return;
        }
        match self.pos {
            Some(p) if layer != self.layer => {
                // Start a new trace from a via or THT pad on the new layer,
                // else add a via at the end of the current trace.
                let restart = match p.fixed {
                    TraceAnchor::Via(u) => Some(BoardItemRef::Via(p.segment, u)),
                    TraceAnchor::FootprintPad { device, pad } => {
                        let c = librepcb_core::project::ComponentInstanceId(device);
                        let tht = cx.board().ok().and_then(|b| {
                            b.device(c).and_then(|d| {
                                d.pad(&pad, cx.project().library(), cx.project().circuit())
                                    .ok()
                                    .flatten()
                                    .map(|v| v.properties().is_tht())
                            })
                        });
                        (tht == Some(true)).then_some(BoardItemRef::FootprintPad(c, pad))
                    }
                    TraceAnchor::Pad(u) => {
                        let tht = cx.board().ok().and_then(|b| {
                            b.net_segment(p.segment)
                                .and_then(|s| s.pads().get(&u))
                                .map(|pad| pad.pad().is_tht())
                        });
                        (tht == Some(true)).then_some(BoardItemRef::Pad(p.segment, u))
                    }
                    TraceAnchor::Junction(_) => None,
                };
                if let Some(start) = restart {
                    let start_pos = cx
                        .board()
                        .ok()
                        .and_then(|b| anchor_position(cx.project(), b, p.segment, p.fixed))
                        .unwrap_or(self.target);
                    self.abort_positioning(cx);
                    self.layer = layer;
                    self.start_positioning(cx, start_pos, true, Some(start));
                } else {
                    self.add_via = true;
                    self.via_layer = Some(layer);
                    if let Err(e) = self.update_positions(cx) {
                        cx.error(e);
                    }
                }
            }
            _ => {
                self.add_via = false;
                self.via_layer = None;
                self.layer = layer;
                if self.pos.is_some()
                    && let Err(e) = self.update_positions(cx)
                {
                    cx.error(e);
                }
            }
        }
    }

    fn update(&mut self, cx: &mut Cx<'_, '_>) {
        if self.pos.is_some()
            && let Err(e) = self.update_positions(cx)
        {
            cx.error(e);
        }
    }

    fn save_width_in_board(&mut self, cx: &mut Cx<'_, '_>) {
        let width = self.width(cx);
        let Ok(board) = cx.board() else {
            return;
        };
        let mut rules = board.design_rules().clone();
        rules.set_default_trace_width(width);
        let id = cx.board_id();
        // Like upstream, the change is part of the active trace group.
        if let Err(e) = cx.exec(EditBoardSettings {
            board: Some(id),
            design_rules: Some(rules),
            ..Default::default()
        }) {
            cx.error(e);
        }
    }

    fn save_width_in_net_class(&mut self, cx: &mut Cx<'_, '_>) {
        let width = self.width(cx);
        let Some(class) = self
            .net()
            .and_then(|n| cx.project().circuit().net_signal(n))
            .map(|n| n.net_class())
        else {
            return;
        };
        if let Err(e) = cx.exec(EditNetClass {
            net_class: class,
            name: None,
            default_trace_width: Some(width),
            default_via_drill: None,
            inherit_defaults: false,
        }) {
            cx.error(e);
        }
    }
}

impl State for DrawTraceState {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        cx.set_cursor(Some(CursorShape::Cross));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.abort_positioning_impl(cx, true);
        cx.set_cursor(None);
        true
    }

    fn process(&mut self, cx: &mut Cx<'_, '_>, input: &BoardFsmInput) -> bool {
        match input {
            BoardFsmInput::Abort => {
                if self.pos.is_some() {
                    // Just finish the current trace, not the tool.
                    self.abort_positioning_impl(cx, true);
                    true
                } else {
                    false
                }
            }
            BoardFsmInput::KeyPressed(KeyEvent {
                key: Key::Shift, ..
            }) if self.pos.is_some() => {
                self.snap = false;
                self.update(cx);
                true
            }
            BoardFsmInput::KeyReleased(KeyEvent {
                key: Key::Shift, ..
            }) if self.pos.is_some() => {
                self.snap = true;
                self.update(cx);
                true
            }
            BoardFsmInput::PointerMoved(e) => {
                if self.pos.is_some() {
                    self.snap = !e.modifiers.shift;
                    self.cursor = e.pos;
                    self.update(cx);
                    true
                } else {
                    self.cursor = e.pos;
                    false
                }
            }
            BoardFsmInput::LeftPressed(e) | BoardFsmInput::LeftDoubleClicked(e) => {
                if self.pos.is_some() {
                    self.add_next_point(cx);
                } else {
                    self.cursor = e.pos;
                    // Shift disables taking the width of existing traces.
                    self.start_positioning(cx, e.pos, !e.modifiers.shift, None);
                }
                true
            }
            BoardFsmInput::RightReleased(e) => {
                self.cursor = e.pos;
                if self.pos.is_some() {
                    self.wire_mode = self.wire_mode.next();
                    self.update(cx);
                    true
                } else {
                    false
                }
            }
            BoardFsmInput::ToolSetting(setting) => {
                match setting {
                    ToolSetting::WireMode(m) => {
                        self.wire_mode = *m;
                        self.update(cx);
                    }
                    ToolSetting::Layer(l) => self.set_layer(cx, *l),
                    ToolSetting::AutoWidth(a) => self.auto_width = *a,
                    ToolSetting::TraceWidth(w) => {
                        self.width = Some(*w);
                        self.update(cx);
                    }
                    ToolSetting::LineWidth(w) => {
                        if let Ok(w) = PositiveLength::new(**w) {
                            self.width = Some(w);
                            self.update(cx);
                        }
                    }
                    ToolSetting::ViaDrill(d) => {
                        let mut s = self.via;
                        s.set_drill(cx, self.net(), *d);
                        self.via = s;
                        self.update(cx);
                    }
                    ToolSetting::ViaSize(size) => {
                        let mut s = self.via;
                        s.set_size(cx, self.net(), *size);
                        self.via = s;
                        self.update(cx);
                    }
                    ToolSetting::SaveTraceWidthInBoard => self.save_width_in_board(cx),
                    ToolSetting::SaveTraceWidthInNetClass => self.save_width_in_net_class(cx),
                    ToolSetting::SaveViaDrillInBoard => {
                        let net = self.net();
                        self.via.save_drill_in_board(cx, net);
                    }
                    ToolSetting::SaveViaDrillInNetClass => {
                        let net = self.net();
                        self.via.save_drill_in_net_class(cx, net);
                    }
                    _ => return false,
                }
                true
            }
            _ => false,
        }
    }

    fn tool_data(&self, cx: &Cx<'_, '_>) -> BoardToolData {
        let p = cx.project();
        let net = self.net();
        BoardToolData {
            wire_mode: self.wire_mode,
            net: ToolNet { auto: false, net },
            net_class_name: net
                .and_then(|n| p.circuit().net_signal(n))
                .and_then(|n| p.circuit().net_class(n.net_class()))
                .map(|c| c.name().to_string())
                .unwrap_or_default(),
            layers: cx.copper_layers(),
            layer: Some(
                self.via_layer
                    .filter(|_| self.add_via)
                    .unwrap_or(self.layer),
            ),
            line_width: Some(UnsignedLength::from(self.width(cx))),
            auto_width: self.auto_width,
            drill: self.via.drill(cx, net),
            auto_drill: self.via.drill.is_none(),
            size: self.via.size(cx, net),
            auto_size: self.via.size.is_none(),
            ..Default::default()
        }
    }
}

/// Simplifies net segments as a separate undo step (upstream
/// `execCmd(new CmdSimplifyBoardNetSegments(...))`); errors are logged.
pub(super) fn simplify_segments(
    cx: &mut Cx<'_, '_>,
    segments: impl IntoIterator<Item = NetSegmentId>,
) {
    let board = cx.board_id();
    let segments = segments
        .into_iter()
        .filter(|s| cx.board().is_ok_and(|b| b.net_segment(*s).is_some()))
        .collect::<std::collections::BTreeSet<_>>();
    if segments.is_empty() {
        return;
    }
    if let Err(e) = cx.exec(super::simplify::SimplifyBoardNetSegments { board, segments }) {
        log::error!("Failed to simplify net segments: {e}");
    }
}
