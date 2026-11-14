//! Port of tests/unittests/core/serialization/serializableobjectmock.h.

use foilfern_core::serialization::{
    DeserializeObject, HasName, HasUuid, List, Result, SExpression, SerializeObject,
};
use foilfern_core::types::Uuid;

/// Minimal element type (not clonable, not comparable).
pub struct MinimalSerializableObjectMock {
    pub value: String,
}

impl MinimalSerializableObjectMock {
    pub fn new(value: &str) -> Self {
        Self {
            value: value.to_owned(),
        }
    }
}

impl DeserializeObject for MinimalSerializableObjectMock {
    fn deserialize(node: &SExpression) -> Result<Self> {
        Ok(Self {
            value: node.child_value("@0")?,
        })
    }
}

impl SerializeObject for MinimalSerializableObjectMock {
    fn serialize(&self, root: &mut List) {
        root.ensure_line_break();
        root.append_child("value", &self.value);
        root.ensure_line_break();
    }
}

/// Element type with UUID and name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerializableObjectMock {
    pub uuid: Uuid,
    pub name: String,
}

impl SerializableObjectMock {
    pub fn new(uuid: Uuid, name: &str) -> Self {
        Self {
            uuid,
            name: name.to_owned(),
        }
    }
}

impl HasUuid for SerializableObjectMock {
    fn uuid(&self) -> Uuid {
        self.uuid
    }
}

impl HasName for SerializableObjectMock {
    fn name(&self) -> &str {
        &self.name
    }
}

impl DeserializeObject for SerializableObjectMock {
    fn deserialize(node: &SExpression) -> Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            name: node.child_value("name/@0")?,
        })
    }
}

impl SerializeObject for SerializableObjectMock {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.ensure_line_break();
        root.append_child("name", &self.name);
        root.ensure_line_break();
    }
}
