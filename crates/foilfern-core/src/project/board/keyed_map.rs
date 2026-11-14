//! Serde helper for the UUID keyed item maps of boards (no upstream
//! counterpart).
//!
//! The maps are serialized like any `BTreeMap` (a JSON object keyed by the
//! UUID); deserialization additionally checks that every key equals the
//! UUID stored in its value, so the model invariant "key = item UUID"
//! cannot be broken by a malformed mutation.

use std::collections::BTreeMap;
use std::fmt::Display;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{
    BoardDevice, BoardHoleData, BoardNetSegment, BoardPadData, BoardPlane, BoardPolygonData,
    BoardStrokeTextData, BoardZoneData,
};
use crate::geometry::{Junction, Trace, Via};
use crate::project::id::{ComponentInstanceId, NetSegmentId, PlaneId};
use crate::types::Uuid;

/// An item stored in a map keyed by one of its attributes.
pub(crate) trait MapKey<K> {
    /// Returns the key of the item.
    fn map_key(&self) -> K;
}

macro_rules! uuid_key {
    ($($ty:ty),*) => {
        $(impl MapKey<Uuid> for $ty {
            fn map_key(&self) -> Uuid {
                self.uuid()
            }
        })*
    };
}

uuid_key!(
    BoardPadData,
    Via,
    Junction,
    Trace,
    BoardZoneData,
    BoardPolygonData,
    BoardStrokeTextData,
    BoardHoleData
);

impl MapKey<ComponentInstanceId> for BoardDevice {
    fn map_key(&self) -> ComponentInstanceId {
        self.component()
    }
}

impl MapKey<NetSegmentId> for BoardNetSegment {
    fn map_key(&self) -> NetSegmentId {
        self.id()
    }
}

impl MapKey<PlaneId> for BoardPlane {
    fn map_key(&self) -> PlaneId {
        self.id()
    }
}

pub(crate) fn serialize<S, K, V>(map: &BTreeMap<K, V>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    K: Serialize,
    V: Serialize,
{
    map.serialize(serializer)
}

pub(crate) fn deserialize<'de, D, K, V>(deserializer: D) -> Result<BTreeMap<K, V>, D::Error>
where
    D: Deserializer<'de>,
    K: Deserialize<'de> + Ord + PartialEq + Display,
    V: Deserialize<'de> + MapKey<K>,
{
    let map = BTreeMap::<K, V>::deserialize(deserializer)?;
    if let Some((key, value)) = map.iter().find(|(k, v)| v.map_key() != **k) {
        return Err(D::Error::custom(format!(
            "map key \"{key}\" does not match the UUID \"{}\" of the item",
            value.map_key()
        )));
    }
    Ok(map)
}
