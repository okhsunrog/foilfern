//! Circuit mutations (upstream `Circuit::add*()`/`remove*()`/`set*Name()`,
//! `ComponentSignalInstance::setNetSignal()` and the registration checks
//! of `NetClass`, `NetSignal`, `Bus` and `ComponentInstance`).
//!
//! Every function validates first (check-then-commit), then modifies the
//! circuit, the reverse index and the journal, and returns the inverse
//! mutation.

use std::collections::BTreeSet;

use super::Mutation;
use crate::project::Project;
use crate::project::change::Change;
use crate::project::circuit::{
    AssemblyVariant, Bus, ComponentInstance, ComponentInstanceProperties, NetClass, NetSignal,
};
use crate::project::error::{EntityKind, Error, Result};
use crate::project::id::{
    AssemblyVariantId, BusId, ComponentInstanceId, ComponentSignalRef, NetClassId, NetSignalId,
};
use crate::project::ref_index::{NetUse, Reference};
use crate::types::Uuid;

// --- Assembly variants ---

pub(super) fn add_assembly_variant(
    p: &mut Project,
    variant: AssemblyVariant,
    index: Option<usize>,
) -> Result<Mutation> {
    let variants = &p.circuit.assembly_variants;
    if variants.contains_uuid(&variant.uuid()) {
        return Err(Error::DuplicateUuid {
            kind: EntityKind::AssemblyVariant,
            uuid: variant.uuid(),
        });
    }
    if variants.contains_name(variant.name()) {
        return Err(Error::DuplicateName {
            kind: EntityKind::AssemblyVariant,
            name: variant.name().to_string(),
        });
    }
    let id = AssemblyVariantId(variant.uuid());
    let index = index.unwrap_or(variants.len());
    p.circuit.assembly_variants.insert(index, variant);
    p.record(Change::AssemblyVariantAdded(id));
    Ok(Mutation::RemoveAssemblyVariant(id))
}

pub(super) fn move_assembly_variant(
    p: &mut Project,
    variant: AssemblyVariantId,
    index: usize,
) -> Result<Mutation> {
    let variants = &mut p.circuit.assembly_variants;
    let old = variants.index_of_uuid(&variant.0).ok_or(Error::NotFound {
        kind: EntityKind::AssemblyVariant,
        uuid: variant.0,
    })?;
    let new = index.min(variants.len() - 1);
    if old != new {
        let value = variants.take(old).expect("index was resolved above");
        variants.insert(new, value);
        p.record(Change::AssemblyVariantOrder);
    }
    Ok(Mutation::MoveAssemblyVariant {
        variant,
        index: old,
    })
}

pub(super) fn remove_assembly_variant(p: &mut Project, id: AssemblyVariantId) -> Result<Mutation> {
    let variants = &p.circuit.assembly_variants;
    let index = variants.index_of_uuid(&id.0).ok_or(Error::NotFound {
        kind: EntityKind::AssemblyVariant,
        uuid: id.0,
    })?;
    if variants.len() == 1 {
        return Err(Error::LastAssemblyVariant);
    }
    let variant = p
        .circuit
        .assembly_variants
        .take(index)
        .expect("index was found above");
    p.record(Change::AssemblyVariantRemoved(id));
    Ok(Mutation::AddAssemblyVariant {
        variant,
        index: Some(index),
    })
}

pub(super) fn update_assembly_variant(
    p: &mut Project,
    variant: AssemblyVariant,
) -> Result<Mutation> {
    let id = AssemblyVariantId(variant.uuid());
    let variants = &p.circuit.assembly_variants;
    if !variants.contains_uuid(&id.0) {
        return Err(Error::NotFound {
            kind: EntityKind::AssemblyVariant,
            uuid: id.0,
        });
    }
    if variants
        .by_name(variant.name(), true)
        .is_some_and(|other| other.uuid() != id.0)
    {
        return Err(Error::DuplicateName {
            kind: EntityKind::AssemblyVariant,
            name: variant.name().to_string(),
        });
    }
    let slot = p
        .circuit
        .assembly_variants
        .by_uuid_mut(&id.0)
        .expect("checked above");
    let old = std::mem::replace(slot, variant);
    p.record(Change::AssemblyVariantChanged(id));
    Ok(Mutation::UpdateAssemblyVariant(old))
}

// --- Net classes ---

pub(super) fn add_net_class(p: &mut Project, net_class: NetClass) -> Result<Mutation> {
    let id = NetClassId(net_class.uuid());
    if p.circuit.net_classes.contains_key(&id) {
        return Err(Error::DuplicateUuid {
            kind: EntityKind::NetClass,
            uuid: id.0,
        });
    }
    check_net_class_name(p, &net_class, None)?;
    p.circuit.net_classes.insert(id, net_class);
    p.record(Change::NetClassAdded(id));
    Ok(Mutation::RemoveNetClass(id))
}

pub(super) fn remove_net_class(p: &mut Project, id: NetClassId) -> Result<Mutation> {
    let net_class = p.circuit.net_class(id).ok_or(Error::NotFound {
        kind: EntityKind::NetClass,
        uuid: id.0,
    })?;
    if p.circuit.net_signals.values().any(|n| n.net_class() == id) {
        return Err(Error::NetClassInUse(net_class.name().to_string()));
    }
    let net_class = p.circuit.net_classes.remove(&id).expect("checked above");
    p.record(Change::NetClassRemoved(id));
    Ok(Mutation::AddNetClass(net_class))
}

pub(super) fn update_net_class(p: &mut Project, net_class: NetClass) -> Result<Mutation> {
    let id = NetClassId(net_class.uuid());
    if !p.circuit.net_classes.contains_key(&id) {
        return Err(Error::NotFound {
            kind: EntityKind::NetClass,
            uuid: id.0,
        });
    }
    check_net_class_name(p, &net_class, Some(id))?;
    let old = p
        .circuit
        .net_classes
        .insert(id, net_class)
        .expect("checked above");
    p.record(Change::NetClassChanged(id));
    Ok(Mutation::UpdateNetClass(old))
}

fn check_net_class_name(
    p: &Project,
    net_class: &NetClass,
    except: Option<NetClassId>,
) -> Result<()> {
    if p.circuit
        .net_class_by_name(net_class.name())
        .is_some_and(|(id, _)| Some(id) != except)
    {
        return Err(Error::DuplicateName {
            kind: EntityKind::NetClass,
            name: net_class.name().to_string(),
        });
    }
    Ok(())
}

// --- Net signals ---

pub(super) fn add_net_signal(p: &mut Project, net_signal: NetSignal) -> Result<Mutation> {
    let id = NetSignalId(net_signal.uuid());
    if p.circuit.net_signals.contains_key(&id) {
        return Err(Error::DuplicateUuid {
            kind: EntityKind::NetSignal,
            uuid: id.0,
        });
    }
    check_net_signal(p, &net_signal, None)?;
    p.circuit.net_signals.insert(id, net_signal);
    p.record(Change::NetSignalAdded(id));
    Ok(Mutation::RemoveNetSignal(id))
}

pub(super) fn remove_net_signal(p: &mut Project, id: NetSignalId) -> Result<Mutation> {
    let net_signal = p.circuit.net_signal(id).ok_or(Error::NotFound {
        kind: EntityKind::NetSignal,
        uuid: id.0,
    })?;
    if p.refs.is_net_used(id) {
        return Err(Error::NetSignalInUse(net_signal.name().to_string()));
    }
    let net_signal = p.circuit.net_signals.remove(&id).expect("checked above");
    p.record(Change::NetSignalRemoved(id));
    Ok(Mutation::AddNetSignal(net_signal))
}

pub(super) fn update_net_signal(p: &mut Project, net_signal: NetSignal) -> Result<Mutation> {
    let id = NetSignalId(net_signal.uuid());
    if !p.circuit.net_signals.contains_key(&id) {
        return Err(Error::NotFound {
            kind: EntityKind::NetSignal,
            uuid: id.0,
        });
    }
    check_net_signal(p, &net_signal, Some(id))?;
    let old = p
        .circuit
        .net_signals
        .insert(id, net_signal)
        .expect("checked above");
    p.record(Change::NetSignalChanged(id));
    Ok(Mutation::UpdateNetSignal(old))
}

fn check_net_signal(
    p: &Project,
    net_signal: &NetSignal,
    except: Option<NetSignalId>,
) -> Result<()> {
    if p.circuit
        .net_signal_by_name(net_signal.name())
        .is_some_and(|(id, _)| Some(id) != except)
    {
        return Err(Error::DuplicateName {
            kind: EntityKind::NetSignal,
            name: net_signal.name().to_string(),
        });
    }
    if !p.circuit.net_classes.contains_key(&net_signal.net_class()) {
        return Err(Error::InexistentNetClass(net_signal.net_class().0));
    }
    Ok(())
}

// --- Buses ---

pub(super) fn add_bus(p: &mut Project, bus: Bus) -> Result<Mutation> {
    let id = BusId(bus.uuid());
    if p.circuit.buses.contains_key(&id) {
        return Err(Error::DuplicateUuid {
            kind: EntityKind::Bus,
            uuid: id.0,
        });
    }
    check_bus_name(p, &bus, None)?;
    p.circuit.buses.insert(id, bus);
    p.record(Change::BusAdded(id));
    Ok(Mutation::RemoveBus(id))
}

pub(super) fn remove_bus(p: &mut Project, id: BusId) -> Result<Mutation> {
    let bus = p.circuit.bus(id).ok_or(Error::NotFound {
        kind: EntityKind::Bus,
        uuid: id.0,
    })?;
    if p.refs.is_bus_used(id) {
        return Err(Error::BusInUse(bus.name().to_string()));
    }
    let bus = p.circuit.buses.remove(&id).expect("checked above");
    p.record(Change::BusRemoved(id));
    Ok(Mutation::AddBus(bus))
}

pub(super) fn update_bus(p: &mut Project, bus: Bus) -> Result<Mutation> {
    let id = BusId(bus.uuid());
    if !p.circuit.buses.contains_key(&id) {
        return Err(Error::NotFound {
            kind: EntityKind::Bus,
            uuid: id.0,
        });
    }
    check_bus_name(p, &bus, Some(id))?;
    let old = p.circuit.buses.insert(id, bus).expect("checked above");
    p.record(Change::BusChanged(id));
    Ok(Mutation::UpdateBus(old))
}

fn check_bus_name(p: &Project, bus: &Bus, except: Option<BusId>) -> Result<()> {
    if p.circuit
        .bus_by_name(bus.name())
        .is_some_and(|(id, _)| Some(id) != except)
    {
        return Err(Error::DuplicateName {
            kind: EntityKind::Bus,
            name: bus.name().to_string(),
        });
    }
    Ok(())
}

// --- Component instances ---

pub(super) fn add_component_instance(
    p: &mut Project,
    component: ComponentInstance,
) -> Result<Mutation> {
    let id = component.id();
    if p.circuit.component_instances.contains_key(&id) {
        return Err(Error::DuplicateUuid {
            kind: EntityKind::Component,
            uuid: id.0,
        });
    }
    check_component_name(p, component.name(), None)?;
    // Library component, symbol variant and signal set (upstream
    // ComponentInstance constructor and ProjectLoader::loadCircuit()).
    let lib_component = p
        .library
        .component(&component.lib_component())
        .ok_or(Error::MissingLibraryComponent(component.lib_component()))?;
    lib_component
        .symbol_variants()
        .required_by_uuid(&component.lib_variant())?;
    let lib_signals: BTreeSet<Uuid> = lib_component.signals().uuids().into_iter().collect();
    for signal in component.signals().keys() {
        if !lib_signals.contains(signal) {
            return Err(Error::InexistentComponentSignal(*signal));
        }
    }
    if component.signals().len() != lib_signals.len() {
        return Err(Error::SignalCountMismatch {
            component: id.0,
            lib_component: component.lib_component(),
        });
    }
    for net in component.connected_nets() {
        if !p.circuit.net_signals.contains_key(&net) {
            return Err(Error::InexistentNetSignal(net.0));
        }
    }
    for (signal, inst) in component.signals() {
        if let Some(net) = inst.net() {
            p.refs.insert(signal_reference(id, *signal, net));
        }
    }
    p.circuit.component_instances.insert(id, component);
    p.record(Change::ComponentAdded(id));
    Ok(Mutation::RemoveComponentInstance(id))
}

pub(super) fn remove_component_instance(
    p: &mut Project,
    id: ComponentInstanceId,
) -> Result<Mutation> {
    let component = p.circuit.component_instance(id).ok_or(Error::NotFound {
        kind: EntityKind::Component,
        uuid: id.0,
    })?;
    if p.is_component_used(id) {
        return Err(Error::ComponentInUse(component.name().to_string()));
    }
    let component = p
        .circuit
        .component_instances
        .remove(&id)
        .expect("checked above");
    for (signal, inst) in component.signals() {
        if let Some(net) = inst.net() {
            p.refs.remove(&signal_reference(id, *signal, net));
        }
    }
    p.record(Change::ComponentRemoved(id));
    Ok(Mutation::AddComponentInstance(component))
}

pub(super) fn update_component_instance(
    p: &mut Project,
    properties: ComponentInstanceProperties,
) -> Result<Mutation> {
    let id = properties.id;
    if !p.circuit.component_instances.contains_key(&id) {
        return Err(Error::NotFound {
            kind: EntityKind::Component,
            uuid: id.0,
        });
    }
    check_component_name(p, &properties.name, Some(id))?;
    let component = p
        .circuit
        .component_instances
        .get_mut(&id)
        .expect("checked above");
    let old = component.set_properties(properties);
    p.record(Change::ComponentChanged(id));
    Ok(Mutation::UpdateComponentInstance(old))
}

pub(super) fn set_component_signal_net(
    p: &mut Project,
    signal: ComponentSignalRef,
    net: Option<NetSignalId>,
) -> Result<Mutation> {
    let component = p
        .circuit
        .component_instance(signal.component)
        .ok_or(Error::NotFound {
            kind: EntityKind::Component,
            uuid: signal.component.0,
        })?;
    let inst = component
        .signal(&signal.signal)
        .ok_or(Error::InexistentComponentSignal(signal.signal))?;
    let old = inst.net();
    if old == net {
        return Ok(Mutation::SetComponentSignalNet { signal, net });
    }
    if let Some(net) = net
        && !p.circuit.net_signals.contains_key(&net)
    {
        return Err(Error::InexistentNetSignal(net.0));
    }
    let wired = p
        .schematics
        .iter()
        .any(|s| s.is_component_signal_wired(signal))
        || p.boards.iter().any(|b| b.is_component_signal_wired(signal));
    if wired {
        let lib_signal_name = p
            .library
            .component(&component.lib_component())
            .and_then(|c| c.signals().by_uuid(&signal.signal))
            .map(|s| s.name().to_string())
            .unwrap_or_else(|| signal.signal.to_string());
        return Err(Error::ComponentSignalInUse {
            component: component.name().to_string(),
            signal: lib_signal_name,
        });
    }
    p.circuit
        .component_instances
        .get_mut(&signal.component)
        .expect("checked above")
        .set_signal_net(&signal.signal, net);
    if let Some(old) = old {
        p.refs
            .remove(&signal_reference(signal.component, signal.signal, old));
    }
    if let Some(net) = net {
        p.refs
            .insert(signal_reference(signal.component, signal.signal, net));
    }
    super::board::component_signal_net_changed(p, signal.component, old, net);
    p.record(Change::ComponentSignalNetChanged {
        signal,
        from: old,
        to: net,
    });
    Ok(Mutation::SetComponentSignalNet { signal, net: old })
}

fn check_component_name(
    p: &Project,
    name: &str,
    except: Option<ComponentInstanceId>,
) -> Result<()> {
    if p.circuit
        .component_instance_by_name(name)
        .is_some_and(|(id, _)| Some(id) != except)
    {
        return Err(Error::DuplicateName {
            kind: EntityKind::Component,
            name: name.to_owned(),
        });
    }
    Ok(())
}

fn signal_reference(component: ComponentInstanceId, signal: Uuid, net: NetSignalId) -> Reference {
    Reference::Net {
        net,
        by: NetUse::ComponentSignal(ComponentSignalRef { component, signal }),
    }
}
