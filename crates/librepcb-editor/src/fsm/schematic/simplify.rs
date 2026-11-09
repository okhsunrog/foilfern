//! Port of libs/librepcb/editor/project/cmd/cmdsimplifyschematicsegments.{h,cpp}.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::algorithm::{AnchorType, NetSegmentSimplifier};
use librepcb_core::geometry::{Junction, NetLine, NetLineAnchor};
use librepcb_core::project::schematic::{SchematicBusSegment, SchematicNetSegment};
use librepcb_core::project::{
    BusSegmentId, BusSegmentRef, Mutation, NetSegmentId, NetSegmentRef, SchematicId,
    SchematicMutation, SymbolId,
};
use librepcb_core::types::{UnsignedLength, Uuid};
use librepcb_i18n::tr;

use crate::commands::circuit::set_component_signal_net;
use crate::commands::schematic::registered_symbol_pins;
use crate::editor::{Command, Transaction};
use crate::error::Result;

/// Simplifies net and bus segments (upstream
/// `CmdSimplifySchematicSegments`): merges overlapping junctions, removes
/// redundant and zero-length lines and needless junctions within straight
/// lines, splits lines at anchors on them and at orthogonal crossings.
/// Segments without lines are removed; pins which lose their lines are
/// disconnected if they are the only placed pin of their component signal.
/// Bus segments are simplified first; bus junctions with attached net lines
/// are kept. Returns whether anything changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimplifySchematicSegments {
    /// The schematic.
    pub schematic: SchematicId,
    /// The net segments (missing ones are ignored).
    pub segments: BTreeSet<NetSegmentId>,
    /// The bus segments (missing ones are ignored).
    pub bus_segments: BTreeSet<BusSegmentId>,
}

impl Command for SimplifySchematicSegments {
    type Output = bool;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdSimplifySchematicSegments",
            "Simplify Schematic Net/Bus Segments"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<bool> {
        let mut changed = false;
        let mut segments = self.segments;
        for segment in &self.bus_segments {
            changed |= simplify_bus_segment(tx, self.schematic, *segment, &mut segments)?;
        }
        for segment in &segments {
            changed |= simplify_segment(tx, self.schematic, *segment)?;
        }
        Ok(changed)
    }
}

/// Upstream `simplifySegment(SI_BusSegment&)`: the attached net segments
/// are removed and re-added unchanged (junctions with net lines are fixed
/// anchors and keep their UUIDs), and are added to `net_segments` to be
/// simplified afterwards.
fn simplify_bus_segment(
    tx: &mut Transaction<'_>,
    schematic: SchematicId,
    segment_id: BusSegmentId,
    net_segments: &mut BTreeSet<NetSegmentId>,
) -> Result<bool> {
    let p = tx.project();
    let Some(s) = p.schematic(schematic) else {
        return Ok(false);
    };
    let Some(segment) = s.bus_segments().get(&segment_id) else {
        return Ok(false);
    };
    let r = BusSegmentRef {
        schematic,
        segment: segment_id,
    };
    if segment.lines().is_empty() {
        tx.apply(Mutation::Schematic(SchematicMutation::RemoveBusSegment(r)))?;
        return Ok(true);
    }
    let mut simplifier = NetSegmentSimplifier::new();
    let mut anchors: BTreeMap<Uuid, usize> = BTreeMap::new();
    let mut junction_of_id: BTreeMap<usize, Uuid> = BTreeMap::new();
    let mut line_of_id: BTreeMap<usize, Uuid> = BTreeMap::new();
    for line in segment.lines().values() {
        let mut ids = [0; 2];
        for (i, anchor) in [line.p1(), line.p2()].into_iter().enumerate() {
            let NetLineAnchor::Junction(j) = anchor else {
                return Err(crate::Error::InvalidArgument(
                    "Bus lines must end at bus junctions.".to_owned(),
                ));
            };
            ids[i] = match anchors.get(&j) {
                Some(id) => *id,
                None => {
                    let pos = segment.junction_position(anchor).unwrap_or_default();
                    let has_net_lines = s.bus_junction_net_segments(segment_id, j).next().is_some();
                    let anchor_type = if has_net_lines {
                        AnchorType::Fixed
                    } else {
                        AnchorType::Junction
                    };
                    let id = simplifier.add_anchor(anchor_type, pos, None, None);
                    anchors.insert(j, id);
                    junction_of_id.insert(id, j);
                    id
                }
            };
        }
        let id = simplifier.add_line(ids[0], ids[1], None, *line.width());
        line_of_id.insert(id, line.uuid());
    }
    let result = simplifier.simplify();
    if !result.modified {
        return Ok(false);
    }
    let old = segment.clone();
    let attached: Vec<SchematicNetSegment> = s
        .attached_net_segments(segment_id)
        .into_iter()
        .filter_map(|id| s.net_segments().get(&id).cloned())
        .collect();
    for ns in &attached {
        tx.apply(Mutation::Schematic(SchematicMutation::RemoveNetSegment(
            NetSegmentRef {
                schematic,
                segment: ns.id(),
            },
        )))?;
    }
    tx.apply(Mutation::Schematic(SchematicMutation::RemoveBusSegment(r)))?;

    let mut new_segment = SchematicBusSegment::new(old.uuid(), old.bus());
    let mut new_junctions: BTreeMap<usize, Uuid> = BTreeMap::new();
    let mut get_or_create = |id: usize, seg: &mut SchematicBusSegment| -> Option<NetLineAnchor> {
        if let Some(j) = new_junctions.get(&id) {
            return Some(NetLineAnchor::Junction(*j));
        }
        let junction = match junction_of_id.get(&id) {
            Some(j) => old.junctions().get(j)?.clone(),
            None => Junction::new(Uuid::new_random(), *result.new_junctions.get(&id)?),
        };
        let uuid = junction.uuid();
        seg.insert_junction(junction);
        new_junctions.insert(id, uuid);
        Some(NetLineAnchor::Junction(uuid))
    };
    let mut lines = Vec::new();
    for line in &result.lines {
        let (Some(p1), Some(p2)) = (
            get_or_create(line.p1, &mut new_segment),
            get_or_create(line.p2, &mut new_segment),
        ) else {
            return Err(crate::Error::InvalidArgument(
                "Bus segment simplification produced an unknown anchor.".to_owned(),
            ));
        };
        let uuid = line_of_id
            .get(&line.id)
            .copied()
            .unwrap_or_else(Uuid::new_random);
        let width = UnsignedLength::new(line.width).unwrap_or_default();
        lines.push(NetLine::new(uuid, width, p1, p2));
    }
    if !lines.is_empty() {
        for line in lines {
            new_segment.insert_line(line);
        }
        for label in old.labels().values() {
            new_segment.insert_label(label.clone());
        }
        tx.apply(Mutation::Schematic(SchematicMutation::AddBusSegment {
            schematic,
            segment: new_segment,
        }))?;
    }
    for ns in attached {
        net_segments.insert(ns.id());
        tx.apply(Mutation::Schematic(SchematicMutation::AddNetSegment {
            schematic,
            segment: ns,
        }))?;
    }
    Ok(true)
}

fn simplify_segment(
    tx: &mut Transaction<'_>,
    schematic: SchematicId,
    segment_id: NetSegmentId,
) -> Result<bool> {
    let p = tx.project();
    let Some(s) = p.schematic(schematic) else {
        return Ok(false);
    };
    let Some(segment) = s.net_segments().get(&segment_id) else {
        return Ok(false);
    };
    let r = NetSegmentRef {
        schematic,
        segment: segment_id,
    };
    // A segment which contains no lines can entirely be removed.
    if segment.lines().is_empty() {
        tx.apply(Mutation::Schematic(SchematicMutation::RemoveNetSegment(r)))?;
        return Ok(true);
    }

    // Collect anchors & lines for the simplification.
    let mut simplifier = NetSegmentSimplifier::new();
    let mut anchors: BTreeMap<NetLineAnchor, usize> = BTreeMap::new();
    let mut anchor_of_id: BTreeMap<usize, NetLineAnchor> = BTreeMap::new();
    let mut line_of_id: BTreeMap<usize, Uuid> = BTreeMap::new();
    let ctx = p.view();
    for line in segment.lines().values() {
        let mut ids = [0; 2];
        for (i, anchor) in [line.p1(), line.p2()].into_iter().enumerate() {
            ids[i] = match anchors.get(&anchor) {
                Some(id) => *id,
                None => {
                    let pos = s
                        .net_line_anchor_position(segment_id, anchor, ctx)
                        .unwrap_or_default();
                    let anchor_type = match anchor {
                        NetLineAnchor::Junction(_) => AnchorType::Junction,
                        _ => AnchorType::Fixed,
                    };
                    let id = simplifier.add_anchor(anchor_type, pos, None, None);
                    anchors.insert(anchor, id);
                    anchor_of_id.insert(id, anchor);
                    id
                }
            };
        }
        let id = simplifier.add_line(ids[0], ids[1], None, *line.width());
        line_of_id.insert(id, line.uuid());
    }

    // Perform the simplification. If nothing was modified, abort here.
    let result = simplifier.simplify();
    if !result.modified {
        return Ok(false);
    }
    let old = segment.clone();
    tx.apply(Mutation::Schematic(SchematicMutation::RemoveNetSegment(r)))?;

    // Disconnect component signals of pins which are no longer connected.
    for id in &result.disconnected_fixed_anchors {
        if let Some(NetLineAnchor::Pin { symbol, pin }) = anchor_of_id.get(id) {
            let p = tx.project();
            let Some(view) = p
                .schematic(schematic)
                .and_then(|s| s.symbols().get(&SymbolId(*symbol)))
                .and_then(|sym| sym.pin(p.view(), *pin).ok().flatten())
            else {
                continue;
            };
            let signal = view.signal();
            if registered_symbol_pins(p, signal)?.len() <= 1 {
                set_component_signal_net(tx, signal, None)?;
            }
        }
    }

    // Add the new segment (same UUID), if there is anything to add.
    let mut new_segment = SchematicNetSegment::new(old.uuid(), old.net());
    let mut new_junctions: BTreeMap<usize, NetLineAnchor> = BTreeMap::new();
    let mut get_or_create = |id: usize, seg: &mut SchematicNetSegment| -> Option<NetLineAnchor> {
        if let Some(a) = new_junctions.get(&id) {
            return Some(*a);
        }
        match anchor_of_id.get(&id) {
            Some(NetLineAnchor::Junction(j)) => {
                let junction = old.junctions().get(j)?.clone();
                seg.insert_junction(junction);
                let a = NetLineAnchor::Junction(*j);
                new_junctions.insert(id, a);
                Some(a)
            }
            Some(other) => Some(*other),
            None => {
                let pos = result.new_junctions.get(&id)?;
                let junction = Junction::new(Uuid::new_random(), *pos);
                let a = NetLineAnchor::Junction(junction.uuid());
                seg.insert_junction(junction);
                new_junctions.insert(id, a);
                Some(a)
            }
        }
    };
    let mut lines = Vec::new();
    for line in &result.lines {
        let (Some(p1), Some(p2)) = (
            get_or_create(line.p1, &mut new_segment),
            get_or_create(line.p2, &mut new_segment),
        ) else {
            return Err(crate::Error::InvalidArgument(
                "Net segment simplification produced an unknown anchor.".to_owned(),
            ));
        };
        let uuid = line_of_id
            .get(&line.id)
            .copied()
            .unwrap_or_else(Uuid::new_random);
        let width = UnsignedLength::new(line.width).unwrap_or_default();
        lines.push(NetLine::new(uuid, width, p1, p2));
    }
    if !lines.is_empty() {
        for line in lines {
            new_segment.insert_line(line);
        }
        for label in old.labels().values() {
            new_segment.insert_label(label.clone());
        }
        tx.apply(Mutation::Schematic(SchematicMutation::AddNetSegment {
            schematic,
            segment: new_segment,
        }))?;
    }
    Ok(true)
}
