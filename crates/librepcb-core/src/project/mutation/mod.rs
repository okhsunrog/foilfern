//! Mutations of the project model (no upstream counterpart: upstream
//! modifies the object graph through the `add*()`/`remove*()`/setter
//! methods of `Circuit`, `Schematic`, `Board` and the items, wrapped by the
//! editor's `Cmd*` undo commands).
//!
//! Commands are data: [`Mutation`] is the single description of every
//! change, [`Project::apply()`] validates it against the current model
//! (check-then-commit: a failed mutation leaves the model untouched),
//! performs it, journals the resulting [`Change`]s and returns the
//! **inverse** mutation. The inverse of a successful mutation, applied
//! immediately afterwards, always succeeds, which is all an undo stack or
//! a transaction ([`Mutation::Batch`]) needs. Removal inverses carry the
//! removed value, so undo restores UUIDs and list positions.
//!
//! The typed methods ([`Project::add_net_signal()`], ...) are convenience
//! wrappers which construct the mutation and apply it.
//!
//! Serde: `Mutation` is serializable (externally tagged enum) for MCP/CLI
//! clients; see `docs/project-model-design.md`.

mod board;
mod circuit;
mod schematic;

use std::collections::BTreeSet;

use super::Project;
use super::board::{Board, BoardProperties};
use super::change::Change;
use super::circuit::{
    AssemblyVariant, Bus, ComponentInstance, ComponentInstanceProperties, NetClass, NetSignal,
};
use super::error::Result;
use super::id::{
    AssemblyVariantId, BoardId, BusId, ComponentInstanceId, ComponentSignalRef, NetClassId,
    NetSignalId, SchematicId,
};
use super::project::{ProjectMetadata, ProjectSettings};
use super::schematic::{Schematic, SchematicProperties};
use crate::library::LibraryBaseElement;
use crate::library::cmp::Component;
use crate::library::dev::Device;
use crate::library::pkg::Package;
use crate::library::sym::Symbol;
use crate::serialization::SExpression;
use crate::types::Uuid;

pub use board::BoardMutation;
pub use schematic::SchematicMutation;

/// A modification of the project model, see the module documentation.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum Mutation {
    // --- Project ---
    /// Replaces the metadata (name, author, version, created, attributes).
    SetProjectMetadata(ProjectMetadata),
    /// Replaces the settings.
    SetProjectSettings(ProjectSettings),
    /// Replaces all ERC message approvals.
    SetErcApprovals(BTreeSet<SExpression>),
    /// Approves or unapproves one ERC message.
    SetErcApproval {
        /// The approval node of the message.
        approval: SExpression,
        /// Whether the message is approved.
        approved: bool,
    },

    /// Replaces output jobs in execution order.
    SetOutputJobs(crate::job::OutputJobList),

    // --- Circuit ---
    /// Adds an assembly variant at `index` (appends if `None`).
    AddAssemblyVariant {
        /// The variant.
        variant: AssemblyVariant,
        /// Position in the list.
        index: Option<usize>,
    },
    /// Moves an assembly variant to a new position (zero is the default).
    MoveAssemblyVariant {
        variant: AssemblyVariantId,
        index: usize,
    },
    /// Removes an assembly variant (not the last one).
    RemoveAssemblyVariant(AssemblyVariantId),
    /// Replaces name and description of an assembly variant.
    UpdateAssemblyVariant(AssemblyVariant),
    /// Adds a net class.
    AddNetClass(NetClass),
    /// Removes an unused net class.
    RemoveNetClass(NetClassId),
    /// Replaces name and design rules of a net class.
    UpdateNetClass(NetClass),
    /// Adds a net signal.
    AddNetSignal(NetSignal),
    /// Removes an unused net signal.
    RemoveNetSignal(NetSignalId),
    /// Replaces name, auto-name flag and net class of a net signal.
    UpdateNetSignal(NetSignal),
    /// Adds a bus.
    AddBus(Bus),
    /// Removes an unused bus.
    RemoveBus(BusId),
    /// Replaces name and properties of a bus.
    UpdateBus(Bus),
    /// Adds a component instance (with its signal connections).
    AddComponentInstance(ComponentInstance),
    /// Removes an unused component instance.
    RemoveComponentInstance(ComponentInstanceId),
    /// Replaces the editable properties of a component instance.
    UpdateComponentInstance(ComponentInstanceProperties),
    /// Connects a component signal to a net (or disconnects it).
    SetComponentSignalNet {
        /// The component signal.
        signal: ComponentSignalRef,
        /// The net, `None` to disconnect.
        net: Option<NetSignalId>,
    },

    // --- Schematics ---
    /// Adds a schematic page at `index` (appends if `None`).
    AddSchematic {
        /// The schematic.
        schematic: Schematic,
        /// Page index.
        index: Option<usize>,
    },
    /// Removes an empty schematic page.
    RemoveSchematic(SchematicId),
    /// Replaces name and grid of a schematic.
    UpdateSchematic(SchematicProperties),
    /// A change of the items of a schematic.
    Schematic(SchematicMutation),

    // --- Boards ---
    /// Adds a board at `index` (appends if `None`).
    AddBoard {
        /// The board.
        board: Board,
        /// Index in the board list (0 = primary board).
        index: Option<usize>,
    },
    /// Removes a board.
    RemoveBoard(BoardId),
    /// Replaces the editable properties of a board.
    UpdateBoard(BoardProperties),
    /// A change of the items of a board.
    Board(BoardMutation),

    /// Several mutations applied in order; if one fails, the already
    /// applied ones are rolled back with their inverses and the error is
    /// returned.
    Batch(Vec<Mutation>),
}

impl Project {
    /// Applies `mutation` and returns its inverse; on error nothing
    /// changed (see the module documentation).
    pub fn apply(&mut self, mutation: Mutation) -> Result<Mutation> {
        match mutation {
            Mutation::SetOutputJobs(jobs) => {
                let old = std::mem::replace(&mut self.output_jobs, jobs);
                self.record(Change::OutputJobs);
                Ok(Mutation::SetOutputJobs(old))
            }
            Mutation::SetProjectMetadata(m) => self.set_project_metadata(m),
            Mutation::SetProjectSettings(s) => self.set_project_settings(s),
            Mutation::SetErcApprovals(a) => self.set_erc_approvals(a),
            Mutation::SetErcApproval { approval, approved } => {
                self.set_erc_approval(approval, approved)
            }
            Mutation::AddAssemblyVariant { variant, index } => {
                circuit::add_assembly_variant(self, variant, index)
            }
            Mutation::MoveAssemblyVariant { variant, index } => {
                circuit::move_assembly_variant(self, variant, index)
            }
            Mutation::RemoveAssemblyVariant(id) => circuit::remove_assembly_variant(self, id),
            Mutation::UpdateAssemblyVariant(v) => circuit::update_assembly_variant(self, v),
            Mutation::AddNetClass(nc) => circuit::add_net_class(self, nc),
            Mutation::RemoveNetClass(id) => circuit::remove_net_class(self, id),
            Mutation::UpdateNetClass(nc) => circuit::update_net_class(self, nc),
            Mutation::AddNetSignal(ns) => circuit::add_net_signal(self, ns),
            Mutation::RemoveNetSignal(id) => circuit::remove_net_signal(self, id),
            Mutation::UpdateNetSignal(ns) => circuit::update_net_signal(self, ns),
            Mutation::AddBus(b) => circuit::add_bus(self, b),
            Mutation::RemoveBus(id) => circuit::remove_bus(self, id),
            Mutation::UpdateBus(b) => circuit::update_bus(self, b),
            Mutation::AddComponentInstance(c) => circuit::add_component_instance(self, c),
            Mutation::RemoveComponentInstance(id) => circuit::remove_component_instance(self, id),
            Mutation::UpdateComponentInstance(p) => circuit::update_component_instance(self, p),
            Mutation::SetComponentSignalNet { signal, net } => {
                circuit::set_component_signal_net(self, signal, net)
            }
            Mutation::AddSchematic { schematic, index } => {
                schematic::add_schematic(self, schematic, index)
            }
            Mutation::RemoveSchematic(id) => schematic::remove_schematic(self, id),
            Mutation::UpdateSchematic(p) => schematic::update_schematic(self, p),
            Mutation::Schematic(m) => schematic::apply(self, m),
            Mutation::AddBoard { board, index } => board::add_board(self, board, index),
            Mutation::RemoveBoard(id) => board::remove_board(self, id),
            Mutation::UpdateBoard(p) => board::update_board(self, p),
            Mutation::Board(m) => board::apply(self, m),
            Mutation::Batch(mutations) => self.apply_batch(mutations),
        }
    }

    fn apply_batch(&mut self, mutations: Vec<Mutation>) -> Result<Mutation> {
        let mut inverses = Vec::with_capacity(mutations.len());
        for m in mutations {
            match self.apply(m) {
                Ok(inverse) => inverses.push(inverse),
                Err(e) => {
                    for inverse in inverses.into_iter().rev() {
                        // The inverse of a successful mutation applied right
                        // afterwards always succeeds (model invariant).
                        if let Err(e) = self.apply(inverse) {
                            log::error!("Rollback of a batch mutation failed: {e}");
                            debug_assert!(false, "rollback failed: {e}");
                        }
                    }
                    return Err(e);
                }
            }
        }
        inverses.reverse();
        Ok(Mutation::Batch(inverses))
    }

    fn set_project_metadata(&mut self, metadata: ProjectMetadata) -> Result<Mutation> {
        let old = std::mem::replace(&mut self.metadata, metadata);
        self.record(Change::ProjectMetadata);
        Ok(Mutation::SetProjectMetadata(old))
    }

    fn set_project_settings(&mut self, settings: ProjectSettings) -> Result<Mutation> {
        let old = std::mem::replace(&mut self.settings, settings);
        self.record(Change::ProjectSettings);
        Ok(Mutation::SetProjectSettings(old))
    }

    fn set_erc_approvals(&mut self, approvals: BTreeSet<SExpression>) -> Result<Mutation> {
        let old = std::mem::replace(&mut self.erc_approvals, approvals);
        self.record(Change::ErcApprovals);
        Ok(Mutation::SetErcApprovals(old))
    }

    fn set_erc_approval(&mut self, approval: SExpression, approved: bool) -> Result<Mutation> {
        let was_approved = if approved {
            !self.erc_approvals.insert(approval.clone())
        } else {
            self.erc_approvals.remove(&approval)
        };
        if was_approved != approved {
            self.record(Change::ErcApprovals);
        }
        Ok(Mutation::SetErcApproval {
            approval,
            approved: was_approved,
        })
    }

    // --- Typed convenience wrappers ---

    /// Adds an assembly variant at `index` (appends if `None`).
    pub fn add_assembly_variant(
        &mut self,
        variant: AssemblyVariant,
        index: Option<usize>,
    ) -> Result<AssemblyVariantId> {
        let id = AssemblyVariantId(variant.uuid());
        self.apply(Mutation::AddAssemblyVariant { variant, index })?;
        Ok(id)
    }

    /// Removes an assembly variant and returns it.
    pub fn remove_assembly_variant(&mut self, id: AssemblyVariantId) -> Result<AssemblyVariant> {
        match self.apply(Mutation::RemoveAssemblyVariant(id))? {
            Mutation::AddAssemblyVariant { variant, .. } => Ok(variant),
            _ => unreachable!("inverse of a removal is the addition"),
        }
    }

    /// Adds a net class.
    pub fn add_net_class(&mut self, net_class: NetClass) -> Result<NetClassId> {
        let id = NetClassId(net_class.uuid());
        self.apply(Mutation::AddNetClass(net_class))?;
        Ok(id)
    }

    /// Removes an unused net class and returns it.
    pub fn remove_net_class(&mut self, id: NetClassId) -> Result<NetClass> {
        match self.apply(Mutation::RemoveNetClass(id))? {
            Mutation::AddNetClass(nc) => Ok(nc),
            _ => unreachable!("inverse of a removal is the addition"),
        }
    }

    /// Adds a net signal.
    pub fn add_net_signal(&mut self, net_signal: NetSignal) -> Result<NetSignalId> {
        let id = NetSignalId(net_signal.uuid());
        self.apply(Mutation::AddNetSignal(net_signal))?;
        Ok(id)
    }

    /// Removes an unused net signal and returns it.
    pub fn remove_net_signal(&mut self, id: NetSignalId) -> Result<NetSignal> {
        match self.apply(Mutation::RemoveNetSignal(id))? {
            Mutation::AddNetSignal(ns) => Ok(ns),
            _ => unreachable!("inverse of a removal is the addition"),
        }
    }

    /// Adds a bus.
    pub fn add_bus(&mut self, bus: Bus) -> Result<BusId> {
        let id = BusId(bus.uuid());
        self.apply(Mutation::AddBus(bus))?;
        Ok(id)
    }

    /// Removes an unused bus and returns it.
    pub fn remove_bus(&mut self, id: BusId) -> Result<Bus> {
        match self.apply(Mutation::RemoveBus(id))? {
            Mutation::AddBus(b) => Ok(b),
            _ => unreachable!("inverse of a removal is the addition"),
        }
    }

    /// Adds a component instance.
    pub fn add_component_instance(
        &mut self,
        component: ComponentInstance,
    ) -> Result<ComponentInstanceId> {
        let id = component.id();
        self.apply(Mutation::AddComponentInstance(component))?;
        Ok(id)
    }

    /// Removes an unused component instance and returns it.
    pub fn remove_component_instance(
        &mut self,
        id: ComponentInstanceId,
    ) -> Result<ComponentInstance> {
        match self.apply(Mutation::RemoveComponentInstance(id))? {
            Mutation::AddComponentInstance(c) => Ok(c),
            _ => unreachable!("inverse of a removal is the addition"),
        }
    }

    /// Connects a component signal to a net (or disconnects it).
    pub fn set_component_signal_net(
        &mut self,
        signal: ComponentSignalRef,
        net: Option<NetSignalId>,
    ) -> Result<()> {
        self.apply(Mutation::SetComponentSignalNet { signal, net })
            .map(drop)
    }

    /// Adds a schematic page at `index` (appends if `None`).
    pub fn add_schematic(
        &mut self,
        schematic: Schematic,
        index: Option<usize>,
    ) -> Result<SchematicId> {
        let id = schematic.id();
        self.apply(Mutation::AddSchematic { schematic, index })?;
        Ok(id)
    }

    /// Removes an empty schematic page and returns it.
    pub fn remove_schematic(&mut self, id: SchematicId) -> Result<Schematic> {
        match self.apply(Mutation::RemoveSchematic(id))? {
            Mutation::AddSchematic { schematic, .. } => Ok(schematic),
            _ => unreachable!("inverse of a removal is the addition"),
        }
    }

    /// Adds a board at `index` (appends if `None`).
    pub fn add_board(&mut self, board: Board, index: Option<usize>) -> Result<BoardId> {
        let id = board.id();
        self.apply(Mutation::AddBoard { board, index })?;
        Ok(id)
    }

    /// Removes a board and returns it.
    pub fn remove_board(&mut self, id: BoardId) -> Result<Board> {
        match self.apply(Mutation::RemoveBoard(id))? {
            Mutation::AddBoard { board, .. } => Ok(board),
            _ => unreachable!("inverse of a removal is the addition"),
        }
    }

    // --- Project library (file backed elements, not `Mutation`s) ---

    /// Adds a symbol to the project library (copying its files into the
    /// project). Not a [`Mutation`]: library elements are file backed and
    /// not serializable; an undo stack keeps the removed element itself.
    pub fn add_library_symbol(&mut self, symbol: Symbol) -> Result<()> {
        let uuid = symbol.metadata().uuid();
        self.library.add_symbol(symbol)?;
        self.record_library_added(super::library::LibraryElementKind::Symbol, uuid);
        Ok(())
    }

    /// Adds a package to the project library, see
    /// [`add_library_symbol()`](Self::add_library_symbol).
    pub fn add_library_package(&mut self, package: Package) -> Result<()> {
        let uuid = package.metadata().uuid();
        self.library.add_package(package)?;
        self.record_library_added(super::library::LibraryElementKind::Package, uuid);
        Ok(())
    }

    /// Adds a component to the project library, see
    /// [`add_library_symbol()`](Self::add_library_symbol).
    pub fn add_library_component(&mut self, component: Component) -> Result<()> {
        let uuid = component.metadata().uuid();
        self.library.add_component(component)?;
        self.record_library_added(super::library::LibraryElementKind::Component, uuid);
        Ok(())
    }

    /// Adds a device to the project library, see
    /// [`add_library_symbol()`](Self::add_library_symbol).
    pub fn add_library_device(&mut self, device: Device) -> Result<()> {
        let uuid = device.metadata().uuid();
        self.library.add_device(device)?;
        self.record_library_added(super::library::LibraryElementKind::Device, uuid);
        Ok(())
    }

    /// Removes a symbol from the project library and returns it (its files
    /// are moved to a temporary file system).
    pub fn remove_library_symbol(&mut self, uuid: &Uuid) -> Result<Symbol> {
        let s = self.library.remove_symbol(uuid)?;
        self.record_library_removed(super::library::LibraryElementKind::Symbol, *uuid);
        Ok(s)
    }

    /// Removes a package from the project library and returns it.
    pub fn remove_library_package(&mut self, uuid: &Uuid) -> Result<Package> {
        let p = self.library.remove_package(uuid)?;
        self.record_library_removed(super::library::LibraryElementKind::Package, *uuid);
        Ok(p)
    }

    /// Removes a component from the project library and returns it.
    pub fn remove_library_component(&mut self, uuid: &Uuid) -> Result<Component> {
        let c = self.library.remove_component(uuid)?;
        self.record_library_removed(super::library::LibraryElementKind::Component, *uuid);
        Ok(c)
    }

    /// Removes a device from the project library and returns it.
    pub fn remove_library_device(&mut self, uuid: &Uuid) -> Result<Device> {
        let d = self.library.remove_device(uuid)?;
        self.record_library_removed(super::library::LibraryElementKind::Device, *uuid);
        Ok(d)
    }

    fn record_library_added(&mut self, kind: super::library::LibraryElementKind, uuid: Uuid) {
        self.record(Change::LibraryElementAdded { kind, uuid });
    }

    fn record_library_removed(&mut self, kind: super::library::LibraryElementKind, uuid: Uuid) {
        self.record(Change::LibraryElementRemoved { kind, uuid });
    }
}
