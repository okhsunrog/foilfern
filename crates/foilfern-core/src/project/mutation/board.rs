//! Board mutations: boards (upstream `Project::addBoard()`,
//! `removeBoard()`, `Board::setName()`) and, in [`BoardMutation`], the
//! board setup and items (upstream `Board::setDesignRules()`,
//! `addDeviceInstance()`, `addNetSegment()`, `BI_NetSegment::addElements()`,
//! `removeElements()`, `setNetSignal()`, `addPlane()`, `addZone()`, ...,
//! and the item setters wrapped by the editor's `CmdBoard*Edit` commands).
//!
//! Every mutation validates the candidate state before changing anything
//! (the upstream `addToBoard()`/`registerNetLine()` checks and the
//! constructor checks of `BI_Device`/`BI_NetLine`), then updates the
//! project's reverse index (nets → segments/planes, components → devices),
//! the board's footprint pad index and the dirty sets of the derived data
//! (air wires per net, plane layers), and journals the item changes.
//!
//! Air wires are marked dirty for every change which may affect them (a
//! superset of upstream's `scheduleAirWiresRebuild()` calls, e.g. also when
//! only traces change); rebuilding is
//! [`Project::rebuild_air_wires()`](crate::project::Project::rebuild_air_wires).

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use super::Mutation;
use crate::fileio::FileSystem;
use crate::geometry::{Trace, TraceAnchor};
use crate::library::LibraryBaseElement;
use crate::project::Project;
use crate::project::board::{
    Board, BoardChange, BoardDevice, BoardItem, BoardItemKind, BoardNetSegment, BoardPlane,
    BoardProperties, BoardSegmentElements, BoardSegmentItems, BoardSettings, FootprintPadKey,
    FootprintPadView, PadOnBoard, PadUse,
};
use crate::project::change::Change;
use crate::project::circuit::Circuit;
use crate::project::error::{EntityKind, Error, Result};
use crate::project::id::{
    BoardId, BoardNetSegmentRef, ComponentInstanceId, DeviceRef, NetSegmentId, NetSignalId,
    PlaneRef,
};
use crate::project::library::ProjectLibrary;
use crate::project::ref_index::{NetUse, Reference};
use crate::serialization::SExpression;
use crate::types::{Layer, Uuid, Version};

/// A change of the setup or items of a board, see
/// [`Mutation::Board`](super::Mutation::Board).
///
/// Serde: externally tagged enum.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum BoardMutation {
    /// Replaces the board settings (upstream `CmdBoardEdit`).
    SetSettings {
        /// The board.
        board: BoardId,
        /// The new settings (boxed: large).
        settings: Box<BoardSettings>,
    },
    /// Replaces the DRC message approvals and their file format version.
    SetDrcApprovals {
        /// The board.
        board: BoardId,
        /// File format version of the approvals.
        version: Version,
        /// The approvals.
        approvals: BTreeSet<SExpression>,
    },
    /// Approves or unapproves one DRC message (upstream
    /// `setDrcMessageApproved()`).
    SetDrcApproval {
        /// The board.
        board: BoardId,
        /// The approval node of the message.
        approval: SExpression,
        /// Whether the message is approved.
        approved: bool,
    },
    /// Replaces the layer visibility (user settings).
    SetLayersVisibility {
        /// The board.
        board: BoardId,
        /// Visibility by layer ID.
        visibility: BTreeMap<String, bool>,
    },
    /// Adds a device (upstream `addDeviceInstance()`).
    AddDevice {
        /// The board.
        board: BoardId,
        /// The device.
        device: BoardDevice,
    },
    /// Removes a device whose pads have no traces (upstream
    /// `removeDeviceInstance()`).
    RemoveDevice(DeviceRef),
    /// Replaces a device of the same component instance (placement, flags,
    /// library elements, attributes, stroke texts). The library device,
    /// footprint and mirror state cannot change while traces are connected.
    UpdateDevice {
        /// The board.
        board: BoardId,
        /// The new device state.
        device: BoardDevice,
    },
    /// Adds a net segment with its elements (upstream `addNetSegment()`).
    AddNetSegment {
        /// The board.
        board: BoardId,
        /// The segment.
        segment: BoardNetSegment,
    },
    /// Removes a net segment with all its elements (upstream
    /// `removeNetSegment()`).
    RemoveNetSegment(BoardNetSegmentRef),
    /// Changes the net of a net segment without vias, junctions and traces
    /// (upstream `BI_NetSegment::setNetSignal()`).
    SetNetSegmentNet {
        /// The segment.
        segment: BoardNetSegmentRef,
        /// The new net.
        net: Option<NetSignalId>,
    },
    /// Adds elements to a net segment (upstream `addElements()`).
    AddNetSegmentElements {
        /// The segment.
        segment: BoardNetSegmentRef,
        /// The new elements.
        elements: BoardSegmentElements,
    },
    /// Removes elements from a net segment (upstream `removeElements()`).
    RemoveNetSegmentElements {
        /// The segment.
        segment: BoardNetSegmentRef,
        /// UUIDs of the elements.
        elements: BoardSegmentItems,
    },
    /// Replaces existing elements of a net segment (upstream setters of
    /// `BI_Pad`, `BI_Via`, `BI_NetPoint`, `BI_NetLine`).
    UpdateNetSegmentElements {
        /// The segment.
        segment: BoardNetSegmentRef,
        /// The new element states (identified by UUID).
        elements: BoardSegmentElements,
    },
    /// Adds a plane (upstream `addPlane()`).
    AddPlane {
        /// The board.
        board: BoardId,
        /// The plane.
        plane: BoardPlane,
    },
    /// Removes a plane (upstream `removePlane()`).
    RemovePlane(PlaneRef),
    /// Replaces a plane (upstream `BI_Plane` setters).
    UpdatePlane {
        /// The board.
        board: BoardId,
        /// The new plane state.
        plane: BoardPlane,
    },
    /// Adds a zone, polygon, stroke text or hole.
    AddItem {
        /// The board.
        board: BoardId,
        /// The item.
        item: BoardItem,
    },
    /// Removes a zone, polygon, stroke text or hole.
    RemoveItem {
        /// The board.
        board: BoardId,
        /// Kind of the item.
        kind: BoardItemKind,
        /// UUID of the item.
        uuid: Uuid,
    },
    /// Replaces a zone, polygon, stroke text or hole.
    UpdateItem {
        /// The board.
        board: BoardId,
        /// The new item state (identified by kind and UUID).
        item: BoardItem,
    },
}

pub(super) fn apply(p: &mut Project, m: BoardMutation) -> Result<Mutation> {
    let inverse = match m {
        BoardMutation::SetSettings { board, settings } => set_settings(p, board, settings)?,
        BoardMutation::SetDrcApprovals {
            board,
            version,
            approvals,
        } => set_drc_approvals(p, board, version, approvals)?,
        BoardMutation::SetDrcApproval {
            board,
            approval,
            approved,
        } => set_drc_approval(p, board, approval, approved)?,
        BoardMutation::SetLayersVisibility { board, visibility } => {
            set_layers_visibility(p, board, visibility)?
        }
        BoardMutation::AddDevice { board, device } => add_device(p, board, device)?,
        BoardMutation::RemoveDevice(r) => remove_device(p, r)?,
        BoardMutation::UpdateDevice { board, device } => update_device(p, board, device)?,
        BoardMutation::AddNetSegment { board, segment } => add_net_segment(p, board, segment)?,
        BoardMutation::RemoveNetSegment(r) => remove_net_segment(p, r)?,
        BoardMutation::SetNetSegmentNet { segment, net } => set_net_segment_net(p, segment, net)?,
        BoardMutation::AddNetSegmentElements { segment, elements } => {
            add_elements(p, segment, elements)?
        }
        BoardMutation::RemoveNetSegmentElements { segment, elements } => {
            remove_elements(p, segment, elements)?
        }
        BoardMutation::UpdateNetSegmentElements { segment, elements } => {
            update_elements(p, segment, elements)?
        }
        BoardMutation::AddPlane { board, plane } => add_plane(p, board, plane)?,
        BoardMutation::RemovePlane(r) => remove_plane(p, r)?,
        BoardMutation::UpdatePlane { board, plane } => update_plane(p, board, plane)?,
        BoardMutation::AddItem { board, item } => add_item(p, board, item)?,
        BoardMutation::RemoveItem { board, kind, uuid } => remove_item(p, board, kind, uuid)?,
        BoardMutation::UpdateItem { board, item } => update_item(p, board, item)?,
    };
    Ok(Mutation::Board(inverse))
}

// --- Boards ---

pub(super) fn add_board(
    p: &mut Project,
    mut board: Board,
    index: Option<usize>,
) -> Result<Mutation> {
    let id = board.id();
    if p.board(id).is_some() {
        return Err(Error::DuplicateUuid {
            kind: EntityKind::Board,
            uuid: id.0,
        });
    }
    check_board_name(p, board.name(), None)?;
    if p.boards
        .iter()
        .any(|b| b.directory_name() == board.directory_name())
    {
        return Err(Error::DuplicateDirectoryName {
            kind: EntityKind::Board,
            name: board.directory_name().to_owned(),
        });
    }
    // Validate all items like `Board::addToProject()` (all items
    // `addToBoard()`).
    let ctx = Ctx::new(p);
    for device in board.devices.values() {
        ctx.validate_device(device)?;
    }
    let mut pad_uses = BTreeMap::new();
    {
        let mut pads = PadCache::default();
        for segment in board.net_segments.values() {
            let uses =
                ctx.validate_segment(&board.devices, segment, &pad_uses, &mut pads, false)?;
            pad_uses.extend(uses);
        }
    }
    for plane in board.planes.values() {
        ctx.check_net(plane.net())?;
    }
    // Commit.
    board.index.pad_uses = pad_uses;
    board.derived = Default::default();
    // upstream: forceAirWiresRebuild(), all planes are calculated.
    let all_nets: Vec<_> = p.circuit.net_signals.keys().map(|n| Some(*n)).collect();
    board.derived.dirty_air_wire_nets.extend(all_nets);
    let copper = board.copper_layers();
    board.derived.invalidate_planes(copper);
    for r in board.references(id) {
        p.refs.insert(r);
    }
    let index = index.unwrap_or(p.boards.len()).min(p.boards.len());
    p.boards.insert(index, board);
    p.record(Change::BoardAdded(id));
    p.record(Change::ProjectMetadata);
    Ok(Mutation::RemoveBoard(id))
}

pub(super) fn remove_board(p: &mut Project, id: BoardId) -> Result<Mutation> {
    let index = board_index(p, id)?;
    // upstream: moves the board directory into a temporary file system
    p.directory
        .remove_dir_recursively(&format!("boards/{}", p.boards[index].directory_name()))?;
    let board = p.boards.remove(index);
    p.refs.remove_board(id);
    p.record(Change::BoardRemoved(id));
    p.record(Change::ProjectMetadata);
    Ok(Mutation::AddBoard {
        board,
        index: Some(index),
    })
}

pub(super) fn update_board(p: &mut Project, properties: BoardProperties) -> Result<Mutation> {
    let id = properties.id;
    let index = board_index(p, id)?;
    check_board_name(p, &properties.name, Some(id))?;
    let old = p.boards[index].set_properties(properties);
    p.record(Change::BoardChanged(id));
    p.record(Change::ProjectMetadata);
    Ok(Mutation::UpdateBoard(old))
}

fn check_board_name(p: &Project, name: &str, except: Option<BoardId>) -> Result<()> {
    if p.board_by_name(name)
        .is_some_and(|b| Some(b.id()) != except)
    {
        return Err(Error::DuplicateName {
            kind: EntityKind::Board,
            name: name.to_owned(),
        });
    }
    Ok(())
}

fn board_index(p: &Project, id: BoardId) -> Result<usize> {
    p.board_index(id).ok_or(Error::NotFound {
        kind: EntityKind::Board,
        uuid: id.0,
    })
}

/// "There is already a pad/via/... with the UUID" (upstream `add*()`).
fn duplicate_item(kind: BoardItemKind, uuid: Uuid) -> Error {
    Error::DuplicateUuid {
        kind: kind.into(),
        uuid,
    }
}

/// A board item which does not exist.
fn item_not_found(kind: BoardItemKind, uuid: Uuid) -> Error {
    Error::NotFound {
        kind: kind.into(),
        uuid,
    }
}

fn record(p: &mut Project, board: BoardId, change: BoardChange) {
    p.record(Change::Board { id: board, change });
}

// --- Circuit hooks ---

/// Called by the circuit mutations after the net of a component signal
/// changed: the footprint pads of the component's devices change their net
/// (upstream `BI_Pad::netSignalChanged()`).
pub(super) fn component_signal_net_changed(
    p: &mut Project,
    component: ComponentInstanceId,
    from: Option<NetSignalId>,
    to: Option<NetSignalId>,
) {
    for board in &mut p.boards {
        if board.devices.contains_key(&component) {
            let copper = board.copper_layers();
            board.derived.schedule_air_wires(from);
            board.derived.schedule_air_wires(to);
            board.derived.invalidate_planes(copper);
        }
    }
}

// --- Settings ---

fn set_settings(
    p: &mut Project,
    id: BoardId,
    settings: Box<BoardSettings>,
) -> Result<BoardMutation> {
    let index = board_index(p, id)?;
    let board = &mut p.boards[index];
    let old = std::mem::replace(&mut board.settings, settings);
    if (old.design_rules != board.settings.design_rules)
        || (old.inner_layer_count != board.settings.inner_layer_count)
    {
        let copper = board.copper_layers();
        board.derived.invalidate_planes(copper);
    }
    record(p, id, BoardChange::Settings);
    Ok(BoardMutation::SetSettings {
        board: id,
        settings: old,
    })
}

fn set_drc_approvals(
    p: &mut Project,
    id: BoardId,
    version: Version,
    approvals: BTreeSet<SExpression>,
) -> Result<BoardMutation> {
    let index = board_index(p, id)?;
    let board = &mut p.boards[index];
    let old_version = std::mem::replace(&mut board.drc_approvals_version, version);
    let old = std::mem::replace(&mut board.drc_approvals, approvals);
    record(p, id, BoardChange::DrcApprovals);
    Ok(BoardMutation::SetDrcApprovals {
        board: id,
        version: old_version,
        approvals: old,
    })
}

fn set_drc_approval(
    p: &mut Project,
    id: BoardId,
    approval: SExpression,
    approved: bool,
) -> Result<BoardMutation> {
    let index = board_index(p, id)?;
    let approvals = &mut p.boards[index].drc_approvals;
    let was_approved = if approved {
        !approvals.insert(approval.clone())
    } else {
        approvals.remove(&approval)
    };
    if was_approved != approved {
        record(p, id, BoardChange::DrcApprovals);
    }
    Ok(BoardMutation::SetDrcApproval {
        board: id,
        approval,
        approved: was_approved,
    })
}

fn set_layers_visibility(
    p: &mut Project,
    id: BoardId,
    visibility: BTreeMap<String, bool>,
) -> Result<BoardMutation> {
    let index = board_index(p, id)?;
    let old = std::mem::replace(&mut p.boards[index].layers_visibility, visibility);
    record(p, id, BoardChange::LayersVisibility);
    Ok(BoardMutation::SetLayersVisibility {
        board: id,
        visibility: old,
    })
}

// --- Validation ---

/// Context of the validation: the circuit and library the board items
/// reference.
struct Ctx<'a> {
    library: &'a ProjectLibrary,
    circuit: &'a Circuit,
}

/// Footprint pad views per device, computed on first use.
#[derive(Default)]
struct PadCache<'a> {
    pads: BTreeMap<ComponentInstanceId, Vec<FootprintPadView<'a>>>,
}

impl<'a> Ctx<'a> {
    fn new(p: &'a Project) -> Self {
        Self {
            library: &p.library,
            circuit: &p.circuit,
        }
    }

    /// Fails if `net` is set but not in the circuit.
    fn check_net(&self, net: Option<NetSignalId>) -> Result<()> {
        match net {
            Some(net) if !self.circuit.net_signals.contains_key(&net) => {
                Err(Error::InexistentNetSignal(net.0))
            }
            _ => Ok(()),
        }
    }

    /// Returns the name of a net for messages (empty if none).
    fn net_name(&self, net: Option<NetSignalId>) -> String {
        net.and_then(|n| self.circuit.net_signal(n))
            .map(|n| n.name().to_string())
            .unwrap_or_default()
    }

    /// The checks of the `BI_Device` constructor, `setModel()` and
    /// `ComponentInstance::registerDevice()`.
    fn validate_device(&self, dev: &BoardDevice) -> Result<()> {
        let component = self
            .circuit
            .component_instance(dev.component())
            .ok_or(Error::InexistentDeviceComponent(dev.component().0))?;
        let lib_component = self
            .library
            .component(&component.lib_component())
            .ok_or(Error::MissingLibraryComponent(component.lib_component()))?;
        if lib_component.schematic_only() {
            return Err(Error::SchematicOnlyComponent(component.name().to_string()));
        }
        let device = self
            .library
            .device(&dev.lib_device())
            .ok_or(Error::MissingLibraryDevice(dev.lib_device()))?;
        if device.component_uuid() != component.lib_component() {
            return Err(Error::DeviceComponentMismatch {
                device: dev.lib_device(),
                component: dev.component().0,
            });
        }
        let package = self
            .library
            .package(&device.package_uuid())
            .ok_or(Error::MissingLibraryPackage(device.package_uuid()))?;
        let footprint = package
            .footprints()
            .required_by_uuid(&dev.lib_footprint())?;
        for item in device.pad_signal_map().iter() {
            if let Some(signal) = item.signal_uuid()
                && component.signal(&signal).is_none()
            {
                return Err(Error::UnknownDeviceSignal {
                    signal,
                    device: dev.lib_device(),
                });
            }
        }
        let mut pads = BTreeSet::new();
        for pad in footprint.pads().iter() {
            if !pads.insert(pad.uuid()) {
                return Err(Error::DuplicateFootprintPad(pad.uuid()));
            }
            if let Some(package_pad) = pad.package_pad_uuid() {
                if !package.pads().contains_uuid(&package_pad) {
                    return Err(Error::MissingPackagePad {
                        pad: package_pad,
                        package: package.metadata().uuid(),
                    });
                }
                if !device.pad_signal_map().contains_uuid(&package_pad) {
                    return Err(Error::PadNotInPadSignalMap {
                        pad: package_pad,
                        device: dev.lib_device(),
                    });
                }
            }
        }
        if let Some(model) = dev.lib_model() {
            package.models().required_by_uuid(&model)?;
        }
        Ok(())
    }

    /// Validates a net segment (candidate state) like upstream
    /// `addElements()`/`removeElements()` with the `BI_NetLine` constructor
    /// and the `registerNetLine()` checks of the anchors. Returns the uses
    /// of footprint pads by the segment's traces.
    fn validate_segment(
        &self,
        devices: &'a BTreeMap<ComponentInstanceId, BoardDevice>,
        segment: &BoardNetSegment,
        pad_uses: &BTreeMap<FootprintPadKey, PadUse>,
        cache: &mut PadCache<'a>,
        removal: bool,
    ) -> Result<BTreeMap<FootprintPadKey, PadUse>> {
        self.check_net(segment.net())?;
        let mut uses = BTreeMap::new();
        for trace in segment.traces().values() {
            let layer = trace.layer();
            if !layer.is_copper() {
                return Err(Error::InvalidTraceLayer {
                    trace: trace.uuid(),
                    layer: layer.name_tr(),
                });
            }
            if trace.p1() == trace.p2() {
                return Err(Error::DegenerateLine {
                    kind: EntityKind::Trace,
                    uuid: trace.uuid(),
                });
            }
            for anchor in [trace.p1(), trace.p2()] {
                let missing = |device_missing| Error::InexistentTraceAnchor {
                    anchor,
                    device_missing,
                };
                match anchor {
                    TraceAnchor::Junction(uuid) => {
                        segment
                            .junctions()
                            .get(&uuid)
                            .ok_or(Error::InexistentNetPoint(uuid))?;
                    }
                    TraceAnchor::Via(uuid) => {
                        let via = segment.vias().get(&uuid).ok_or(missing(false))?;
                        if !crate::geometry::Via::is_on_layer_between(
                            layer,
                            via.start_layer(),
                            via.end_layer(),
                        ) {
                            return Err(Error::TraceViaLayerMismatch(uuid));
                        }
                    }
                    TraceAnchor::Pad(uuid) => {
                        let pad = segment.pads().get(&uuid).ok_or(missing(false))?;
                        let on_board = PadOnBoard {
                            pad: pad.pad(),
                            mirrored: false,
                        };
                        if !on_board.is_on_layer(layer) {
                            return Err(Error::TraceLayerMismatch {
                                layer: layer.name_tr(),
                                pad: uuid.to_string(),
                                device: String::new(),
                                lib_device: String::new(),
                                pad_layer: on_board.solder_layer().name_tr(),
                            });
                        }
                    }
                    TraceAnchor::FootprintPad { device, pad } => {
                        let key = (ComponentInstanceId(device), pad);
                        let dev = devices.get(&key.0).ok_or(missing(true))?;
                        let views = match cache.pads.entry(key.0) {
                            Entry::Occupied(e) => e.into_mut(),
                            Entry::Vacant(e) => e.insert(dev.pads(self.library, self.circuit)?),
                        };
                        let view = views
                            .iter()
                            .find(|v| v.uuid() == pad)
                            .ok_or(missing(false))?;
                        let names = || {
                            let device_name = self
                                .circuit
                                .component_instance(key.0)
                                .map(|c| c.name().to_string())
                                .unwrap_or_default();
                            let lib_device_name = self
                                .library
                                .device(&dev.lib_device())
                                .map(|d| d.metadata().name().to_string())
                                .unwrap_or_default();
                            (view.name_or_uuid(), device_name, lib_device_name)
                        };
                        if view.net() != segment.net() {
                            let (pad, device, lib_device) = names();
                            return Err(Error::TracePadNetMismatch {
                                trace_net: self.net_name(segment.net()),
                                pad,
                                device,
                                lib_device,
                                pad_net: self.net_name(view.net()),
                            });
                        }
                        if !view.is_on_layer(layer) {
                            let (pad, device, lib_device) = names();
                            return Err(Error::TraceLayerMismatch {
                                layer: layer.name_tr(),
                                pad,
                                device,
                                lib_device,
                                pad_layer: view.solder_layer().name_tr(),
                            });
                        }
                        if pad_uses
                            .get(&key)
                            .is_some_and(|u| u.segment != segment.id())
                        {
                            let (pad, device, lib_device) = names();
                            return Err(Error::PadInMultipleSegments {
                                pad,
                                device,
                                lib_device,
                            });
                        }
                        uses.insert(
                            key,
                            PadUse {
                                segment: segment.id(),
                                signal: view.component_signal().map(|s| s.signal),
                            },
                        );
                    }
                }
            }
        }
        for junction in segment.junctions().keys() {
            let layers: BTreeSet<Layer> = segment
                .traces_at(TraceAnchor::Junction(*junction))
                .map(Trace::layer)
                .collect();
            if layers.len() > 1 {
                return Err(Error::JunctionLayerMismatch(*junction));
            }
        }
        if !segment.is_cohesive() {
            return Err(Error::NetSegmentNotCohesive {
                uuid: segment.uuid(),
                after_removal: removal,
            });
        }
        Ok(uses)
    }
}

// --- Devices ---

/// The nets of the pads of a component's device (all nets of the component
/// signals, a superset).
fn device_nets(circuit: &Circuit, component: ComponentInstanceId) -> Vec<Option<NetSignalId>> {
    circuit
        .component_instance(component)
        .map(|c| c.signals().values().map(|s| s.net()).collect())
        .unwrap_or_default()
}

fn schedule_device(p: &mut Project, index: usize, component: ComponentInstanceId) {
    let nets = device_nets(&p.circuit, component);
    let board = &mut p.boards[index];
    let copper = board.copper_layers();
    for net in nets.into_iter().flatten() {
        board.derived.schedule_air_wires(Some(net));
    }
    board.derived.invalidate_planes(copper);
}

fn add_device(p: &mut Project, id: BoardId, device: BoardDevice) -> Result<BoardMutation> {
    let index = board_index(p, id)?;
    let component = device.component();
    if p.boards[index].devices.contains_key(&component) {
        return Err(Error::DuplicateDevice(component.0));
    }
    Ctx::new(p).validate_device(&device)?;
    p.boards[index].devices.insert(component, device);
    p.refs.insert(Reference::Device {
        component,
        board: id,
    });
    schedule_device(p, index, component);
    record(p, id, BoardChange::DeviceAdded(component));
    Ok(BoardMutation::RemoveDevice(DeviceRef {
        board: id,
        component,
    }))
}

/// A device cannot be removed, mirrored or replaced while traces are
/// connected to its pads (upstream `LogicError` in
/// `BI_Pad::removeFromBoard()` / `BI_Device::setMirrored()`).
fn device_in_use(component: ComponentInstanceId) -> Error {
    Error::ItemInUse {
        kind: EntityKind::Device,
        uuid: component.0,
    }
}

fn remove_device(p: &mut Project, r: DeviceRef) -> Result<BoardMutation> {
    let index = board_index(p, r.board)?;
    let board = &p.boards[index];
    if !board.devices.contains_key(&r.component) {
        return Err(Error::NotFound {
            kind: EntityKind::Device,
            uuid: r.component.0,
        });
    }
    if board.is_device_used(r.component) {
        return Err(device_in_use(r.component));
    }
    schedule_device(p, index, r.component);
    let device = p.boards[index]
        .devices
        .remove(&r.component)
        .expect("checked above");
    p.refs.remove(&Reference::Device {
        component: r.component,
        board: r.board,
    });
    record(p, r.board, BoardChange::DeviceRemoved(r.component));
    Ok(BoardMutation::AddDevice {
        board: r.board,
        device,
    })
}

fn update_device(p: &mut Project, id: BoardId, device: BoardDevice) -> Result<BoardMutation> {
    let index = board_index(p, id)?;
    let component = device.component();
    let board = &p.boards[index];
    let old = board.devices.get(&component).ok_or(Error::NotFound {
        kind: EntityKind::Device,
        uuid: component.0,
    })?;
    Ctx::new(p).validate_device(&device)?;
    let geometry_changed = (old.lib_device() != device.lib_device())
        || (old.lib_footprint() != device.lib_footprint())
        || (old.mirrored() != device.mirrored());
    if geometry_changed && board.is_device_used(component) {
        return Err(device_in_use(component));
    }
    let placement_changed = geometry_changed
        || (old.position() != device.position())
        || (old.rotation() != device.rotation());
    let old = p.boards[index]
        .devices
        .insert(component, device)
        .expect("checked above");
    if placement_changed {
        schedule_device(p, index, component);
    }
    record(p, id, BoardChange::DeviceChanged(component));
    Ok(BoardMutation::UpdateDevice {
        board: id,
        device: old,
    })
}

// --- Net segments ---

/// The copper layers affected by elements of a segment (for plane
/// invalidation, upstream `invalidatePlanes()` of the elements): vias and
/// THT pads affect all copper layers.
fn element_layers(
    copper: &BTreeSet<Layer>,
    segment: &BoardNetSegment,
    items: &BoardSegmentItems,
) -> BTreeSet<Layer> {
    let mut layers = BTreeSet::new();
    if items.vias.iter().any(|v| segment.vias().contains_key(v)) {
        return copper.clone();
    }
    for pad in items.pads.iter().filter_map(|u| segment.pads().get(u)) {
        if pad.pad().is_tht() {
            return copper.clone();
        }
        let on_board = PadOnBoard {
            pad: pad.pad(),
            mirrored: false,
        };
        layers.insert(on_board.solder_layer());
    }
    layers.extend(
        items
            .traces
            .iter()
            .filter_map(|u| segment.traces().get(u))
            .map(Trace::layer),
    );
    layers.extend(
        items
            .junctions
            .iter()
            .filter_map(|u| segment.junction_layer(u)),
    );
    layers
}

/// All elements of a segment.
fn all_items(segment: &BoardNetSegment) -> BoardSegmentItems {
    BoardSegmentItems {
        pads: segment.pads().keys().copied().collect(),
        vias: segment.vias().keys().copied().collect(),
        junctions: segment.junctions().keys().copied().collect(),
        traces: segment.traces().keys().copied().collect(),
    }
}

/// Replaces the footprint pad uses of a segment in the board index.
fn set_segment_pad_uses(
    board: &mut Board,
    segment: NetSegmentId,
    uses: BTreeMap<FootprintPadKey, PadUse>,
) {
    board.index.pad_uses.retain(|_, u| u.segment != segment);
    board.index.pad_uses.extend(uses);
}

fn segment_reference(board: BoardId, segment: &BoardNetSegment) -> Option<Reference> {
    segment.net().map(|net| Reference::Net {
        net,
        by: NetUse::BoardSegment(board, segment.id()),
    })
}

fn add_net_segment(
    p: &mut Project,
    id: BoardId,
    segment: BoardNetSegment,
) -> Result<BoardMutation> {
    let index = board_index(p, id)?;
    let board = &p.boards[index];
    if board.net_segments.contains_key(&segment.id()) {
        return Err(Error::DuplicateUuid {
            kind: EntityKind::NetSegment,
            uuid: segment.uuid(),
        });
    }
    let uses = Ctx::new(p).validate_segment(
        &board.devices,
        &segment,
        &board.index.pad_uses,
        &mut PadCache::default(),
        false,
    )?;
    let segment_id = segment.id();
    if let Some(r) = segment_reference(id, &segment) {
        p.refs.insert(r);
    }
    let board = &mut p.boards[index];
    let layers = element_layers(&board.copper_layers(), &segment, &all_items(&segment));
    board.derived.invalidate_planes(layers);
    board.derived.schedule_air_wires(segment.net());
    set_segment_pad_uses(board, segment_id, uses);
    board.net_segments.insert(segment_id, segment);
    record(p, id, BoardChange::NetSegmentAdded(segment_id));
    Ok(BoardMutation::RemoveNetSegment(BoardNetSegmentRef {
        board: id,
        segment: segment_id,
    }))
}

fn segment_not_found(r: BoardNetSegmentRef) -> Error {
    Error::NotFound {
        kind: EntityKind::NetSegment,
        uuid: r.segment.0,
    }
}

fn remove_net_segment(p: &mut Project, r: BoardNetSegmentRef) -> Result<BoardMutation> {
    let index = board_index(p, r.board)?;
    let board = &mut p.boards[index];
    let segment = board
        .net_segments
        .remove(&r.segment)
        .ok_or_else(|| segment_not_found(r))?;
    let layers = element_layers(&board.copper_layers(), &segment, &all_items(&segment));
    board.derived.invalidate_planes(layers);
    board.derived.schedule_air_wires(segment.net());
    set_segment_pad_uses(board, r.segment, BTreeMap::new());
    if let Some(reference) = segment_reference(r.board, &segment) {
        p.refs.remove(&reference);
    }
    record(p, r.board, BoardChange::NetSegmentRemoved(r.segment));
    Ok(BoardMutation::AddNetSegment {
        board: r.board,
        segment,
    })
}

fn set_net_segment_net(
    p: &mut Project,
    r: BoardNetSegmentRef,
    net: Option<NetSignalId>,
) -> Result<BoardMutation> {
    let index = board_index(p, r.board)?;
    let segment = p.boards[index]
        .net_segments
        .get(&r.segment)
        .ok_or_else(|| segment_not_found(r))?;
    let old = segment.net();
    if old == net {
        return Ok(BoardMutation::SetNetSegmentNet { segment: r, net });
    }
    if segment.is_used() {
        // upstream: `LogicError` in `BI_NetSegment::setNetSignal()`
        return Err(Error::ItemInUse {
            kind: EntityKind::NetSegment,
            uuid: r.segment.0,
        });
    }
    Ctx::new(p).check_net(net)?;
    if let Some(reference) = segment_reference(r.board, segment) {
        p.refs.remove(&reference);
    }
    let board = &mut p.boards[index];
    let copper = board.copper_layers();
    let segment = board
        .net_segments
        .get_mut(&r.segment)
        .expect("checked above");
    segment.set_net(net);
    let reference = segment_reference(r.board, segment);
    let layers = element_layers(&copper, segment, &all_items(segment));
    board.derived.invalidate_planes(layers);
    board.derived.schedule_air_wires(old);
    board.derived.schedule_air_wires(net);
    if let Some(reference) = reference {
        p.refs.insert(reference);
    }
    record(
        p,
        r.board,
        BoardChange::NetSegmentNetChanged {
            segment: r.segment,
            from: old,
            to: net,
        },
    );
    Ok(BoardMutation::SetNetSegmentNet {
        segment: r,
        net: old,
    })
}

/// Sorts the elements by UUID (canonical order of inverses).
fn sorted_elements(mut e: BoardSegmentElements) -> BoardSegmentElements {
    e.pads.sort_by_key(|x| x.uuid());
    e.vias.sort_by_key(|x| x.uuid());
    e.junctions.sort_by_key(|x| x.uuid());
    e.traces.sort_by_key(|x| x.uuid());
    e
}

/// Sorts the UUIDs (canonical order of inverses).
fn sorted_items(mut i: BoardSegmentItems) -> BoardSegmentItems {
    i.pads.sort();
    i.vias.sort();
    i.junctions.sort();
    i.traces.sort();
    i
}

/// Validates the candidate state of a segment and commits it: replaces
/// the segment, updates the pad index, invalidates the layers of the
/// changed elements (old and new state) and schedules the air wires.
fn commit_segment(
    p: &mut Project,
    index: usize,
    candidate: BoardNetSegment,
    changed: &BoardSegmentItems,
    removal: bool,
) -> Result<()> {
    let board = &p.boards[index];
    let uses = Ctx::new(p).validate_segment(
        &board.devices,
        &candidate,
        &board.index.pad_uses,
        &mut PadCache::default(),
        removal,
    )?;
    let board = &mut p.boards[index];
    let copper = board.copper_layers();
    let old = &board.net_segments[&candidate.id()];
    let mut layers = element_layers(&copper, old, changed);
    layers.extend(element_layers(&copper, &candidate, changed));
    board.derived.invalidate_planes(layers);
    board.derived.schedule_air_wires(candidate.net());
    set_segment_pad_uses(board, candidate.id(), uses);
    board.net_segments.insert(candidate.id(), candidate);
    Ok(())
}

fn add_elements(
    p: &mut Project,
    r: BoardNetSegmentRef,
    elements: BoardSegmentElements,
) -> Result<BoardMutation> {
    let index = board_index(p, r.board)?;
    let mut candidate = p.boards[index]
        .net_segments
        .get(&r.segment)
        .ok_or_else(|| segment_not_found(r))?
        .clone();
    let duplicate = duplicate_item;
    for pad in &elements.pads {
        if candidate.insert_pad(pad.clone()).is_some() {
            return Err(duplicate(BoardItemKind::Pad, pad.uuid()));
        }
    }
    for via in &elements.vias {
        if candidate.insert_via(via.clone()).is_some() {
            return Err(duplicate(BoardItemKind::Via, via.uuid()));
        }
    }
    for junction in &elements.junctions {
        if candidate.insert_junction(junction.clone()).is_some() {
            return Err(duplicate(BoardItemKind::Junction, junction.uuid()));
        }
    }
    for trace in &elements.traces {
        if candidate.insert_trace(trace.clone()).is_some() {
            return Err(duplicate(BoardItemKind::Trace, trace.uuid()));
        }
    }
    let items = sorted_items(elements.uuids());
    commit_segment(p, index, candidate, &items, false)?;
    record(
        p,
        r.board,
        BoardChange::NetSegmentElementsAdded {
            segment: r.segment,
            items: items.clone(),
        },
    );
    Ok(BoardMutation::RemoveNetSegmentElements {
        segment: r,
        elements: items,
    })
}

fn remove_elements(
    p: &mut Project,
    r: BoardNetSegmentRef,
    items: BoardSegmentItems,
) -> Result<BoardMutation> {
    let index = board_index(p, r.board)?;
    let old = p.boards[index]
        .net_segments
        .get(&r.segment)
        .ok_or_else(|| segment_not_found(r))?;
    let mut candidate = old.clone();
    let missing = |kind, uuid: &Uuid| item_not_found(kind, *uuid);
    let mut removed = BoardSegmentElements::default();
    for uuid in &items.traces {
        let trace = candidate.remove_trace(uuid);
        removed
            .traces
            .push(trace.ok_or_else(|| missing(BoardItemKind::Trace, uuid))?);
    }
    for uuid in &items.junctions {
        let junction = candidate.remove_junction(uuid);
        removed
            .junctions
            .push(junction.ok_or_else(|| missing(BoardItemKind::Junction, uuid))?);
    }
    for uuid in &items.vias {
        let via = candidate.remove_via(uuid);
        removed
            .vias
            .push(via.ok_or_else(|| missing(BoardItemKind::Via, uuid))?);
    }
    for uuid in &items.pads {
        let pad = candidate.remove_pad(uuid);
        removed
            .pads
            .push(pad.ok_or_else(|| missing(BoardItemKind::Pad, uuid))?);
    }
    let items = sorted_items(items);
    commit_segment(p, index, candidate, &items, true)?;
    record(
        p,
        r.board,
        BoardChange::NetSegmentElementsRemoved {
            segment: r.segment,
            items,
        },
    );
    Ok(BoardMutation::AddNetSegmentElements {
        segment: r,
        elements: sorted_elements(removed),
    })
}

fn update_elements(
    p: &mut Project,
    r: BoardNetSegmentRef,
    elements: BoardSegmentElements,
) -> Result<BoardMutation> {
    let index = board_index(p, r.board)?;
    let mut candidate = p.boards[index]
        .net_segments
        .get(&r.segment)
        .ok_or_else(|| segment_not_found(r))?
        .clone();
    let items = elements.uuids();
    for (kind, uuids) in [
        (BoardItemKind::Pad, &items.pads),
        (BoardItemKind::Via, &items.vias),
        (BoardItemKind::Junction, &items.junctions),
        (BoardItemKind::Trace, &items.traces),
    ] {
        let mut seen = BTreeSet::new();
        if let Some(uuid) = uuids.iter().find(|u| !seen.insert(**u)) {
            return Err(duplicate_item(kind, *uuid));
        }
    }
    let missing = item_not_found;
    let mut old = BoardSegmentElements::default();
    for pad in elements.pads {
        let uuid = pad.uuid();
        if !candidate.pads().contains_key(&uuid) {
            return Err(missing(BoardItemKind::Pad, uuid));
        }
        old.pads.extend(candidate.insert_pad(pad));
    }
    for via in elements.vias {
        let uuid = via.uuid();
        let Some(previous) = candidate.insert_via(via.clone()) else {
            return Err(missing(BoardItemKind::Via, uuid));
        };
        // upstream `BI_Via::setLayers()`
        let layers_changed = (previous.start_layer() != via.start_layer())
            || (previous.end_layer() != via.end_layer());
        if layers_changed
            && candidate.traces_at(TraceAnchor::Via(uuid)).any(|t| {
                !crate::geometry::Via::is_on_layer_between(
                    t.layer(),
                    via.start_layer(),
                    via.end_layer(),
                )
            })
        {
            return Err(Error::ViaLayersInUse(uuid));
        }
        old.vias.push(previous);
    }
    for junction in elements.junctions {
        let uuid = junction.uuid();
        let previous = candidate.insert_junction(junction);
        old.junctions
            .push(previous.ok_or_else(|| missing(BoardItemKind::Junction, uuid))?);
    }
    for trace in elements.traces {
        let uuid = trace.uuid();
        let previous = candidate.insert_trace(trace);
        old.traces
            .push(previous.ok_or_else(|| missing(BoardItemKind::Trace, uuid))?);
    }
    let items = sorted_items(items);
    commit_segment(p, index, candidate, &items, false)?;
    record(
        p,
        r.board,
        BoardChange::NetSegmentElementsChanged {
            segment: r.segment,
            items,
        },
    );
    Ok(BoardMutation::UpdateNetSegmentElements {
        segment: r,
        elements: sorted_elements(old),
    })
}

// --- Planes ---

fn plane_reference(board: BoardId, plane: &BoardPlane) -> Option<Reference> {
    plane.net().map(|net| Reference::Net {
        net,
        by: NetUse::Plane(board, plane.id()),
    })
}

fn add_plane(p: &mut Project, id: BoardId, plane: BoardPlane) -> Result<BoardMutation> {
    let index = board_index(p, id)?;
    if p.boards[index].planes.contains_key(&plane.id()) {
        return Err(Error::DuplicateUuid {
            kind: EntityKind::Plane,
            uuid: plane.uuid(),
        });
    }
    Ctx::new(p).check_net(plane.net())?;
    if let Some(r) = plane_reference(id, &plane) {
        p.refs.insert(r);
    }
    let plane_id = plane.id();
    let board = &mut p.boards[index];
    board.derived.invalidate_planes([plane.layer()]);
    board.derived.schedule_air_wires(plane.net());
    board.planes.insert(plane_id, plane);
    record(p, id, BoardChange::PlaneAdded(plane_id));
    Ok(BoardMutation::RemovePlane(PlaneRef {
        board: id,
        plane: plane_id,
    }))
}

fn remove_plane(p: &mut Project, r: PlaneRef) -> Result<BoardMutation> {
    let index = board_index(p, r.board)?;
    let board = &mut p.boards[index];
    let plane = board.planes.remove(&r.plane).ok_or(Error::NotFound {
        kind: EntityKind::Plane,
        uuid: r.plane.0,
    })?;
    board.derived.invalidate_planes([plane.layer()]);
    board.derived.schedule_air_wires(plane.net());
    board.derived.plane_fragments.remove(&r.plane);
    if let Some(reference) = plane_reference(r.board, &plane) {
        p.refs.remove(&reference);
    }
    record(p, r.board, BoardChange::PlaneRemoved(r.plane));
    Ok(BoardMutation::AddPlane {
        board: r.board,
        plane,
    })
}

fn update_plane(p: &mut Project, id: BoardId, plane: BoardPlane) -> Result<BoardMutation> {
    let index = board_index(p, id)?;
    if !p.boards[index].planes.contains_key(&plane.id()) {
        return Err(Error::NotFound {
            kind: EntityKind::Plane,
            uuid: plane.uuid(),
        });
    }
    Ctx::new(p).check_net(plane.net())?;
    let new_reference = plane_reference(id, &plane);
    let board = &mut p.boards[index];
    board.derived.invalidate_planes([plane.layer()]);
    board.derived.schedule_air_wires(plane.net());
    let old = board
        .planes
        .insert(plane.id(), plane)
        .expect("checked above");
    board.derived.invalidate_planes([old.layer()]);
    board.derived.schedule_air_wires(old.net());
    if let Some(r) = plane_reference(id, &old) {
        p.refs.remove(&r);
    }
    if let Some(r) = new_reference {
        p.refs.insert(r);
    }
    record(p, id, BoardChange::PlaneChanged(old.id()));
    Ok(BoardMutation::UpdatePlane {
        board: id,
        plane: old,
    })
}

// --- Zones, polygons, stroke texts, holes ---

/// The copper layers whose planes are affected by an item (upstream
/// `invalidatePlanes()` of `BI_Zone`, `BI_Polygon`, `BI_StrokeText`,
/// `BI_Hole`).
fn item_layers(copper: &BTreeSet<Layer>, item: &BoardItem) -> BTreeSet<Layer> {
    let single = |layer: Layer| -> BTreeSet<Layer> {
        if layer.is_copper() {
            BTreeSet::from([layer])
        } else {
            BTreeSet::new()
        }
    };
    match item {
        BoardItem::Zone(zone) => zone.layers().clone(),
        BoardItem::Polygon(polygon) if polygon.layer().is_board_edge() => copper.clone(),
        BoardItem::Polygon(polygon) => single(polygon.layer()),
        BoardItem::StrokeText(text) => single(text.layer()),
        BoardItem::Hole(_) => copper.clone(),
    }
}

/// Inserts or replaces an item, returns the previous one.
fn insert_item(board: &mut Board, item: BoardItem) -> Option<BoardItem> {
    match item {
        BoardItem::Zone(x) => board.zones.insert(x.uuid(), x).map(BoardItem::Zone),
        BoardItem::Polygon(x) => board.polygons.insert(x.uuid(), x).map(BoardItem::Polygon),
        BoardItem::StrokeText(x) => board
            .stroke_texts
            .insert(x.uuid(), x)
            .map(BoardItem::StrokeText),
        BoardItem::Hole(x) => board.holes.insert(x.uuid(), x).map(BoardItem::Hole),
    }
}

fn add_item(p: &mut Project, id: BoardId, item: BoardItem) -> Result<BoardMutation> {
    let index = board_index(p, id)?;
    let (kind, uuid) = (item.kind(), item.uuid());
    let board = &mut p.boards[index];
    if board.item(kind, &uuid).is_some() {
        return Err(duplicate_item(kind, uuid));
    }
    let layers = item_layers(&board.copper_layers(), &item);
    board.derived.invalidate_planes(layers);
    insert_item(board, item);
    record(p, id, BoardChange::ItemAdded { kind, uuid });
    Ok(BoardMutation::RemoveItem {
        board: id,
        kind,
        uuid,
    })
}

fn remove_item(
    p: &mut Project,
    id: BoardId,
    kind: BoardItemKind,
    uuid: Uuid,
) -> Result<BoardMutation> {
    let index = board_index(p, id)?;
    let board = &mut p.boards[index];
    let item = match kind {
        BoardItemKind::Zone => board.zones.remove(&uuid).map(BoardItem::Zone),
        BoardItemKind::Polygon => board.polygons.remove(&uuid).map(BoardItem::Polygon),
        BoardItemKind::StrokeText => board.stroke_texts.remove(&uuid).map(BoardItem::StrokeText),
        BoardItemKind::Hole => board.holes.remove(&uuid).map(BoardItem::Hole),
        _ => None,
    }
    .ok_or_else(|| item_not_found(kind, uuid))?;
    let layers = item_layers(&board.copper_layers(), &item);
    board.derived.invalidate_planes(layers);
    record(p, id, BoardChange::ItemRemoved { kind, uuid });
    Ok(BoardMutation::AddItem { board: id, item })
}

fn update_item(p: &mut Project, id: BoardId, item: BoardItem) -> Result<BoardMutation> {
    let index = board_index(p, id)?;
    let (kind, uuid) = (item.kind(), item.uuid());
    let board = &mut p.boards[index];
    if board.item(kind, &uuid).is_none() {
        return Err(item_not_found(kind, uuid));
    }
    let copper = board.copper_layers();
    let mut layers = item_layers(&copper, &item);
    let old = insert_item(board, item).expect("checked above");
    layers.extend(item_layers(&copper, &old));
    board.derived.invalidate_planes(layers);
    record(p, id, BoardChange::ItemChanged { kind, uuid });
    Ok(BoardMutation::UpdateItem {
        board: id,
        item: old,
    })
}

// --- Typed convenience wrappers ---

impl Project {
    /// Adds a device to a board.
    pub fn add_board_device(&mut self, board: BoardId, device: BoardDevice) -> Result<DeviceRef> {
        let component = device.component();
        self.apply(Mutation::Board(BoardMutation::AddDevice { board, device }))?;
        Ok(DeviceRef { board, component })
    }

    /// Adds a net segment to a board.
    pub fn add_board_net_segment(
        &mut self,
        board: BoardId,
        segment: BoardNetSegment,
    ) -> Result<BoardNetSegmentRef> {
        let id = segment.id();
        self.apply(Mutation::Board(BoardMutation::AddNetSegment {
            board,
            segment,
        }))?;
        Ok(BoardNetSegmentRef { board, segment: id })
    }

    /// Adds a plane to a board.
    pub fn add_board_plane(&mut self, board: BoardId, plane: BoardPlane) -> Result<PlaneRef> {
        let id = plane.id();
        self.apply(Mutation::Board(BoardMutation::AddPlane { board, plane }))?;
        Ok(PlaneRef { board, plane: id })
    }

    /// Adds a zone, polygon, stroke text or hole to a board.
    pub fn add_board_item(&mut self, board: BoardId, item: BoardItem) -> Result<()> {
        self.apply(Mutation::Board(BoardMutation::AddItem { board, item }))
            .map(drop)
    }
}
