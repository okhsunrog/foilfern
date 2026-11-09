//! Schematic bus commands: port of the model changes of
//! libs/librepcb/editor/project/schematic/fsm/schematiceditorstate_drawbus
//! (`startPositioning()`/`addNextJunction()`) and of
//! libs/librepcb/editor/project/cmd/cmdbusadd, cmdcombinebuses,
//! cmdcombineschematicbussegments, cmdchangebusofschematicbussegment and
//! cmdschematicbuslabeladd (`.{h,cpp}`).
//!
//! Differences to upstream: bus segments are combined keeping the element
//! UUIDs (upstream creates new junctions and labels); net segments attached
//! to a bus segment which is removed and re-added are re-added with their
//! UUIDs (like upstream) and with their lines re-attached to the junctions
//! of the new segment.

use std::collections::BTreeMap;

use librepcb_core::geometry::{Junction, NetLabel, NetLine, NetLineAnchor};
use librepcb_core::project::circuit::Bus;
use librepcb_core::project::schematic::{SchematicBusSegment, SchematicNetSegment};
use librepcb_core::project::{
    BusId, BusSegmentId, BusSegmentRef, Mutation, NetSegmentId, NetSegmentRef, Project,
    SchematicId, SchematicMutation,
};
use librepcb_core::types::{BusName, Point, Uuid};
use librepcb_core::utils::toolbox;
use librepcb_i18n::tr;

use super::circuit::remove_unused_nets;
use super::resolve;
use super::schematic::net_label_orientation;
use crate::editor::{Command, Transaction};
use crate::error::{Error, Result};

fn sch(m: SchematicMutation) -> Mutation {
    Mutation::Schematic(m)
}

/// Returns the schematic containing the bus segment.
pub(crate) fn bus_segment_schematic(p: &Project, segment: BusSegmentId) -> Result<SchematicId> {
    p.schematics()
        .iter()
        .find(|s| s.bus_segments().contains_key(&segment))
        .map(|s| s.id())
        .ok_or_else(|| Error::not_found("Bus segment", segment))
}

/// Adds a bus (upstream `CmdBusAdd`): without name, an automatic name
/// `B<n>` is generated and marked as auto name.
pub(crate) fn add_bus(tx: &mut Transaction<'_>, name: Option<BusName>) -> Result<BusId> {
    let circuit = tx.project().circuit();
    let (name, auto) = match name {
        Some(name) => (name, false),
        None => {
            let name = (1..)
                .map(|i| format!("B{i}"))
                .find(|n| circuit.bus_by_name(n).is_none())
                .unwrap_or_else(|| "B".to_owned());
            (
                BusName::new(&name).map_err(|e| Error::InvalidArgument(e.to_string()))?,
                true,
            )
        }
    };
    let bus = Bus::new(Uuid::new_random(), name, auto, false, None);
    let id = BusId(bus.uuid());
    tx.apply(Mutation::AddBus(bus))?;
    Ok(id)
}

/// Removes the net segments attached to the bus segments `segments` and
/// returns them (to re-add them with [`readd_net_segments()`]).
fn remove_attached_net_segments(
    tx: &mut Transaction<'_>,
    schematic: SchematicId,
    segments: &[BusSegmentId],
) -> Result<Vec<SchematicNetSegment>> {
    let p = tx.project();
    let s = p
        .schematic(schematic)
        .ok_or_else(|| Error::not_found("Schematic", schematic))?;
    let mut ids: Vec<NetSegmentId> = segments
        .iter()
        .flat_map(|b| s.attached_net_segments(*b))
        .collect();
    ids.sort();
    ids.dedup();
    let removed: Vec<SchematicNetSegment> = ids
        .iter()
        .filter_map(|id| s.net_segments().get(id).cloned())
        .collect();
    for id in ids {
        tx.apply(sch(SchematicMutation::RemoveNetSegment(NetSegmentRef {
            schematic,
            segment: id,
        })))?;
    }
    Ok(removed)
}

/// Re-adds net segments whose lines end at bus junctions, replacing the
/// bus junction anchors by `map(old_segment, junction)` (`None`: keep).
fn readd_net_segments(
    tx: &mut Transaction<'_>,
    schematic: SchematicId,
    segments: Vec<SchematicNetSegment>,
    map: impl Fn(Uuid, Uuid) -> Option<NetLineAnchor>,
) -> Result<()> {
    for old in segments {
        let mut segment = SchematicNetSegment::new(old.uuid(), old.net());
        for junction in old.junctions().values() {
            segment.insert_junction(junction.clone());
        }
        for line in old.lines().values() {
            let replace = |a: NetLineAnchor| match a {
                NetLineAnchor::BusJunction { segment, junction } => {
                    map(segment, junction).unwrap_or(a)
                }
                other => other,
            };
            let mut line = line.clone();
            let (p1, p2) = (replace(line.p1()), replace(line.p2()));
            line.set_anchors(p1, p2);
            segment.insert_line(line);
        }
        for label in old.labels().values() {
            segment.insert_label(label.clone());
        }
        tx.apply(sch(SchematicMutation::AddNetSegment { schematic, segment }))?;
    }
    Ok(())
}

/// Returns a copy of a bus segment assigned to another bus.
fn with_bus(segment: &SchematicBusSegment, bus: BusId) -> SchematicBusSegment {
    let mut result = SchematicBusSegment::new(segment.uuid(), bus);
    for j in segment.junctions().values() {
        result.insert_junction(j.clone());
    }
    for l in segment.lines().values() {
        result.insert_line(l.clone());
    }
    for l in segment.labels().values() {
        result.insert_label(l.clone());
    }
    result
}

/// Moves all bus segments of `to_remove` to `result` and removes
/// `to_remove` (upstream `CmdCombineBuses`); the bus properties are merged
/// (prefix net names if any bus does, the smaller maximum trace length
/// difference).
pub(crate) fn combine_buses(
    tx: &mut Transaction<'_>,
    to_remove: BusId,
    result: BusId,
) -> Result<()> {
    if to_remove == result {
        return Ok(());
    }
    let p = tx.project();
    let old = p
        .circuit()
        .bus(to_remove)
        .ok_or_else(|| Error::not_found("Bus", to_remove))?
        .clone();
    let mut new = p
        .circuit()
        .bus(result)
        .ok_or_else(|| Error::not_found("Bus", result))?
        .clone();
    let uses: Vec<(SchematicId, BusSegmentId)> = p.bus_uses(to_remove).copied().collect();
    let mut per_schematic: BTreeMap<SchematicId, Vec<BusSegmentId>> = BTreeMap::new();
    for (s, b) in uses {
        per_schematic.entry(s).or_default().push(b);
    }
    for (schematic, segments) in per_schematic {
        let nets = remove_attached_net_segments(tx, schematic, &segments)?;
        for id in &segments {
            let seg = tx
                .project()
                .schematic(schematic)
                .and_then(|s| s.bus_segments().get(id))
                .ok_or_else(|| Error::not_found("Bus segment", id))?
                .clone();
            tx.apply(sch(SchematicMutation::RemoveBusSegment(BusSegmentRef {
                schematic,
                segment: *id,
            })))?;
            tx.apply(sch(SchematicMutation::AddBusSegment {
                schematic,
                segment: with_bus(&seg, result),
            }))?;
        }
        readd_net_segments(tx, schematic, nets, |_, _| None)?;
    }
    new.set_prefix_net_names(new.prefix_net_names() || old.prefix_net_names());
    let merged = match (
        new.max_trace_length_difference(),
        old.max_trace_length_difference(),
    ) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    new.set_max_trace_length_difference(merged);
    tx.apply(Mutation::UpdateBus(new))?;
    tx.apply(Mutation::RemoveBus(to_remove))?;
    Ok(())
}

/// Changes the bus of a bus segment (upstream
/// `CmdChangeBusOfSchematicBusSegment`): if it is the only segment of its
/// bus, the buses are combined; otherwise the segment is re-added with the
/// new bus (with its attached net segments).
pub(crate) fn change_bus_of_bus_segment(
    tx: &mut Transaction<'_>,
    r: BusSegmentRef,
    new_bus: BusId,
) -> Result<()> {
    let p = tx.project();
    let seg = p
        .schematic(r.schematic)
        .and_then(|s| s.bus_segments().get(&r.segment))
        .ok_or_else(|| Error::not_found("Bus segment", r.segment))?
        .clone();
    if seg.bus() == new_bus {
        return Ok(());
    }
    if p.bus_uses(seg.bus()).count() == 1 {
        return combine_buses(tx, seg.bus(), new_bus);
    }
    let nets = remove_attached_net_segments(tx, r.schematic, &[r.segment])?;
    tx.apply(sch(SchematicMutation::RemoveBusSegment(r)))?;
    tx.apply(sch(SchematicMutation::AddBusSegment {
        schematic: r.schematic,
        segment: with_bus(&seg, new_bus),
    }))?;
    readd_net_segments(tx, r.schematic, nets, |_, _| None)
}

/// Moves all elements of the bus segment `from` into `into` (both of the
/// same bus and schematic) together with the `junctions` and `lines`
/// connecting them, re-attaches the net lines at junctions of `from` and
/// removes `from` (upstream `CmdCombineSchematicBusSegments`, keeping the
/// element UUIDs). Unused nets and buses are removed afterwards.
pub(crate) fn combine_bus_segments(
    tx: &mut Transaction<'_>,
    schematic: SchematicId,
    from: BusSegmentId,
    into: BusSegmentId,
    mut junctions: Vec<Junction>,
    mut lines: Vec<NetLine>,
) -> Result<()> {
    let p = tx.project();
    let s = p
        .schematic(schematic)
        .ok_or_else(|| Error::not_found("Schematic", schematic))?;
    let old = s
        .bus_segments()
        .get(&from)
        .ok_or_else(|| Error::not_found("Bus segment", from))?
        .clone();
    let new = s
        .bus_segments()
        .get(&into)
        .ok_or_else(|| Error::not_found("Bus segment", into))?;
    if from == into || old.bus() != new.bus() {
        return Err(Error::InvalidArgument(
            "Cannot combine bus segments of different buses.".to_owned(),
        ));
    }
    let nets = remove_attached_net_segments(tx, schematic, &[from])?;
    tx.apply(sch(SchematicMutation::RemoveBusSegment(BusSegmentRef {
        schematic,
        segment: from,
    })))?;
    let into_ref = BusSegmentRef {
        schematic,
        segment: into,
    };
    junctions.extend(old.junctions().values().cloned());
    lines.extend(old.lines().values().cloned());
    tx.apply(sch(SchematicMutation::AddBusSegmentElements {
        segment: into_ref,
        junctions,
        lines,
    }))?;
    for label in old.labels().values() {
        tx.apply(sch(SchematicMutation::AddBusLabel {
            segment: into_ref,
            label: label.clone(),
        }))?;
    }
    let from_uuid = from.0;
    readd_net_segments(tx, schematic, nets, |segment, junction| {
        (segment == from_uuid).then_some(NetLineAnchor::BusJunction {
            segment: into.0,
            junction,
        })
    })?;
    remove_unused_nets(tx)?;
    Ok(())
}

/// An end of a bus line (see [`DrawBus`]).
///
/// Serde: externally tagged like [`WireAnchor`](super::WireAnchor).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BusAnchor {
    /// An existing junction of a bus segment.
    Junction {
        /// The bus segment.
        segment: BusSegmentId,
        /// The junction.
        junction: Uuid,
    },
    /// An existing bus line, split at the point nearest to `position`.
    Line {
        /// The bus segment.
        segment: BusSegmentId,
        /// The line.
        line: Uuid,
        /// The position (projected onto the line).
        position: Point,
    },
    /// A new, unconnected junction.
    Point(Point),
}

/// A resolved bus line end.
#[derive(Debug, Clone, Copy)]
struct BusEnd {
    segment: Option<BusSegmentId>,
    junction: Option<Uuid>,
    position: Point,
}

/// Splits a bus line at the point nearest to `position` (upstream: "split
/// line" in the bus and wire tools); returns the new junction and its
/// position.
pub(crate) fn split_bus_line(
    tx: &mut Transaction<'_>,
    schematic: SchematicId,
    segment: BusSegmentId,
    line: Uuid,
    position: Point,
) -> Result<(Uuid, Point)> {
    let p = tx.project();
    let seg = p
        .schematic(schematic)
        .and_then(|s| s.bus_segments().get(&segment))
        .ok_or(Error::DifferentSchematics)?;
    let l = seg
        .lines()
        .get(&line)
        .ok_or_else(|| Error::not_found("Bus line", line))?;
    let pos = |a| {
        seg.junction_position(a)
            .ok_or_else(|| Error::not_found("Bus junction", line))
    };
    let (p1, p2) = (pos(l.p1())?, pos(l.p2())?);
    let split = toolbox::nearest_point_on_line(position, p1, p2);
    let (width, a1, a2) = (l.width(), l.p1(), l.p2());
    let junction = Junction::new(Uuid::new_random(), split);
    let uuid = junction.uuid();
    let j = NetLineAnchor::Junction(uuid);
    let r = BusSegmentRef { schematic, segment };
    tx.apply(sch(SchematicMutation::AddBusSegmentElements {
        segment: r,
        junctions: vec![junction],
        lines: vec![
            NetLine::new(Uuid::new_random(), width, j, a1),
            NetLine::new(Uuid::new_random(), width, j, a2),
        ],
    }))?;
    tx.apply(sch(SchematicMutation::RemoveBusSegmentElements {
        segment: r,
        junctions: Vec::new(),
        lines: vec![line],
    }))?;
    Ok((uuid, split))
}

impl BusEnd {
    fn resolve(
        tx: &mut Transaction<'_>,
        schematic: SchematicId,
        anchor: &BusAnchor,
    ) -> Result<Self> {
        match anchor {
            BusAnchor::Junction { segment, junction } => {
                let position = tx
                    .project()
                    .schematic(schematic)
                    .and_then(|s| s.bus_segments().get(segment))
                    .ok_or(Error::DifferentSchematics)?
                    .junctions()
                    .get(junction)
                    .ok_or_else(|| Error::not_found("Bus junction", junction))?
                    .position();
                Ok(Self {
                    segment: Some(*segment),
                    junction: Some(*junction),
                    position,
                })
            }
            BusAnchor::Line {
                segment,
                line,
                position,
            } => {
                let (junction, position) =
                    split_bus_line(tx, schematic, *segment, *line, *position)?;
                Ok(Self {
                    segment: Some(*segment),
                    junction: Some(junction),
                    position,
                })
            }
            BusAnchor::Point(position) => Ok(Self {
                segment: None,
                junction: None,
                position: *position,
            }),
        }
    }
}

/// Result of [`DrawBus`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BusResult {
    /// The schematic.
    pub schematic: SchematicId,
    /// The bus segment containing the new lines.
    pub segment: BusSegmentId,
    /// The bus.
    pub bus: BusId,
    /// The new junctions.
    pub junctions: Vec<Uuid>,
    /// The new bus lines.
    pub lines: Vec<Uuid>,
    /// The bus label added at the start (a new segment of a chosen bus).
    pub label: Option<Uuid>,
}

/// Draws bus lines (see [`DrawBus`]).
pub(crate) fn draw_bus(
    tx: &mut Transaction<'_>,
    schematic: Option<SchematicId>,
    start: &BusAnchor,
    end: &BusAnchor,
    points: &[Point],
    bus: Option<BusId>,
) -> Result<BusResult> {
    let p = tx.project();
    let hint = |a: &BusAnchor| -> Result<Option<SchematicId>> {
        Ok(match a {
            BusAnchor::Junction { segment, .. } | BusAnchor::Line { segment, .. } => {
                Some(bus_segment_schematic(p, *segment)?)
            }
            BusAnchor::Point(_) => None,
        })
    };
    let hints = [hint(start)?, hint(end)?, schematic];
    let mut known = hints.iter().flatten();
    let schematic = match known.next() {
        Some(first) => {
            if known.any(|s| s != first) {
                return Err(Error::DifferentSchematics);
            }
            *first
        }
        None => resolve::schematic(p, None)?.id(),
    };
    if let Some(bus) = bus
        && p.circuit().bus(bus).is_none()
    {
        return Err(Error::not_found("Bus", bus));
    }

    let a = BusEnd::resolve(tx, schematic, start)?;
    let b = BusEnd::resolve(tx, schematic, end)?;
    let mut positions: Vec<Point> = vec![a.position];
    for p in points.iter().copied().chain([b.position]) {
        if positions.last() != Some(&p) {
            positions.push(p);
        }
    }
    if positions.len() < 2 {
        return Err(Error::InvalidArgument(
            "The start and end of the bus line are at the same position.".to_owned(),
        ));
    }
    if a.junction.is_some() && a.junction == b.junction {
        return Err(Error::InvalidArgument(
            "The start and end of the bus line are the same junction.".to_owned(),
        ));
    }

    // Determine the bus: of the start segment, else the chosen one (then
    // a label is added at the start), else a new bus (unless the end is on
    // a segment whose bus wins anyway).
    let segment_bus = |tx: &Transaction<'_>, segment: BusSegmentId| -> Result<BusId> {
        tx.project()
            .schematic(schematic)
            .and_then(|s| s.bus_segments().get(&segment))
            .map(|s| s.bus())
            .ok_or_else(|| Error::not_found("Bus segment", segment))
    };
    let add_label = a.segment.is_none() && bus.is_some();

    // Junctions and lines.
    let width = SchematicBusSegment::default_line_width();
    let mut junctions = Vec::new();
    let mut anchors = Vec::new();
    let last = positions.len() - 1;
    for (i, position) in positions.iter().enumerate() {
        let existing = match i {
            0 => a.junction,
            i if i == last => b.junction,
            _ => None,
        };
        anchors.push(NetLineAnchor::Junction(existing.unwrap_or_else(|| {
            let junction = Junction::new(Uuid::new_random(), *position);
            let uuid = junction.uuid();
            junctions.push(junction);
            uuid
        })));
    }
    let lines: Vec<NetLine> = anchors
        .windows(2)
        .map(|w| NetLine::new(Uuid::new_random(), width, w[0], w[1]))
        .collect();
    let result_junctions: Vec<Uuid> = junctions.iter().map(Junction::uuid).collect();
    let result_lines: Vec<Uuid> = lines.iter().map(NetLine::uuid).collect();
    let seg_ref = |segment| BusSegmentRef { schematic, segment };

    let segment = match (a.segment, b.segment) {
        (Some(s1), Some(s2)) if s1 != s2 => {
            // Change the bus if needed (upstream `addNextJunction()`): the
            // start segment moves to the bus of the end segment, unless
            // only the end segment's bus has an automatic name.
            let this_bus = segment_bus(tx, s1)?;
            let other_bus = segment_bus(tx, s2)?;
            if this_bus != other_bus {
                let circuit = tx.project().circuit();
                let auto = |id| circuit.bus(id).is_some_and(Bus::has_auto_name);
                if auto(other_bus) && !auto(this_bus) {
                    change_bus_of_bus_segment(tx, seg_ref(s2), this_bus)?;
                } else {
                    change_bus_of_bus_segment(tx, seg_ref(s1), other_bus)?;
                }
            }
            combine_bus_segments(tx, schematic, s1, s2, junctions, lines)?;
            s2
        }
        (Some(s), _) | (None, Some(s)) => {
            if a.segment.is_none()
                && let Some(chosen) = bus
            {
                // A new segment of the chosen bus combined into the end
                // segment: the end segment moves to the chosen bus if its
                // bus has an automatic name.
                let other_bus = segment_bus(tx, s)?;
                let circuit = tx.project().circuit();
                let auto = |id| circuit.bus(id).is_some_and(Bus::has_auto_name);
                if other_bus != chosen && auto(other_bus) && !auto(chosen) {
                    change_bus_of_bus_segment(tx, seg_ref(s), chosen)?;
                }
            }
            tx.apply(sch(SchematicMutation::AddBusSegmentElements {
                segment: seg_ref(s),
                junctions,
                lines,
            }))?;
            s
        }
        (None, None) => {
            let bus = match bus {
                Some(bus) => bus,
                None => add_bus(tx, None)?,
            };
            let mut segment = SchematicBusSegment::new(Uuid::new_random(), bus);
            for junction in junctions {
                segment.insert_junction(junction);
            }
            for line in lines {
                segment.insert_line(line);
            }
            let id = segment.id();
            tx.apply(sch(SchematicMutation::AddBusSegment { schematic, segment }))?;
            id
        }
    };

    // Label at the start of a new segment of a chosen bus.
    let mut label = None;
    if add_label {
        let dir_pos = positions.get(1).copied().unwrap_or(a.position);
        let (rotation, mirrored) = net_label_orientation(a.position, dir_pos);
        let l = NetLabel::new(Uuid::new_random(), a.position, rotation, mirrored);
        label = Some(l.uuid());
        tx.apply(sch(SchematicMutation::AddBusLabel {
            segment: seg_ref(segment),
            label: l,
        }))?;
    }
    let bus = segment_bus(tx, segment)?;
    Ok(BusResult {
        schematic,
        segment,
        bus,
        junctions: result_junctions,
        lines: result_lines,
        label,
    })
}

/// Draws bus lines from `start` over `points` to `end` (upstream
/// `SchematicEditorState_DrawBus`): creates a bus segment (of `bus`, with a
/// bus label at the start, or of a new bus with an automatic name) if
/// neither end is on a bus segment, extends the segment of the connected
/// end, or combines the segments (and buses) of both ends.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DrawBus {
    /// The schematic (default: the schematic of the anchors, or the first
    /// schematic if both ends are points).
    #[serde(default)]
    pub schematic: Option<SchematicId>,
    /// The start.
    pub start: BusAnchor,
    /// The end.
    pub end: BusAnchor,
    /// Intermediate points (corners).
    #[serde(default)]
    pub points: Vec<Point>,
    /// The bus of a new segment (default: a new bus).
    #[serde(default)]
    pub bus: Option<BusId>,
}

impl Command for DrawBus {
    type Output = BusResult;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::SchematicEditorState_DrawBus",
            "Draw Wire"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<BusResult> {
        draw_bus(
            tx,
            self.schematic,
            &self.start,
            &self.end,
            &self.points,
            self.bus,
        )
    }
}

/// Adds a bus label to a bus segment (upstream `CmdSchematicBusLabelAdd`,
/// as `SchematicEditorState_AddLabel` does on bus lines).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddBusLabel {
    /// The bus segment.
    pub segment: BusSegmentId,
    /// The position.
    pub position: Point,
    /// The rotation.
    #[serde(default)]
    pub rotation: librepcb_core::types::Angle,
    /// Whether the label is mirrored.
    #[serde(default)]
    pub mirrored: bool,
}

impl Command for AddBusLabel {
    /// UUID of the label.
    type Output = Uuid;

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdSchematicBusLabelAdd", "Add Bus Label")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<Uuid> {
        let schematic = bus_segment_schematic(tx.project(), self.segment)?;
        let label = NetLabel::new(
            Uuid::new_random(),
            self.position,
            self.rotation,
            self.mirrored,
        );
        let uuid = label.uuid();
        tx.apply(sch(SchematicMutation::AddBusLabel {
            segment: BusSegmentRef {
                schematic,
                segment: self.segment,
            },
            label,
        }))?;
        Ok(uuid)
    }
}

/// Renames the bus of a bus segment (upstream `RenameBusSegmentDialog`):
/// the whole bus is renamed (`CmdBusEdit`) or merged into an existing bus
/// with that name (`CmdCombineBuses`), or only the segment is moved to the
/// existing or a new bus with that name (`CmdBusAdd`,
/// `CmdChangeBusOfSchematicBusSegment`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RenameBusSegment {
    /// The bus segment.
    pub segment: BusSegmentRef,
    /// The new bus name.
    pub name: BusName,
    /// Rename (or merge) the whole bus instead of moving only the segment.
    pub whole_bus: bool,
}

impl Command for RenameBusSegment {
    /// The bus of the segment afterwards.
    type Output = BusId;

    fn text(&self) -> String {
        if self.whole_bus {
            tr!("librepcb::editor::CmdBusEdit", "Edit Bus")
        } else {
            tr!(
                "librepcb::editor::RenameBusSegmentDialog",
                "Change Bus of Bus Segment"
            )
        }
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<BusId> {
        let p = tx.project();
        let old = p
            .schematic(self.segment.schematic)
            .and_then(|s| s.bus_segments().get(&self.segment.segment))
            .ok_or_else(|| Error::not_found("Bus segment", self.segment.segment))?
            .bus();
        let existing = p
            .circuit()
            .bus_by_name(self.name.as_str())
            .map(|(id, _)| id);
        match (existing, self.whole_bus) {
            (Some(bus), _) if bus == old => Ok(old),
            (Some(bus), true) => {
                combine_buses(tx, old, bus)?;
                Ok(bus)
            }
            (None, true) => {
                let mut bus = tx
                    .project()
                    .circuit()
                    .bus(old)
                    .ok_or_else(|| Error::not_found("Bus", old))?
                    .clone();
                bus.set_name(self.name, false);
                tx.apply(Mutation::UpdateBus(bus))?;
                Ok(old)
            }
            (existing, false) => {
                let bus = match existing {
                    Some(bus) => bus,
                    None => add_bus(tx, Some(self.name))?,
                };
                change_bus_of_bus_segment(tx, self.segment, bus)?;
                Ok(bus)
            }
        }
    }
}
