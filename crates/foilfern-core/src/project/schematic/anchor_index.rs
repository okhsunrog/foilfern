//! Anchor index of a schematic and the connectivity rules of its segments
//! (replaces upstream's `registerNetLine()`/`registerBusLine()` of
//! `SI_SymbolPin`, `SI_NetPoint` and `SI_BusJunction`, the net line
//! anchor resolution of `ProjectLoader::loadSchematicNetSegment()` /
//! `loadSchematicBusSegment()`, and the checks of
//! `SI_NetSegment::addNetPointsAndNetLines()`/`SI_BusSegment::addJunctionsAndLines()`).
//!
//! The index answers "which net segment's lines touch this pin / bus
//! junction", which upstream keeps as registered net line lists in the
//! anchors. It is non-persistent, maintained by the schematic mutations and
//! rebuilt when a schematic is added to a project.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use super::{Schematic, SchematicBusSegment, SchematicNetSegment, SymbolPinView};
use crate::geometry::NetLineAnchor;
use crate::library::LibraryBaseElement;
use crate::project::ProjectView;
use crate::project::error::{EntityKind, Error, Result};
use crate::project::id::{BusSegmentId, ComponentSignalRef, NetSegmentId, SymbolId};
use crate::types::Uuid;

/// Use of a symbol pin by the lines of a net segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PinUse {
    /// The net segment whose lines end at the pin.
    pub segment: NetSegmentId,
    /// The component signal of the pin.
    pub signal: ComponentSignalRef,
}

/// The external anchors of a net segment, as validated by
/// [`check_net_segment()`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SegmentAnchors {
    /// Connected pins with their component signal.
    pub pins: BTreeMap<(SymbolId, Uuid), ComponentSignalRef>,
    /// Connected bus junctions.
    pub bus_junctions: BTreeSet<(BusSegmentId, Uuid)>,
}

/// Which net segments are connected to the pins and bus junctions of a
/// schematic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AnchorIndex {
    /// Connected pins (one segment per pin).
    pins: BTreeMap<(SymbolId, Uuid), PinUse>,
    /// Bus junctions with net lines (upstream allows several segments).
    bus_junctions: BTreeMap<(BusSegmentId, Uuid), BTreeSet<NetSegmentId>>,
}

impl AnchorIndex {
    /// Returns the use of a pin.
    pub fn pin(&self, symbol: SymbolId, pin: Uuid) -> Option<&PinUse> {
        self.pins.get(&(symbol, pin))
    }

    /// Whether lines end at any pin of the component signal.
    pub fn is_signal_wired(&self, signal: ComponentSignalRef) -> bool {
        self.pins.values().any(|u| u.signal == signal)
    }

    /// Whether lines end at any pin of the symbol.
    pub fn is_symbol_wired(&self, symbol: SymbolId) -> bool {
        self.pins.keys().any(|(s, _)| *s == symbol)
    }

    /// Returns the net segments with lines ending at a bus junction.
    pub fn bus_junction_segments(
        &self,
        segment: BusSegmentId,
        junction: Uuid,
    ) -> impl Iterator<Item = NetSegmentId> + '_ {
        self.bus_junctions
            .get(&(segment, junction))
            .into_iter()
            .flatten()
            .copied()
    }

    /// Whether net lines end at any junction of the bus segment.
    pub fn is_bus_segment_wired(&self, segment: BusSegmentId) -> bool {
        self.bus_junctions.keys().any(|(s, _)| *s == segment)
    }

    /// Records the anchors of a net segment.
    pub fn add_segment(&mut self, id: NetSegmentId, anchors: &SegmentAnchors) {
        for (pin, signal) in &anchors.pins {
            self.pins.insert(
                *pin,
                PinUse {
                    segment: id,
                    signal: *signal,
                },
            );
        }
        for junction in &anchors.bus_junctions {
            self.bus_junctions.entry(*junction).or_default().insert(id);
        }
    }

    /// Forgets the anchors of a net segment (its current content).
    pub fn remove_segment(&mut self, id: NetSegmentId, segment: &SchematicNetSegment) {
        for pin in segment.connected_pins() {
            if self.pins.get(&pin).is_some_and(|u| u.segment == id) {
                self.pins.remove(&pin);
            }
        }
        for junction in segment.connected_bus_junctions() {
            if let Some(segments) = self.bus_junctions.get_mut(&junction) {
                segments.remove(&id);
                if segments.is_empty() {
                    self.bus_junctions.remove(&junction);
                }
            }
        }
    }
}

/// Validates a net segment of `schematic` (which need not contain it yet)
/// against the schematic's items, the circuit and `index` (the anchors of
/// the other segments), like upstream does when loading and when adding or
/// removing elements:
///
/// 1. every line anchor exists (junction of this segment, bus junction,
///    connected symbol pin) and the two anchors of a line differ;
/// 2. pins belong to the net of the segment and are not connected to
///    another segment;
/// 3. the segment is cohesive (`after_removal` selects upstream's message).
///
/// Returns the external anchors for the index.
pub(crate) fn check_net_segment(
    schematic: &Schematic,
    index: &AnchorIndex,
    segment: &SchematicNetSegment,
    ctx: ProjectView<'_>,
    after_removal: bool,
) -> Result<SegmentAnchors> {
    let net = ctx
        .circuit
        .net_signal(segment.net())
        .ok_or(Error::InexistentNetSignal(segment.net().0))?;
    // Resolve the anchors (upstream: net line construction).
    let mut pins: BTreeMap<SymbolId, Vec<SymbolPinView<'_>>> = BTreeMap::new();
    let mut anchors = SegmentAnchors::default();
    for line in segment.lines().values() {
        for anchor in [line.p1(), line.p2()] {
            match anchor {
                NetLineAnchor::Junction(junction) => {
                    if !segment.junctions().contains_key(&junction) {
                        return Err(Error::InexistentNetPoint(junction));
                    }
                }
                NetLineAnchor::BusJunction {
                    segment: bus_segment,
                    junction,
                } => {
                    let id = BusSegmentId(bus_segment);
                    let bus = schematic
                        .bus_segments
                        .get(&id)
                        .ok_or(Error::InexistentBusSegment(bus_segment))?;
                    if !bus.junctions().contains_key(&junction) {
                        return Err(Error::InexistentBusJunction {
                            segment: bus_segment,
                            junction,
                        });
                    }
                    anchors.bus_junctions.insert((id, junction));
                }
                NetLineAnchor::Pin { symbol, pin } => {
                    let id = SymbolId(symbol);
                    let views = match pins.entry(id) {
                        Entry::Occupied(e) => e.into_mut(),
                        Entry::Vacant(e) => {
                            let s = schematic
                                .symbols
                                .get(&id)
                                .ok_or(Error::InexistentSymbol(symbol))?;
                            e.insert(s.pins(ctx)?)
                        }
                    };
                    if !views.iter().any(|p| p.uuid() == pin) {
                        return Err(Error::InexistentSymbolPin { symbol, pin });
                    }
                }
            }
        }
        if line.p1() == line.p2() {
            return Err(Error::DegenerateLine {
                kind: EntityKind::NetLine,
                uuid: line.uuid(),
            });
        }
    }
    // Register the lines at the pins (upstream `SI_SymbolPin::registerNetLine()`).
    for line in segment.lines().values() {
        for anchor in [line.p1(), line.p2()] {
            let NetLineAnchor::Pin { symbol, pin } = anchor else {
                continue;
            };
            let id = SymbolId(symbol);
            let view = pins[&id]
                .iter()
                .find(|p| p.uuid() == pin)
                .expect("pin was resolved above");
            let names = || -> Result<(String, String)> {
                let resolved = schematic.symbols[&id].resolve(ctx)?;
                Ok((
                    resolved.component.name().to_string(),
                    resolved.lib_component.metadata().name().to_string(),
                ))
            };
            if view.net() != Some(segment.net()) {
                let (component, lib_component) = names()?;
                return Err(Error::AnchorNetMismatch {
                    net: net.name().to_string(),
                    signal: view.lib_signal().name().to_string(),
                    component,
                    lib_component,
                    pin_net: view
                        .net()
                        .and_then(|n| ctx.circuit.net_signal(n))
                        .map(|n| n.name().to_string())
                        .unwrap_or_default(),
                });
            }
            if index
                .pin(id, pin)
                .is_some_and(|u| u.segment != segment.id())
            {
                let (component, lib_component) = names()?;
                return Err(Error::AnchorInMultipleSegments {
                    signal: view.lib_signal().name().to_string(),
                    component,
                    lib_component,
                });
            }
            anchors.pins.insert((id, pin), view.signal());
        }
    }
    if !segment.is_cohesive() {
        return Err(Error::NetSegmentNotCohesive {
            uuid: segment.uuid(),
            after_removal,
        });
    }
    Ok(anchors)
}

/// Validates a bus segment like upstream `loadSchematicBusSegment()` and
/// `SI_BusSegment::addJunctionsAndLines()`: the bus exists, every line
/// connects two different junctions of the segment, and the segment is
/// cohesive.
pub(crate) fn check_bus_segment(segment: &SchematicBusSegment, ctx: ProjectView<'_>) -> Result<()> {
    if ctx.circuit.bus(segment.bus()).is_none() {
        return Err(Error::InexistentBus(segment.bus().0));
    }
    for line in segment.lines().values() {
        for anchor in [line.p1(), line.p2()] {
            let junction = match anchor {
                NetLineAnchor::Junction(j) => j,
                NetLineAnchor::BusJunction { junction, .. } => junction,
                NetLineAnchor::Pin { pin, .. } => pin,
            };
            if !matches!(anchor, NetLineAnchor::Junction(_))
                || !segment.junctions().contains_key(&junction)
            {
                return Err(Error::InexistentBusJunction {
                    segment: segment.uuid(),
                    junction,
                });
            }
        }
        if line.p1() == line.p2() {
            return Err(Error::DegenerateLine {
                kind: EntityKind::BusLine,
                uuid: line.uuid(),
            });
        }
    }
    if !segment.is_cohesive() {
        return Err(Error::BusSegmentNotCohesive(segment.uuid()));
    }
    Ok(())
}
