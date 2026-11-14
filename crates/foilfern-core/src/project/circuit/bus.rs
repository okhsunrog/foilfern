//! Port of libs/librepcb/core/project/circuit/bus.{h,cpp}.
//!
//! Differences to upstream: no circuit back-pointer, no `isAddedToCircuit`
//! state; the bus segment registration (`getSchematicBusSegments()`,
//! `getConnectedNetSignals()`, `isUsed()`) is answered by the project
//! through its reverse index; the `nameChanged` signal is not ported.

use crate::geometry::{impl_uuid, property};
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::{BusName, UnsignedLength, Uuid};

/// A bus of the circuit (a named bundle of nets).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Bus {
    uuid: Uuid,
    name: BusName,
    has_auto_name: bool,
    prefix_net_names: bool,
    max_trace_length_difference: Option<UnsignedLength>,
}

impl Bus {
    /// Creates a bus.
    pub fn new(
        uuid: Uuid,
        name: BusName,
        auto_name: bool,
        prefix_net_names: bool,
        max_trace_length_difference: Option<UnsignedLength>,
    ) -> Self {
        Self {
            uuid,
            name,
            has_auto_name: auto_name,
            prefix_net_names,
            max_trace_length_difference,
        }
    }

    /// Returns the name.
    pub fn name(&self) -> &BusName {
        &self.name
    }

    /// Returns whether the name was generated automatically.
    pub fn has_auto_name(&self) -> bool {
        self.has_auto_name
    }

    /// Sets the name and whether it is an auto-generated name. Returns
    /// whether anything was modified.
    pub fn set_name(&mut self, name: BusName, auto_name: bool) -> bool {
        if name == self.name && auto_name == self.has_auto_name {
            return false;
        }
        self.name = name;
        self.has_auto_name = auto_name;
        // upstream: emits nameChanged()
        true
    }

    property!(
        /// Returns whether the names of the bus nets are prefixed with the
        /// bus name.
        copy prefix_net_names: bool, set_prefix_net_names
    );
    property!(
        /// Returns the maximum allowed trace length difference between the
        /// nets of the bus (`None` = unlimited).
        copy max_trace_length_difference: Option<UnsignedLength>, set_max_trace_length_difference
    );
}

impl_uuid!(Bus);

impl SerializeObject for Bus {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("auto", &self.has_auto_name);
        root.append_child("name", &self.name);
        root.ensure_line_break();
        root.append_child("prefix_nets", &self.prefix_net_names);
        let diff = match self.max_trace_length_difference {
            Some(v) => SExpression::token(v.to_string()),
            None => SExpression::token("none"),
        };
        root.append_child("max_trace_length_difference", &diff);
        root.ensure_line_break();
    }
}

impl DeserializeObject for Bus {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        let diff = node.required_child("max_trace_length_difference/@0")?;
        let max_trace_length_difference = if diff.value()? == "none" {
            None
        } else {
            Some(serialization::FromSExpression::from_sexpression(diff)?)
        };
        Ok(Self {
            uuid: node.child_value("@0")?,
            name: node.child_value("name/@0")?,
            has_auto_name: node.child_value("auto/@0")?,
            prefix_net_names: node.child_value("prefix_nets/@0")?,
            max_trace_length_difference,
        })
    }
}
