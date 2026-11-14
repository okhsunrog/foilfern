//! Serde support for value types whose wire representation is their string
//! form (no upstream counterpart).
//!
//! The project model (mutations, change events) is serializable so that
//! it can be sent over the wire (MCP, CLI). Value types which already have a
//! canonical string form (`Display`/`FromStr`, e.g. UUIDs, versions, layer
//! IDs, validated string newtypes) use that form as JSON string; see
//! `docs/project-model-design.md`, section "Serde representation".

/// Implements `serde::Serialize` via `Display` and `serde::Deserialize`
/// via `FromStr` (the error's `Display` becomes the serde error message).
macro_rules! serde_string {
    ($ty:ty) => {
        impl serde::Serialize for $ty {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> serde::Deserialize<'de> for $ty {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s =
                    <std::borrow::Cow<'de, str> as serde::Deserialize>::deserialize(deserializer)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

pub(crate) use serde_string;
