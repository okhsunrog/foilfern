//! Port of libs/librepcb/core/project/erc/electricalrulecheckmessages.{h,cpp}.
//!
//! Typed identifiers replace downcasts and raw object pointers. The common
//! message holds translated text, approval and highlighting paths; the
//! schematic identifier is kept separately, like upstream `ErcMsgBase`.

use crate::geometry::Path;
use crate::project::{
    BusId, BusSegmentRef, ComponentInstanceId, ComponentSignalRef, NetClassId, NetSegmentRef,
    NetSignalId, SchematicId, SymbolRef,
};
use crate::rule_check::{RuleCheckMessage, Severity};
use crate::types::Uuid;

/// The entities involved in an ERC finding, for navigation and fixes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErcMessageKind {
    /// Net class with no nets (only checked if multiple classes exist).
    UnusedNetClass(NetClassId),
    /// Bus without schematic bus segments.
    UnusedBus(BusId),
    /// Net connected to fewer than two physical component signals.
    OpenNet(NetSignalId),
    /// Labeled net entering a bus only once.
    OpenNetInBus { bus: BusId, net: NetSignalId },
    /// Net entering a bus without a label.
    UnnamedNetInBus { bus: BusId, net: NetSignalId },
    /// Segment without labels and with an open wire end.
    OpenWire { segment: NetSegmentRef, line: Uuid },
    /// Required component signal without a net.
    UnconnectedRequiredSignal(ComponentSignalRef),
    /// Attached net name differs from a component signal's forced name.
    ForcedNetNameConflict(ComponentSignalRef),
    /// Required gate absent from schematics.
    UnplacedRequiredGate {
        component: ComponentInstanceId,
        gate: Uuid,
    },
    /// Optional gate absent from schematics.
    UnplacedOptionalGate {
        component: ComponentInstanceId,
        gate: Uuid,
    },
    /// Electrically connected pin with no visible wire.
    ConnectedPinWithoutWire { symbol: SymbolRef, pin: Uuid },
    /// Net junction without wires.
    UnconnectedNetJunction {
        segment: NetSegmentRef,
        junction: Uuid,
    },
    /// Bus junction without bus lines, even if net lines touch it.
    UnconnectedBusJunction {
        segment: BusSegmentRef,
        junction: Uuid,
    },
}

impl ErcMessageKind {
    fn approval_data(&self) -> (&'static str, Vec<(&'static str, Uuid)>) {
        match *self {
            Self::UnusedNetClass(id) => ("unused_netclass", vec![("netclass", id.0)]),
            Self::UnusedBus(id) => ("unused_bus", vec![("bus", id.0)]),
            Self::OpenNet(id) => ("open_net", vec![("net", id.0)]),
            Self::OpenNetInBus { bus, net } => {
                ("open_net_in_bus", vec![("bus", bus.0), ("net", net.0)])
            }
            Self::UnnamedNetInBus { bus, net } => {
                ("unnamed_net_in_bus", vec![("bus", bus.0), ("net", net.0)])
            }
            Self::OpenWire { segment, .. } => ("open_wire", vec![("segment", segment.segment.0)]),
            Self::UnconnectedRequiredSignal(signal) => (
                "unconnected_required_signal",
                vec![("component", signal.component.0), ("signal", signal.signal)],
            ),
            Self::ForcedNetNameConflict(signal) => (
                "forced_net_name_conflict",
                vec![("component", signal.component.0), ("signal", signal.signal)],
            ),
            Self::UnplacedRequiredGate { component, gate } => (
                "unplaced_required_gate",
                vec![("component", component.0), ("gate", gate)],
            ),
            Self::UnplacedOptionalGate { component, gate } => (
                "unplaced_optional_gate",
                vec![("component", component.0), ("gate", gate)],
            ),
            Self::ConnectedPinWithoutWire { symbol, pin } => (
                "connected_pin_without_wire",
                vec![
                    ("schematic", symbol.schematic.0),
                    ("symbol", symbol.symbol.0),
                    ("pin", pin),
                ],
            ),
            Self::UnconnectedNetJunction { segment, junction } => (
                "unconnected_junction",
                vec![
                    ("schematic", segment.schematic.0),
                    ("netsegment", segment.segment.0),
                    ("junction", junction),
                ],
            ),
            Self::UnconnectedBusJunction { segment, junction } => (
                "unconnected_junction",
                vec![
                    ("schematic", segment.schematic.0),
                    ("bussegment", segment.segment.0),
                    ("junction", junction),
                ],
            ),
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct Location {
    pub schematic: Option<SchematicId>,
    pub paths: Vec<Path>,
}

/// A fully resolved ERC finding. Approvals do not suppress findings;
/// callers compare `rule_message().approval()` with the project's approvals.
#[derive(Debug, Clone, PartialEq)]
pub struct ElectricalRuleCheckMessage {
    kind: ErcMessageKind,
    schematic: Option<SchematicId>,
    message: RuleCheckMessage,
}

static_assertions::assert_impl_all!(ElectricalRuleCheckMessage: Send, Sync);

impl ElectricalRuleCheckMessage {
    pub(super) fn new(
        kind: ErcMessageKind,
        severity: Severity,
        title: String,
        description: String,
        location: Location,
    ) -> Self {
        let (name, fields) = kind.approval_data();
        let mut message = RuleCheckMessage::new(severity, title, description, name, location.paths);
        let multiline = fields.len() > 1;
        for (key, value) in fields {
            if multiline {
                message.approval_mut().ensure_line_break();
            }
            message.approval_mut().append_child(key, &value);
        }
        if multiline {
            message.approval_mut().ensure_line_break();
        }
        Self {
            kind,
            schematic: location.schematic,
            message,
        }
    }

    /// Returns the typed finding and its model identifiers.
    pub fn kind(&self) -> &ErcMessageKind {
        &self.kind
    }
    /// Returns the common severity, text, approval and highlighting paths.
    pub fn rule_message(&self) -> &RuleCheckMessage {
        &self.message
    }
    /// Returns the schematic to open when navigating to this finding.
    pub fn schematic(&self) -> Option<SchematicId> {
        self.schematic
    }
}
