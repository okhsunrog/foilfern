//! Port of libs/librepcb/core/library/dev/devicecheckmessages.{h,cpp}.

use foilfern_i18n::tr;

use crate::rule_check::{RuleCheckMessage, Severity};

/// Messages of the device check (upstream `Msg*` classes in
/// devicecheckmessages.h).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeviceCheckMessage {
    /// No parts (MPNs) added (upstream `MsgDeviceHasNoParts`).
    DeviceHasNoParts,
    /// None of the pads is connected (upstream
    /// `MsgNoPadsInDeviceConnected`).
    NoPadsConnected,
}

impl DeviceCheckMessage {
    /// Returns the generic rule check message.
    pub fn to_message(&self) -> RuleCheckMessage {
        match self {
            Self::DeviceHasNoParts => RuleCheckMessage::new(
                Severity::Hint,
                tr!("MsgDeviceHasNoParts", "No part numbers added"),
                tr!(
                    "MsgDeviceHasNoParts",
                    "There are no orderable parts added to the device. It's recommended \
                     (but not mandatory) to add the concrete manufacturer part numbers \
                     this device is valid for. These MPNs are used by the BOM export to \
                     make BOMs of projects much more complete and accurate.\n\n\
                     If this device doesn't represent an orderable part, just ignore \
                     this message."
                ),
                "no_parts",
                Vec::new(),
            ),
            Self::NoPadsConnected => RuleCheckMessage::new(
                // Only warning because it could be a false-positive.
                Severity::Warning,
                tr!("MsgNoPadsInDeviceConnected", "No pads connected"),
                tr!(
                    "MsgNoPadsInDeviceConnected",
                    "The chosen package contains pads, but none of them are connected \
                     to component signals. So these pads have no electrical function \
                     and when adding the device to a PCB, no traces can be connected to \
                     them.\n\nTo fix this issue, connect the package pads to their \
                     corresponding component signals in the table widget.\n\nIf all \
                     pads have only a mechanical purpose and thus don't need to be \
                     connected to component signals, this message can be ignored."
                ),
                "no_pads_connected",
                Vec::new(),
            ),
        }
    }
}
