//! Port of libs/librepcb/core/library/dev/part.{h,cpp}.

use crate::attribute::AttributeList;
use crate::geometry::{object_list, property};
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::SimpleString;

/// An orderable part (manufacturer part number) of a device.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Part {
    mpn: SimpleString,
    manufacturer: SimpleString,
    attributes: AttributeList,
}

impl Part {
    /// Creates a part.
    pub fn new(mpn: SimpleString, manufacturer: SimpleString, attributes: AttributeList) -> Self {
        Self {
            mpn,
            manufacturer,
            attributes,
        }
    }

    /// Returns whether MPN, manufacturer and attributes are empty.
    pub fn is_empty(&self) -> bool {
        self.mpn.is_empty() && self.manufacturer.is_empty() && self.attributes.is_empty()
    }

    property!(
        /// Returns the manufacturer part number.
        ref mpn: SimpleString, set_mpn
    );
    property!(
        /// Returns the manufacturer.
        ref manufacturer: SimpleString, set_manufacturer
    );

    /// Returns the attributes.
    pub fn attributes(&self) -> &AttributeList {
        &self.attributes
    }

    /// Returns the attributes for modification.
    pub fn attributes_mut(&mut self) -> &mut AttributeList {
        // upstream: emits onEdited(AttributesEdited) on modifications
        &mut self.attributes
    }

    /// Returns the non-empty translated attribute values (with unit).
    pub fn attribute_values_tr(&self) -> Vec<String> {
        self.attributes
            .iter()
            .map(|a| a.value_tr(true).trim().to_owned())
            .filter(|v| !v.is_empty())
            .collect()
    }

    /// Returns `KEY=value` for all attributes (translated values with unit).
    pub fn attribute_key_values_tr(&self) -> Vec<String> {
        self.attributes
            .iter()
            .map(|a| format!("{}={}", a.key(), a.value_tr(true)))
            .collect()
    }
}

object_list!(
    /// List of [`Part`]s.
    PartList,
    PartListTag,
    Part,
    "part"
);

impl SerializeObject for Part {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.mpn);
        root.append_child("manufacturer", &self.manufacturer);
        root.ensure_line_break();
        self.attributes.serialize(root);
        root.ensure_line_break();
    }
}

impl DeserializeObject for Part {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            mpn: node.child_value("@0")?,
            manufacturer: node.child_value("manufacturer/@0")?,
            attributes: AttributeList::deserialize(node)?,
        })
    }
}
