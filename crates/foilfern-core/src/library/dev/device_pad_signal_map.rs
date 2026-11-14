//! Port of libs/librepcb/core/library/dev/devicepadsignalmap.{h,cpp}.

use crate::geometry::{object_list, property};
use crate::serialization::{
    self, DeserializeObject, FromSExpression, HasUuid, List, SExpression, SerializeObject,
};
use crate::types::Uuid;

/// Maps a package pad to a component signal.
///
/// The pad and signal UUIDs are part of the device's interface and must
/// never be changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DevicePadSignalMapItem {
    pad_uuid: Uuid,
    signal_uuid: Option<Uuid>,
    is_optional: bool,
}

impl DevicePadSignalMapItem {
    /// Creates a mapping (`signal` is `None` if the pad is unconnected).
    pub fn new(pad: Uuid, signal: Option<Uuid>, is_optional: bool) -> Self {
        Self {
            pad_uuid: pad,
            signal_uuid: signal,
            is_optional,
        }
    }

    /// Returns the UUID of the package pad.
    pub fn pad_uuid(&self) -> Uuid {
        self.pad_uuid
    }

    property!(
        /// Returns the UUID of the component signal (`None` if the pad is
        /// not connected).
        copy signal_uuid: Option<Uuid>, set_signal_uuid
    );
    property!(
        /// Returns whether the pad is optional, i.e. may be left
        /// unconnected in the board (e.g. thermal pads).
        copy is_optional: bool, set_optional
    );
}

impl HasUuid for DevicePadSignalMapItem {
    /// Returns the pad UUID (the key of the map).
    fn uuid(&self) -> Uuid {
        self.pad_uuid
    }
}

object_list!(
    /// Pad-signal-map of a device (one item per package pad).
    DevicePadSignalMap,
    DevicePadSignalMapTag,
    DevicePadSignalMapItem,
    "pad"
);

impl SerializeObject for DevicePadSignalMapItem {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.pad_uuid);
        root.append_child("optional", &self.is_optional);
        root.ensure_line_break();
        root.append_child("signal", &self.signal_uuid);
        root.ensure_line_break();
    }
}

impl DeserializeObject for DevicePadSignalMapItem {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            pad_uuid: node.child_value("@0")?,
            signal_uuid: node.child_value("signal/@0")?,
            // The "optional" attribute was introduced after LibrePCB
            // 2.0.0-rc1 was released for testing, thus it is still allowed to
            // be missing.
            is_optional: node
                .child("optional/@0")
                .map(bool::from_sexpression)
                .transpose()?
                .unwrap_or(false),
        })
    }
}
