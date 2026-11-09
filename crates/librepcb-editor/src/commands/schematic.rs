//! Schematic wiring and removal commands: port of the result of
//! libs/librepcb/editor/project/schematic/fsm/schematiceditorstate_drawwire
//! (`startPositioning()`/`addNextNetPoint()`), of
//! libs/librepcb/editor/project/cmd/cmdcombineschematicnetsegments,
//! cmdschematicnetlabeladd and cmdremoveselectedschematicitems
//! (`.{h,cpp}`), plus the agent oriented conveniences [`ConnectPins`] and
//! [`ConnectPinToNet`].
//!
//! Differences to upstream: wires are given as anchors and points instead
//! of mouse clicks (no snapping, no grid); segments are combined with the
//! element UUIDs kept (upstream creates new junctions and labels); only
//! valid forced net names are considered, an invalid one is ignored with a
//! log warning (upstream shows a message box); buses are not supported by the wiring commands (bus
//! junctions stay fixed anchors when splitting).

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::geometry::{Junction, NetLabel, NetLine, NetLineAnchor};
use librepcb_core::project::schematic::{
    SchematicBusSegment, SchematicNetSegment, SchematicNetSegmentSplitter,
};
use librepcb_core::project::{
    BusSegmentId, BusSegmentRef, ComponentSignalRef, Mutation, NetSegmentId, NetSegmentRef,
    NetSignalId, Project, ProjectAttributeLookup, SchematicId, SchematicMutation, SymbolId,
    SymbolRef,
};
use librepcb_core::types::{Angle, CircuitIdentifier, Length, Point, UnsignedLength, Uuid};
use librepcb_core::utils::toolbox;
use librepcb_i18n::tr;

use super::board::{BoardSelection, remove_board_items};
use super::circuit::{
    add_net, change_net_of_schematic_segment, default_net_class, remove_unused_nets,
    set_component_signal_net,
};
use super::library::remove_unused_library_elements;
use super::{PinRef, resolve};
use crate::editor::{Command, Transaction};
use crate::error::{Error, Result};

fn sch(m: SchematicMutation) -> Mutation {
    Mutation::Schematic(m)
}

/// Returns the schematic containing the net segment.
pub(crate) fn segment_schematic(p: &Project, segment: NetSegmentId) -> Result<SchematicId> {
    p.schematics()
        .iter()
        .find(|s| s.net_segments().contains_key(&segment))
        .map(|s| s.id())
        .ok_or_else(|| Error::not_found("Net segment", segment))
}

/// Returns the schematic containing the symbol.
pub(crate) fn symbol_schematic(p: &Project, symbol: SymbolId) -> Result<SchematicId> {
    p.schematics()
        .iter()
        .find(|s| s.symbols().contains_key(&symbol))
        .map(|s| s.id())
        .ok_or_else(|| Error::not_found("Symbol", symbol))
}

/// How the corner of a wire between two points is placed (upstream
/// `SchematicEditorState_DrawWire::WireMode`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WireMode {
    /// Horizontal, then vertical.
    #[default]
    HV,
    /// Vertical, then horizontal.
    VH,
    /// 90°, then 45°.
    Deg9045,
    /// 45°, then 90°.
    Deg4590,
    /// Straight line.
    Straight,
}

impl WireMode {
    /// Returns the corner point between `p1` and `p2` (upstream
    /// `calcMiddlePointPos()`); equals `p1` for straight lines.
    pub fn middle_point(self, p1: Point, p2: Point) -> Point {
        let delta = p2 - p1;
        let sign = |l: Length| if l >= Length::ZERO { 1 } else { -1 };
        match self {
            Self::HV => Point::new(p2.x, p1.y),
            Self::VH => Point::new(p1.x, p2.y),
            Self::Deg9045 => {
                if delta.x.abs() >= delta.y.abs() {
                    Point::new(p2.x - delta.y.abs() * sign(delta.x), p1.y)
                } else {
                    Point::new(p1.x, p2.y - delta.x.abs() * sign(delta.y))
                }
            }
            Self::Deg4590 => {
                if delta.x.abs() >= delta.y.abs() {
                    Point::new(p1.x + delta.y.abs() * sign(delta.x), p2.y)
                } else {
                    Point::new(p2.x, p1.y + delta.x.abs() * sign(delta.y))
                }
            }
            Self::Straight => p1,
        }
    }
}

/// An end of a wire.
///
/// Serde: externally tagged, e.g. `{"Pin": {"component": "R1", "pin":
/// "1"}}` or `{"Point": {"x": 0, "y": 0}}`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WireAnchor {
    /// A pin of a placed symbol, by component and pin name.
    Pin(PinRef),
    /// A pin of a placed symbol, by UUIDs.
    SymbolPin {
        /// The symbol.
        symbol: SymbolId,
        /// UUID of the library symbol pin.
        pin: Uuid,
    },
    /// An existing junction of a net segment.
    Junction {
        /// The net segment.
        segment: NetSegmentId,
        /// The junction.
        junction: Uuid,
    },
    /// An existing net line, split at the point nearest to `position`
    /// (upstream: clicking on a net line).
    Line {
        /// The net segment.
        segment: NetSegmentId,
        /// The line.
        line: Uuid,
        /// The position (projected onto the line).
        position: Point,
    },
    /// A junction of a bus segment (the wire becomes a bus member).
    BusJunction {
        /// The bus segment.
        segment: BusSegmentId,
        /// The junction.
        junction: Uuid,
    },
    /// A bus line, split at the point nearest to `position` (upstream:
    /// clicking on a bus line).
    BusLine {
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

/// A resolved wire end.
#[derive(Debug, Clone, Copy)]
struct End {
    /// `None`: a new junction at `position`.
    anchor: Option<NetLineAnchor>,
    position: Point,
    /// The segment of the anchor (junctions) or of the lines at the pin.
    segment: Option<NetSegmentId>,
    /// The component signal and its net, for pins.
    pin: Option<(ComponentSignalRef, Option<NetSignalId>)>,
}

impl End {
    fn schematic_hint(anchor: &WireAnchor, p: &Project) -> Result<Option<SchematicId>> {
        Ok(match anchor {
            WireAnchor::Pin(r) => Some(resolve::pin(p, r)?.schematic),
            WireAnchor::SymbolPin { symbol, .. } => Some(symbol_schematic(p, *symbol)?),
            WireAnchor::Junction { segment, .. } | WireAnchor::Line { segment, .. } => {
                Some(segment_schematic(p, *segment)?)
            }
            WireAnchor::BusJunction { segment, .. } | WireAnchor::BusLine { segment, .. } => {
                Some(super::bus::bus_segment_schematic(p, *segment)?)
            }
            WireAnchor::Point(_) => None,
        })
    }

    /// Resolves an anchor, splitting net lines (upstream: "split netline").
    fn resolve(
        tx: &mut Transaction<'_>,
        schematic: SchematicId,
        anchor: &WireAnchor,
    ) -> Result<End> {
        let p = tx.project();
        let s = p
            .schematic(schematic)
            .ok_or_else(|| Error::not_found("Schematic", schematic))?;
        let pin_end = |symbol: SymbolId, pin: Uuid| -> Result<End> {
            let view = s
                .symbols()
                .get(&symbol)
                .ok_or_else(|| Error::not_found("Symbol", symbol))?
                .pin(p.view(), pin)?
                .ok_or_else(|| Error::not_found("Pin", pin))?;
            Ok(End {
                anchor: Some(view.anchor()),
                position: view.position(),
                segment: s.pin_net_segment(symbol, pin),
                pin: Some((view.signal(), view.net())),
            })
        };
        match anchor {
            WireAnchor::Pin(r) => {
                let resolved = resolve::pin(p, r)?;
                if resolved.schematic != schematic {
                    return Err(Error::DifferentSchematics);
                }
                pin_end(resolved.symbol, resolved.pin)
            }
            WireAnchor::SymbolPin { symbol, pin } => pin_end(*symbol, *pin),
            WireAnchor::Junction { segment, junction } => {
                let position = s
                    .net_segments()
                    .get(segment)
                    .ok_or(Error::DifferentSchematics)?
                    .junctions()
                    .get(junction)
                    .ok_or_else(|| Error::not_found("Junction", junction))?
                    .position();
                Ok(End {
                    anchor: Some(NetLineAnchor::Junction(*junction)),
                    position,
                    segment: Some(*segment),
                    pin: None,
                })
            }
            WireAnchor::Line {
                segment,
                line,
                position,
            } => {
                let seg = s
                    .net_segments()
                    .get(segment)
                    .ok_or(Error::DifferentSchematics)?;
                let l = seg
                    .lines()
                    .get(line)
                    .ok_or_else(|| Error::not_found("Net line", line))?;
                let pos = |a| {
                    s.net_line_anchor_position(*segment, a, p.view())
                        .ok_or_else(|| Error::not_found("Net line anchor", line))
                };
                let (p1, p2) = (pos(l.p1())?, pos(l.p2())?);
                let split = toolbox::nearest_point_on_line(*position, p1, p2);
                let width = l.width();
                let (a1, a2) = (l.p1(), l.p2());
                let junction = Junction::new(Uuid::new_random(), split);
                let j = NetLineAnchor::Junction(junction.uuid());
                let r = NetSegmentRef {
                    schematic,
                    segment: *segment,
                };
                tx.apply(sch(SchematicMutation::AddNetSegmentElements {
                    segment: r,
                    junctions: vec![junction],
                    lines: vec![
                        NetLine::new(Uuid::new_random(), width, j, a1),
                        NetLine::new(Uuid::new_random(), width, j, a2),
                    ],
                }))?;
                tx.apply(sch(SchematicMutation::RemoveNetSegmentElements {
                    segment: r,
                    junctions: Vec::new(),
                    lines: vec![*line],
                }))?;
                Ok(End {
                    anchor: Some(j),
                    position: split,
                    segment: Some(*segment),
                    pin: None,
                })
            }
            WireAnchor::BusJunction { segment, junction } => {
                let position = s
                    .bus_segments()
                    .get(segment)
                    .ok_or(Error::DifferentSchematics)?
                    .junctions()
                    .get(junction)
                    .ok_or_else(|| Error::not_found("Bus junction", junction))?
                    .position();
                Ok(End {
                    anchor: Some(NetLineAnchor::BusJunction {
                        segment: segment.0,
                        junction: *junction,
                    }),
                    position,
                    segment: None,
                    pin: None,
                })
            }
            WireAnchor::BusLine {
                segment,
                line,
                position,
            } => {
                let (junction, position) =
                    super::bus::split_bus_line(tx, schematic, *segment, *line, *position)?;
                Ok(End {
                    anchor: Some(NetLineAnchor::BusJunction {
                        segment: segment.0,
                        junction,
                    }),
                    position,
                    segment: None,
                    pin: None,
                })
            }
            WireAnchor::Point(position) => Ok(End {
                anchor: None,
                position: *position,
                segment: None,
                pin: None,
            }),
        }
    }
}

/// Result of the wiring commands.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WireResult {
    /// The schematic.
    pub schematic: SchematicId,
    /// The net segment containing the wire.
    pub segment: NetSegmentId,
    /// The net of the wire.
    pub net: NetSignalId,
    /// The new junctions.
    pub junctions: Vec<Uuid>,
    /// The new net lines.
    pub lines: Vec<Uuid>,
}

/// Moves all elements of the net segment `from` into `into` (both of the
/// same net and schematic) together with the `junctions` and `lines`
/// connecting them, and removes `from` (upstream
/// `CmdCombineSchematicNetSegments`, keeping the element UUIDs; upstream
/// replaces the connecting junction by the other anchor). Unused nets are
/// removed afterwards, like upstream.
pub(crate) fn combine_schematic_segments(
    tx: &mut Transaction<'_>,
    schematic: SchematicId,
    from: NetSegmentId,
    into: NetSegmentId,
    mut junctions: Vec<Junction>,
    mut lines: Vec<NetLine>,
) -> Result<()> {
    let p = tx.project();
    let s = p
        .schematic(schematic)
        .ok_or_else(|| Error::not_found("Schematic", schematic))?;
    let old = s
        .net_segments()
        .get(&from)
        .ok_or_else(|| Error::not_found("Net segment", from))?
        .clone();
    let new = s
        .net_segments()
        .get(&into)
        .ok_or_else(|| Error::not_found("Net segment", into))?;
    if from == into || old.net() != new.net() {
        return Err(Error::InvalidArgument(
            "Cannot combine net segments of different nets.".to_owned(),
        ));
    }
    tx.apply(sch(SchematicMutation::RemoveNetSegment(NetSegmentRef {
        schematic,
        segment: from,
    })))?;
    let into_ref = NetSegmentRef {
        schematic,
        segment: into,
    };
    junctions.extend(old.junctions().values().cloned());
    lines.extend(old.lines().values().cloned());
    tx.apply(sch(SchematicMutation::AddNetSegmentElements {
        segment: into_ref,
        junctions,
        lines,
    }))?;
    for label in old.labels().values() {
        tx.apply(sch(SchematicMutation::AddNetLabel {
            segment: into_ref,
            label: label.clone(),
        }))?;
    }
    remove_unused_nets(tx)?;
    Ok(())
}

/// Applies an explicitly requested net name to the net of a segment
/// (upstream: the forced net name handling at the end of
/// `addNextNetPoint()`): the segment moves to an existing net with that
/// name, or its net is renamed.
pub(crate) fn apply_net_name(
    tx: &mut Transaction<'_>,
    r: NetSegmentRef,
    name: &CircuitIdentifier,
) -> Result<NetSignalId> {
    let p = tx.project();
    let net = p
        .schematic(r.schematic)
        .and_then(|s| s.net_segments().get(&r.segment))
        .ok_or_else(|| Error::not_found("Net segment", r.segment))?
        .net();
    let signal = p
        .circuit()
        .net_signal(net)
        .ok_or_else(|| Error::not_found("Net", net))?;
    if signal.name() == name {
        return Ok(net);
    }
    if let Some((existing, _)) = p.circuit().net_signal_by_name(name.as_str()) {
        change_net_of_schematic_segment(tx, r, existing)?;
        return Ok(existing);
    }
    let mut signal = signal.clone();
    signal.set_name(name.clone(), false);
    tx.apply(Mutation::UpdateNetSignal(signal))?;
    Ok(net)
}

/// Returns the forced net name of a component signal (upstream
/// `ComponentSignalInstance::getForcedNetSignalName()`: the library
/// signal's forced net name with attributes like `{{VALUE}}` substituted),
/// or `None` if the signal has none or it is not a valid net name.
pub(crate) fn forced_net_name(
    p: &Project,
    signal: ComponentSignalRef,
) -> Option<CircuitIdentifier> {
    let component = p.circuit().component_instance(signal.component)?;
    let lib_signal = p
        .library()
        .component(&component.lib_component())?
        .signals()
        .by_uuid(&signal.signal)?;
    if !lib_signal.is_net_signal_name_forced() {
        return None;
    }
    let name = ProjectAttributeLookup::for_component(p, component, None, None)
        .substitute(lib_signal.forced_net_name());
    if name.is_empty() {
        return None;
    }
    match CircuitIdentifier::new(&name) {
        Ok(name) => Some(name),
        Err(_) => {
            log::warn!(
                "Could not apply the forced net name because '{name}' is not a valid net name."
            );
            None
        }
    }
}

/// Returns the forced net names of the pins connected to a net segment
/// (upstream `SI_NetSegment::getForcedNetNames()`).
fn segment_forced_net_names(
    p: &Project,
    schematic: SchematicId,
    segment: NetSegmentId,
) -> BTreeSet<CircuitIdentifier> {
    let Some(s) = p.schematic(schematic) else {
        return BTreeSet::new();
    };
    let Some(seg) = s.net_segments().get(&segment) else {
        return BTreeSet::new();
    };
    seg.connected_pins()
        .into_iter()
        .filter_map(|(symbol, pin)| {
            let view = s.symbols().get(&symbol)?.pin(p.view(), pin).ok()??;
            forced_net_name(p, view.signal())
        })
        .collect()
}

/// Whether a pin connected to the net segment forces a net name.
fn segment_has_forced_net_name(p: &Project, schematic: SchematicId, segment: NetSegmentId) -> bool {
    !segment_forced_net_names(p, schematic, segment).is_empty()
}

/// Draws a wire (upstream `SchematicEditorState_DrawWire`), see [`DrawWire`].
pub(crate) fn draw_wire(
    tx: &mut Transaction<'_>,
    schematic: Option<SchematicId>,
    start: &WireAnchor,
    end: &WireAnchor,
    points: &[Point],
    net_name: Option<&CircuitIdentifier>,
) -> Result<WireResult> {
    // Determine the schematic.
    let hints = [
        End::schematic_hint(start, tx.project())?,
        End::schematic_hint(end, tx.project())?,
        schematic,
    ];
    let mut known = hints.iter().flatten();
    let schematic = match known.next() {
        Some(first) => {
            if known.any(|s| s != first) {
                return Err(Error::DifferentSchematics);
            }
            *first
        }
        None => resolve::schematic(tx.project(), None)?.id(),
    };
    let seg_ref = |segment| NetSegmentRef { schematic, segment };

    // Resolve the anchors (may split lines).
    let a = End::resolve(tx, schematic, start)?;
    let b = End::resolve(tx, schematic, end)?;

    // Positions of the wire, without consecutive duplicates.
    let mut positions: Vec<Point> = vec![a.position];
    for p in points.iter().copied().chain([b.position]) {
        if positions.last() != Some(&p) {
            positions.push(p);
        }
    }
    if positions.len() < 2 {
        return Err(Error::InvalidArgument(
            "The start and end of the wire are at the same position.".to_owned(),
        ));
    }
    if a.anchor.is_some() && a.anchor == b.anchor {
        return Err(Error::InvalidArgument(
            "The start and end of the wire are the same anchor.".to_owned(),
        ));
    }

    // The net name to apply: the requested one, else the forced net name of
    // an unconnected pin at the end or the start (upstream:
    // `otherForcedNetName` of the end pin, `forcedNetName` of the start pin).
    let forced = |end: &End| {
        end.pin
            .filter(|_| end.segment.is_none())
            .and_then(|(signal, _)| forced_net_name(tx.project(), signal))
    };
    let requested = net_name
        .cloned()
        .or_else(|| forced(&b))
        .or_else(|| forced(&a));
    let net_name = requested.as_ref();

    // Determine the net and the target segment (upstream: the fixed start
    // anchor determines the current segment/net, the end anchor's segment
    // is combined with it).
    let named_net = net_name.and_then(|n| {
        tx.project()
            .circuit()
            .net_signal_by_name(n.as_str())
            .map(|(id, _)| id)
    });
    let segment_net = |tx: &Transaction<'_>, segment| -> Result<NetSignalId> {
        tx.project()
            .schematic(schematic)
            .and_then(|s| s.net_segments().get(&segment))
            .map(|s| s.net())
            .ok_or_else(|| Error::not_found("Net segment", segment))
    };
    // (target segment, net, segment to combine into the target)
    let (target, net, merge_from) = match (a.segment, b.segment) {
        (None, None) => {
            let pin_net = a.pin.and_then(|(_, n)| n).or(b.pin.and_then(|(_, n)| n));
            let net = match (named_net, pin_net) {
                (Some(n), _) | (None, Some(n)) => n,
                (None, None) => {
                    let class = default_net_class(tx)?;
                    add_net(tx, net_name.cloned(), class)?
                }
            };
            (None, net, None)
        }
        (Some(s), None) | (None, Some(s)) => (Some(s), segment_net(tx, s)?, None),
        (Some(s1), Some(s2)) if s1 == s2 => (Some(s1), segment_net(tx, s1)?, None),
        (Some(s1), Some(s2)) => {
            let this_net = segment_net(tx, s1)?;
            let other_net = segment_net(tx, s2)?;
            if this_net != other_net {
                let p = tx.project();
                let circuit = p.circuit();
                let this_auto = circuit
                    .net_signal(this_net)
                    .is_some_and(|n| n.has_auto_name());
                let other_auto = circuit
                    .net_signal(other_net)
                    .is_some_and(|n| n.has_auto_name());
                // Segments with forced net names keep their net.
                let other_forced = segment_has_forced_net_name(p, schematic, s2);
                let this_forced = segment_has_forced_net_name(p, schematic, s1);
                if other_forced {
                    change_net_of_schematic_segment(tx, seg_ref(s1), other_net)?;
                } else if this_forced || (other_auto && !this_auto) {
                    change_net_of_schematic_segment(tx, seg_ref(s2), this_net)?;
                } else {
                    change_net_of_schematic_segment(tx, seg_ref(s1), other_net)?;
                }
            }
            (Some(s2), segment_net(tx, s2)?, Some(s1))
        }
    };

    // Connect the pins without lines to the net.
    for end in [&a, &b] {
        if let (None, Some((signal, pin_net))) = (end.segment, end.pin)
            && pin_net != Some(net)
        {
            tx.apply(Mutation::SetComponentSignalNet {
                signal,
                net: Some(net),
            })?;
        }
    }

    // Create the junctions and lines.
    let width = SchematicNetSegment::default_line_width();
    let mut junctions = Vec::new();
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
            let anchor = NetLineAnchor::Junction(junction.uuid());
            junctions.push(junction);
            anchor
        }));
    }
    let lines: Vec<NetLine> = anchors
        .windows(2)
        .map(|w| NetLine::new(Uuid::new_random(), width, w[0], w[1]))
        .collect();
    let result_junctions = junctions.iter().map(Junction::uuid).collect();
    let result_lines = lines.iter().map(NetLine::uuid).collect();
    let segment = match (target, merge_from) {
        (Some(segment), Some(from)) => {
            combine_schematic_segments(tx, schematic, from, segment, junctions, lines)?;
            segment
        }
        (Some(segment), None) => {
            tx.apply(sch(SchematicMutation::AddNetSegmentElements {
                segment: seg_ref(segment),
                junctions,
                lines,
            }))?;
            segment
        }
        (None, _) => {
            let mut segment = SchematicNetSegment::new(Uuid::new_random(), net);
            for junction in junctions {
                segment.insert_junction(junction);
            }
            for line in lines {
                segment.insert_line(line);
            }
            let id = segment.id();
            tx.apply(sch(SchematicMutation::AddNetSegment { schematic, segment }))?;
            id
        }
    };

    // Apply the requested net name.
    let net = match net_name {
        Some(name) => apply_net_name(tx, seg_ref(segment), name)?,
        None => net,
    };
    Ok(WireResult {
        schematic,
        segment,
        net,
        junctions: result_junctions,
        lines: result_lines,
    })
}

/// Draws a wire from `start` over `points` to `end` (upstream
/// `SchematicEditorState_DrawWire`): creates a net segment (and a net with
/// an automatic name) if neither end is connected, extends the segment of
/// the connected end, or combines the segments (and nets) of both ends.
/// Unconnected pins are connected to the net. If `net` is given, the
/// resulting segment is assigned to the existing net with that name, or
/// its net is renamed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DrawWire {
    /// The schematic (default: the schematic of the anchors, or the first
    /// schematic if both ends are points).
    #[serde(default)]
    pub schematic: Option<SchematicId>,
    /// The start.
    pub start: WireAnchor,
    /// The end.
    pub end: WireAnchor,
    /// Intermediate points (corners).
    #[serde(default)]
    pub points: Vec<Point>,
    /// Name of the net the wire should belong to.
    #[serde(default)]
    pub net: Option<CircuitIdentifier>,
}

impl Command for DrawWire {
    type Output = WireResult;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::SchematicEditorState_DrawWire",
            "Draw Wire"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<WireResult> {
        draw_wire(
            tx,
            self.schematic,
            &self.start,
            &self.end,
            &self.points,
            self.net.as_ref(),
        )
    }
}

/// Connects two pins with a wire routed straight or with one corner (the
/// pins must be on the same schematic page; use [`ConnectPinToNet`] across
/// pages).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConnectPins {
    /// The first pin.
    pub a: PinRef,
    /// The second pin.
    pub b: PinRef,
    /// How the corner is placed (default: horizontal, then vertical).
    #[serde(default)]
    pub mode: WireMode,
    /// Name of the net the wire should belong to.
    #[serde(default)]
    pub net: Option<CircuitIdentifier>,
}

impl Command for ConnectPins {
    type Output = WireResult;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::SchematicEditorState_DrawWire",
            "Draw Wire"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<WireResult> {
        let a = resolve::pin(tx.project(), &self.a)?;
        let b = resolve::pin(tx.project(), &self.b)?;
        if a.schematic != b.schematic {
            return Err(Error::DifferentSchematics);
        }
        let middle = self.mode.middle_point(a.position, b.position);
        let points: Vec<Point> = [middle]
            .into_iter()
            .filter(|m| *m != a.position && *m != b.position)
            .collect();
        draw_wire(
            tx,
            Some(a.schematic),
            &WireAnchor::SymbolPin {
                symbol: a.symbol,
                pin: a.pin,
            },
            &WireAnchor::SymbolPin {
                symbol: b.symbol,
                pin: b.pin,
            },
            &points,
            self.net.as_ref(),
        )
    }
}

/// Default length of pin stubs (2.54 mm).
pub(crate) const DEFAULT_STUB_LENGTH: Length = Length::new(2_540_000);

/// Returns rotation and mirror state of a net label at `pos` whose wire
/// continues towards `dir_pos` (upstream `updateNetLabelPosition()`).
pub(crate) fn net_label_orientation(pos: Point, dir_pos: Point) -> (Angle, bool) {
    let dir = toolbox::angle_between_points(pos, dir_pos).rounded(Angle::DEG90);
    let mirror = dir.mapped_to_0_360deg() >= Angle::DEG180;
    let rotation = if mirror { dir + Angle::DEG180 } else { dir };
    (rotation.mapped_to_0_360deg(), mirror)
}

/// Result of [`ConnectPinToNet`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PinNetResult {
    /// The net segment of the pin.
    pub segment: NetSegmentId,
    /// The net.
    pub net: NetSignalId,
    /// The stub wire, if one was drawn.
    pub wire: Option<WireResult>,
    /// The added net label, if any.
    pub label: Option<Uuid>,
}

/// Connects a pin to a net by name: an unconnected pin gets a short stub
/// wire (pointing away from the symbol) with a net label, a connected pin's
/// segment is moved to the net (or the net is renamed if no net has the
/// name yet) and gets a label if it has none. Nets with the same name are
/// connected across pages through their labels, like upstream.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConnectPinToNet {
    /// The pin.
    pub pin: PinRef,
    /// The net name.
    pub net: CircuitIdentifier,
    /// Whether a net label is added (default: true).
    #[serde(default = "default_true")]
    pub label: bool,
    /// Length of the stub wire (default: 2.54 mm).
    #[serde(default)]
    pub stub_length: Option<UnsignedLength>,
}

fn default_true() -> bool {
    true
}

impl Command for ConnectPinToNet {
    type Output = PinNetResult;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::SchematicEditorState_DrawWire",
            "Draw Wire"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<PinNetResult> {
        let pin = resolve::pin(tx.project(), &self.pin)?;
        let schematic = pin.schematic;
        let existing = tx
            .project()
            .schematic(schematic)
            .and_then(|s| s.pin_net_segment(pin.symbol, pin.pin));
        let (segment, net, wire, label_pos, dir_pos) = match existing {
            Some(segment) => {
                let r = NetSegmentRef { schematic, segment };
                let net = apply_net_name(tx, r, &self.net)?;
                (segment, net, None, pin.position, None)
            }
            None => {
                let length = self.stub_length.map_or(DEFAULT_STUB_LENGTH, |l| *l);
                if length <= Length::ZERO {
                    return Err(Error::InvalidArgument(
                        "The stub length must be greater than zero.".to_owned(),
                    ));
                }
                // The pin points from its connection point towards the
                // symbol body, the stub the other way.
                let end = Point::new(pin.position.x + length, pin.position.y)
                    .rotated(pin.rotation + Angle::DEG180, pin.position);
                let wire = draw_wire(
                    tx,
                    Some(schematic),
                    &WireAnchor::SymbolPin {
                        symbol: pin.symbol,
                        pin: pin.pin,
                    },
                    &WireAnchor::Point(end),
                    &[],
                    Some(&self.net),
                )?;
                (wire.segment, wire.net, Some(wire), end, Some(pin.position))
            }
        };
        let has_labels = tx
            .project()
            .schematic(schematic)
            .and_then(|s| s.net_segments().get(&segment))
            .is_some_and(|s| !s.labels().is_empty());
        let mut label = None;
        if self.label && !has_labels {
            let (rotation, mirrored) = match dir_pos {
                Some(dir_pos) => net_label_orientation(label_pos, dir_pos),
                None => (Angle::DEG0, false),
            };
            let l = NetLabel::new(Uuid::new_random(), label_pos, rotation, mirrored);
            label = Some(l.uuid());
            tx.apply(sch(SchematicMutation::AddNetLabel {
                segment: NetSegmentRef { schematic, segment },
                label: l,
            }))?;
        }
        Ok(PinNetResult {
            segment,
            net,
            wire,
            label,
        })
    }
}

/// Adds a net label to a net segment (upstream `CmdSchematicNetLabelAdd`,
/// as `SchematicEditorState_AddLabel` does).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddNetLabel {
    /// The net segment.
    pub segment: NetSegmentId,
    /// The position.
    pub position: Point,
    /// The rotation.
    #[serde(default)]
    pub rotation: Angle,
    /// Whether the label is mirrored.
    #[serde(default)]
    pub mirrored: bool,
}

impl Command for AddNetLabel {
    /// UUID of the label.
    type Output = Uuid;

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdSchematicNetLabelAdd", "Add Net Label")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<Uuid> {
        let schematic = segment_schematic(tx.project(), self.segment)?;
        let label = NetLabel::new(
            Uuid::new_random(),
            self.position,
            self.rotation,
            self.mirrored,
        );
        let uuid = label.uuid();
        tx.apply(sch(SchematicMutation::AddNetLabel {
            segment: NetSegmentRef {
                schematic,
                segment: self.segment,
            },
            label,
        }))?;
        Ok(uuid)
    }
}

/// Items of a schematic to remove, see [`RemoveSchematicItems`].
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SchematicSelection {
    /// Symbols (their wires are removed; components without symbols are
    /// removed with their devices).
    #[serde(default)]
    pub symbols: Vec<SymbolId>,
    /// Net lines as (segment, line).
    #[serde(default)]
    pub net_lines: Vec<(NetSegmentId, Uuid)>,
    /// Net labels as (segment, label).
    #[serde(default)]
    pub net_labels: Vec<(NetSegmentId, Uuid)>,
    /// Bus lines as (bus segment, line); junctions whose lines are all
    /// removed are removed too (net lines attached to them end at new net
    /// junctions).
    #[serde(default)]
    pub bus_lines: Vec<(BusSegmentId, Uuid)>,
    /// Bus labels as (bus segment, label).
    #[serde(default)]
    pub bus_labels: Vec<(BusSegmentId, Uuid)>,
    /// Texts of symbols as (symbol, text).
    #[serde(default)]
    pub symbol_texts: Vec<(SymbolId, Uuid)>,
    /// Polygons.
    #[serde(default)]
    pub polygons: Vec<Uuid>,
    /// Texts.
    #[serde(default)]
    pub texts: Vec<Uuid>,
    /// Images.
    #[serde(default)]
    pub images: Vec<Uuid>,
}

/// Removes schematic items (upstream `CmdRemoveSelectedSchematicItems`, see
/// [`RemoveSchematicItems`]). Returns whether anything was removed.
pub(crate) fn remove_schematic_items(
    tx: &mut Transaction<'_>,
    schematic: SchematicId,
    selection: &SchematicSelection,
) -> Result<bool> {
    let p = tx.project();
    let s = p
        .schematic(schematic)
        .ok_or_else(|| Error::not_found("Schematic", schematic))?;
    let ctx = p.view();
    let symbols: BTreeSet<SymbolId> = selection.symbols.iter().copied().collect();
    for symbol in &symbols {
        if !s.symbols().contains_key(symbol) {
            return Err(Error::not_found("Symbol", symbol));
        }
    }

    // Collect the lines and labels to remove per segment (selected lines,
    // lines at pins of selected symbols, selected labels).
    let mut lines: BTreeMap<NetSegmentId, BTreeSet<Uuid>> = BTreeMap::new();
    let mut labels: BTreeMap<NetSegmentId, BTreeSet<Uuid>> = BTreeMap::new();
    for (segment, line) in &selection.net_lines {
        let seg = s
            .net_segments()
            .get(segment)
            .ok_or_else(|| Error::not_found("Net segment", segment))?;
        if !seg.lines().contains_key(line) {
            return Err(Error::not_found("Net line", line));
        }
        lines.entry(*segment).or_default().insert(*line);
    }
    for (segment, label) in &selection.net_labels {
        let seg = s
            .net_segments()
            .get(segment)
            .ok_or_else(|| Error::not_found("Net segment", segment))?;
        if !seg.labels().contains_key(label) {
            return Err(Error::not_found("Net label", label));
        }
        labels.entry(*segment).or_default().insert(*label);
    }
    for (id, segment) in s.net_segments() {
        for line in segment.lines().values() {
            let at_symbol = [line.p1(), line.p2()].into_iter().any(|a| {
                matches!(a, NetLineAnchor::Pin { symbol, .. } if symbols.contains(&SymbolId(symbol)))
            });
            if at_symbol {
                lines.entry(*id).or_default().insert(line.uuid());
            }
        }
    }

    // Bus items (upstream `addSelectedBusLines()`, `addSelectedBusLabels()`
    // and `addJunctionsOfBusLines(true)`).
    let mut bus_lines: BTreeMap<BusSegmentId, BTreeSet<Uuid>> = BTreeMap::new();
    let mut bus_labels: BTreeMap<BusSegmentId, BTreeSet<Uuid>> = BTreeMap::new();
    for (segment, line) in &selection.bus_lines {
        let seg = s
            .bus_segments()
            .get(segment)
            .ok_or_else(|| Error::not_found("Bus segment", segment))?;
        if !seg.lines().contains_key(line) {
            return Err(Error::not_found("Bus line", line));
        }
        bus_lines.entry(*segment).or_default().insert(*line);
    }
    for (segment, label) in &selection.bus_labels {
        let seg = s
            .bus_segments()
            .get(segment)
            .ok_or_else(|| Error::not_found("Bus segment", segment))?;
        if !seg.labels().contains_key(label) {
            return Err(Error::not_found("Bus label", label));
        }
        bus_labels.entry(*segment).or_default().insert(*label);
    }
    let mut bus_junctions: BTreeSet<(BusSegmentId, Uuid)> = BTreeSet::new();
    for (segment, removed) in &bus_lines {
        let seg = &s.bus_segments()[segment];
        for line in removed {
            for anchor in [seg.lines()[line].p1(), seg.lines()[line].p2()] {
                if let NetLineAnchor::Junction(j) = anchor
                    && seg.lines_at(j).all(|l| removed.contains(&l.uuid()))
                {
                    bus_junctions.insert((*segment, j));
                }
            }
        }
    }
    let affected_buses: BTreeSet<BusSegmentId> =
        bus_lines.keys().chain(bus_labels.keys()).copied().collect();
    let mut affected: BTreeSet<NetSegmentId> = lines.keys().chain(labels.keys()).copied().collect();
    for bus in &affected_buses {
        affected.extend(s.attached_net_segments(*bus));
    }

    // Split the affected segments (upstream `removeNetSegmentItems()`).
    struct Remaining {
        net: NetSignalId,
        segment: librepcb_core::project::schematic::SplitSegment,
    }
    struct Plan {
        segment: NetSegmentId,
        disconnect: BTreeSet<ComponentSignalRef>,
        remaining: Vec<Remaining>,
    }
    let mut plans = Vec::new();
    for id in &affected {
        let segment = &s.net_segments()[id];
        let no_lines = BTreeSet::new();
        let removed_lines = lines.get(id).unwrap_or(&no_lines);
        let removed_labels = labels.get(id).unwrap_or(&no_lines);
        let mut splitter = SchematicNetSegmentSplitter::new();
        for (symbol, pin) in segment.connected_pins() {
            let anchor = NetLineAnchor::Pin {
                symbol: symbol.0,
                pin,
            };
            if let Some(pos) = s.net_line_anchor_position(*id, anchor, ctx) {
                splitter.add_fixed_anchor(anchor, pos, false);
            }
        }
        for (bus_segment, junction) in segment.connected_bus_junctions() {
            let anchor = NetLineAnchor::BusJunction {
                segment: bus_segment.0,
                junction,
            };
            if let Some(pos) = s.net_line_anchor_position(*id, anchor, ctx) {
                let replace = bus_junctions.contains(&(bus_segment, junction));
                splitter.add_fixed_anchor(anchor, pos, replace);
            }
        }
        for junction in segment.junctions().values() {
            splitter.add_junction(junction.clone());
        }
        for line in segment.lines().values() {
            if !removed_lines.contains(&line.uuid()) {
                splitter.add_net_line(line.clone());
            }
        }
        for label in segment.labels().values() {
            if !removed_labels.contains(&label.uuid()) {
                splitter.add_net_label(label.clone());
            }
        }
        // Pins whose lines are all removed.
        let mut disconnected_pins = BTreeSet::new();
        for (symbol, pin) in segment.connected_pins() {
            let anchor = NetLineAnchor::Pin {
                symbol: symbol.0,
                pin,
            };
            if segment
                .lines_at(anchor)
                .all(|l| removed_lines.contains(&l.uuid()))
            {
                disconnected_pins.insert((symbol, pin));
            }
        }
        // Component signals whose (placed) pins are all disconnected.
        let mut pin_signals: BTreeMap<ComponentSignalRef, BTreeSet<(SymbolId, Uuid)>> =
            BTreeMap::new();
        for (symbol, pin) in &disconnected_pins {
            if let Some(view) = s.symbols()[symbol].pin(ctx, *pin)? {
                pin_signals.entry(view.signal()).or_default();
            }
        }
        let mut disconnect = BTreeSet::new();
        for signal in pin_signals.keys() {
            let registered = registered_symbol_pins(p, *signal)?;
            if registered
                .iter()
                .all(|(sch_id, pin)| *sch_id == schematic && disconnected_pins.contains(pin))
            {
                disconnect.insert(*signal);
            }
        }
        plans.push(Plan {
            segment: *id,
            disconnect,
            remaining: splitter
                .split()
                .into_iter()
                .map(|segment| Remaining {
                    net: s.net_segments()[id].net(),
                    segment,
                })
                .collect(),
        });
    }

    // Split the affected bus segments (upstream `removeBusSegmentItems()`).
    struct BusPlan {
        segment: SchematicBusSegment,
        remaining: Vec<librepcb_core::project::schematic::SplitSegment>,
    }
    let mut bus_plans = Vec::new();
    for id in &affected_buses {
        let segment = &s.bus_segments()[id];
        let none = BTreeSet::new();
        let removed_lines = bus_lines.get(id).unwrap_or(&none);
        let removed_labels = bus_labels.get(id).unwrap_or(&none);
        let mut splitter = SchematicNetSegmentSplitter::new();
        for junction in segment.junctions().values() {
            if !bus_junctions.contains(&(*id, junction.uuid())) {
                splitter.add_junction(junction.clone());
            }
        }
        for line in segment.lines().values() {
            if !removed_lines.contains(&line.uuid()) {
                splitter.add_net_line(line.clone());
            }
        }
        for label in segment.labels().values() {
            if !removed_labels.contains(&label.uuid()) {
                splitter.add_net_label(label.clone());
            }
        }
        bus_plans.push(BusPlan {
            segment: segment.clone(),
            remaining: splitter.split(),
        });
    }

    let mut changed = false;
    let mut remaining = Vec::new();
    for plan in plans {
        tx.apply(sch(SchematicMutation::RemoveNetSegment(NetSegmentRef {
            schematic,
            segment: plan.segment,
        })))?;
        for signal in plan.disconnect {
            set_component_signal_net(tx, signal, None)?;
        }
        remaining.extend(plan.remaining);
        changed = true;
    }

    // Re-add the remaining parts of the bus segments; parts without labels
    // get new buses (with the properties of the old bus). Bus junctions
    // keep their UUIDs; `bus_junction_map` maps them to their new segment.
    let mut bus_junction_map: BTreeMap<(Uuid, Uuid), Uuid> = BTreeMap::new();
    for plan in bus_plans {
        let old_id = plan.segment.id();
        tx.apply(sch(SchematicMutation::RemoveBusSegment(BusSegmentRef {
            schematic,
            segment: old_id,
        })))?;
        changed = true;
        let old_bus = tx
            .project()
            .circuit()
            .bus(plan.segment.bus())
            .cloned()
            .ok_or_else(|| Error::not_found("Bus", plan.segment.bus()))?;
        let mut new_segments = Vec::new();
        for part in plan.remaining {
            let mut segment = SchematicBusSegment::new(Uuid::new_random(), plan.segment.bus());
            for junction in part.junctions {
                bus_junction_map.insert((old_id.0, junction.uuid()), segment.uuid());
                segment.insert_junction(junction);
            }
            for line in part.lines {
                segment.insert_line(line);
            }
            for label in part.labels {
                segment.insert_label(label);
            }
            new_segments.push(segment);
        }
        for mut segment in new_segments {
            if segment.labels().is_empty() {
                let bus = super::bus::add_bus(tx, None)?;
                let mut new_bus = tx
                    .project()
                    .circuit()
                    .bus(bus)
                    .cloned()
                    .ok_or_else(|| Error::not_found("Bus", bus))?;
                new_bus.set_prefix_net_names(old_bus.prefix_net_names());
                new_bus.set_max_trace_length_difference(old_bus.max_trace_length_difference());
                tx.apply(Mutation::UpdateBus(new_bus))?;
                let mut s2 = SchematicBusSegment::new(segment.uuid(), bus);
                for j in segment.junctions().values() {
                    s2.insert_junction(j.clone());
                }
                for l in segment.lines().values() {
                    s2.insert_line(l.clone());
                }
                segment = s2;
            }
            tx.apply(sch(SchematicMutation::AddBusSegment { schematic, segment }))?;
        }
    }
    // Net lines at kept bus junctions end at the junctions' new segments.
    for part in &mut remaining {
        for line in &mut part.segment.lines {
            let map = |a: NetLineAnchor| match a {
                NetLineAnchor::BusJunction { segment, junction } => bus_junction_map
                    .get(&(segment, junction))
                    .map_or(a, |new| NetLineAnchor::BusJunction {
                        segment: *new,
                        junction,
                    }),
                other => other,
            };
            let (p1, p2) = (map(line.p1()), map(line.p2()));
            line.set_anchors(p1, p2);
        }
    }

    // Add the remaining parts as new segments (upstream
    // `addRemainingNetSegmentItems()`).
    let mut new_segments = Vec::new();
    for part in remaining {
        let mut segment = SchematicNetSegment::new(Uuid::new_random(), part.net);
        for junction in part.segment.junctions {
            segment.insert_junction(junction);
        }
        for line in part.segment.lines {
            segment.insert_line(line);
        }
        let has_labels = !part.segment.labels.is_empty();
        for label in part.segment.labels {
            segment.insert_label(label);
        }
        new_segments.push((segment.id(), part.net, has_labels));
        tx.apply(sch(SchematicMutation::AddNetSegment { schematic, segment }))?;
    }
    // Assign new nets to unlabeled parts (after all segments were added,
    // so that nets are not removed too early).
    // Parts with exactly one forced net name get that net (upstream
    // `SI_NetSegment::getForcedNetName()`).
    for (segment, old_net, has_labels) in new_segments {
        let mut forced = segment_forced_net_names(tx.project(), schematic, segment);
        let forced = (forced.len() == 1).then(|| forced.pop_first()).flatten();
        if has_labels && forced.is_none() {
            continue;
        }
        let circuit = tx.project().circuit();
        let old = circuit.net_signal(old_net);
        if let (Some(name), Some(old)) = (&forced, old)
            && old.name() == name
        {
            continue;
        }
        let existing = forced
            .as_ref()
            .and_then(|name| circuit.net_signal_by_name(name.as_str()))
            .map(|(id, _)| id);
        let class = old.map(|n| n.net_class());
        let net = match (existing, class) {
            (Some(net), _) => net,
            (None, Some(class)) => add_net(tx, forced, class)?,
            (None, None) => {
                let class = default_net_class(tx)?;
                add_net(tx, forced, class)?
            }
        };
        change_net_of_schematic_segment(tx, NetSegmentRef { schematic, segment }, net)?;
    }

    // Texts.
    for (symbol, text) in &selection.symbol_texts {
        tx.apply(sch(SchematicMutation::RemoveSymbolText {
            symbol: SymbolRef {
                schematic,
                symbol: *symbol,
            },
            text: *text,
        }))?;
        changed = true;
    }
    for text in &selection.texts {
        tx.apply(sch(SchematicMutation::RemoveText {
            schematic,
            text: *text,
        }))?;
        changed = true;
    }

    // Symbols, devices and component instances.
    for symbol in symbols {
        let component = tx
            .project()
            .schematic(schematic)
            .and_then(|s| s.symbols().get(&symbol))
            .ok_or_else(|| Error::not_found("Symbol", symbol))?
            .component();
        tx.apply(sch(SchematicMutation::RemoveSymbol(SymbolRef {
            schematic,
            symbol,
        })))?;
        changed = true;
        let has_symbols = tx
            .project()
            .component_uses(component)
            .is_some_and(|u| !u.symbols.is_empty());
        if !has_symbols {
            remove_component_devices(tx, component)?;
            tx.apply(Mutation::RemoveComponentInstance(component))?;
        }
    }

    for polygon in &selection.polygons {
        tx.apply(sch(SchematicMutation::RemovePolygon {
            schematic,
            polygon: *polygon,
        }))?;
        changed = true;
    }
    for image in &selection.images {
        super::image::remove_schematic_image(tx, schematic, *image)?;
        changed = true;
    }

    if changed {
        remove_unused_nets(tx)?;
        remove_unused_library_elements(tx)?;
    }
    Ok(changed)
}

/// Removes the devices of a component from all boards (upstream
/// `CmdRemoveBoardItems::removeDeviceInstances()` per board).
pub(crate) fn remove_component_devices(
    tx: &mut Transaction<'_>,
    component: librepcb_core::project::ComponentInstanceId,
) -> Result<()> {
    let boards: Vec<_> = tx
        .project()
        .component_uses(component)
        .map(|u| u.devices.iter().copied().collect())
        .unwrap_or_default();
    for board in boards {
        let selection = BoardSelection {
            devices: vec![component],
            ..BoardSelection::default()
        };
        remove_board_items(tx, board, &selection, false)?;
    }
    Ok(())
}

/// Returns all pins of placed symbols of the component which belong to the
/// signal (upstream `ComponentSignalInstance::getRegisteredSymbolPins()`).
pub(crate) fn registered_symbol_pins(
    p: &Project,
    signal: ComponentSignalRef,
) -> Result<Vec<(SchematicId, (SymbolId, Uuid))>> {
    let mut result = Vec::new();
    let Some(uses) = p.component_uses(signal.component) else {
        return Ok(result);
    };
    for (schematic, symbol) in uses.symbols.values() {
        let Some(s) = p
            .schematic(*schematic)
            .and_then(|s| s.symbols().get(symbol))
        else {
            continue;
        };
        for view in s.pins(p.view())? {
            if view.signal() == signal {
                result.push((*schematic, (*symbol, view.uuid())));
            }
        }
    }
    Ok(result)
}

/// Removes schematic items (upstream `CmdRemoveSelectedSchematicItems`):
/// the affected net segments are split into their remaining cohesive parts
/// (unlabeled parts get new nets), pins which lose all their lines are
/// disconnected from their nets (removing traces at their pads), symbols
/// are removed, and components without symbols are removed with their
/// devices. Finally unused nets and library elements are removed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemoveSchematicItems {
    /// The schematic.
    pub schematic: SchematicId,
    /// The items.
    pub selection: SchematicSelection,
}

impl Command for RemoveSchematicItems {
    /// Whether anything was removed.
    type Output = bool;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdRemoveSelectedSchematicItems",
            "Remove Schematic Elements"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<bool> {
        remove_schematic_items(tx, self.schematic, &self.selection)
    }
}
