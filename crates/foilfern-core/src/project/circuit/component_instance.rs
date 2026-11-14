//! Port of libs/librepcb/core/project/circuit/componentinstance.{h,cpp}.
//!
//! Differences to upstream: the library component is referenced by UUID
//! (resolved through the project library) instead of a pointer; no circuit
//! back-pointer, no `isAddedToCircuit` state. The registered symbols and
//! devices (`getSymbols()`, `getDevices()`, `getPrimaryDevice()`,
//! `getUsedDeviceUuids()`, `isUsed()`) are answered by the project through
//! its reverse index; the `attributesChanged`/`primaryDeviceChanged`
//! signals are not ported.

use std::collections::{BTreeMap, BTreeSet};

use super::{ComponentAssemblyOptionList, ComponentSignalInstance};
use crate::attribute::AttributeList;
use crate::geometry::{impl_uuid, property};
use crate::library::LibraryBaseElement;
use crate::library::cmp::Component;
use crate::library::dev::Part;
use crate::project::error::Result;
use crate::project::id::{AssemblyVariantId, ComponentInstanceId, NetSignalId};
use crate::serialization::{List, SerializeObject};
use crate::types::{CircuitIdentifier, SimpleString, Uuid};

/// An instance of a library component in the circuit (e.g. "R5").
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ComponentInstance {
    uuid: Uuid,
    name: CircuitIdentifier,
    value: String,
    lib_component: Uuid,
    lib_variant: Uuid,
    attributes: AttributeList,
    assembly_options: ComponentAssemblyOptionList,
    lock_assembly: bool,
    /// Keyed by the UUID of the library component signal.
    signals: BTreeMap<Uuid, ComponentSignalInstance>,
}

impl ComponentInstance {
    /// Creates an instance of `lib_component` with the symbol variant
    /// `variant`, the component's default value and attributes, and all
    /// signals unconnected.
    ///
    /// Fails if `variant` is not a symbol variant of the component.
    pub fn new(
        uuid: Uuid,
        lib_component: &Component,
        variant: Uuid,
        name: CircuitIdentifier,
        lock_assembly: bool,
    ) -> Result<Self> {
        lib_component.symbol_variants().required_by_uuid(&variant)?;
        Ok(Self {
            uuid,
            name,
            value: lib_component.default_value().clone(),
            lib_component: lib_component.metadata().uuid(),
            lib_variant: variant,
            attributes: lib_component.attributes().clone(),
            assembly_options: ComponentAssemblyOptionList::new(),
            lock_assembly,
            signals: lib_component
                .signals()
                .iter()
                .map(|s| (s.uuid(), ComponentSignalInstance::default()))
                .collect(),
        })
    }

    /// Returns the typed identifier.
    pub fn id(&self) -> ComponentInstanceId {
        ComponentInstanceId(self.uuid)
    }

    property!(
        /// Returns the name (designator, e.g. "R5").
        ref name: CircuitIdentifier, set_name
    );
    property!(
        /// Returns the value (e.g. "10k").
        ref value: String, set_value
    );
    property!(
        /// Returns the attributes.
        ref attributes: AttributeList, set_attributes
    );
    property!(
        /// Returns the assembly options.
        ref assembly_options: ComponentAssemblyOptionList, set_assembly_options
    );
    property!(
        /// Returns whether the assembly options are locked for the board
        /// editor.
        copy lock_assembly: bool, set_lock_assembly
    );

    /// Device used for default board placement, from the first assembly option.
    pub fn default_device(&self) -> Option<Uuid> {
        self.assembly_options
            .iter()
            .next()
            .map(|option| option.device())
    }

    /// Returns the UUID of the library component.
    pub fn lib_component(&self) -> Uuid {
        self.lib_component
    }

    /// Returns the UUID of the symbol variant of the library component.
    pub fn lib_variant(&self) -> Uuid {
        self.lib_variant
    }

    /// Returns the signal instances, keyed by library signal UUID.
    pub fn signals(&self) -> &BTreeMap<Uuid, ComponentSignalInstance> {
        &self.signals
    }

    /// Returns the signal instance of a library signal.
    pub fn signal(&self, signal: &Uuid) -> Option<&ComponentSignalInstance> {
        self.signals.get(signal)
    }

    /// Connects a signal to `net` (or disconnects it). Returns whether the
    /// net changed, or `None` if the signal does not exist.
    ///
    /// Not validated here; adding the instance to a project checks that
    /// the nets exist.
    pub fn set_signal_net(&mut self, signal: &Uuid, net: Option<NetSignalId>) -> Option<bool> {
        self.signals.get_mut(signal).map(|s| s.set_net(net))
    }

    /// Returns the nets of all connected signals (without duplicates).
    pub fn connected_nets(&self) -> BTreeSet<NetSignalId> {
        self.signals.values().filter_map(|s| s.net()).collect()
    }

    /// Whether the component is schematic-only and has no assembly options
    /// (upstream `isPureSchematicOnly()`).
    pub fn is_pure_schematic_only(&self, lib_component: &Component) -> bool {
        lib_component.schematic_only() && self.assembly_options.is_empty()
    }

    /// Returns the UUIDs of all devices listed in the assembly options
    /// (upstream `getCompatibleDevices()`).
    pub fn compatible_devices(&self) -> BTreeSet<Uuid> {
        self.assembly_options.iter().map(|o| o.device()).collect()
    }

    /// Returns the parts to assemble in `assembly_variant` (all variants if
    /// `None`); an option without parts yields one empty part with the
    /// option's attributes (upstream `getParts()`).
    pub fn parts(&self, assembly_variant: Option<AssemblyVariantId>) -> Vec<Part> {
        let mut parts = Vec::new();
        for option in &self.assembly_options {
            if assembly_variant.is_none_or(|av| option.assembly_variants().contains(&av)) {
                parts.extend(option.parts().iter().cloned());
                if option.parts().is_empty() {
                    parts.push(Part::new(
                        SimpleString::default(),
                        SimpleString::default(),
                        option.attributes().clone(),
                    ));
                }
            }
        }
        parts
    }

    /// Replaces the editable properties (upstream `CmdComponentInstanceEdit`).
    /// Returns the previous properties.
    pub fn set_properties(
        &mut self,
        p: ComponentInstanceProperties,
    ) -> ComponentInstanceProperties {
        let old = self.properties();
        self.name = p.name;
        self.value = p.value;
        self.attributes = p.attributes;
        self.assembly_options = p.assembly_options;
        self.lock_assembly = p.lock_assembly;
        old
    }

    /// Returns the editable properties.
    pub fn properties(&self) -> ComponentInstanceProperties {
        ComponentInstanceProperties {
            id: self.id(),
            name: self.name.clone(),
            value: self.value.clone(),
            attributes: self.attributes.clone(),
            assembly_options: self.assembly_options.clone(),
            lock_assembly: self.lock_assembly,
        }
    }
}

impl_uuid!(ComponentInstance);

impl SerializeObject for ComponentInstance {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.ensure_line_break();
        root.append_child("lib_component", &self.lib_component);
        root.ensure_line_break();
        root.append_child("lib_variant", &self.lib_variant);
        root.ensure_line_break();
        root.append_child("name", &self.name);
        root.append_child("value", &self.value);
        root.ensure_line_break();
        root.append_child("lock_assembly", &self.lock_assembly);
        root.ensure_line_break();
        self.attributes.serialize(root);
        root.ensure_line_break();
        self.assembly_options.serialize(root);
        root.ensure_line_break();
        for (uuid, signal) in &self.signals {
            let node = root.append_list("signal");
            node.append_value(uuid);
            node.append_child("net", &signal.net());
            root.ensure_line_break();
        }
        root.ensure_line_break();
    }
}

/// The editable properties of a [`ComponentInstance`] (everything except
/// UUID, library references and signal connections).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ComponentInstanceProperties {
    /// The component instance.
    pub id: ComponentInstanceId,
    /// The name (designator).
    pub name: CircuitIdentifier,
    /// The value.
    pub value: String,
    /// The attributes.
    pub attributes: AttributeList,
    /// The assembly options.
    pub assembly_options: ComponentAssemblyOptionList,
    /// Whether the assembly options are locked for the board editor.
    pub lock_assembly: bool,
}
