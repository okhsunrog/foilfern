//! Port of libs/librepcb/core/library/cmp/componentpinsignalmap.{h,cpp}.

use super::cmp_sig_pin_display_type::CmpSigPinDisplayType;
use crate::geometry::{object_list, property};
use crate::serialization::{self, DeserializeObject, HasUuid, List, SExpression, SerializeObject};
use crate::types::Uuid;

/// Maps a symbol pin to a component signal.
///
/// The pin and signal UUIDs are part of the component's interface and must
/// never be changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ComponentPinSignalMapItem {
    pin_uuid: Uuid,
    signal_uuid: Option<Uuid>,
    display_type: CmpSigPinDisplayType,
}

impl ComponentPinSignalMapItem {
    /// Creates a mapping (`signal` is `None` if the pin is unconnected).
    pub fn new(pin: Uuid, signal: Option<Uuid>, display_type: CmpSigPinDisplayType) -> Self {
        Self {
            pin_uuid: pin,
            signal_uuid: signal,
            display_type,
        }
    }

    /// Returns the UUID of the symbol pin.
    pub fn pin_uuid(&self) -> Uuid {
        self.pin_uuid
    }

    property!(
        /// Returns the UUID of the component signal (`None` if the pin is
        /// not connected).
        copy signal_uuid: Option<Uuid>, set_signal_uuid
    );
    property!(
        /// Returns what text is displayed at the pin.
        copy display_type: CmpSigPinDisplayType, set_display_type
    );
}

impl HasUuid for ComponentPinSignalMapItem {
    /// Returns the pin UUID (the key of the map).
    fn uuid(&self) -> Uuid {
        self.pin_uuid
    }
}

object_list!(
    /// Pin-signal-map of a symbol variant item (one item per symbol pin).
    ComponentPinSignalMap,
    ComponentPinSignalMapTag,
    ComponentPinSignalMapItem,
    "pin"
);

impl ComponentPinSignalMap {
    /// Creates a map with unconnected items for the given pins (upstream
    /// `ComponentPinSignalMapHelpers::create()`).
    pub fn with_pins(
        pins: impl IntoIterator<Item = Uuid>,
        display_type: CmpSigPinDisplayType,
    ) -> Self {
        pins.into_iter()
            .map(|pin| ComponentPinSignalMapItem::new(pin, None, display_type))
            .collect()
    }
}

impl SerializeObject for ComponentPinSignalMapItem {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.pin_uuid);
        root.append_child("signal", &self.signal_uuid);
        root.append_child("text", &self.display_type);
    }
}

impl DeserializeObject for ComponentPinSignalMapItem {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            pin_uuid: node.child_value("@0")?,
            signal_uuid: node.child_value("signal/@0")?,
            display_type: node.child_value("text/@0")?,
        })
    }
}
