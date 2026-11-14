//! Reverse index of references between project entities (replaces the
//! upstream registration lists `NetSignal::mRegisteredComponentSignals`,
//! `mRegisteredSchematicNetSegments`, `mRegisteredBoardNetSegments`,
//! `mRegisteredBoardPlanes`, `ComponentInstance::mRegisteredSymbols`,
//! `mRegisteredDevices`, `Bus::mRegisteredSchematicBusSegments`).
//!
//! The forward references live in the items (a board segment stores its
//! net); this index answers "who references X" for removal checks, ERC and
//! derived data invalidation. It is private to the project module: rebuilt
//! after loading ([`RefIndex::build()`]), updated incrementally by every
//! mutation that adds, removes or re-targets a referencing item, and
//! checked against a full rebuild in tests.

use std::collections::{BTreeMap, BTreeSet};

use super::Project;
use super::id::{
    BoardId, BusId, BusSegmentId, ComponentInstanceId, ComponentSignalRef, NetSegmentId,
    NetSignalId, PlaneId, SchematicId, SymbolId,
};
use crate::types::Uuid;

/// A use of a net signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NetUse {
    /// A component signal is connected to the net.
    ComponentSignal(ComponentSignalRef),
    /// A schematic net segment belongs to the net.
    SchematicSegment(SchematicId, NetSegmentId),
    /// A board net segment belongs to the net.
    BoardSegment(BoardId, NetSegmentId),
    /// A board plane belongs to the net.
    Plane(BoardId, PlaneId),
}

/// A reference from an item into the circuit, as enumerated by the items
/// (`Schematic::references()`, `Board::references()`) and maintained by the
/// item mutations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reference {
    /// A net signal is used.
    Net {
        /// The net.
        net: NetSignalId,
        /// By whom.
        by: NetUse,
    },
    /// A bus is used by a schematic bus segment.
    Bus {
        /// The bus.
        bus: BusId,
        /// The bus segment.
        by: (SchematicId, BusSegmentId),
    },
    /// A gate (symbol variant item) of a component instance is placed as a
    /// symbol.
    Symbol {
        /// The component instance.
        component: ComponentInstanceId,
        /// UUID of the symbol variant item ("gate").
        gate: Uuid,
        /// Where the symbol is placed.
        at: (SchematicId, SymbolId),
    },
    /// A component instance is placed as a device on a board.
    Device {
        /// The component instance.
        component: ComponentInstanceId,
        /// The board.
        board: BoardId,
    },
}

impl Reference {
    fn schematic(&self) -> Option<SchematicId> {
        match self {
            Self::Net {
                by: NetUse::SchematicSegment(s, _),
                ..
            } => Some(*s),
            Self::Bus { by: (s, _), .. } => Some(*s),
            Self::Symbol { at: (s, _), .. } => Some(*s),
            _ => None,
        }
    }

    fn board(&self) -> Option<BoardId> {
        match self {
            Self::Net {
                by: NetUse::BoardSegment(b, _) | NetUse::Plane(b, _),
                ..
            } => Some(*b),
            Self::Device { board, .. } => Some(*board),
            _ => None,
        }
    }
}

/// Uses of a component instance.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComponentUses {
    /// Placed symbols by gate (symbol variant item UUID); upstream
    /// `mRegisteredSymbols`. All symbols of a component are in the same
    /// schematic.
    pub symbols: BTreeMap<Uuid, (SchematicId, SymbolId)>,
    /// Boards with a device of the component; upstream `mRegisteredDevices`.
    pub devices: BTreeSet<BoardId>,
}

impl ComponentUses {
    /// Number of registered symbols and devices (upstream
    /// `getRegisteredElementsCount()`).
    pub fn count(&self) -> usize {
        self.symbols.len() + self.devices.len()
    }
}

/// The reverse index, see the module documentation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefIndex {
    net_uses: BTreeMap<NetSignalId, BTreeSet<NetUse>>,
    bus_uses: BTreeMap<BusId, BTreeSet<(SchematicId, BusSegmentId)>>,
    component_uses: BTreeMap<ComponentInstanceId, ComponentUses>,
    /// All references, for removal of whole schematics/boards.
    all: BTreeSet<Reference>,
}

impl RefIndex {
    /// Builds the index from scratch.
    pub fn build(project: &Project) -> Self {
        let mut index = Self::default();
        for (id, cmp) in project.circuit().component_instances() {
            for (signal, inst) in cmp.signals() {
                if let Some(net) = inst.net() {
                    index.insert(Reference::Net {
                        net,
                        by: NetUse::ComponentSignal(ComponentSignalRef {
                            component: *id,
                            signal: *signal,
                        }),
                    });
                }
            }
        }
        for (id, schematic) in project.schematics_with_ids() {
            for r in schematic.references(id) {
                index.insert(r);
            }
        }
        for (id, board) in project.boards_with_ids() {
            for r in board.references(id) {
                index.insert(r);
            }
        }
        index
    }

    /// Records a reference (idempotent).
    pub fn insert(&mut self, r: Reference) {
        match r {
            Reference::Net { net, by } => {
                self.net_uses.entry(net).or_default().insert(by);
            }
            Reference::Bus { bus, by } => {
                self.bus_uses.entry(bus).or_default().insert(by);
            }
            Reference::Symbol {
                component,
                gate,
                at,
            } => {
                self.component_uses
                    .entry(component)
                    .or_default()
                    .symbols
                    .insert(gate, at);
            }
            Reference::Device { component, board } => {
                self.component_uses
                    .entry(component)
                    .or_default()
                    .devices
                    .insert(board);
            }
        }
        self.all.insert(r);
    }

    /// Forgets a reference (no-op if unknown).
    pub fn remove(&mut self, r: &Reference) {
        match r {
            Reference::Net { net, by } => {
                if let Some(uses) = self.net_uses.get_mut(net) {
                    uses.remove(by);
                    if uses.is_empty() {
                        self.net_uses.remove(net);
                    }
                }
            }
            Reference::Bus { bus, by } => {
                if let Some(uses) = self.bus_uses.get_mut(bus) {
                    uses.remove(by);
                    if uses.is_empty() {
                        self.bus_uses.remove(bus);
                    }
                }
            }
            Reference::Symbol {
                component, gate, ..
            } => {
                if let Some(uses) = self.component_uses.get_mut(component) {
                    uses.symbols.remove(gate);
                    if uses.count() == 0 {
                        self.component_uses.remove(component);
                    }
                }
            }
            Reference::Device { component, board } => {
                if let Some(uses) = self.component_uses.get_mut(component) {
                    uses.devices.remove(board);
                    if uses.count() == 0 {
                        self.component_uses.remove(component);
                    }
                }
            }
        }
        self.all.remove(r);
    }

    /// Forgets all references originating from a schematic.
    pub fn remove_schematic(&mut self, id: SchematicId) {
        let refs: Vec<_> = self
            .all
            .iter()
            .filter(|r| r.schematic() == Some(id))
            .copied()
            .collect();
        for r in &refs {
            self.remove(r);
        }
    }

    /// Forgets all references originating from a board.
    pub fn remove_board(&mut self, id: BoardId) {
        let refs: Vec<_> = self
            .all
            .iter()
            .filter(|r| r.board() == Some(id))
            .copied()
            .collect();
        for r in &refs {
            self.remove(r);
        }
    }

    /// Returns all uses of a net signal (upstream `getComponentSignals()`,
    /// `getSchematicNetSegments()`, `getBoardNetSegments()`,
    /// `getBoardPlanes()`).
    pub fn net_uses(&self, net: NetSignalId) -> impl Iterator<Item = &NetUse> {
        self.net_uses.get(&net).into_iter().flatten()
    }

    /// Number of uses of a net signal (upstream
    /// `NetSignal::getRegisteredElementsCount()`).
    pub fn net_use_count(&self, net: NetSignalId) -> usize {
        self.net_uses.get(&net).map_or(0, BTreeSet::len)
    }

    /// Whether a net signal is referenced (upstream `NetSignal::isUsed()`).
    pub fn is_net_used(&self, net: NetSignalId) -> bool {
        self.net_use_count(net) > 0
    }

    /// Returns all bus segments of a bus.
    pub fn bus_uses(&self, bus: BusId) -> impl Iterator<Item = &(SchematicId, BusSegmentId)> {
        self.bus_uses.get(&bus).into_iter().flatten()
    }

    /// Whether a bus has bus segments (upstream `Bus::isUsed()`).
    pub fn is_bus_used(&self, bus: BusId) -> bool {
        self.bus_uses.contains_key(&bus)
    }

    /// Returns the symbols and devices of a component instance.
    pub fn component_uses(&self, component: ComponentInstanceId) -> Option<&ComponentUses> {
        self.component_uses.get(&component)
    }
}
