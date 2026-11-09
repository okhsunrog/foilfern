//! Port of libs/librepcb/core/project/erc/electricalrulecheckmessages.{h,cpp}.
//!
//! Upstream has one `ErcMsg*` subclass of `RuleCheckMessage` per message
//! kind; here [`ErcMessageKind`] identifies the kind and the affected items,
//! and [`ErcMessage`] bundles it with the generic [`RuleCheckMessage`]
//! (severity, texts, approval, locations) and the schematic the locations
//! refer to. The texts and the approval nodes are identical to upstream
//! (approvals are stored in `circuit/erc.lp`).

use librepcb_i18n::tr;

use crate::project::id::{
    BusId, BusSegmentRef, ComponentInstanceId, ComponentSignalRef, NetClassId, NetSegmentRef,
    NetSignalId, SchematicId, SymbolId,
};
use crate::rule_check::{RuleCheckMessage, Severity};
use crate::serialization::{List, SExpression};
use crate::types::Uuid;

/// The kind of an ERC message and the items it refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErcMessageKind {
    /// A net class without nets (upstream `ErcMsgUnusedNetClass`).
    UnusedNetClass(NetClassId),
    /// A bus without bus segments (upstream `ErcMsgUnusedBus`).
    UnusedBus(BusId),
    /// A net connected to less than two pins (upstream `ErcMsgOpenNet`).
    OpenNet(NetSignalId),
    /// A net attached only once to a bus (upstream `ErcMsgOpenNetInBus`).
    OpenNetInBus {
        /// The bus.
        bus: BusId,
        /// The net of the segment.
        net: NetSignalId,
        /// The attached net segment.
        segment: NetSegmentRef,
    },
    /// A net segment without net label attached to a bus (upstream
    /// `ErcMsgUnnamedNetInBus`).
    UnnamedNetInBus {
        /// The bus.
        bus: BusId,
        /// The net of the segment.
        net: NetSignalId,
        /// The attached net segment.
        segment: NetSegmentRef,
    },
    /// A wire with an open end (upstream `ErcMsgOpenWireInSegment`).
    OpenWireInSegment {
        /// The net segment.
        segment: NetSegmentRef,
        /// The open net line.
        line: Uuid,
    },
    /// A required component signal without net (upstream
    /// `ErcMsgUnconnectedRequiredSignal`).
    UnconnectedRequiredSignal(ComponentSignalRef),
    /// A component signal connected to a net which is not named as forced
    /// by the library (upstream `ErcMsgForcedNetSignalNameConflict`).
    ForcedNetSignalNameConflict(ComponentSignalRef),
    /// A required gate is not placed (upstream `ErcMsgUnplacedRequiredGate`).
    UnplacedRequiredGate {
        /// The component instance.
        component: ComponentInstanceId,
        /// The gate (symbol variant item UUID).
        gate: Uuid,
    },
    /// An optional gate is not placed (upstream
    /// `ErcMsgUnplacedOptionalGate`).
    UnplacedOptionalGate {
        /// The component instance.
        component: ComponentInstanceId,
        /// The gate (symbol variant item UUID).
        gate: Uuid,
    },
    /// A pin connected to a net without wire (upstream
    /// `ErcMsgConnectedPinWithoutWire`).
    ConnectedPinWithoutWire {
        /// The schematic.
        schematic: SchematicId,
        /// The symbol.
        symbol: SymbolId,
        /// The library pin UUID.
        pin: Uuid,
    },
    /// A net segment junction without lines (upstream
    /// `ErcMsgUnconnectedJunction` of an `SI_NetPoint`).
    UnconnectedNetJunction {
        /// The net segment.
        segment: NetSegmentRef,
        /// The junction.
        junction: Uuid,
    },
    /// A bus segment junction without lines (upstream
    /// `ErcMsgUnconnectedJunction` of an `SI_BusJunction`).
    UnconnectedBusJunction {
        /// The bus segment.
        segment: BusSegmentRef,
        /// The junction.
        junction: Uuid,
    },
}

/// A message of the electrical rule check (upstream `ErcMsgBase`).
#[derive(Debug, Clone)]
pub struct ErcMessage {
    kind: ErcMessageKind,
    schematic: Option<SchematicId>,
    message: RuleCheckMessage,
}

impl ErcMessage {
    /// Returns the kind and the affected items.
    pub fn kind(&self) -> ErcMessageKind {
        self.kind
    }

    /// Returns the schematic the [locations](RuleCheckMessage::locations)
    /// refer to, if any (upstream `getSchematic()`).
    pub fn schematic(&self) -> Option<SchematicId> {
        self.schematic
    }

    /// Returns the generic message (severity, texts, approval, locations).
    pub fn message(&self) -> &RuleCheckMessage {
        &self.message
    }

    /// Returns the approval node (shortcut for `message().approval()`).
    pub fn approval(&self) -> &SExpression {
        self.message.approval()
    }
}

impl From<ErcMessage> for RuleCheckMessage {
    fn from(msg: ErcMessage) -> Self {
        msg.message
    }
}

/// Texts of a message, computed by the check (names are resolved there).
pub(super) struct Texts {
    pub severity: Severity,
    pub message: String,
    pub description: String,
    pub approval_name: &'static str,
}

/// Names of the items a message refers to, resolved by the check.
#[derive(Default)]
pub(super) struct Names<'a> {
    /// Net class, bus, net or component name (first argument).
    pub a: &'a str,
    /// Second argument (net, signal, gate suffix, pin name).
    pub b: &'a str,
    /// Third argument (forced net name conflicts: component name).
    pub c: &'a str,
    /// Fourth argument (forced net name conflicts: signal name).
    pub d: &'a str,
}

impl ErcMessageKind {
    /// Returns the texts of the message with the resolved `names`.
    pub(super) fn texts(&self, n: &Names<'_>) -> Texts {
        let (severity, message, description, approval_name) = match self {
            Self::UnusedNetClass(_) => (
                Severity::Hint,
                tr!("ErcMsgUnusedNetClass", "Unused net class: '{0}'", n.a),
                tr!(
                    "ErcMsgUnusedNetClass",
                    "There are no nets assigned to the net class, so you could remove it."
                ),
                "unused_netclass",
            ),
            Self::UnusedBus(_) => (
                Severity::Hint,
                tr!("ErcMsgUnusedBus", "Unused bus: '{0}'", n.a),
                // Not translated upstream.
                "There's a bus in the circuit without any schematics using it. This should \
                 not happen, please report it as a bug. But no worries, this issue is not \
                 harmful at all so you can safely ignore this message."
                    .to_owned(),
                "unused_bus",
            ),
            Self::OpenNet(_) => (
                Severity::Warning,
                tr!("ErcMsgOpenNet", "Less than two pins in net: '{0}'", n.a),
                tr!(
                    "ErcMsgOpenNet",
                    "The net is connected to less than two pins, so it does not represent an \
                     electrical connection. Check if you missed to connect more pins."
                ),
                "open_net",
            ),
            Self::OpenNetInBus { .. } => (
                Severity::Hint, // Not sure if a warning would be justified...
                tr!(
                    "ErcMsgOpenNetInBus",
                    "Bus contains unused net: '{0}:{1}'",
                    n.a,
                    n.b
                ),
                tr!(
                    "ErcMsgOpenNetInBus",
                    "The net is connected to the bus, but is not leaving the bus ^anywhere. \
                     Check if you missed to make a connection."
                ),
                "open_net_in_bus",
            ),
            Self::UnnamedNetInBus { .. } => (
                Severity::Warning,
                tr!(
                    "ErcMsgUnnamedNetInBus",
                    "Bus contains unnamed net: '{0}:{1}'",
                    n.a,
                    n.b
                ),
                tr!(
                    "ErcMsgUnnamedNetInBus",
                    "A wire without a net label is connected to the bus, which makes it \
                     impossible for this net to leave the bus somewhere else. Add a net label \
                     to the wire to explicitly specify the net."
                ),
                "unnamed_net_in_bus",
            ),
            Self::OpenWireInSegment { .. } => (
                Severity::Warning,
                tr!("ErcMsgOpenWireInSegment", "Open wire in net: '{0}'", n.a),
                tr!(
                    "ErcMsgOpenWireInSegment",
                    "The wire has an open (unconnected) end with no net label attached, thus is \
                     looks like a mistake. Check if a connection to another wire or pin is \
                     missing (denoted by a cross mark)."
                ),
                "open_wire",
            ),
            Self::UnconnectedRequiredSignal(_) => (
                Severity::Error,
                tr!(
                    "ErcMsgUnconnectedRequiredSignal",
                    "Unconnected component signal: '{0}:{1}'",
                    n.a,
                    n.b
                ),
                tr!(
                    "ErcMsgUnconnectedRequiredSignal",
                    "The component signal is marked as required, but is not connected to any \
                     net. Add a wire to the corresponding symbol pin to connect it to a net."
                ),
                "unconnected_required_signal",
            ),
            Self::ForcedNetSignalNameConflict(_) => (
                Severity::Error,
                tr!(
                    "ErcMsgForcedNetSignalNameConflict",
                    "Net name conflict: '{0}' != '{1}' ('{2}:{3}')",
                    n.a,
                    n.b,
                    n.c,
                    n.d
                ),
                tr!(
                    "ErcMsgForcedNetSignalNameConflict",
                    "The component signal requires the attached net to be named '{0}', but it \
                     is named '{1}'. Either rename the net manually or remove this connection.",
                    n.b,
                    n.a
                ),
                "forced_net_name_conflict",
            ),
            Self::UnplacedRequiredGate { .. } => (
                Severity::Error,
                tr!(
                    "ErcMsgUnplacedRequiredSymbol",
                    "Unplaced required gate: '{0}:{1}'",
                    n.a,
                    n.b
                ),
                tr!(
                    "ErcMsgUnplacedRequiredSymbol",
                    "The gate '{0}' of '{1}' is marked as required, but it is not added to the \
                     schematic.",
                    n.b,
                    n.a
                ),
                "unplaced_required_gate",
            ),
            Self::UnplacedOptionalGate { .. } => (
                Severity::Warning,
                tr!(
                    "ErcMsgUnplacedOptionalSymbol",
                    "Unplaced gate: '{0}:{1}'",
                    n.a,
                    n.b
                ),
                tr!(
                    "ErcMsgUnplacedOptionalSymbol",
                    "The optional gate '{0}' of '{1}' is not added to the schematic.",
                    n.b,
                    n.a
                ),
                "unplaced_optional_gate",
            ),
            Self::ConnectedPinWithoutWire { .. } => (
                Severity::Warning,
                tr!(
                    "ErcMsgConnectedPinWithoutWire",
                    "Connected pin without wire: '{0}:{1}'",
                    n.a,
                    n.b
                ),
                tr!(
                    "ErcMsgConnectedPinWithoutWire",
                    "The pin is electrically connected to a net, but has no wire attached so \
                     this connection is not visible in the schematic. Add a wire to make the \
                     connection visible."
                ),
                "connected_pin_without_wire",
            ),
            Self::UnconnectedNetJunction { .. } => (
                Severity::Hint,
                tr!(
                    "ErcMsgUnconnectedJunction",
                    "Unconnected junction in net: '{0}'",
                    n.a
                ),
                // Not translated upstream.
                "There's an invisible junction in the schematic without any wire attached. \
                 This should not happen, please report it as a bug. But no worries, this \
                 issue is not harmful at all so you can safely ignore this message."
                    .to_owned(),
                "unconnected_junction",
            ),
            Self::UnconnectedBusJunction { .. } => (
                Severity::Hint,
                tr!(
                    "ErcMsgUnconnectedJunction",
                    "Unconnected junction in bus: '{0}'",
                    n.a
                ),
                // Not translated upstream.
                "There's an invisible junction in the schematic without any line attached. \
                 This should not happen, please report it as a bug. But no worries, this \
                 issue is not harmful at all so you can safely ignore this message."
                    .to_owned(),
                "unconnected_junction",
            ),
        };
        Texts {
            severity,
            message,
            description,
            approval_name,
        }
    }

    /// Appends the details of the approval node (upstream constructors).
    fn append_approval(&self, a: &mut List) {
        match *self {
            Self::UnusedNetClass(id) => {
                a.append_child("netclass", &id.0);
            }
            Self::UnusedBus(id) => {
                a.append_child("bus", &id.0);
            }
            Self::OpenNet(id) => {
                a.append_child("net", &id.0);
            }
            Self::OpenNetInBus { bus, net, .. } | Self::UnnamedNetInBus { bus, net, .. } => {
                a.ensure_line_break();
                a.append_child("bus", &bus.0);
                a.ensure_line_break();
                a.append_child("net", &net.0);
                a.ensure_line_break();
            }
            Self::OpenWireInSegment { segment, .. } => {
                a.append_child("segment", &segment.segment.0);
            }
            Self::UnconnectedRequiredSignal(s) | Self::ForcedNetSignalNameConflict(s) => {
                a.ensure_line_break();
                a.append_child("component", &s.component.0);
                a.ensure_line_break();
                a.append_child("signal", &s.signal);
                a.ensure_line_break();
            }
            Self::UnplacedRequiredGate { component, gate }
            | Self::UnplacedOptionalGate { component, gate } => {
                a.ensure_line_break();
                a.append_child("component", &component.0);
                a.ensure_line_break();
                a.append_child("gate", &gate);
                a.ensure_line_break();
            }
            Self::ConnectedPinWithoutWire {
                schematic,
                symbol,
                pin,
            } => {
                a.ensure_line_break();
                a.append_child("schematic", &schematic.0);
                a.ensure_line_break();
                a.append_child("symbol", &symbol.0);
                a.ensure_line_break();
                a.append_child("pin", &pin);
                a.ensure_line_break();
            }
            Self::UnconnectedNetJunction { segment, junction } => {
                a.ensure_line_break();
                a.append_child("schematic", &segment.schematic.0);
                a.ensure_line_break();
                a.append_child("netsegment", &segment.segment.0);
                a.ensure_line_break();
                a.append_child("junction", &junction);
                a.ensure_line_break();
            }
            Self::UnconnectedBusJunction { segment, junction } => {
                a.ensure_line_break();
                a.append_child("schematic", &segment.schematic.0);
                a.ensure_line_break();
                a.append_child("bussegment", &segment.segment.0);
                a.ensure_line_break();
                a.append_child("junction", &junction);
                a.ensure_line_break();
            }
        }
    }
}

/// Builds an [`ErcMessage`] (upstream `ErcMsg*` constructors).
pub(super) fn build_message(
    kind: ErcMessageKind,
    names: &Names<'_>,
    location: super::electrical_rule_check::Location,
) -> ErcMessage {
    let texts = kind.texts(names);
    let mut message = RuleCheckMessage::new(
        texts.severity,
        texts.message,
        texts.description,
        texts.approval_name,
        location.paths,
    );
    kind.append_approval(message.approval_mut());
    ErcMessage {
        kind,
        schematic: location.schematic,
        message,
    }
}
