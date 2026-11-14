//! Port of libs/librepcb/core/project/circuit/circuit.{h,cpp}.
//!
//! Differences to upstream: the circuit is a plain container; all
//! `add*()`/`remove*()`/`set*Name()` operations with their validation live
//! in the project mutations ([`Project::apply()`](crate::project::Project::apply)),
//! and the Qt signals are replaced by the project's change journal.

use std::collections::BTreeMap;

use super::{AssemblyVariant, AssemblyVariantList, Bus, ComponentInstance, NetClass, NetSignal};
use crate::project::id::{AssemblyVariantId, BusId, ComponentInstanceId, NetClassId, NetSignalId};
use crate::serialization::{List, SerializeObject};

/// The netlist of a project: assembly variants, net classes, net signals,
/// buses and component instances.
///
/// UUID keyed collections are ordered by UUID, which is also the order in
/// the saved file (upstream `QMap`).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Circuit {
    pub(crate) assembly_variants: AssemblyVariantList,
    pub(crate) net_classes: BTreeMap<NetClassId, NetClass>,
    pub(crate) net_signals: BTreeMap<NetSignalId, NetSignal>,
    pub(crate) buses: BTreeMap<BusId, Bus>,
    pub(crate) component_instances: BTreeMap<ComponentInstanceId, ComponentInstance>,
}

impl Circuit {
    /// Creates an empty circuit.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the assembly variants (the first one is the default).
    pub fn assembly_variants(&self) -> &AssemblyVariantList {
        &self.assembly_variants
    }

    /// Returns the assembly variant with the given identifier.
    pub fn assembly_variant(&self, id: AssemblyVariantId) -> Option<&AssemblyVariant> {
        self.assembly_variants.by_uuid(&id.0)
    }

    /// Returns the net classes.
    pub fn net_classes(&self) -> &BTreeMap<NetClassId, NetClass> {
        &self.net_classes
    }

    /// Returns the net class with the given identifier.
    pub fn net_class(&self, id: NetClassId) -> Option<&NetClass> {
        self.net_classes.get(&id)
    }

    /// Returns the net class with the given name.
    pub fn net_class_by_name(&self, name: &str) -> Option<(NetClassId, &NetClass)> {
        self.net_classes
            .iter()
            .find(|(_, c)| c.name().as_str() == name)
            .map(|(id, c)| (*id, c))
    }

    /// Returns the net signals.
    pub fn net_signals(&self) -> &BTreeMap<NetSignalId, NetSignal> {
        &self.net_signals
    }

    /// Returns the net signal with the given identifier.
    pub fn net_signal(&self, id: NetSignalId) -> Option<&NetSignal> {
        self.net_signals.get(&id)
    }

    /// Returns the net signal with the given name.
    pub fn net_signal_by_name(&self, name: &str) -> Option<(NetSignalId, &NetSignal)> {
        self.net_signals
            .iter()
            .find(|(_, n)| n.name().as_str() == name)
            .map(|(id, n)| (*id, n))
    }

    /// Returns the buses.
    pub fn buses(&self) -> &BTreeMap<BusId, Bus> {
        &self.buses
    }

    /// Returns the bus with the given identifier.
    pub fn bus(&self, id: BusId) -> Option<&Bus> {
        self.buses.get(&id)
    }

    /// Returns the bus with the given name.
    pub fn bus_by_name(&self, name: &str) -> Option<(BusId, &Bus)> {
        self.buses
            .iter()
            .find(|(_, b)| b.name().as_str() == name)
            .map(|(id, b)| (*id, b))
    }

    /// Returns the component instances.
    pub fn component_instances(&self) -> &BTreeMap<ComponentInstanceId, ComponentInstance> {
        &self.component_instances
    }

    /// Returns the component instance with the given identifier.
    pub fn component_instance(&self, id: ComponentInstanceId) -> Option<&ComponentInstance> {
        self.component_instances.get(&id)
    }

    /// Returns the component instance with the given name.
    pub fn component_instance_by_name(
        &self,
        name: &str,
    ) -> Option<(ComponentInstanceId, &ComponentInstance)> {
        self.component_instances
            .iter()
            .find(|(_, c)| c.name().as_str() == name)
            .map(|(id, c)| (*id, c))
    }

    /// Returns a free net name `N1`, `N2`, ... (upstream
    /// `generateAutoNetSignalName()`).
    pub fn generate_auto_net_signal_name(&self) -> String {
        (1..)
            .map(|i| format!("N{i}"))
            .find(|name| self.net_signal_by_name(name).is_none())
            .expect("an unused name exists")
    }

    /// Returns a free bus name `B1`, `B2`, ... (upstream
    /// `generateAutoBusName()`).
    pub fn generate_auto_bus_name(&self) -> String {
        (1..)
            .map(|i| format!("B{i}"))
            .find(|name| self.bus_by_name(name).is_none())
            .expect("an unused name exists")
    }

    /// Returns a free component name `<prefix>1`, `<prefix>2`, ... (`?` if
    /// the prefix is empty; upstream `generateAutoComponentInstanceName()`).
    pub fn generate_auto_component_instance_name(&self, prefix: &str) -> String {
        let prefix = if prefix.is_empty() { "?" } else { prefix };
        (1..)
            .map(|i| format!("{prefix}{i}"))
            .find(|name| self.component_instance_by_name(name).is_none())
            .expect("an unused name exists")
    }
}

impl SerializeObject for Circuit {
    /// Writes the content of `circuit/circuit.lp` (upstream
    /// `Circuit::serialize()`).
    fn serialize(&self, root: &mut List) {
        root.ensure_line_break();
        self.assembly_variants.serialize(root);
        root.ensure_line_break();
        for obj in self.net_classes.values() {
            root.ensure_line_break();
            obj.serialize(root.append_list("netclass"));
        }
        for obj in self.net_signals.values() {
            root.ensure_line_break();
            obj.serialize(root.append_list("net"));
        }
        for obj in self.buses.values() {
            root.ensure_line_break();
            obj.serialize(root.append_list("bus"));
        }
        for obj in self.component_instances.values() {
            root.ensure_line_break();
            obj.serialize(root.append_list("component"));
        }
        root.ensure_line_break();
    }
}
