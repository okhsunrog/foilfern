//! Port of libs/librepcb/core/library/pkg/footprintpad.{h,cpp}.
//!
//! Upstream `FootprintPad` derives from `Pad`; here it embeds a [`Pad`],
//! accessible with [`FootprintPad::pad()`] and [`FootprintPad::pad_mut()`].

use crate::geometry::{Pad, object_list, property};
use crate::serialization::{self, DeserializeObject, HasUuid, List, SExpression, SerializeObject};
use crate::types::Uuid;

/// A pad of a footprint.
///
/// The UUID is part of the package's interface and must never be changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FootprintPad {
    pad: Pad,
    /// The connected package pad, or `None` if the pad is electrically not
    /// connected (e.g. mechanical-only pads).
    package_pad_uuid: Option<Uuid>,
}

impl FootprintPad {
    /// Creates a footprint pad.
    pub fn new(pad: Pad, package_pad_uuid: Option<Uuid>) -> Self {
        Self {
            pad,
            package_pad_uuid,
        }
    }

    /// Returns the UUID.
    pub fn uuid(&self) -> Uuid {
        self.pad.uuid()
    }

    /// Returns a copy with another UUID (upstream
    /// `FootprintPad(uuid, other)`).
    pub fn with_uuid(&self, uuid: Uuid) -> Self {
        Self {
            pad: self.pad.with_uuid(uuid),
            package_pad_uuid: self.package_pad_uuid,
        }
    }

    /// Returns the pad attributes (upstream base class `Pad`).
    pub fn pad(&self) -> &Pad {
        &self.pad
    }

    /// Returns the pad attributes for modification.
    // upstream: the Pad setters emit onEdited(...Changed)
    pub fn pad_mut(&mut self) -> &mut Pad {
        &mut self.pad
    }

    property!(
        /// Returns the UUID of the connected package pad (`None` if the pad
        /// is electrically not connected).
        copy package_pad_uuid: Option<Uuid>, set_package_pad_uuid
    );
}

impl HasUuid for FootprintPad {
    fn uuid(&self) -> Uuid {
        self.pad.uuid()
    }
}

object_list!(
    /// List of [`FootprintPad`]s.
    FootprintPadList,
    FootprintPadListTag,
    FootprintPad,
    "pad"
);

impl SerializeObject for FootprintPad {
    fn serialize(&self, root: &mut List) {
        self.pad.serialize_with(root, |root| {
            root.append_child("package_pad", &self.package_pad_uuid);
        });
    }
}

impl DeserializeObject for FootprintPad {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            pad: Pad::deserialize(node)?,
            package_pad_uuid: node.child_value("package_pad/@0")?,
        })
    }
}
