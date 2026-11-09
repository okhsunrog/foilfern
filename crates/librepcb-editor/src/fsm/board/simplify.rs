//! Port of libs/librepcb/editor/project/cmd/cmdsimplifyboardnetsegments.{h,cpp}.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::algorithm::{AnchorType, NetSegmentSimplifier};
use librepcb_core::geometry::{Junction, Trace, TraceAnchor};
use librepcb_core::project::board::BoardNetSegment;
use librepcb_core::project::{BoardId, BoardMutation, BoardNetSegmentRef, Mutation, NetSegmentId};
use librepcb_core::types::{Layer, PositiveLength, Uuid};
use librepcb_i18n::tr;

use crate::editor::{Command, Transaction};
use crate::error::{Error, Result};

/// Simplifies board net segments (upstream `CmdSimplifyBoardNetSegments`):
/// merges overlapping junctions, removes redundant and zero-length traces
/// and needless junctions within straight traces, and splits traces at
/// anchors on them. Empty segments are removed. Returns whether anything
/// changed.
///
/// Like upstream, the simplified segment keeps its UUID and the UUIDs of
/// its pads, vias, junctions and traces; new junctions get random UUIDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimplifyBoardNetSegments {
    /// The board.
    pub board: BoardId,
    /// The net segments (missing ones are ignored).
    pub segments: BTreeSet<NetSegmentId>,
}

impl Command for SimplifyBoardNetSegments {
    type Output = bool;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdSimplifyBoardNetSegments",
            "Simplify Board Net Segments"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<bool> {
        let mut changed = false;
        for segment in &self.segments {
            changed |= simplify_segment(tx, self.board, *segment)?;
        }
        Ok(changed)
    }
}

fn simplify_segment(tx: &mut Transaction<'_>, board_id: BoardId, id: NetSegmentId) -> Result<bool> {
    let p = tx.project();
    let Some(board) = p.board(board_id) else {
        return Ok(false);
    };
    let Some(segment) = board.net_segment(id) else {
        return Ok(false);
    };
    let r = BoardNetSegmentRef {
        board: board_id,
        segment: id,
    };
    // A segment which contains no traces/vias/pads can entirely be removed.
    if segment.pads().is_empty() && segment.vias().is_empty() && segment.traces().is_empty() {
        tx.apply(Mutation::Board(BoardMutation::RemoveNetSegment(r)))?;
        return Ok(true);
    }
    // A segment which contains no traces cannot be simplified.
    if segment.traces().is_empty() {
        return Ok(false);
    }

    // Collect anchors & lines for the simplification.
    let mut simplifier = NetSegmentSimplifier::new();
    let mut anchors: BTreeMap<TraceAnchor, usize> = BTreeMap::new();
    let mut lines: BTreeMap<usize, Uuid> = BTreeMap::new();
    let mut add_anchor = |simplifier: &mut NetSegmentSimplifier, anchor: TraceAnchor| {
        if let Some(id) = anchors.get(&anchor) {
            return Ok(*id);
        }
        let pos = board
            .anchor_position(segment, anchor, p.library(), p.circuit())
            .ok_or_else(|| Error::not_found("Trace anchor", format!("{anchor:?}")))?;
        let id = match anchor {
            TraceAnchor::Pad(uuid) => {
                let pad = &segment.pads()[&uuid];
                let (start, end) = if pad.pad().is_tht() {
                    (Layer::TOP_COPPER, Layer::BOT_COPPER)
                } else {
                    (pad.pad().smt_layer(), pad.pad().smt_layer())
                };
                simplifier.add_anchor(AnchorType::Fixed, pos, Some(start), Some(end))
            }
            TraceAnchor::FootprintPad { device, pad } => {
                let view = board
                    .device(librepcb_core::project::ComponentInstanceId(device))
                    .and_then(|d| d.pad(&pad, p.library(), p.circuit()).ok().flatten())
                    .ok_or_else(|| Error::not_found("Pad", pad))?;
                let (start, end) = if view.properties().is_tht() {
                    (Layer::TOP_COPPER, Layer::BOT_COPPER)
                } else {
                    (view.solder_layer(), view.solder_layer())
                };
                simplifier.add_anchor(AnchorType::Fixed, pos, Some(start), Some(end))
            }
            TraceAnchor::Via(uuid) => {
                let via = &segment.vias()[&uuid];
                simplifier.add_anchor(
                    AnchorType::Via,
                    pos,
                    Some(via.start_layer()),
                    Some(via.end_layer()),
                )
            }
            TraceAnchor::Junction(uuid) => {
                let layer = segment
                    .junction_layer(&uuid)
                    .ok_or_else(|| Error::InvalidArgument("Unhandled anchor type.".to_owned()))?;
                simplifier.add_anchor(AnchorType::Junction, pos, Some(layer), Some(layer))
            }
        };
        anchors.insert(anchor, id);
        Ok::<usize, Error>(id)
    };
    for trace in segment.traces().values() {
        let p1 = add_anchor(&mut simplifier, trace.p1())?;
        let p2 = add_anchor(&mut simplifier, trace.p2())?;
        let id = simplifier.add_line(p1, p2, Some(trace.layer()), *trace.width());
        lines.insert(id, trace.uuid());
    }

    // Perform the simplification. If nothing was modified, abort here.
    let result = simplifier.simplify();
    if !result.modified {
        return Ok(false);
    }
    let anchor_of_id: BTreeMap<usize, TraceAnchor> =
        anchors.iter().map(|(a, id)| (*id, *a)).collect();
    let old = segment.clone();

    // Remove the old segment.
    tx.apply(Mutation::Board(BoardMutation::RemoveNetSegment(r)))?;

    // Add the new segment (same UUID), if there is anything to add.
    let mut junctions: BTreeMap<usize, Junction> = BTreeMap::new();
    let mut get_or_create = |id: usize| -> Option<TraceAnchor> {
        if let Some(j) = junctions.get(&id) {
            return Some(TraceAnchor::Junction(j.uuid()));
        }
        match anchor_of_id.get(&id) {
            Some(TraceAnchor::Junction(uuid)) => {
                let junction = old.junctions().get(uuid)?.clone();
                junctions.insert(id, junction);
                Some(TraceAnchor::Junction(*uuid))
            }
            Some(other) => Some(*other),
            None => {
                let pos = result.new_junctions.get(&id)?;
                let junction = Junction::new(Uuid::new_random(), *pos);
                let uuid = junction.uuid();
                junctions.insert(id, junction);
                Some(TraceAnchor::Junction(uuid))
            }
        }
    };
    let mut traces = Vec::new();
    for line in &result.lines {
        let (Some(p1), Some(p2), Some(layer), Ok(width)) = (
            get_or_create(line.p1),
            get_or_create(line.p2),
            line.layer,
            PositiveLength::new(line.width),
        ) else {
            return Err(Error::InvalidArgument(
                "Board net segment simplification produced an invalid trace.".to_owned(),
            ));
        };
        let uuid = lines
            .get(&line.id)
            .copied()
            .unwrap_or_else(Uuid::new_random);
        traces.push(Trace::new(uuid, layer, width, p1, p2));
    }
    if !old.pads().is_empty() || !old.vias().is_empty() || !traces.is_empty() {
        let new_segment = BoardNetSegment::with_elements(
            old.uuid(),
            old.net(),
            old.pads().values().cloned(),
            old.vias().values().cloned(),
            junctions.into_values(),
            traces,
        );
        tx.apply(Mutation::Board(BoardMutation::AddNetSegment {
            board: board_id,
            segment: new_segment,
        }))?;
    }
    Ok(true)
}
