//! Circuit part of the project loader (upstream
//! `ProjectLoader::loadCircuit()`).

use std::collections::BTreeSet;

use super::parse_file;
use crate::attribute::AttributeList;
use crate::project::Project;
use crate::project::circuit::{
    AssemblyVariant, Bus, ComponentAssemblyOptionList, ComponentInstance, NetClass, NetSignal,
};
use crate::project::error::{Error, Result};
use crate::project::id::NetSignalId;
use crate::serialization::DeserializeObject;
use crate::types::Uuid;

pub(super) fn load_circuit(p: &mut Project) -> Result<()> {
    let root = parse_file(p, "circuit/circuit.lp")?;
    for node in root.children_named("variant") {
        p.add_assembly_variant(AssemblyVariant::deserialize(node)?, None)?;
    }
    if p.circuit.assembly_variants.is_empty() {
        return Err(Error::NoAssemblyVariants);
    }
    for node in root.children_named("netclass") {
        p.add_net_class(NetClass::deserialize(node)?)?;
    }
    for node in root.children_named("net") {
        let net_signal = NetSignal::deserialize(node)?;
        if p.circuit.net_class(net_signal.net_class()).is_none() {
            return Err(Error::InexistentNetClass(net_signal.net_class().0));
        }
        p.add_net_signal(net_signal)?;
    }
    for node in root.children_named("bus") {
        p.add_bus(Bus::deserialize(node)?)?;
    }
    for node in root.children_named("component") {
        let lib_component_uuid: Uuid = node.child_value("lib_component/@0")?;
        let lib_component = p
            .library
            .component(&lib_component_uuid)
            .ok_or(Error::MissingLibraryComponent(lib_component_uuid))?;
        let mut cmp = ComponentInstance::new(
            node.child_value("@0")?,
            lib_component,
            node.child_value("lib_variant/@0")?,
            node.child_value("name/@0")?,
            node.child_value("lock_assembly/@0")?,
        )?;
        cmp.set_value(node.child_value("value/@0")?);
        cmp.set_attributes(AttributeList::deserialize(node)?);
        cmp.set_assembly_options(ComponentAssemblyOptionList::deserialize(node)?);
        let mut loaded_signals = BTreeSet::new();
        for child in node.children_named("signal") {
            let signal_uuid: Uuid = child.child_value("@0")?;
            if cmp.signal(&signal_uuid).is_none() {
                return Err(Error::InexistentComponentSignal(signal_uuid));
            }
            if !loaded_signals.insert(signal_uuid) {
                return Err(Error::DuplicateComponentSignal(signal_uuid));
            }
            let net: Option<NetSignalId> = child.child_value("net/@0")?;
            if let Some(net) = net
                && p.circuit.net_signal(net).is_none()
            {
                return Err(Error::InexistentNetSignal(net.0));
            }
            cmp.set_signal_net(&signal_uuid, net);
        }
        if loaded_signals.len() != cmp.signals().len() {
            return Err(Error::SignalCountMismatch {
                component: cmp.uuid(),
                lib_component: lib_component_uuid,
            });
        }
        p.add_component_instance(cmp)?;
    }
    log::debug!("Successfully loaded circuit.");
    Ok(())
}
