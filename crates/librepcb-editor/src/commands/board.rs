//! Board commands: port of the result of
//! libs/librepcb/editor/project/board/fsm/boardeditorstate_drawtrace,
//! boardeditorstate_addvia, boardeditorstate_drawplane,
//! boardeditorstate_drawpolygon, boardeditorstate_addhole,
//! boardeditorstate_drawzone and boardeditorstate_addstroketext, and of
//! libs/librepcb/editor/project/cmd/cmdcombineboardnetsegments,
//! cmdboardsplitnetline, cmdremoveboarditems, cmdboardplaneadd,
//! cmdboardplaneedit, cmdboardpolygonadd, cmdboardholeadd, cmdboardzoneadd,
//! cmdboardstroketextadd and cmdboardedit (`.{h,cpp}`).
//!
//! Differences to upstream: traces are given as anchors and points instead
//! of mouse clicks (no snapping); segments are combined with the element
//! UUIDs kept (upstream creates new junctions); a via connects to the
//! traces of its net at its position in one new segment (upstream splits
//! and combines step by step); removing traces from other commands does
//! not remove unused library elements (only [`RemoveBoardItems`] does, like
//! upstream).

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::geometry::{Junction, Path, Trace, TraceAnchor, Vertex, Via, ZoneRules};
use librepcb_core::library::org::BoardDesignRuleCheckSettings;
use librepcb_core::project::board::{
    BoardDesignRules, BoardHoleData, BoardItem, BoardItemKind, BoardNetSegment,
    BoardNetSegmentSplitter, BoardPlane, BoardPolygonData, BoardSegmentElements, BoardSettings,
    BoardStrokeTextData, BoardZoneData, PlaneConnectStyle,
};
use librepcb_core::project::{
    BoardId, BoardMutation, BoardNetSegmentRef, ComponentInstanceId, DeviceRef, Mutation,
    NetSegmentId, NetSignalId, PlaneId, PlaneRef, Project,
};
use librepcb_core::types::{
    Alignment, Angle, Layer, Length, MaskConfig, Point, PositiveLength, StrokeTextSpacing,
    UnsignedLength, Uuid,
};
use librepcb_core::utils::toolbox;
use librepcb_i18n::tr;

use super::library::remove_unused_library_elements;
use super::{NetRef, PadRef, resolve};
use crate::editor::{Command, Transaction};
use crate::error::{Error, Result};

fn brd(m: BoardMutation) -> Mutation {
    Mutation::Board(m)
}

fn board_ref(tx: &Transaction<'_>, board: Option<BoardId>) -> Result<BoardId> {
    Ok(resolve::board(tx.project(), board)?.id())
}

/// Items of a board to remove, see [`RemoveBoardItems`]. Net segment
/// elements are given as (segment, element).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BoardSelection {
    /// Devices (by component); traces at their pads end in junctions.
    #[serde(default)]
    pub devices: Vec<ComponentInstanceId>,
    /// Standalone pads (replaced by junctions in their traces).
    #[serde(default)]
    pub pads: Vec<(NetSegmentId, Uuid)>,
    /// Vias (replaced by junctions in their traces).
    #[serde(default)]
    pub vias: Vec<(NetSegmentId, Uuid)>,
    /// Junctions (with their traces).
    #[serde(default)]
    pub junctions: Vec<(NetSegmentId, Uuid)>,
    /// Traces.
    #[serde(default)]
    pub traces: Vec<(NetSegmentId, Uuid)>,
    /// Planes.
    #[serde(default)]
    pub planes: Vec<PlaneId>,
    /// Zones.
    #[serde(default)]
    pub zones: Vec<Uuid>,
    /// Polygons.
    #[serde(default)]
    pub polygons: Vec<Uuid>,
    /// Stroke texts of the board.
    #[serde(default)]
    pub stroke_texts: Vec<Uuid>,
    /// Stroke texts of devices as (component, text).
    #[serde(default)]
    pub device_texts: Vec<(ComponentInstanceId, Uuid)>,
    /// Holes.
    #[serde(default)]
    pub holes: Vec<Uuid>,
}

#[derive(Default)]
struct SegmentItems {
    pads_to_disconnect: BTreeMap<TraceAnchor, Point>,
    pads: BTreeSet<Uuid>,
    vias: BTreeSet<Uuid>,
    junctions: BTreeSet<Uuid>,
    traces: BTreeSet<Uuid>,
}

/// Removes board items (upstream `CmdRemoveBoardItems`, see
/// [`RemoveBoardItems`]); `cleanup_library` removes unused library
/// elements afterwards. Returns whether anything was removed.
pub(crate) fn remove_board_items(
    tx: &mut Transaction<'_>,
    board_id: BoardId,
    selection: &BoardSelection,
    cleanup_library: bool,
) -> Result<bool> {
    let p = tx.project();
    let board = p
        .board(board_id)
        .ok_or_else(|| Error::not_found("Board", board_id))?;

    // Determine the affected net segments and their items to remove.
    let mut items: BTreeMap<NetSegmentId, SegmentItems> = BTreeMap::new();
    for component in &selection.devices {
        let device = board
            .device(*component)
            .ok_or_else(|| Error::not_found("Device", component))?;
        for pad in device.pads(p.library(), p.circuit())? {
            if let Some(segment) = board.footprint_pad_segment(*component, pad.uuid()) {
                items
                    .entry(segment)
                    .or_default()
                    .pads_to_disconnect
                    .insert(pad.trace_anchor(), pad.position());
            }
        }
    }
    let segment = |id: &NetSegmentId| {
        board
            .net_segment(*id)
            .ok_or_else(|| Error::not_found("Net segment", id))
    };
    for (id, uuid) in &selection.pads {
        if !segment(id)?.pads().contains_key(uuid) {
            return Err(Error::not_found("Pad", uuid));
        }
        items.entry(*id).or_default().pads.insert(*uuid);
    }
    for (id, uuid) in &selection.vias {
        if !segment(id)?.vias().contains_key(uuid) {
            return Err(Error::not_found("Via", uuid));
        }
        items.entry(*id).or_default().vias.insert(*uuid);
    }
    for (id, uuid) in &selection.junctions {
        let seg = segment(id)?;
        if !seg.junctions().contains_key(uuid) {
            return Err(Error::not_found("Junction", uuid));
        }
        let entry = items.entry(*id).or_default();
        entry.junctions.insert(*uuid);
        entry
            .traces
            .extend(seg.traces_at(TraceAnchor::Junction(*uuid)).map(Trace::uuid));
    }
    for (id, uuid) in &selection.traces {
        if !segment(id)?.traces().contains_key(uuid) {
            return Err(Error::not_found("Trace", uuid));
        }
        items.entry(*id).or_default().traces.insert(*uuid);
    }

    // Split the affected segments (upstream `removeNetSegmentItems()`).
    let mut plans = Vec::new();
    for (id, remove) in &items {
        let seg = segment(id)?;
        let mut splitter = BoardNetSegmentSplitter::new();
        for (anchor, pos) in &remove.pads_to_disconnect {
            splitter.replace_footprint_pad_by_junctions(*anchor, *pos);
        }
        for pad in seg.pads().values() {
            splitter.add_pad(pad.clone(), remove.pads.contains(&pad.uuid()));
        }
        for via in seg.vias().values() {
            splitter.add_via(via.clone(), remove.vias.contains(&via.uuid()));
        }
        for junction in seg.junctions().values() {
            if !remove.junctions.contains(&junction.uuid()) {
                splitter.add_junction(junction.clone());
            }
        }
        for trace in seg.traces().values() {
            if !remove.traces.contains(&trace.uuid()) {
                splitter.add_trace(trace, Uuid::new_random);
            }
        }
        plans.push((*id, seg.net(), splitter.split()));
    }
    let mut changed = false;
    for (id, net, parts) in plans {
        tx.apply(brd(BoardMutation::RemoveNetSegment(BoardNetSegmentRef {
            board: board_id,
            segment: id,
        })))?;
        changed = true;
        for part in parts {
            let segment = BoardNetSegment::with_elements(
                Uuid::new_random(),
                net,
                part.pads,
                part.vias,
                part.junctions,
                part.traces,
            );
            tx.apply(brd(BoardMutation::AddNetSegment {
                board: board_id,
                segment,
            }))?;
        }
    }

    // Devices, planes and other items.
    for component in &selection.devices {
        tx.apply(brd(BoardMutation::RemoveDevice(DeviceRef {
            board: board_id,
            component: *component,
        })))?;
        changed = true;
    }
    for plane in &selection.planes {
        tx.apply(brd(BoardMutation::RemovePlane(PlaneRef {
            board: board_id,
            plane: *plane,
        })))?;
        changed = true;
    }
    let mut remove_items = |kind: BoardItemKind, uuids: &[Uuid]| -> Result<()> {
        for uuid in uuids {
            tx.apply(brd(BoardMutation::RemoveItem {
                board: board_id,
                kind,
                uuid: *uuid,
            }))?;
            changed = true;
        }
        Ok(())
    };
    remove_items(BoardItemKind::Zone, &selection.zones)?;
    remove_items(BoardItemKind::Polygon, &selection.polygons)?;
    remove_items(BoardItemKind::StrokeText, &selection.stroke_texts)?;
    // Device stroke texts (upstream `CmdDeviceStrokeTextRemove`).
    let mut texts_per_device: BTreeMap<ComponentInstanceId, Vec<Uuid>> = BTreeMap::new();
    for (component, text) in &selection.device_texts {
        if !selection.devices.contains(component) {
            texts_per_device.entry(*component).or_default().push(*text);
        }
    }
    for (component, texts) in texts_per_device {
        let mut device = tx
            .project()
            .board(board_id)
            .and_then(|b| b.device(component))
            .ok_or_else(|| Error::not_found("Device", component))?
            .clone();
        for text in texts {
            device
                .remove_stroke_text(&text)
                .ok_or_else(|| Error::not_found("Stroke text", text))?;
        }
        tx.apply(brd(BoardMutation::UpdateDevice {
            board: board_id,
            device,
        }))?;
        changed = true;
    }
    for hole in &selection.holes {
        tx.apply(brd(BoardMutation::RemoveItem {
            board: board_id,
            kind: BoardItemKind::Hole,
            uuid: *hole,
        }))?;
        changed = true;
    }
    if changed && cleanup_library {
        remove_unused_library_elements(tx)?;
    }
    Ok(changed)
}

/// Removes board items (upstream `CmdRemoveBoardItems`): the affected net
/// segments are split into their remaining cohesive parts (removed vias,
/// pads and device pads are replaced by junctions in the remaining
/// traces), then devices, planes, zones, polygons, texts and holes are
/// removed, and finally unused library elements.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemoveBoardItems {
    /// The board (default: the primary board).
    #[serde(default)]
    pub board: Option<BoardId>,
    /// The items.
    pub selection: BoardSelection,
}

impl Command for RemoveBoardItems {
    /// Whether anything was removed.
    type Output = bool;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdRemoveBoardItems",
            "Remove Board Items"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<bool> {
        let board = board_ref(tx, self.board)?;
        remove_board_items(tx, board, &self.selection, true)
    }
}

/// An end of a trace.
///
/// Serde: externally tagged, e.g. `{"Pad": {"component": "R1", "pad":
/// "1"}}`, `{"Via": "<uuid>"}` or `{"Point": {"x": 0, "y": 0}}`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TraceEndpoint {
    /// A footprint pad, by component and pad name.
    Pad(PadRef),
    /// A footprint pad, by UUIDs.
    FootprintPad {
        /// The component (device).
        component: ComponentInstanceId,
        /// UUID of the footprint pad.
        pad: Uuid,
    },
    /// An existing via.
    Via(Uuid),
    /// An existing standalone board pad.
    BoardPad(Uuid),
    /// An existing junction.
    Junction(Uuid),
    /// An existing trace, split at the point nearest to `position`
    /// (upstream `CmdBoardSplitNetLine`).
    Trace {
        /// The trace.
        trace: Uuid,
        /// The position (projected onto the trace).
        position: Point,
    },
    /// A new junction.
    Point(Point),
}

/// A resolved trace end.
#[derive(Debug, Clone, Copy)]
struct TraceEnd {
    /// `None`: a new junction at `position`.
    anchor: Option<TraceAnchor>,
    position: Point,
    segment: Option<NetSegmentId>,
    /// The net, if the end determines it.
    net: Option<Option<NetSignalId>>,
    /// Solder layer of an SMT pad.
    smt_layer: Option<Layer>,
}

/// Returns the segment containing a via, junction or trace.
fn find_segment(
    p: &Project,
    board: BoardId,
    what: &'static str,
    uuid: Uuid,
    contains: impl Fn(&BoardNetSegment) -> bool,
) -> Result<NetSegmentId> {
    p.board(board)
        .and_then(|b| {
            b.net_segments()
                .values()
                .find(|s| contains(s))
                .map(|s| s.id())
        })
        .ok_or_else(|| Error::not_found(what, uuid))
}

impl TraceEnd {
    fn resolve(tx: &mut Transaction<'_>, board: BoardId, end: &TraceEndpoint) -> Result<TraceEnd> {
        let p = tx.project();
        let b = p
            .board(board)
            .ok_or_else(|| Error::not_found("Board", board))?;
        let segment_net = |id: NetSegmentId| b.net_segment(id).and_then(|s| s.net());
        let pad_end = |component: ComponentInstanceId, pad: Uuid| -> Result<TraceEnd> {
            let device = b
                .device(component)
                .ok_or_else(|| Error::not_found("Device", component))?;
            let view = device
                .pad(&pad, p.library(), p.circuit())?
                .ok_or_else(|| Error::not_found("Pad", pad))?;
            let net = view.net().ok_or(Error::PadNotConnected)?;
            Ok(TraceEnd {
                anchor: Some(view.trace_anchor()),
                position: view.position(),
                segment: b.footprint_pad_segment(component, pad),
                net: Some(Some(net)),
                smt_layer: (!view.properties().is_tht()).then(|| view.solder_layer()),
            })
        };
        match end {
            TraceEndpoint::Pad(r) => {
                let resolved = resolve::pad(p, b, r)?;
                pad_end(resolved.component, resolved.pad)
            }
            TraceEndpoint::FootprintPad { component, pad } => pad_end(*component, *pad),
            TraceEndpoint::Via(uuid) => {
                let id = find_segment(p, board, "Via", *uuid, |s| s.vias().contains_key(uuid))?;
                let position = b.net_segment(id).expect("found above").vias()[uuid].position();
                Ok(TraceEnd {
                    anchor: Some(TraceAnchor::Via(*uuid)),
                    position,
                    segment: Some(id),
                    net: Some(segment_net(id)),
                    smt_layer: None,
                })
            }
            TraceEndpoint::BoardPad(uuid) => {
                let id = find_segment(p, board, "Pad", *uuid, |s| s.pads().contains_key(uuid))?;
                let pad = b.net_segment(id).expect("found above").pads()[uuid].pad();
                let net = segment_net(id).ok_or(Error::PadNotConnected)?;
                Ok(TraceEnd {
                    anchor: Some(TraceAnchor::Pad(*uuid)),
                    position: pad.position(),
                    segment: Some(id),
                    net: Some(Some(net)),
                    smt_layer: None,
                })
            }
            TraceEndpoint::Junction(uuid) => {
                let id = find_segment(p, board, "Junction", *uuid, |s| {
                    s.junctions().contains_key(uuid)
                })?;
                let position = b.net_segment(id).expect("found above").junctions()[uuid].position();
                Ok(TraceEnd {
                    anchor: Some(TraceAnchor::Junction(*uuid)),
                    position,
                    segment: Some(id),
                    net: Some(segment_net(id)),
                    smt_layer: None,
                })
            }
            TraceEndpoint::Trace { trace, position } => {
                let id = find_segment(p, board, "Trace", *trace, |s| {
                    s.traces().contains_key(trace)
                })?;
                let seg = b.net_segment(id).expect("found above");
                let t = seg.traces()[trace].clone();
                let pos = |a| {
                    b.anchor_position(seg, a, p.library(), p.circuit())
                        .ok_or_else(|| Error::not_found("Trace anchor", trace))
                };
                let split = toolbox::nearest_point_on_line(*position, pos(t.p1())?, pos(t.p2())?);
                let net = seg.net();
                let junction = Junction::new(Uuid::new_random(), split);
                let j = TraceAnchor::Junction(junction.uuid());
                let r = BoardNetSegmentRef { board, segment: id };
                // upstream `CmdBoardSplitNetLine`
                tx.apply(brd(BoardMutation::AddNetSegmentElements {
                    segment: r,
                    elements: BoardSegmentElements {
                        junctions: vec![junction],
                        traces: vec![
                            Trace::new(Uuid::new_random(), t.layer(), t.width(), j, t.p1()),
                            Trace::new(Uuid::new_random(), t.layer(), t.width(), j, t.p2()),
                        ],
                        ..BoardSegmentElements::default()
                    },
                }))?;
                tx.apply(brd(BoardMutation::RemoveNetSegmentElements {
                    segment: r,
                    elements: librepcb_core::project::board::BoardSegmentItems {
                        traces: vec![*trace],
                        ..Default::default()
                    },
                }))?;
                Ok(TraceEnd {
                    anchor: Some(j),
                    position: split,
                    segment: Some(id),
                    net: Some(net),
                    smt_layer: None,
                })
            }
            TraceEndpoint::Point(position) => Ok(TraceEnd {
                anchor: None,
                position: *position,
                segment: None,
                net: None,
                smt_layer: None,
            }),
        }
    }
}

/// Moves all elements of the board net segment `from` into `into`
/// together with the `elements` connecting them, and removes `from`
/// (upstream `CmdCombineBoardNetSegments`, keeping the element UUIDs;
/// upstream replaces the connecting junction by the other anchor).
pub(crate) fn combine_board_segments(
    tx: &mut Transaction<'_>,
    board: BoardId,
    from: NetSegmentId,
    into: NetSegmentId,
    mut elements: BoardSegmentElements,
) -> Result<()> {
    let b = resolve::board(tx.project(), Some(board))?;
    let old = b
        .net_segment(from)
        .ok_or_else(|| Error::not_found("Net segment", from))?
        .clone();
    let new = b
        .net_segment(into)
        .ok_or_else(|| Error::not_found("Net segment", into))?;
    if from == into || old.net() != new.net() {
        return Err(Error::InvalidArgument(
            "Cannot combine net segments of different nets.".to_owned(),
        ));
    }
    tx.apply(brd(BoardMutation::RemoveNetSegment(BoardNetSegmentRef {
        board,
        segment: from,
    })))?;
    tx.apply(brd(BoardMutation::AddNetSegmentElements {
        segment: BoardNetSegmentRef {
            board,
            segment: into,
        },
        elements: {
            elements.pads.extend(old.pads().values().cloned());
            elements.vias.extend(old.vias().values().cloned());
            elements.junctions.extend(old.junctions().values().cloned());
            elements.traces.extend(old.traces().values().cloned());
            elements
        },
    }))
}

/// Result of [`AddTrace`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TraceResult {
    /// The board.
    pub board: BoardId,
    /// The net segment containing the trace.
    pub segment: NetSegmentId,
    /// The net.
    pub net: Option<NetSignalId>,
    /// The layer of the traces.
    pub layer: Layer,
    /// The new junctions.
    pub junctions: Vec<Uuid>,
    /// The new traces.
    pub traces: Vec<Uuid>,
}

/// Draws a trace from `start` over `points` to `end` on one copper layer
/// (upstream `BoardEditorState_DrawTrace`): extends the net segment of a
/// connected end, combines the segments of both ends (which must belong to
/// the same net), or creates a new segment. Pads without net cannot be
/// connected (like upstream).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddTrace {
    /// The board (default: the primary board).
    #[serde(default)]
    pub board: Option<BoardId>,
    /// The start.
    pub start: TraceEndpoint,
    /// The end.
    pub end: TraceEndpoint,
    /// Intermediate points.
    #[serde(default)]
    pub points: Vec<Point>,
    /// The copper layer (default: the layer of an SMT pad at an end, else
    /// top copper).
    #[serde(default)]
    pub layer: Option<Layer>,
    /// The width (default: the median width of the traces at the start
    /// anchor, else the net class' default trace width, else the board's
    /// design rules default; like upstream's automatic width).
    #[serde(default)]
    pub width: Option<PositiveLength>,
    /// The net if no end determines it (both ends are points).
    #[serde(default)]
    pub net: Option<NetRef>,
}

impl Command for AddTrace {
    type Output = TraceResult;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::BoardEditorState_DrawTrace",
            "Draw Board Trace"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<TraceResult> {
        let board = board_ref(tx, self.board)?;
        let a = TraceEnd::resolve(tx, board, &self.start)?;
        let b = TraceEnd::resolve(tx, board, &self.end)?;
        if a.anchor.is_some() && a.anchor == b.anchor {
            return Err(Error::InvalidArgument(
                "The start and end of the trace are the same anchor.".to_owned(),
            ));
        }
        let mut positions = vec![a.position];
        for p in self.points.iter().copied().chain([b.position]) {
            if positions.last() != Some(&p) {
                positions.push(p);
            }
        }
        if positions.len() < 2 {
            return Err(Error::InvalidArgument(
                "The start and end of the trace are at the same position.".to_owned(),
            ));
        }

        // The net.
        let p = tx.project();
        let explicit = self
            .net
            .as_ref()
            .map(|r| resolve::required_net(p, r))
            .transpose()?;
        let net = match (a.net, b.net) {
            (Some(n1), Some(n2)) if n1 != n2 => {
                return Err(Error::NetMismatch(
                    resolve::net_name(p, n1),
                    resolve::net_name(p, n2),
                ));
            }
            (Some(n), _) | (None, Some(n)) => n,
            (None, None) => explicit,
        };
        if let Some(e) = explicit
            && net != Some(e)
        {
            return Err(Error::NetMismatch(
                resolve::net_name(p, net),
                resolve::net_name(p, Some(e)),
            ));
        }

        // Layer and width.
        let layer = self
            .layer
            .or(a.smt_layer)
            .or(b.smt_layer)
            .unwrap_or(Layer::TOP_COPPER);
        let b_ref = resolve::board(p, Some(board))?;
        if !b_ref.copper_layers().contains(&layer) {
            return Err(Error::InvalidArgument(tr!(
                "librepcb::editor::BoardEditorState_DrawTrace",
                "Invalid layer selected."
            )));
        }
        // Upstream automatic width: the median width of the traces at the
        // start anchor, else the net class' default, else the design rules'
        // default.
        let median_at_start = a.anchor.zip(a.segment).and_then(|(anchor, segment)| {
            let mut widths: Vec<PositiveLength> = b_ref
                .net_segment(segment)?
                .traces_at(anchor)
                .map(Trace::width)
                .collect();
            widths.sort();
            widths.get(widths.len() / 2).copied()
        });
        let width = match self.width.or(median_at_start) {
            Some(w) => w,
            None => net
                .and_then(|n| p.circuit().net_signal(n))
                .and_then(|n| p.circuit().net_class(n.net_class()))
                .and_then(|c| c.default_trace_width())
                .unwrap_or_else(|| b_ref.design_rules().default_trace_width()),
        };

        // The target segment and the segment to combine into it.
        let (target, merge_from) = match (a.segment, b.segment) {
            (None, None) => (None, None),
            (Some(s), None) | (None, Some(s)) => (Some(s), None),
            (Some(s1), Some(s2)) if s1 == s2 => (Some(s1), None),
            (Some(s1), Some(s2)) => (Some(s2), Some(s1)),
        };

        // The elements.
        let mut elements = BoardSegmentElements::default();
        let mut anchors = Vec::new();
        let last = positions.len() - 1;
        for (i, position) in positions.iter().enumerate() {
            let existing = match i {
                0 => a.anchor,
                i if i == last => b.anchor,
                _ => None,
            };
            anchors.push(existing.unwrap_or_else(|| {
                let junction = Junction::new(Uuid::new_random(), *position);
                let anchor = TraceAnchor::Junction(junction.uuid());
                elements.junctions.push(junction);
                anchor
            }));
        }
        elements.traces = anchors
            .windows(2)
            .map(|w| Trace::new(Uuid::new_random(), layer, width, w[0], w[1]))
            .collect();
        let junctions = elements.junctions.iter().map(Junction::uuid).collect();
        let traces = elements.traces.iter().map(Trace::uuid).collect();
        let segment = match (target, merge_from) {
            (Some(segment), Some(from)) => {
                combine_board_segments(tx, board, from, segment, elements)?;
                segment
            }
            (Some(segment), None) => {
                tx.apply(brd(BoardMutation::AddNetSegmentElements {
                    segment: BoardNetSegmentRef { board, segment },
                    elements,
                }))?;
                segment
            }
            (None, _) => {
                let segment = BoardNetSegment::with_elements(
                    Uuid::new_random(),
                    net,
                    elements.pads,
                    elements.vias,
                    elements.junctions,
                    elements.traces,
                );
                let id = segment.id();
                tx.apply(brd(BoardMutation::AddNetSegment { board, segment }))?;
                id
            }
        };
        Ok(TraceResult {
            board,
            segment,
            net,
            layer,
            junctions,
            traces,
        })
    }
}

/// Result of [`AddVia`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ViaResult {
    /// The new net segment of the via.
    pub segment: NetSegmentId,
    /// The via.
    pub via: Uuid,
}

/// Adds a via of `net` (upstream `BoardEditorState_AddVia::fixPosition()`):
/// traces of the same net passing the via position are split there and
/// junctions of the same net at the position are replaced by the via; all
/// affected net segments are combined with the via into one new segment.
pub(crate) fn add_via_connected(
    tx: &mut Transaction<'_>,
    board: BoardId,
    via: Via,
    net: Option<NetSignalId>,
) -> Result<ViaResult> {
    let p = tx.project();
    let b = resolve::board(p, Some(board))?;
    let pos = via.position();
    let via_anchor = TraceAnchor::Via(via.uuid());
    let mut affected: Vec<NetSegmentId> = Vec::new();
    let mut elements = BoardSegmentElements {
        vias: vec![via.clone()],
        ..BoardSegmentElements::default()
    };
    for seg in b.net_segments().values().filter(|s| s.net() == net) {
        // Junctions at the position (hit area: the widest trace at them).
        let found: BTreeSet<Uuid> = seg
            .junctions()
            .values()
            .filter(|j| {
                let radius = seg
                    .traces_at(TraceAnchor::Junction(j.uuid()))
                    .map(|t| *t.width() / 2)
                    .max()
                    .unwrap_or(Length::ZERO);
                *(j.position() - pos).length() <= radius
            })
            .map(|j| j.uuid())
            .collect();
        let is_found = |a: TraceAnchor| matches!(a, TraceAnchor::Junction(j) if found.contains(&j));
        // Traces through the position (not attached to a found junction).
        let mut split = BTreeSet::new();
        for trace in seg.traces().values() {
            if is_found(trace.p1()) || is_found(trace.p2()) {
                continue;
            }
            let (Some(p1), Some(p2)) = (
                b.anchor_position(seg, trace.p1(), p.library(), p.circuit()),
                b.anchor_position(seg, trace.p2(), p.library(), p.circuit()),
            ) else {
                continue;
            };
            let (distance, _) = toolbox::shortest_distance_between_point_and_line(pos, p1, p2);
            if *distance <= *trace.width() / 2 {
                split.insert(trace.uuid());
            }
        }
        if found.is_empty() && split.is_empty() {
            continue;
        }
        elements.pads.extend(seg.pads().values().cloned());
        elements.vias.extend(seg.vias().values().cloned());
        elements.junctions.extend(
            seg.junctions()
                .values()
                .filter(|j| !found.contains(&j.uuid()))
                .cloned(),
        );
        for trace in seg.traces().values() {
            if split.contains(&trace.uuid()) {
                for end in [trace.p1(), trace.p2()] {
                    elements.traces.push(Trace::new(
                        Uuid::new_random(),
                        trace.layer(),
                        trace.width(),
                        via_anchor,
                        end,
                    ));
                }
                continue;
            }
            let map = |a: TraceAnchor| if is_found(a) { via_anchor } else { a };
            let (a1, a2) = (map(trace.p1()), map(trace.p2()));
            if a1 != a2 {
                let mut trace = trace.clone();
                trace.set_anchors(a1, a2);
                elements.traces.push(trace);
            }
        }
        affected.push(seg.id());
    }
    for segment in affected {
        tx.apply(brd(BoardMutation::RemoveNetSegment(BoardNetSegmentRef {
            board,
            segment,
        })))?;
    }
    let segment = BoardNetSegment::with_elements(
        Uuid::new_random(),
        net,
        elements.pads,
        elements.vias,
        elements.junctions,
        elements.traces,
    );
    let id = segment.id();
    tx.apply(brd(BoardMutation::AddNetSegment { board, segment }))?;
    Ok(ViaResult {
        segment: id,
        via: via.uuid(),
    })
}

/// Adds a via (upstream `BoardEditorState_AddVia`): through all layers,
/// drill and size from the design rules, no stop mask opening by default.
/// Traces and junctions of the same net at the position are connected to
/// it (upstream `fixPosition()`); otherwise it is placed in a new net
/// segment. Connect it with [`AddTrace`] (`Via` endpoint).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddVia {
    /// The board (default: the primary board).
    #[serde(default)]
    pub board: Option<BoardId>,
    /// The position.
    pub position: Point,
    /// The net (default: none).
    #[serde(default)]
    pub net: Option<NetRef>,
    /// Start layer (default: top copper).
    #[serde(default)]
    pub start_layer: Option<Layer>,
    /// End layer (default: bottom copper).
    #[serde(default)]
    pub end_layer: Option<Layer>,
    /// Drill diameter (default: automatic).
    #[serde(default)]
    pub drill_diameter: Option<PositiveLength>,
    /// Size (default: automatic).
    #[serde(default)]
    pub size: Option<PositiveLength>,
    /// Stop mask exposure (default: off).
    #[serde(default)]
    pub exposure: Option<MaskConfig>,
}

impl Command for AddVia {
    type Output = ViaResult;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::BoardEditorState_AddVia",
            "Add via to board"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<ViaResult> {
        let board = board_ref(tx, self.board)?;
        let net = self
            .net
            .as_ref()
            .map(|r| resolve::required_net(tx.project(), r))
            .transpose()?;
        let via = Via::new(
            Uuid::new_random(),
            self.start_layer.unwrap_or(Layer::TOP_COPPER),
            self.end_layer.unwrap_or(Layer::BOT_COPPER),
            self.position,
            self.drill_diameter,
            self.size,
            self.exposure.unwrap_or(MaskConfig::Off),
        )
        .map_err(|e| Error::InvalidArgument(e.to_string()))?;
        add_via_connected(tx, board, via, net)
    }
}

/// Settings of a plane; `None` fields keep the current (or default)
/// value.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlaneSettings {
    /// Minimum copper width.
    #[serde(default)]
    pub min_width: Option<UnsignedLength>,
    /// Minimum clearance to other copper.
    #[serde(default)]
    pub min_clearance_to_copper: Option<UnsignedLength>,
    /// Minimum clearance to the board outline.
    #[serde(default)]
    pub min_clearance_to_board: Option<UnsignedLength>,
    /// Minimum clearance to non-plated holes.
    #[serde(default)]
    pub min_clearance_to_npth: Option<UnsignedLength>,
    /// Whether unconnected islands are kept.
    #[serde(default)]
    pub keep_islands: Option<bool>,
    /// Fill priority.
    #[serde(default)]
    pub priority: Option<i32>,
    /// How pads are connected.
    #[serde(default)]
    pub connect_style: Option<PlaneConnectStyle>,
    /// Thermal relief gap.
    #[serde(default)]
    pub thermal_gap: Option<PositiveLength>,
    /// Thermal relief spoke width.
    #[serde(default)]
    pub thermal_spoke_width: Option<PositiveLength>,
    /// Whether the plane is locked.
    #[serde(default)]
    pub locked: Option<bool>,
}

impl PlaneSettings {
    fn apply_to(&self, plane: &mut BoardPlane) {
        if let Some(v) = self.min_width {
            plane.set_min_width(v);
        }
        if let Some(v) = self.min_clearance_to_copper {
            plane.set_min_clearance_to_copper(v);
        }
        if let Some(v) = self.min_clearance_to_board {
            plane.set_min_clearance_to_board(v);
        }
        if let Some(v) = self.min_clearance_to_npth {
            plane.set_min_clearance_to_npth(v);
        }
        if let Some(v) = self.keep_islands {
            plane.set_keep_islands(v);
        }
        if let Some(v) = self.priority {
            plane.set_priority(v);
        }
        if let Some(v) = self.connect_style {
            plane.set_connect_style(v);
        }
        if let Some(v) = self.thermal_gap {
            plane.set_thermal_gap(v);
        }
        if let Some(v) = self.thermal_spoke_width {
            plane.set_thermal_spoke_width(v);
        }
        if let Some(v) = self.locked {
            plane.set_locked(v);
        }
    }
}

/// Returns the automatic plane outline (upstream
/// `BoardEditorState_DrawPlane::determineAutoPlaneOutline()`): a rectangle
/// around the bounding rectangle of the board outlines (incl. arcs and the
/// outlines of footprints) with a spacing of a multiple of `grid` (at least
/// 2 mm), not at the left X coordinate of an existing plane.
pub(crate) fn auto_plane_outline(p: &Project, board: BoardId, grid: Length) -> Result<Path> {
    let b = resolve::board(p, Some(board))?;
    let bbox = p.board_bounding_rect(board).map_err(Error::from_export)?;
    let Some((bottom_left, top_right)) = bbox else {
        return Err(Error::InvalidArgument(tr!(
            "librepcb::editor::BoardEditorState_DrawPlane",
            "Could not determine the bounding box of board. Make sure a valid board outline polygon is present."
        )));
    };
    let used_left: BTreeSet<Length> = b
        .planes()
        .values()
        .flat_map(|plane| plane.outline().vertices().iter().map(|v| v.pos.x))
        .collect();
    let mut grid = grid;
    while grid < Length::new(2_000_000) {
        grid *= 2;
    }
    let mut min_space = -(grid / 2);
    let mut left;
    loop {
        min_space += grid;
        left = (bottom_left.x - min_space).rounded_down_to(grid);
        if !used_left.contains(&left) {
            break;
        }
    }
    let bottom = (bottom_left.y - min_space).rounded_down_to(grid);
    let top = (top_right.y + min_space).rounded_up_to(grid);
    let right = (top_right.x + min_space).rounded_up_to(grid);
    Ok(Path::rect(Point::new(left, bottom), Point::new(right, top)))
}

/// Adds a copper plane (upstream `BoardEditorState_DrawPlane` /
/// `CmdBoardPlaneAdd`) with thermal relief connections and the upstream
/// default settings unless overridden.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddPlane {
    /// The board (default: the primary board).
    #[serde(default)]
    pub board: Option<BoardId>,
    /// The net (default: none).
    #[serde(default)]
    pub net: Option<NetRef>,
    /// The copper layer (default: top copper).
    #[serde(default)]
    pub layer: Option<Layer>,
    /// The outline (default: a rectangle around the board outline, like
    /// upstream's automatic plane outline).
    #[serde(default)]
    pub outline: Option<Path>,
    /// Settings overriding the defaults.
    #[serde(default)]
    pub settings: PlaneSettings,
}

impl Command for AddPlane {
    type Output = PlaneId;

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdBoardPlaneAdd", "Add plane to board")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<PlaneId> {
        let board = board_ref(tx, self.board)?;
        let net = self
            .net
            .as_ref()
            .map(|r| resolve::required_net(tx.project(), r))
            .transpose()?;
        let outline = match self.outline {
            Some(o) => o,
            None => {
                let grid = resolve::board(tx.project(), Some(board))?
                    .settings()
                    .grid_interval;
                auto_plane_outline(tx.project(), board, *grid)?
            }
        };
        let layer = self.layer.unwrap_or(Layer::TOP_COPPER);
        if !layer.is_copper() {
            return Err(Error::InvalidArgument(format!(
                "The layer \"{}\" is not a copper layer.",
                layer.id()
            )));
        }
        let mut plane = BoardPlane::new(Uuid::new_random(), layer, net, outline);
        plane.set_connect_style(PlaneConnectStyle::ThermalRelief);
        self.settings.apply_to(&mut plane);
        let id = plane.id();
        tx.apply(brd(BoardMutation::AddPlane { board, plane }))?;
        Ok(id)
    }
}

/// Modifies a plane (upstream `CmdBoardPlaneEdit`); `None` fields are
/// kept.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EditPlane {
    /// The board (default: the primary board).
    #[serde(default)]
    pub board: Option<BoardId>,
    /// The plane.
    pub plane: PlaneId,
    /// The new net (`Some(None)` = no net).
    #[serde(default)]
    pub net: Option<Option<NetRef>>,
    /// The new copper layer.
    #[serde(default)]
    pub layer: Option<Layer>,
    /// The new outline.
    #[serde(default)]
    pub outline: Option<Path>,
    /// Settings to change.
    #[serde(default)]
    pub settings: PlaneSettings,
}

impl Command for EditPlane {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdBoardPlaneEdit", "Edit plane")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<()> {
        let board = board_ref(tx, self.board)?;
        let mut plane = resolve::board(tx.project(), Some(board))?
            .plane(self.plane)
            .ok_or_else(|| Error::not_found("Plane", self.plane))?
            .clone();
        if let Some(net) = &self.net {
            let net = net
                .as_ref()
                .map(|r| resolve::required_net(tx.project(), r))
                .transpose()?;
            plane.set_net(net);
        }
        if let Some(layer) = self.layer {
            plane.set_layer(layer);
        }
        if let Some(outline) = self.outline {
            plane.set_outline(outline);
        }
        self.settings.apply_to(&mut plane);
        tx.apply(brd(BoardMutation::UpdatePlane { board, plane }))
    }
}

/// Adds a polygon to a board (upstream `BoardEditorState_DrawPolygon` /
/// `CmdBoardPolygonAdd`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddBoardPolygon {
    /// The board (default: the primary board).
    #[serde(default)]
    pub board: Option<BoardId>,
    /// The layer.
    pub layer: Layer,
    /// The path.
    pub path: Path,
    /// Line width (default: 0).
    #[serde(default)]
    pub line_width: Option<UnsignedLength>,
    /// Whether it is filled.
    #[serde(default)]
    pub filled: bool,
    /// Whether it is a grab area.
    #[serde(default)]
    pub grab_area: bool,
}

impl Command for AddBoardPolygon {
    /// UUID of the polygon.
    type Output = Uuid;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdBoardPolygonAdd",
            "Add polygon to board"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<Uuid> {
        let board = board_ref(tx, self.board)?;
        let polygon = BoardPolygonData::new(
            Uuid::new_random(),
            self.layer,
            self.line_width.unwrap_or(UnsignedLength::ZERO),
            self.path,
            self.filled,
            self.grab_area,
            false,
        );
        let uuid = polygon.uuid();
        tx.apply(brd(BoardMutation::AddItem {
            board,
            item: BoardItem::Polygon(polygon),
        }))?;
        Ok(uuid)
    }
}

/// Replaces the board outline: removes all polygons on the board outline
/// layer and adds one with the given (closed) path.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SetBoardOutline {
    /// The board (default: the primary board).
    #[serde(default)]
    pub board: Option<BoardId>,
    /// The outline (closed automatically).
    pub outline: Path,
}

impl SetBoardOutline {
    /// Creates the command for a rectangular outline.
    pub fn rect(board: Option<BoardId>, p1: Point, p2: Point) -> Self {
        Self {
            board,
            outline: Path::rect(p1, p2),
        }
    }
}

impl Command for SetBoardOutline {
    /// UUID of the outline polygon.
    type Output = Uuid;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdBoardPolygonAdd",
            "Add polygon to board"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<Uuid> {
        let board = board_ref(tx, self.board)?;
        let existing: Vec<Uuid> = resolve::board(tx.project(), Some(board))?
            .polygons()
            .values()
            .filter(|p| p.layer() == Layer::BOARD_OUTLINES)
            .map(|p| p.uuid())
            .collect();
        for uuid in existing {
            tx.apply(brd(BoardMutation::RemoveItem {
                board,
                kind: BoardItemKind::Polygon,
                uuid,
            }))?;
        }
        let mut outline = self.outline;
        outline.close();
        let polygon = BoardPolygonData::new(
            Uuid::new_random(),
            Layer::BOARD_OUTLINES,
            UnsignedLength::ZERO,
            outline,
            false,
            false,
            false,
        );
        let uuid = polygon.uuid();
        tx.apply(brd(BoardMutation::AddItem {
            board,
            item: BoardItem::Polygon(polygon),
        }))?;
        Ok(uuid)
    }
}

/// Adds a non-plated hole (upstream `BoardEditorState_AddHole` /
/// `CmdBoardHoleAdd`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddHole {
    /// The board (default: the primary board).
    #[serde(default)]
    pub board: Option<BoardId>,
    /// The position.
    pub position: Point,
    /// The diameter.
    pub diameter: PositiveLength,
    /// Stop mask opening (default: automatic).
    #[serde(default)]
    pub stop_mask: Option<MaskConfig>,
}

impl Command for AddHole {
    /// UUID of the hole.
    type Output = Uuid;

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdBoardHoleAdd", "Add hole to board")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<Uuid> {
        let board = board_ref(tx, self.board)?;
        let hole = BoardHoleData::new(
            Uuid::new_random(),
            self.diameter,
            librepcb_core::geometry::NonEmptyPath::from_point(self.position),
            self.stop_mask.unwrap_or(MaskConfig::Automatic),
            false,
        );
        let uuid = hole.uuid();
        tx.apply(brd(BoardMutation::AddItem {
            board,
            item: BoardItem::Hole(hole),
        }))?;
        Ok(uuid)
    }
}

/// Adds a keepout zone (upstream `BoardEditorState_DrawZone` /
/// `CmdBoardZoneAdd`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddZone {
    /// The board (default: the primary board).
    #[serde(default)]
    pub board: Option<BoardId>,
    /// The copper layers.
    pub layers: BTreeSet<Layer>,
    /// The rules.
    pub rules: ZoneRules,
    /// The outline.
    pub outline: Path,
}

impl Command for AddZone {
    /// UUID of the zone.
    type Output = Uuid;

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdBoardZoneAdd", "Add zone to board")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<Uuid> {
        let board = board_ref(tx, self.board)?;
        let zone = BoardZoneData::new(
            Uuid::new_random(),
            self.layers,
            self.rules,
            self.outline,
            false,
        )?;
        let uuid = zone.uuid();
        tx.apply(brd(BoardMutation::AddItem {
            board,
            item: BoardItem::Zone(zone),
        }))?;
        Ok(uuid)
    }
}

/// Adds a stroke text to a board (upstream `BoardEditorState_AddStrokeText`
/// / `CmdBoardStrokeTextAdd`); texts on bottom layers are mirrored.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddStrokeText {
    /// The board (default: the primary board).
    #[serde(default)]
    pub board: Option<BoardId>,
    /// The layer.
    pub layer: Layer,
    /// The text (may contain attributes like `{{PROJECT}}`).
    pub text: String,
    /// The position.
    pub position: Point,
    /// The rotation.
    #[serde(default)]
    pub rotation: Angle,
    /// The height (default: 1.5 mm).
    #[serde(default)]
    pub height: Option<PositiveLength>,
    /// The stroke width (default: 0.2 mm).
    #[serde(default)]
    pub stroke_width: Option<UnsignedLength>,
    /// The alignment (default: left bottom).
    #[serde(default)]
    pub align: Option<Alignment>,
}

impl Command for AddStrokeText {
    /// UUID of the text.
    type Output = Uuid;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdBoardStrokeTextAdd",
            "Add text to board"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<Uuid> {
        let board = board_ref(tx, self.board)?;
        let height = match self.height {
            Some(h) => h,
            None => PositiveLength::new(Length::new(1_500_000))?,
        };
        let stroke_width = match self.stroke_width {
            Some(w) => w,
            None => UnsignedLength::new(Length::new(200_000))?,
        };
        let text = BoardStrokeTextData::new(
            Uuid::new_random(),
            self.layer,
            self.text,
            self.position,
            self.rotation,
            height,
            stroke_width,
            StrokeTextSpacing::Auto,
            StrokeTextSpacing::Auto,
            self.align.unwrap_or_default(),
            self.layer.is_bottom(),
            true,
            false,
        );
        let uuid = text.uuid();
        tx.apply(brd(BoardMutation::AddItem {
            board,
            item: BoardItem::StrokeText(text),
        }))?;
        Ok(uuid)
    }
}

/// Replaces a zone, polygon, stroke text or hole (same kind and UUID;
/// upstream `CmdBoard*Edit`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UpdateBoardItem {
    /// The board (default: the primary board).
    #[serde(default)]
    pub board: Option<BoardId>,
    /// The new item state.
    pub item: BoardItem,
}

impl Command for UpdateBoardItem {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdBoardPolygonEdit", "Edit polygon")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<()> {
        let board = board_ref(tx, self.board)?;
        tx.apply(brd(BoardMutation::UpdateItem {
            board,
            item: self.item,
        }))
    }
}

/// Modifies the board setup (upstream `CmdBoardEdit`): the whole settings,
/// or only the design rules, the DRC settings and/or the number of inner
/// layers; `None` fields are kept.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EditBoardSettings {
    /// The board (default: the primary board).
    #[serde(default)]
    pub board: Option<BoardId>,
    /// All settings (applied first).
    #[serde(default)]
    pub settings: Option<Box<BoardSettings>>,
    /// The design rules.
    #[serde(default)]
    pub design_rules: Option<BoardDesignRules>,
    /// The DRC settings.
    #[serde(default)]
    pub drc_settings: Option<BoardDesignRuleCheckSettings>,
    /// The number of inner copper layers.
    #[serde(default)]
    pub inner_layer_count: Option<u32>,
}

impl Command for EditBoardSettings {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdBoardEdit", "Modify Board Setup")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<()> {
        let board = board_ref(tx, self.board)?;
        let mut settings = match self.settings {
            Some(s) => s,
            None => Box::new(
                resolve::board(tx.project(), Some(board))?
                    .settings()
                    .clone(),
            ),
        };
        if let Some(rules) = self.design_rules {
            settings.design_rules = rules;
        }
        if let Some(drc) = self.drc_settings {
            settings.drc_settings = drc;
        }
        if let Some(count) = self.inner_layer_count {
            if count as usize > Layer::INNER_COPPER_COUNT {
                return Err(Error::InvalidArgument(format!(
                    "At most {} inner layers are supported.",
                    Layer::INNER_COPPER_COUNT
                )));
            }
            settings.inner_layer_count = count;
        }
        tx.apply(brd(BoardMutation::SetSettings { board, settings }))
    }
}

/// Returns the vertices of a closed rectangular path (helper for callers
/// building outlines).
pub fn rect_path(p1: Point, p2: Point) -> Path {
    Path::rect(p1, p2)
}

/// Returns a polygon path through the given points (closed).
pub fn closed_path(points: &[Point]) -> Path {
    let mut path = Path::new(points.iter().map(|p| Vertex::at(*p)).collect());
    path.close();
    path
}
