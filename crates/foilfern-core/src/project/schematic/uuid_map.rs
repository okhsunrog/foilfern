//! Serde helper for UUID keyed maps of schematic items (no upstream
//! counterpart).
//!
//! The maps are serialized as JSON objects keyed by UUID (see
//! `docs/project-model-design.md`, "Serde representation"); since every
//! value carries its UUID too, deserialization checks that key and value
//! agree, so that a deserialized item can be inserted without further
//! checks.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};

use crate::serialization::HasUuid;
use crate::types::Uuid;

/// Deserializes a `BTreeMap<K, V>` and rejects entries whose key differs
/// from the UUID of the value.
pub(crate) fn deserialize<'de, D, K, V>(deserializer: D) -> Result<BTreeMap<K, V>, D::Error>
where
    D: Deserializer<'de>,
    K: Deserialize<'de> + Ord + Copy + Into<Uuid>,
    V: Deserialize<'de> + HasUuid,
{
    let map = BTreeMap::<K, V>::deserialize(deserializer)?;
    for (key, value) in &map {
        let key: Uuid = (*key).into();
        if key != value.uuid() {
            return Err(serde::de::Error::custom(format!(
                "map key \"{key}\" does not match the UUID \"{}\" of its value",
                value.uuid()
            )));
        }
    }
    Ok(map)
}
