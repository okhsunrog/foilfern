//! Port of libs/librepcb/core/geometry/zone.{h,cpp}.
//!
//! The `QFlags` of upstream are [`bitflags`] types.

use bitflags::bitflags;

use super::{Path, impl_uuid, object_list, property};
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::Uuid;

bitflags! {
    /// Board sides a [`Zone`] applies to (upstream `Zone::Layers`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct ZoneLayers: u32 {
        /// Top layers.
        const TOP = 1 << 0;
        /// Inner layers.
        const INNER = 1 << 1;
        /// Bottom layers.
        const BOTTOM = 1 << 2;
    }
}

bitflags! {
    /// Keepout rules of a [`Zone`] (upstream `Zone::Rules`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct ZoneRules: u32 {
        /// No copper (except planes).
        const NO_COPPER = 1 << 0;
        /// No planes.
        const NO_PLANES = 1 << 1;
        /// No stop mask exposure.
        const NO_EXPOSURE = 1 << 2;
        /// No devices.
        const NO_DEVICES = 1 << 3;
    }
}

/// Serde for the flag types: an array of flag names (e.g.
/// `["NO_COPPER", "NO_PLANES"]`), unknown names are rejected.
macro_rules! serde_flag_names {
    ($ty:ty) => {
        impl serde::Serialize for $ty {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_seq(self.iter_names().map(|(name, _)| name))
            }
        }

        impl<'de> serde::Deserialize<'de> for $ty {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let names = <Vec<std::borrow::Cow<'de, str>>>::deserialize(deserializer)?;
                names.iter().try_fold(Self::empty(), |flags, name| {
                    Self::from_name(name)
                        .map(|flag| flags | flag)
                        .ok_or_else(|| serde::de::Error::custom(format!("unknown flag: {name}")))
                })
            }
        }
    };
}

serde_flag_names!(ZoneLayers);
serde_flag_names!(ZoneRules);

/// A keepout zone of a footprint or board.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Zone {
    uuid: Uuid,
    layers: ZoneLayers,
    rules: ZoneRules,
    outline: Path,
}

impl Zone {
    /// Creates a zone.
    pub fn new(uuid: Uuid, layers: ZoneLayers, rules: ZoneRules, outline: Path) -> Self {
        Self {
            uuid,
            layers,
            rules,
            outline,
        }
    }

    property!(
        /// Returns the layers.
        copy layers: ZoneLayers, set_layers
    );
    property!(
        /// Returns the rules.
        copy rules: ZoneRules, set_rules
    );
    property!(
        /// Returns the outline.
        ref outline: Path, set_outline
    );
}

impl_uuid!(Zone);

object_list!(
    /// List of [`Zone`]s.
    ZoneList,
    ZoneListTag,
    Zone,
    "zone"
);

impl SerializeObject for Zone {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.ensure_line_break();
        let rules = self.rules;
        root.append_child("no_copper", &rules.contains(ZoneRules::NO_COPPER));
        root.append_child("no_planes", &rules.contains(ZoneRules::NO_PLANES));
        root.append_child("no_exposure", &rules.contains(ZoneRules::NO_EXPOSURE));
        root.append_child("no_devices", &rules.contains(ZoneRules::NO_DEVICES));
        root.ensure_line_break();
        let layers = self.layers;
        root.append_child("top", &layers.contains(ZoneLayers::TOP));
        root.append_child("inner", &layers.contains(ZoneLayers::INNER));
        root.append_child("bottom", &layers.contains(ZoneLayers::BOTTOM));
        root.ensure_line_break();
        self.outline.serialize(root);
        root.ensure_line_break();
    }
}

impl DeserializeObject for Zone {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        let mut layers = ZoneLayers::empty();
        layers.set(ZoneLayers::TOP, node.child_value("top/@0")?);
        layers.set(ZoneLayers::INNER, node.child_value("inner/@0")?);
        layers.set(ZoneLayers::BOTTOM, node.child_value("bottom/@0")?);
        let mut rules = ZoneRules::empty();
        rules.set(ZoneRules::NO_COPPER, node.child_value("no_copper/@0")?);
        rules.set(ZoneRules::NO_PLANES, node.child_value("no_planes/@0")?);
        rules.set(ZoneRules::NO_EXPOSURE, node.child_value("no_exposure/@0")?);
        rules.set(ZoneRules::NO_DEVICES, node.child_value("no_devices/@0")?);
        Ok(Self {
            uuid: node.child_value("@0")?,
            layers,
            rules,
            outline: Path::deserialize(node)?,
        })
    }
}
