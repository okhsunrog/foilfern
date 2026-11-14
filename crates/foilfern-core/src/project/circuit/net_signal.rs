//! Port of libs/librepcb/core/project/circuit/netsignal.{h,cpp}.
//!
//! Differences to upstream: no circuit back-pointer, no `isAddedToCircuit`
//! state; the registration lists (`getComponentSignals()`,
//! `getSchematicNetSegments()`, `getBoardNetSegments()`,
//! `getBoardPlanes()`, `isUsed()`, `hasNetLabels()`, `isNameForced()`,
//! `isAnonymous()`) are answered by the project through its reverse index;
//! the `nameChanged` signal is not ported.

use crate::geometry::{impl_uuid, property};
use crate::project::id::NetClassId;
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::{CircuitIdentifier, Uuid};

/// A net signal (electrical net) of the circuit.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NetSignal {
    uuid: Uuid,
    name: CircuitIdentifier,
    has_auto_name: bool,
    net_class: NetClassId,
}

impl NetSignal {
    /// Creates a net signal.
    pub fn new(
        uuid: Uuid,
        net_class: NetClassId,
        name: CircuitIdentifier,
        auto_name: bool,
    ) -> Self {
        Self {
            uuid,
            name,
            has_auto_name: auto_name,
            net_class,
        }
    }

    /// Returns the name.
    pub fn name(&self) -> &CircuitIdentifier {
        &self.name
    }

    /// Returns whether the name was generated automatically.
    pub fn has_auto_name(&self) -> bool {
        self.has_auto_name
    }

    /// Sets the name and whether it is an auto-generated name. Returns
    /// whether anything was modified.
    pub fn set_name(&mut self, name: CircuitIdentifier, auto_name: bool) -> bool {
        if name == self.name && auto_name == self.has_auto_name {
            return false;
        }
        self.name = name;
        self.has_auto_name = auto_name;
        // upstream: emits nameChanged()
        true
    }

    property!(
        /// Returns the net class.
        copy net_class: NetClassId, set_net_class
    );
}

impl_uuid!(NetSignal);

impl SerializeObject for NetSignal {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("auto", &self.has_auto_name);
        root.append_child("name", &self.name);
        root.ensure_line_break();
        root.append_child("netclass", &self.net_class);
        root.ensure_line_break();
    }
}

impl DeserializeObject for NetSignal {
    /// Loads the net signal; the net class is not validated here (see the
    /// project loader).
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            name: node.child_value("name/@0")?,
            has_auto_name: node.child_value("auto/@0")?,
            net_class: node.child_value("netclass/@0")?,
        })
    }
}
