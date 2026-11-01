//! Port of libs/librepcb/core/project/erc/electricalrulecheck.{h,cpp} and
//! the location helpers from electricalrulecheckmessages.cpp.
//!
//! Rules use the existing project connectivity queries and reverse index.
//! No separate graph algorithm or Qt-style object registrations are needed.

use super::messages::Location;
use super::{ElectricalRuleCheckMessage as Message, ErcMessageKind as Kind};
use crate::geometry::{NetLine, NetLineAnchor, Path};
use crate::project::circuit::{ComponentInstance, NetSignal};
use crate::project::schematic::{Schematic, SchematicNetSegment, SchematicSymbol};
use crate::project::{
    BusSegmentRef, ComponentInstanceId, ComponentSignalRef, Error, NetSegmentId, NetSegmentRef,
    NetSignalId, NetUse, Project, ProjectAttributeLookup, Result, SymbolRef,
};
use crate::rule_check::Severity;
use crate::types::{Length, Point, PositiveLength};
use librepcb_i18n::tr;
use std::collections::{BTreeMap, BTreeSet};

/// Runs upstream's ERC rules, including findings the user has already
/// approved. Each run is independent and leaves the project unchanged.
#[derive(Debug, Clone, Copy)]
pub struct ElectricalRuleCheck<'a> {
    project: &'a Project,
}

static_assertions::assert_impl_all!(ElectricalRuleCheck<'_>: Send, Sync);

impl<'a> ElectricalRuleCheck<'a> {
    /// Creates a read-only checker for a project.
    pub fn new(project: &'a Project) -> Self {
        Self { project }
    }

    /// Returns all findings in rule order. Invalid references propagate
    /// project errors instead of being reported as a successful check.
    pub fn run_checks(&self) -> Result<Vec<Message>> {
        let mut messages = Vec::new();
        self.check_net_classes(&mut messages);
        let open_nets = self.check_nets(&mut messages)?;
        self.check_buses(&mut messages)?;
        self.check_components(&mut messages)?;
        self.check_schematics(&open_nets, &mut messages)?;
        Ok(messages)
    }

    fn check_net_classes(&self, messages: &mut Vec<Message>) {
        let circuit = self.project.circuit();
        if circuit.net_classes().len() <= 1 {
            return;
        }
        for (id, class) in circuit.net_classes() {
            if !circuit
                .net_signals()
                .values()
                .any(|net| net.net_class() == *id)
            {
                messages.push(Message::new(
                    Kind::UnusedNetClass(*id),
                    Severity::Hint,
                    tr!(
                        "ErcMsgUnusedNetClass",
                        "Unused net class: '{0}'",
                        class.name()
                    ),
                    tr!(
                        "ErcMsgUnusedNetClass",
                        "There are no nets assigned to the net class, so you could remove it."
                    ),
                    Location::default(),
                ));
            }
        }
    }

    fn check_nets(&self, messages: &mut Vec<Message>) -> Result<BTreeSet<NetSignalId>> {
        let mut open = BTreeSet::new();
        for (id, net) in self.project.circuit().net_signals() {
            let mut real_signals = 0;
            for usage in self.project.net_signal_uses(*id) {
                if let NetUse::ComponentSignal(signal) = usage {
                    let component = self.component(signal.component)?;
                    let library = self
                        .project
                        .library()
                        .component(&component.lib_component())
                        .ok_or(Error::MissingLibraryComponent(component.lib_component()))?;
                    if !library.schematic_only() {
                        real_signals += 1;
                    }
                }
            }
            if real_signals < 2 {
                open.insert(*id);
                messages.push(Message::new(Kind::OpenNet(*id), Severity::Warning,
                    tr!("ErcMsgOpenNet", "Less than two pins in net: '{0}'", net.name()),
                    tr!("ErcMsgOpenNet", "The net is connected to less than two pins, so it does not represent an electrical connection. Check if you missed to connect more pins."),
                    self.net_location(*id)?));
            }
        }
        Ok(open)
    }

    fn check_buses(&self, messages: &mut Vec<Message>) -> Result<()> {
        for (id, bus) in self.project.circuit().buses() {
            let uses: Vec<_> = self.project.bus_uses(*id).copied().collect();
            if uses.is_empty() {
                messages.push(Message::new(Kind::UnusedBus(*id), Severity::Hint,
                    tr!("ErcMsgUnusedBus", "Unused bus: '{0}'", bus.name()),
                    "There's a bus in the circuit without any schematics using it. This should not happen, please report it as a bug. But no worries, this issue is not harmful at all so you can safely ignore this message.".to_owned(),
                    Location::default()));
            }
            let mut nets: BTreeMap<NetSignalId, Vec<(&Schematic, &SchematicNetSegment)>> =
                BTreeMap::new();
            for (schematic, segment) in uses {
                // The reverse index contains only entities in this project.
                if let Some(schematic) = self.project.schematic(schematic) {
                    for net_segment in schematic.attached_net_segments(segment) {
                        if let Some(net_segment) = schematic.net_segments().get(&net_segment) {
                            nets.entry(net_segment.net())
                                .or_default()
                                .push((schematic, net_segment));
                        }
                    }
                }
            }
            for (net_id, segments) in nets {
                let unnamed = segments
                    .iter()
                    .any(|(_, segment)| segment.labels().is_empty());
                let Some(&(schematic, segment)) = segments.first() else {
                    continue;
                };
                let net = self.net(net_id)?;
                if unnamed {
                    messages.push(Message::new(Kind::UnnamedNetInBus { bus: *id, net: net_id }, Severity::Warning,
                        tr!("ErcMsgUnnamedNetInBus", "Bus contains unnamed net: '{0}:{1}'", bus.name(), net.name()),
                        tr!("ErcMsgUnnamedNetInBus", "A wire without a net label is connected to the bus, which makes it impossible for this net to leave the bus somewhere else. Add a net label to the wire to explicitly specify the net."),
                        self.segment_location(schematic, segment)?));
                } else if segments.len() == 1 {
                    messages.push(Message::new(Kind::OpenNetInBus { bus: *id, net: net_id }, Severity::Hint,
                        tr!("ErcMsgOpenNetInBus", "Bus contains unused net: '{0}:{1}'", bus.name(), net.name()),
                        tr!("ErcMsgOpenNetInBus", "The net is connected to the bus, but is not leaving the bus ^anywhere. Check if you missed to make a connection."),
                        self.segment_location(schematic, segment)?));
                }
            }
        }
        Ok(())
    }

    fn check_components(&self, messages: &mut Vec<Message>) -> Result<()> {
        for (id, component) in self.project.circuit().component_instances() {
            let library = self
                .project
                .library()
                .component(&component.lib_component())
                .ok_or(Error::MissingLibraryComponent(component.lib_component()))?;
            let lookup = ProjectAttributeLookup::component(self.project, component, None, None);
            for (signal_id, instance) in component.signals() {
                let signal = library
                    .signals()
                    .by_uuid(signal_id)
                    .ok_or(Error::InexistentComponentSignal(*signal_id))?;
                let reference = ComponentSignalRef {
                    component: *id,
                    signal: *signal_id,
                };
                if signal.is_required() && instance.net().is_none() {
                    messages.push(Message::new(Kind::UnconnectedRequiredSignal(reference), Severity::Error,
                        tr!("ErcMsgUnconnectedRequiredSignal", "Unconnected component signal: '{0}:{1}'", component.name(), signal.name()),
                        tr!("ErcMsgUnconnectedRequiredSignal", "The component signal is marked as required, but is not connected to any net. Add a wire to the corresponding symbol pin to connect it to a net."),
                        self.signal_location(reference)?));
                } else if signal.is_net_signal_name_forced() {
                    let forced = lookup.substitute(signal.forced_net_name())?;
                    let net_name = instance
                        .net()
                        .map(|id| self.net(id).map(|net| net.name().to_string()))
                        .transpose()?
                        .unwrap_or_default();
                    if forced != net_name {
                        messages.push(Message::new(Kind::ForcedNetNameConflict(reference), Severity::Error,
                            tr!("ErcMsgForcedNetSignalNameConflict", "Net name conflict: '{0}' != '{1}' ('{2}:{3}')", net_name, forced, component.name(), signal.name()),
                            tr!("ErcMsgForcedNetSignalNameConflict", "The component signal requires the attached net to be named '{0}', but it is named '{1}'. Either rename the net manually or remove this connection.", forced, net_name),
                            self.signal_location(reference)?));
                    }
                }
            }
            let variant = library
                .symbol_variants()
                .required_by_uuid(&component.lib_variant())?;
            for gate in variant.symbol_items() {
                if self
                    .project
                    .component_uses(*id)
                    .is_some_and(|uses| uses.symbols.contains_key(&gate.uuid()))
                {
                    continue;
                }
                let location = self.component_location(*id)?;
                let finding = if gate.is_required() {
                    Message::new(
                        Kind::UnplacedRequiredGate {
                            component: *id,
                            gate: gate.uuid(),
                        },
                        Severity::Error,
                        tr!(
                            "ErcMsgUnplacedRequiredGate",
                            "Unplaced required gate: '{0}:{1}'",
                            component.name(),
                            gate.suffix()
                        ),
                        tr!(
                            "ErcMsgUnplacedRequiredGate",
                            "The gate '{0}' of '{1}' is marked as required, but it is not added to the schematic.",
                            gate.suffix(),
                            component.name()
                        ),
                        location,
                    )
                } else {
                    Message::new(
                        Kind::UnplacedOptionalGate {
                            component: *id,
                            gate: gate.uuid(),
                        },
                        Severity::Warning,
                        tr!(
                            "ErcMsgUnplacedOptionalGate",
                            "Unplaced gate: '{0}:{1}'",
                            component.name(),
                            gate.suffix()
                        ),
                        tr!(
                            "ErcMsgUnplacedOptionalGate",
                            "The optional gate '{0}' of '{1}' is not added to the schematic.",
                            gate.suffix(),
                            component.name()
                        ),
                        location,
                    )
                };
                messages.push(finding);
            }
        }
        Ok(())
    }

    fn check_schematics(
        &self,
        open_nets: &BTreeSet<NetSignalId>,
        messages: &mut Vec<Message>,
    ) -> Result<()> {
        for schematic in self.project.schematics() {
            for symbol in schematic.symbols().values() {
                for pin in symbol.pins(self.project.view())? {
                    if pin.net().is_some()
                        && schematic.pin_net_segment(symbol.id(), pin.uuid()).is_none()
                    {
                        messages.push(Message::new(Kind::ConnectedPinWithoutWire {
                            symbol: SymbolRef { schematic: schematic.id(), symbol: symbol.id() }, pin: pin.uuid(),
                        }, Severity::Warning,
                        tr!("ErcMsgConnectedPinWithoutWire", "Connected pin without wire: '{0}:{1}'", symbol.name(self.project.view())?, pin.name()),
                        tr!("ErcMsgConnectedPinWithoutWire", "The pin is electrically connected to a net, but has no wire attached so this connection is not visible in the schematic. Add a wire to make the connection visible."),
                        circle_location(schematic, pin.position(), 1_100_000)));
                    }
                }
            }
            for segment in schematic.net_segments().values() {
                let reference = NetSegmentRef {
                    schematic: schematic.id(),
                    segment: segment.id(),
                };
                for junction in segment.junctions().values() {
                    if segment
                        .lines_at(NetLineAnchor::Junction(junction.uuid()))
                        .next()
                        .is_none()
                    {
                        messages.push(Message::new(Kind::UnconnectedNetJunction { segment: reference, junction: junction.uuid() }, Severity::Hint,
                            tr!("ErcMsgUnconnectedJunction", "Unconnected junction in net: '{0}'", self.net(segment.net())?.name()),
                            "There's an invisible junction in the schematic without any wire attached. This should not happen, please report it as a bug. But no worries, this issue is not harmful at all so you can safely ignore this message.".to_owned(),
                            circle_location(schematic, junction.position(), 1_100_000)));
                    }
                }
                if segment.labels().is_empty() && !open_nets.contains(&segment.net()) {
                    for line in segment.lines().values() {
                        if self.is_open(schematic, segment, line.p1())?
                            || self.is_open(schematic, segment, line.p2())?
                        {
                            messages.push(Message::new(Kind::OpenWire { segment: reference, line: line.uuid() }, Severity::Warning,
                                tr!("ErcMsgOpenWireInSegment", "Open wire in net: '{0}'", self.net(segment.net())?.name()),
                                tr!("ErcMsgOpenWireInSegment", "The wire has an open (unconnected) end with no net label attached, thus is looks like a mistake. Check if a connection to another wire or pin is missing (denoted by a cross mark)."),
                                self.line_location(schematic, segment.id(), line)?));
                            break;
                        }
                    }
                }
            }
            for segment in schematic.bus_segments().values() {
                for junction in segment.junctions().values() {
                    if segment.lines_at(junction.uuid()).next().is_none() {
                        let bus = self
                            .project
                            .circuit()
                            .bus(segment.bus())
                            .ok_or(Error::InexistentBus(segment.bus().0))?;
                        messages.push(Message::new(Kind::UnconnectedBusJunction { segment: BusSegmentRef { schematic: schematic.id(), segment: segment.id() }, junction: junction.uuid() }, Severity::Hint,
                            tr!("ErcMsgUnconnectedJunction", "Unconnected junction in bus: '{0}'", bus.name()),
                            "There's an invisible junction in the schematic without any line attached. This should not happen, please report it as a bug. But no worries, this issue is not harmful at all so you can safely ignore this message.".to_owned(),
                            circle_location(schematic, junction.position(), 2_000_000)));
                    }
                }
            }
        }
        Ok(())
    }

    fn is_open(
        &self,
        schematic: &Schematic,
        segment: &SchematicNetSegment,
        anchor: NetLineAnchor,
    ) -> Result<bool> {
        Ok(match anchor {
            NetLineAnchor::Junction(_) => segment.lines_at(anchor).count() < 2,
            NetLineAnchor::Pin { .. } => segment.lines_at(anchor).next().is_none(),
            NetLineAnchor::BusJunction {
                segment: bus_id,
                junction,
            } => {
                let bus = schematic
                    .bus_segments()
                    .get(&bus_id.into())
                    .ok_or(Error::InexistentBusSegment(bus_id))?;
                let mut count = bus.lines_at(junction).count();
                for net_segment in schematic.bus_junction_net_segments(bus.id(), junction) {
                    if let Some(segment) = schematic.net_segments().get(&net_segment) {
                        count += segment.lines_at(anchor).count();
                    }
                }
                count <= 1
            }
        })
    }

    fn component(&self, id: ComponentInstanceId) -> Result<&ComponentInstance> {
        self.project
            .circuit()
            .component_instance(id)
            .ok_or(Error::InexistentComponent(id.0))
    }
    fn net(&self, id: NetSignalId) -> Result<&NetSignal> {
        self.project
            .circuit()
            .net_signal(id)
            .ok_or(Error::InexistentNetSignal(id.0))
    }

    fn component_location(&self, component: ComponentInstanceId) -> Result<Location> {
        if let Some(uses) = self.project.component_uses(component) {
            for (schematic, symbol) in uses.symbols.values() {
                if let Some(schematic) = self.project.schematic(*schematic)
                    && let Some(symbol) = schematic.symbols().get(symbol)
                {
                    return Ok(symbol_location(schematic, symbol));
                }
            }
        }
        Ok(Location::default())
    }

    fn signal_location(&self, signal: ComponentSignalRef) -> Result<Location> {
        if let Some(uses) = self.project.component_uses(signal.component) {
            for (schematic, symbol) in uses.symbols.values() {
                if let Some(schematic) = self.project.schematic(*schematic)
                    && let Some(symbol) = schematic.symbols().get(symbol)
                {
                    for pin in symbol.pins(self.project.view())? {
                        if pin.signal() == signal {
                            return Ok(circle_location(schematic, pin.position(), 1_100_000));
                        }
                    }
                }
            }
        }
        self.component_location(signal.component)
    }

    fn net_location(&self, net: NetSignalId) -> Result<Location> {
        for usage in self.project.net_signal_uses(net) {
            if let NetUse::SchematicSegment(schematic, segment) = usage
                && let Some(schematic) = self.project.schematic(*schematic)
                && let Some(segment) = schematic.net_segments().get(segment)
            {
                let location = self.segment_location(schematic, segment)?;
                if location.schematic.is_some() {
                    return Ok(location);
                }
            }
        }
        for usage in self.project.net_signal_uses(net) {
            if let NetUse::ComponentSignal(signal) = usage {
                let location = self.signal_location(*signal)?;
                if location.schematic.is_some() {
                    return Ok(location);
                }
            }
        }
        Ok(Location::default())
    }

    fn segment_location(
        &self,
        schematic: &Schematic,
        segment: &SchematicNetSegment,
    ) -> Result<Location> {
        let mut location = Location::default();
        for line in segment.lines().values() {
            let line_location = self.line_location(schematic, segment.id(), line)?;
            location.schematic = line_location.schematic;
            location.paths.extend(line_location.paths);
        }
        Ok(location)
    }

    fn line_location(
        &self,
        schematic: &Schematic,
        segment: NetSegmentId,
        line: &NetLine,
    ) -> Result<Location> {
        let position = |anchor| {
            schematic
                .net_line_anchor_position(segment, anchor, self.project.view())
                .ok_or(match anchor {
                    NetLineAnchor::Junction(id) => Error::InexistentNetPoint(id),
                    NetLineAnchor::Pin { symbol, pin } => {
                        Error::InexistentSymbolPin { symbol, pin }
                    }
                    NetLineAnchor::BusJunction { segment, junction } => {
                        Error::InexistentBusJunction { segment, junction }
                    }
                })
        };
        let width = line
            .width()
            .to_nm()
            .checked_add(1)
            .ok_or(crate::types::Error::Range {
                value: line.width().to_nm() as f64 + 1.0,
                min: 1,
                max: i64::MAX,
            })?;
        Ok(Location {
            schematic: Some(schematic.id()),
            paths: vec![Path::obround_line(
                position(line.p1())?,
                position(line.p2())?,
                PositiveLength::new(Length::new(width))?,
            )],
        })
    }
}

fn circle_location(schematic: &Schematic, position: Point, diameter: i64) -> Location {
    // All callers provide a positive constant diameter in nanometers.
    let diameter = PositiveLength::new(Length::new(diameter)).expect("positive constant diameter");
    Location {
        schematic: Some(schematic.id()),
        paths: vec![Path::circle(diameter).translated(position)],
    }
}
fn symbol_location(schematic: &Schematic, symbol: &SchematicSymbol) -> Location {
    circle_location(schematic, symbol.position(), 2_000_000)
}
