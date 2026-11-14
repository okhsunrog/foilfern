//! Port of libs/librepcb/core/project/erc/electricalrulecheck.{h,cpp}.
//!
//! Differences to upstream (none affects the messages or approvals):
//! - Items are visited in UUID order where upstream iterates a `QHash` or
//!   `QSet` (nets attached to a bus) or a registration list (net segments
//!   of a net, bus segments of a bus: here schematic page order resp.
//!   schematic UUID order, then segment UUID order). This only decides the
//!   order of the messages and which of several equivalent items a message
//!   location points to.
//! - Symbols whose pins cannot be computed from the project library
//!   (inconsistent library, which the loader rejects) are skipped.

use std::collections::{BTreeMap, BTreeSet};

use super::electrical_rule_check_messages::{ErcMessage, ErcMessageKind, Names, build_message};
use crate::geometry::{NetLine, NetLineAnchor, Path};
use crate::library::cmp::Component;
use crate::project::attribute_lookup::ProjectAttributeLookup;
use crate::project::circuit::ComponentInstance;
use crate::project::id::{
    BusSegmentId, BusSegmentRef, ComponentSignalRef, NetSegmentId, NetSegmentRef, NetSignalId,
    SchematicId,
};
use crate::project::ref_index::NetUse;
use crate::project::schematic::{Schematic, SchematicNetSegment, SchematicSymbol, SymbolPinView};
use crate::project::{BusId, Project};
use crate::types::{Length, Point, PositiveLength, Uuid};

/// Runs the electrical rule check on `project` (upstream
/// `ElectricalRuleCheck::runChecks()`).
///
/// Use [`all_approvals()`](crate::rule_check::all_approvals) on the
/// [messages](ErcMessage::message) to get the approvals of all messages,
/// e.g. to compare them with [`Project::erc_approvals()`].
pub fn run_erc(project: &Project) -> Vec<ErcMessage> {
    let mut check = Check {
        p: project,
        open_nets: BTreeSet::new(),
        msgs: Vec::new(),
    };
    check.check_net_classes();
    check.check_net_signals();
    check.check_buses();
    check.check_components();
    check.check_schematics();
    check.msgs
}

/// Where a message points to (upstream `ErcMsgBase::setLocation()`).
#[derive(Debug, Default)]
pub(super) struct Location {
    pub schematic: Option<SchematicId>,
    pub paths: Vec<Path>,
}

struct Check<'a> {
    p: &'a Project,
    open_nets: BTreeSet<NetSignalId>,
    msgs: Vec<ErcMessage>,
}

impl<'a> Check<'a> {
    fn push(&mut self, kind: ErcMessageKind, names: Names<'_>, location: Location) {
        self.msgs.push(build_message(kind, &names, location));
    }

    fn net_name(&self, net: NetSignalId) -> &'a str {
        self.p
            .circuit
            .net_signal(net)
            .map_or("", |n| n.name().as_str())
    }

    fn lib_component(&self, cmp: &ComponentInstance) -> Option<&'a Component> {
        self.p.library.component(&cmp.lib_component())
    }

    // --- Checks ---

    fn check_net_classes(&mut self) {
        let circuit = &self.p.circuit;
        // Don't warn if there's only one netclass, as we need one to be used
        // as default when adding a new wire.
        if circuit.net_classes().len() <= 1 {
            return;
        }
        for (id, net_class) in circuit.net_classes() {
            let used = circuit.net_signals().values().any(|n| n.net_class() == *id);
            if !used {
                let names = Names {
                    a: net_class.name().as_str(),
                    ..Names::default()
                };
                self.push(
                    ErcMessageKind::UnusedNetClass(*id),
                    names,
                    Location::default(),
                );
            }
        }
    }

    fn check_net_signals(&mut self) {
        for (id, net) in self.p.circuit.net_signals() {
            // Raise a warning if the net signal is connected to less than two
            // component signals. But do not count component signals of
            // schematic-only components since these are just "virtual"
            // connections, i.e. not represented by a real pad (see
            // https://github.com/LibrePCB/LibrePCB/issues/739).
            let real_count = self
                .p
                .net_signal_uses(*id)
                .filter(|u| match u {
                    NetUse::ComponentSignal(s) => !self
                        .p
                        .circuit
                        .component_instance(s.component)
                        .and_then(|c| self.lib_component(c))
                        .is_some_and(|c| c.schematic_only()),
                    _ => false,
                })
                .count();
            if real_count < 2 {
                self.open_nets.insert(*id);
                let location = self.net_location(*id);
                let names = Names {
                    a: net.name().as_str(),
                    ..Names::default()
                };
                self.push(ErcMessageKind::OpenNet(*id), names, location);
            }
        }
    }

    fn check_buses(&mut self) {
        for (id, bus) in self.p.circuit.buses() {
            let bus_segments: Vec<(SchematicId, BusSegmentId)> =
                self.p.bus_uses(*id).copied().collect();
            if bus_segments.is_empty() {
                let names = Names {
                    a: bus.name().as_str(),
                    ..Names::default()
                };
                self.push(ErcMessageKind::UnusedBus(*id), names, Location::default());
            }

            // Collect all connected net segments.
            let mut net_segments: BTreeMap<NetSignalId, Vec<NetSegmentRef>> = BTreeMap::new();
            let mut unnamed_nets: BTreeSet<NetSignalId> = BTreeSet::new();
            for (sch_id, bs) in bus_segments {
                let Some(schematic) = self.p.schematic(sch_id) else {
                    continue;
                };
                for ns_id in schematic.attached_net_segments(bs) {
                    let Some(ns) = schematic.net_segments().get(&ns_id) else {
                        continue;
                    };
                    net_segments
                        .entry(ns.net())
                        .or_default()
                        .push(NetSegmentRef {
                            schematic: sch_id,
                            segment: ns_id,
                        });
                    if ns.labels().is_empty() {
                        unnamed_nets.insert(ns.net());
                    }
                }
            }

            // Warn about net segments without net label.
            for net in &unnamed_nets {
                if let Some(segment) = net_segments.get(net).and_then(|s| s.first()) {
                    self.push_bus_net_message(*id, *net, *segment, true);
                }
            }

            // Warn about nets which are attached only once to the bus.
            for (net, segments) in &net_segments {
                if let [segment] = segments.as_slice()
                    && !unnamed_nets.contains(net)
                {
                    self.push_bus_net_message(*id, *net, *segment, false);
                }
            }
        }
    }

    fn push_bus_net_message(
        &mut self,
        bus: BusId,
        net: NetSignalId,
        segment: NetSegmentRef,
        unnamed: bool,
    ) {
        let kind = if unnamed {
            ErcMessageKind::UnnamedNetInBus { bus, net, segment }
        } else {
            ErcMessageKind::OpenNetInBus { bus, net, segment }
        };
        let mut location = Location::default();
        if let Some(schematic) = self.p.schematic(segment.schematic)
            && let Some(ns) = schematic.net_segments().get(&segment.segment)
        {
            self.add_segment_location(&mut location, schematic, ns);
        }
        let names = Names {
            a: self.p.circuit.bus(bus).map_or("", |b| b.name().as_str()),
            b: self.net_name(net),
            ..Names::default()
        };
        self.push(kind, names, location);
    }

    fn check_components(&mut self) {
        for (id, cmp) in self.p.circuit.component_instances() {
            self.check_component_signals(cmp);

            // Check for unplaced gates.
            let Some(variant) = self
                .lib_component(cmp)
                .and_then(|c| c.symbol_variants().by_uuid(&cmp.lib_variant()))
            else {
                continue;
            };
            let placed = self.p.component_uses(*id);
            for gate in variant.symbol_items().iter() {
                if placed.is_some_and(|u| u.symbols.contains_key(&gate.uuid())) {
                    continue;
                }
                let kind = if gate.is_required() {
                    ErcMessageKind::UnplacedRequiredGate {
                        component: *id,
                        gate: gate.uuid(),
                    }
                } else {
                    ErcMessageKind::UnplacedOptionalGate {
                        component: *id,
                        gate: gate.uuid(),
                    }
                };
                let mut location = Location::default();
                self.add_component_location(&mut location, cmp);
                let names = Names {
                    a: cmp.name().as_str(),
                    b: gate.suffix().as_str(),
                    ..Names::default()
                };
                self.push(kind, names, location);
            }
        }
    }

    fn check_component_signals(&mut self, cmp: &'a ComponentInstance) {
        let Some(lib_component) = self.lib_component(cmp) else {
            return;
        };
        for (signal_uuid, sig) in cmp.signals() {
            let Some(lib_signal) = lib_component.signals().by_uuid(signal_uuid) else {
                continue;
            };
            // Check for forced net name conflict.
            let net_name = sig.net().map_or("", |n| self.net_name(n));
            let signal = ComponentSignalRef {
                component: cmp.id(),
                signal: *signal_uuid,
            };
            if lib_signal.is_required() && sig.net().is_none() {
                let mut location = Location::default();
                self.add_signal_location(&mut location, cmp, *signal_uuid);
                let names = Names {
                    a: cmp.name().as_str(),
                    b: lib_signal.name().as_str(),
                    ..Names::default()
                };
                self.push(
                    ErcMessageKind::UnconnectedRequiredSignal(signal),
                    names,
                    location,
                );
            } else if lib_signal.is_net_signal_name_forced() {
                let forced = ProjectAttributeLookup::for_component(self.p, cmp, None, None)
                    .substitute(lib_signal.forced_net_name());
                if forced != net_name {
                    let mut location = Location::default();
                    self.add_signal_location(&mut location, cmp, *signal_uuid);
                    let names = Names {
                        a: net_name,
                        b: &forced,
                        c: cmp.name().as_str(),
                        d: lib_signal.name().as_str(),
                    };
                    self.push(
                        ErcMessageKind::ForcedNetSignalNameConflict(signal),
                        names,
                        location,
                    );
                }
            }
        }
    }

    fn check_schematics(&mut self) {
        for schematic in &self.p.schematics {
            self.check_symbols(schematic);
            self.check_net_segments(schematic);
            self.check_bus_segments(schematic);
        }
    }

    fn check_symbols(&mut self, schematic: &'a Schematic) {
        for symbol in schematic.symbols().values() {
            let Ok(pins) = symbol.pins(self.p.view()) else {
                continue;
            };
            for pin in pins {
                if pin.net().is_some() && !is_pin_wired(schematic, &pin) {
                    let name = symbol.name(self.p.view()).unwrap_or_default();
                    let names = Names {
                        a: &name,
                        b: pin.name(),
                        ..Names::default()
                    };
                    let location = Location {
                        schematic: Some(schematic.id()),
                        paths: vec![pin_circle(pin.position())],
                    };
                    self.push(
                        ErcMessageKind::ConnectedPinWithoutWire {
                            schematic: schematic.id(),
                            symbol: symbol.id(),
                            pin: pin.uuid(),
                        },
                        names,
                        location,
                    );
                }
            }
        }
    }

    fn check_net_segments(&mut self, schematic: &'a Schematic) {
        for (id, segment) in schematic.net_segments() {
            let segment_ref = NetSegmentRef {
                schematic: schematic.id(),
                segment: *id,
            };
            // Unconnected junctions.
            for (uuid, junction) in segment.junctions() {
                if segment
                    .lines_at(NetLineAnchor::Junction(*uuid))
                    .next()
                    .is_none()
                {
                    let names = Names {
                        a: self.net_name(segment.net()),
                        ..Names::default()
                    };
                    let location = Location {
                        schematic: Some(schematic.id()),
                        paths: vec![pin_circle(junction.position())],
                    };
                    self.push(
                        ErcMessageKind::UnconnectedNetJunction {
                            segment: segment_ref,
                            junction: *uuid,
                        },
                        names,
                        location,
                    );
                }
            }

            // If there are no net labels, check for any open wire. But only
            // if there's no "open net" warning on the net raised, since this
            // would be quite a duplicate warning.
            if segment.labels().is_empty() && !self.open_nets.contains(&segment.net()) {
                let open_line = segment.lines().values().find(|l| {
                    is_anchor_open(schematic, segment, l.p1())
                        || is_anchor_open(schematic, segment, l.p2())
                });
                if let Some(line) = open_line {
                    let mut location = Location {
                        schematic: Some(schematic.id()),
                        paths: Vec::new(),
                    };
                    location
                        .paths
                        .extend(self.line_obround(schematic, *id, line));
                    let names = Names {
                        a: self.net_name(segment.net()),
                        ..Names::default()
                    };
                    self.push(
                        ErcMessageKind::OpenWireInSegment {
                            segment: segment_ref,
                            line: line.uuid(),
                        },
                        names,
                        location,
                    );
                }
            }
        }
    }

    fn check_bus_segments(&mut self, schematic: &'a Schematic) {
        for (id, segment) in schematic.bus_segments() {
            for (uuid, junction) in segment.junctions() {
                if segment.lines_at(*uuid).next().is_none() {
                    let names = Names {
                        a: self
                            .p
                            .circuit
                            .bus(segment.bus())
                            .map_or("", |b| b.name().as_str()),
                        ..Names::default()
                    };
                    let location = Location {
                        schematic: Some(schematic.id()),
                        paths: vec![symbol_circle(junction.position())],
                    };
                    self.push(
                        ErcMessageKind::UnconnectedBusJunction {
                            segment: BusSegmentRef {
                                schematic: schematic.id(),
                                segment: *id,
                            },
                            junction: *uuid,
                        },
                        names,
                        location,
                    );
                }
            }
        }
    }

    // --- Locations ---

    /// Upstream `setLocation(const NetSignal&)`.
    fn net_location(&self, net: NetSignalId) -> Location {
        let mut location = Location::default();
        for schematic in &self.p.schematics {
            for segment in schematic.net_segments().values() {
                if segment.net() == net
                    && self.add_segment_location(&mut location, schematic, segment)
                {
                    return location;
                }
            }
        }
        for u in self.p.net_signal_uses(net) {
            if let NetUse::ComponentSignal(s) = u
                && let Some(cmp) = self.p.circuit.component_instance(s.component)
                && self.add_signal_location(&mut location, cmp, s.signal)
            {
                return location;
            }
        }
        location
    }

    /// Upstream `setLocation(const SI_NetSegment&)`.
    fn add_segment_location(
        &self,
        location: &mut Location,
        schematic: &Schematic,
        segment: &SchematicNetSegment,
    ) -> bool {
        for line in segment.lines().values() {
            location
                .paths
                .extend(self.line_obround(schematic, segment.id(), line));
        }
        if segment.lines().is_empty() {
            false
        } else {
            location.schematic = Some(schematic.id());
            true
        }
    }

    /// Upstream `setLocation(const ComponentSignalInstance&)`.
    fn add_signal_location(
        &self,
        location: &mut Location,
        cmp: &ComponentInstance,
        signal: Uuid,
    ) -> bool {
        for (schematic, symbol) in self.component_symbols(cmp) {
            let Ok(pins) = symbol.pins(self.p.view()) else {
                continue;
            };
            if let Some(pin) = pins.iter().find(|p| p.signal().signal == signal) {
                location.paths.push(pin_circle(pin.position()));
                location.schematic = Some(schematic.id());
                return true;
            }
        }
        self.add_component_location(location, cmp)
    }

    /// Upstream `setLocation(const ComponentInstance&)`.
    fn add_component_location(&self, location: &mut Location, cmp: &ComponentInstance) -> bool {
        if let Some((schematic, symbol)) = self.component_symbols(cmp).next() {
            location.schematic = Some(schematic.id());
            location.paths.push(symbol_circle(symbol.position()));
            true
        } else {
            false
        }
    }

    /// Returns the placed symbols of a component, in gate UUID order.
    fn component_symbols(
        &self,
        cmp: &ComponentInstance,
    ) -> impl Iterator<Item = (&'a Schematic, &'a SchematicSymbol)> + use<'a> {
        let p = self.p;
        p.component_uses(cmp.id())
            .into_iter()
            .flat_map(|u| u.symbols.values())
            .filter_map(move |(sch, sym)| {
                let schematic = p.schematic(*sch)?;
                Some((schematic, schematic.symbols().get(sym)?))
            })
    }

    /// Upstream `Path::obround(p1, p2, width + 1)` of a net line.
    fn line_obround(
        &self,
        schematic: &Schematic,
        segment: NetSegmentId,
        line: &NetLine,
    ) -> Option<Path> {
        let view = self.p.view();
        let p1 = schematic.net_line_anchor_position(segment, line.p1(), view)?;
        let p2 = schematic.net_line_anchor_position(segment, line.p2(), view)?;
        let width = PositiveLength::new(*line.width() + Length::new(1)).ok()?;
        Some(Path::obround_line(p1, p2, width))
    }
}

/// Whether wires are attached to the pin (upstream
/// `!pin->getNetLines().isEmpty()`).
fn is_pin_wired(schematic: &Schematic, pin: &SymbolPinView<'_>) -> bool {
    schematic
        .pin_net_segment(pin.symbol(), pin.uuid())
        .and_then(|s| schematic.net_segments().get(&s))
        .is_some_and(|s| s.lines_at(pin.anchor()).next().is_some())
}

/// Upstream `isOpen()` of a net line anchor: a junction with less than two
/// lines, a bus junction with at most one (bus or net) line; pins are never
/// open once a line is attached.
fn is_anchor_open(
    schematic: &Schematic,
    segment: &SchematicNetSegment,
    anchor: NetLineAnchor,
) -> bool {
    match anchor {
        NetLineAnchor::Junction(_) => segment.lines_at(anchor).count() < 2,
        NetLineAnchor::Pin { .. } => segment.lines_at(anchor).next().is_none(),
        NetLineAnchor::BusJunction {
            segment: bus_segment,
            junction,
        } => {
            let bus_lines = schematic
                .bus_segments()
                .get(&BusSegmentId(bus_segment))
                .map_or(0, |s| s.lines_at(junction).count());
            let net_lines: usize = schematic
                .bus_junction_net_segments(BusSegmentId(bus_segment), junction)
                .filter_map(|s| schematic.net_segments().get(&s))
                .map(|s| s.lines_at(anchor).count())
                .sum();
            (bus_lines + net_lines) <= 1
        }
    }
}

/// Location of a pin or net junction (circle with 1.1mm diameter).
fn pin_circle(position: Point) -> Path {
    Path::circle(PositiveLength::new(Length::new(1_100_000)).expect("positive constant"))
        .translated(position)
}

/// Location of a symbol or bus junction (circle with 2mm diameter).
fn symbol_circle(position: Point) -> Path {
    Path::circle(PositiveLength::new(Length::new(2_000_000)).expect("positive constant"))
        .translated(position)
}
