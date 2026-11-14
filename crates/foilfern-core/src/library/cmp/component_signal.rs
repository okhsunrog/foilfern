//! Port of libs/librepcb/core/library/cmp/componentsignal.{h,cpp}.

use crate::geometry::{impl_uuid, object_list, property};
use crate::serialization::{self, DeserializeObject, HasName, List, SExpression, SerializeObject};
use crate::types::{CircuitIdentifier, SignalRole, Uuid};

/// A signal of a component.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ComponentSignal {
    uuid: Uuid,
    name: CircuitIdentifier,
    role: SignalRole,
    forced_net_name: String,
    is_required: bool,
    is_negated: bool,
    is_clock: bool,
}

impl ComponentSignal {
    /// Creates a signal.
    pub fn new(
        uuid: Uuid,
        name: CircuitIdentifier,
        role: SignalRole,
        forced_net_name: impl Into<String>,
        is_required: bool,
        is_negated: bool,
        is_clock: bool,
    ) -> Self {
        Self {
            uuid,
            name,
            role,
            forced_net_name: forced_net_name.into(),
            is_required,
            is_negated,
            is_clock,
        }
    }

    property!(
        /// Returns the name.
        ref name: CircuitIdentifier, set_name
    );
    property!(
        /// Returns the electrical role.
        copy role: SignalRole, set_role
    );
    property!(
        /// Returns the forced net name (empty if not forced; may contain
        /// attribute placeholders like `{{VALUE}}`).
        ref forced_net_name: String, set_forced_net_name
    );
    property!(
        /// Returns whether the signal must be connected.
        copy is_required: bool, set_is_required
    );
    property!(
        /// Returns whether the signal is negated (display only).
        copy is_negated: bool, set_is_negated
    );
    property!(
        /// Returns whether the signal is a clock (display only).
        copy is_clock: bool, set_is_clock
    );

    /// Returns whether a net name is forced.
    pub fn is_net_signal_name_forced(&self) -> bool {
        !self.forced_net_name.is_empty()
    }
}

impl_uuid!(ComponentSignal);

impl HasName for ComponentSignal {
    fn name(&self) -> &str {
        self.name.as_str()
    }
}

object_list!(
    /// List of [`ComponentSignal`]s.
    ComponentSignalList,
    ComponentSignalListTag,
    ComponentSignal,
    "signal"
);

impl SerializeObject for ComponentSignal {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("name", &self.name);
        root.append_child("role", &self.role);
        root.ensure_line_break();
        root.append_child("required", &self.is_required);
        root.append_child("negated", &self.is_negated);
        root.append_child("clock", &self.is_clock);
        root.append_child("forced_net", &self.forced_net_name);
        root.ensure_line_break();
    }
}

impl DeserializeObject for ComponentSignal {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            name: node.child_value("name/@0")?,
            role: node.child_value("role/@0")?,
            forced_net_name: node.child_value("forced_net/@0")?,
            is_required: node.child_value("required/@0")?,
            is_negated: node.child_value("negated/@0")?,
            is_clock: node.child_value("clock/@0")?,
        })
    }
}
