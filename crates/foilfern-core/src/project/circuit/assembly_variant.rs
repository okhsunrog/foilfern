//! Port of libs/librepcb/core/project/circuit/assemblyvariant.{h,cpp}.
//!
//! Differences to upstream: the `onEdited` signal is not ported (changes are
//! journaled by the project).

use crate::geometry::{impl_uuid, object_list, property};
use crate::serialization::{self, DeserializeObject, HasName, List, SExpression, SerializeObject};
use crate::types::{FileProofName, Uuid};

/// An assembly variant of the circuit (e.g. "Std", "Lite"), which selects
/// the parts to assemble.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AssemblyVariant {
    uuid: Uuid,
    name: FileProofName,
    description: String,
}

impl AssemblyVariant {
    /// Creates an assembly variant.
    pub fn new(uuid: Uuid, name: FileProofName, description: impl Into<String>) -> Self {
        Self {
            uuid,
            name,
            description: description.into(),
        }
    }

    property!(
        /// Returns the name.
        ref name: FileProofName, set_name
    );
    property!(
        /// Returns the description.
        ref description: String, set_description
    );

    /// Returns `name (description)` or just the name if there is no
    /// description.
    pub fn display_text(&self) -> String {
        if self.description.is_empty() {
            self.name.to_string()
        } else {
            format!("{} ({})", self.name, self.description)
        }
    }
}

impl_uuid!(AssemblyVariant);

impl HasName for AssemblyVariant {
    fn name(&self) -> &str {
        self.name.as_str()
    }
}

impl SerializeObject for AssemblyVariant {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("name", &self.name);
        root.ensure_line_break();
        root.append_child("description", &self.description);
        root.ensure_line_break();
    }
}

impl DeserializeObject for AssemblyVariant {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            name: node.child_value("name/@0")?,
            description: node.child_value("description/@0")?,
        })
    }
}

object_list!(
    /// The assembly variants of a circuit, in user defined order (the first
    /// one is the default).
    AssemblyVariantList,
    AssemblyVariantListTag,
    AssemblyVariant,
    "variant"
);
