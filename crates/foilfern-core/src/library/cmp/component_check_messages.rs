//! Port of libs/librepcb/core/library/cmp/componentcheckmessages.{h,cpp}.

use foilfern_i18n::tr;

use super::component_signal::ComponentSignal;
use super::component_symbol_variant::ComponentSymbolVariant;
use crate::rule_check::{RuleCheckMessage, Severity};
use crate::types::CircuitIdentifier;

/// Messages of the component check (upstream `Msg*` classes in
/// componentcheckmessages.h).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ComponentCheckMessage {
    /// Multiple signals have the same name (upstream
    /// `MsgDuplicateSignalName`).
    DuplicateSignalName {
        /// The signal name.
        name: CircuitIdentifier,
    },
    /// No default value (upstream `MsgMissingComponentDefaultValue`).
    MissingDefaultValue,
    /// No prefix (upstream `MsgMissingComponentPrefix`).
    MissingPrefix,
    /// No symbol variant (upstream `MsgMissingSymbolVariant`).
    MissingSymbolVariant,
    /// A symbol variant has no items (upstream
    /// `MsgMissingSymbolVariantItem`).
    MissingSymbolVariantItem {
        /// The symbol variant.
        symbol_variant: ComponentSymbolVariant,
    },
    /// A signal name starts with an inversion sign which has no function
    /// (upstream `MsgNonFunctionalComponentSignalInversionSign`).
    NonFunctionalSignalInversionSign {
        /// The signal.
        signal: ComponentSignal,
    },
    /// No pins of a symbol variant are connected (upstream
    /// `MsgNoPinsInSymbolVariantConnected`).
    NoPinsInSymbolVariantConnected {
        /// The symbol variant.
        symbol_variant: ComponentSymbolVariant,
    },
    /// Forced nets on a multi-signal component (upstream
    /// `MsgSuspiciousForcedNets`).
    SuspiciousForcedNets,
}

impl ComponentCheckMessage {
    /// Returns the generic rule check message.
    pub fn to_message(&self) -> RuleCheckMessage {
        match self {
            Self::DuplicateSignalName { name } => {
                let mut msg = RuleCheckMessage::new(
                    Severity::Error,
                    tr!(
                        "MsgDuplicateSignalName",
                        "Duplicate signal name: '{0}'",
                        name
                    ),
                    tr!(
                        "MsgDuplicateSignalName",
                        "All component signals must have unique names, otherwise they \
                         cannot be distinguished later in the device editor. If your part \
                         has several pins which are electrically exactly equal (e.g. \
                         multiple GND pins), you should add only one of these pins as a \
                         component signal. The assignment to multiple pins should be done \
                         in the device editor instead."
                    ),
                    "duplicate_signal_name",
                    Vec::new(),
                );
                msg.approval_mut().append_child("name", name.as_str());
                msg
            }
            Self::MissingDefaultValue => RuleCheckMessage::new(
                Severity::Warning,
                tr!("MsgMissingComponentDefaultValue", "No default value set"),
                tr!(
                    "MsgMissingComponentDefaultValue",
                    "Most components should have a default value set. The default value \
                     becomes the component's value when adding it to a schematic. It can \
                     also contain placeholders which are substituted later in the \
                     schematic. Commonly used default values are:\n\n\
                     Generic parts (e.g. a diode): {0}\n\
                     Specific parts (e.g. a microcontroller): {1}\n\
                     Passive parts: Using an attribute, e.g. {2}",
                    "'{{MPN or DEVICE}}'",
                    "'{{MPN or DEVICE or COMPONENT}}'",
                    "'{{RESISTANCE}}'"
                ),
                "empty_default_value",
                Vec::new(),
            ),
            Self::MissingPrefix => RuleCheckMessage::new(
                Severity::Warning,
                tr!("MsgMissingComponentPrefix", "No component prefix set"),
                tr!(
                    "MsgMissingComponentPrefix",
                    "Most components should have a prefix defined. The prefix is used \
                     to generate the component's name when adding it to a schematic. \
                     For example the prefix 'R' (resistor) leads to component names \
                     'R1', 'R2', 'R3' etc."
                ),
                "empty_prefix",
                Vec::new(),
            ),
            Self::MissingSymbolVariant => RuleCheckMessage::new(
                Severity::Error,
                tr!("MsgMissingSymbolVariant", "No symbol variant defined"),
                tr!(
                    "MsgMissingSymbolVariant",
                    "Every component requires at least one symbol variant, otherwise it \
                     can't be added to schematics."
                ),
                "missing_variants",
                Vec::new(),
            ),
            Self::MissingSymbolVariantItem { symbol_variant } => {
                let mut msg = RuleCheckMessage::new(
                    Severity::Error,
                    tr!(
                        "MsgMissingSymbolVariantItem",
                        "Symbol variant '{0}' has no items",
                        symbol_variant.name()
                    ),
                    tr!(
                        "MsgMissingSymbolVariantItem",
                        "Every symbol variant requires at least one symbol item, otherwise \
                         it can't be added to schematics."
                    ),
                    "missing_gates",
                    Vec::new(),
                );
                variant_approval(&mut msg, symbol_variant);
                msg
            }
            Self::NonFunctionalSignalInversionSign { signal } => {
                let mut msg = RuleCheckMessage::new(
                    Severity::Hint,
                    tr!(
                        "MsgNonFunctionalComponentSignalInversionSign",
                        "Non-functional inversion sign: '{0}'",
                        signal.name()
                    ),
                    tr!(
                        "MsgNonFunctionalComponentSignalInversionSign",
                        "The signal name seems to start with an inversion sign, but \
                         LibrePCB uses a different sign to indicate inversion.\n\nIt's \
                         recommended to prefix inverted signal names with '{0}', regardless \
                         of the inversion sign used in the parts datasheet.",
                        "!"
                    ),
                    "nonfunctional_inversion_sign",
                    Vec::new(),
                );
                let approval = msg.approval_mut();
                approval.ensure_line_break();
                approval.append_child("signal", &signal.uuid());
                approval.ensure_line_break();
                msg
            }
            Self::NoPinsInSymbolVariantConnected { symbol_variant } => {
                let mut msg = RuleCheckMessage::new(
                    Severity::Error,
                    tr!(
                        "MsgNoPinsInSymbolVariantConnected",
                        "No pins connected in '{0}'",
                        symbol_variant.name()
                    ),
                    tr!(
                        "MsgNoPinsInSymbolVariantConnected",
                        "The chosen symbols contain pins, but none of them are connected \
                         to component signals. So when adding this component to a \
                         schematic, no wires can be attached to them.\n\nTo fix this \
                         issue, connect the symbol pins to their corresponding component \
                         signals in the symbol variant editor dialog."
                    ),
                    "no_pins_connected",
                    Vec::new(),
                );
                variant_approval(&mut msg, symbol_variant);
                msg
            }
            Self::SuspiciousForcedNets => RuleCheckMessage::new(
                Severity::Warning,
                tr!("MsgSuspiciousForcedNets", "Suspicious use of forced nets"),
                tr!(
                    "MsgSuspiciousForcedNets",
                    "At least one signal of this component has a forced net set, which \
                     is very unusual and can cause serious troubles if not used \
                     intentionally.\n\nPlease consult the user manual to ensure this is \
                     what you want. If you're unsure, clear the forced net on all \
                     component signals."
                ),
                "suspicious_forced_nets",
                Vec::new(),
            ),
        }
    }
}

fn variant_approval(msg: &mut RuleCheckMessage, symbol_variant: &ComponentSymbolVariant) {
    let approval = msg.approval_mut();
    approval.ensure_line_break();
    approval.append_child("variant", &symbol_variant.uuid());
    approval.ensure_line_break();
}
