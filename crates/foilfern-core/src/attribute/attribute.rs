//! Port of libs/librepcb/core/attribute/attribute.{h,cpp}.

use super::{AttributeKey, AttributeType, AttributeUnit, Error};
use crate::serialization::{
    self, DeserializeObject, HasName, List, ListTagName, SExpression, SerializableObjectList,
    SerializeObject,
};

/// An attribute: a key with a typed value and an optional unit, e.g.
/// `VOLTAGE = 4.2 V`.
///
/// The unit is always available for the type and the value always valid for
/// the type (see [`AttributeType::is_unit_available()`] and
/// [`AttributeType::is_value_valid()`]).
///
/// Serialized as `(attribute "KEY" (type voltage) (unit volt) (value "4.2"))`.
///
/// Serde: `{"key": "KEY", "type": "voltage", "unit": "volt", "value": "4.2"}`
/// (`unit` is `null` for types without units), validated when
/// deserializing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "AttributeRepr", into = "AttributeRepr")]
pub struct Attribute {
    key: AttributeKey,
    attribute_type: AttributeType,
    value: String,
    unit: Option<&'static AttributeUnit>,
}

/// Serde representation of [`Attribute`].
#[derive(serde::Serialize, serde::Deserialize)]
struct AttributeRepr {
    key: AttributeKey,
    #[serde(rename = "type")]
    attribute_type: AttributeType,
    unit: Option<String>,
    value: String,
}

impl From<Attribute> for AttributeRepr {
    fn from(a: Attribute) -> Self {
        Self {
            key: a.key,
            attribute_type: a.attribute_type,
            unit: a.unit.map(|u| u.name().to_owned()),
            value: a.value,
        }
    }
}

impl TryFrom<AttributeRepr> for Attribute {
    type Error = Error;
    fn try_from(r: AttributeRepr) -> Result<Self, Error> {
        let unit = r
            .attribute_type
            .unit_from_string(r.unit.as_deref().unwrap_or("none"))?;
        Self::new(r.key, r.attribute_type, r.value, unit)
    }
}

impl Attribute {
    /// Creates an attribute, or returns an error if `unit` is not available
    /// for `attribute_type` or `value` is not valid for it.
    pub fn new(
        key: AttributeKey,
        attribute_type: AttributeType,
        value: impl Into<String>,
        unit: Option<&'static AttributeUnit>,
    ) -> Result<Self, Error> {
        let value = value.into();
        check(attribute_type, &value, unit)?;
        Ok(Self {
            key,
            attribute_type,
            value,
            unit,
        })
    }

    /// Returns the key.
    pub fn key(&self) -> &AttributeKey {
        &self.key
    }

    /// Returns the type.
    pub fn attribute_type(&self) -> AttributeType {
        self.attribute_type
    }

    /// Returns the unit (`None` for types without units).
    pub fn unit(&self) -> Option<&'static AttributeUnit> {
        self.unit
    }

    /// Returns the raw value.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Returns the value as displayed to the user, optionally with the unit
    /// symbol (see [`AttributeType::printable_value_tr()`]).
    pub fn value_tr(&self, show_unit: bool) -> String {
        self.attribute_type
            .printable_value_tr(&self.value, self.unit.filter(|_| show_unit))
    }

    /// Sets the key. Returns whether it was changed.
    pub fn set_key(&mut self, key: AttributeKey) -> bool {
        if key == self.key {
            return false;
        }
        self.key = key;
        // upstream: emits onEdited(KeyChanged)
        true
    }

    /// Sets type, value and unit together. Returns whether anything was
    /// changed, or an error (without modifying `self`) if the combination
    /// is invalid.
    pub fn set_type_value_unit(
        &mut self,
        attribute_type: AttributeType,
        value: impl Into<String>,
        unit: Option<&'static AttributeUnit>,
    ) -> Result<bool, Error> {
        let value = value.into();
        if attribute_type == self.attribute_type && value == self.value && unit == self.unit {
            return Ok(false);
        }
        check(attribute_type, &value, unit)?;
        self.attribute_type = attribute_type;
        self.value = value;
        self.unit = unit;
        // upstream: emits onEdited(TypeValueUnitChanged)
        Ok(true)
    }
}

/// Validates a type/value/unit combination (upstream
/// `checkAttributesValidity()`).
fn check(
    attribute_type: AttributeType,
    value: &str,
    unit: Option<&'static AttributeUnit>,
) -> Result<(), Error> {
    if attribute_type.is_unit_available(unit) && attribute_type.is_value_valid(value) {
        Ok(())
    } else {
        Err(Error::InvalidTypeValueUnit {
            type_name: attribute_type.name(),
            value: value.to_owned(),
            unit: unit.map_or("-", AttributeUnit::name),
        })
    }
}

impl SerializeObject for Attribute {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.key);
        root.append_child("type", &self.attribute_type);
        match self.unit {
            Some(unit) => root.append_child("unit", unit),
            None => root.append_child("unit", &SExpression::token("none")),
        };
        root.append_child("value", &self.value);
    }
}

impl DeserializeObject for Attribute {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        let key = node.child_value("@0")?;
        let attribute_type: AttributeType = node.child_value("type/@0")?;
        let value: String = node.child_value("value/@0")?;
        let unit = attribute_type.unit_from_string(node.required_child("unit/@0")?.value()?)?;
        Ok(Self::new(key, attribute_type, value, unit)?)
    }
}

impl HasName for Attribute {
    /// Returns the key (required for lookups by name in [`AttributeList`]).
    fn name(&self) -> &str {
        self.key.as_str()
    }
}

/// Tag name provider of [`AttributeList`].
#[derive(Debug, Clone, Copy)]
pub struct AttributeListTag;

impl ListTagName for AttributeListTag {
    const TAG_NAME: &'static str = "attribute";
}

/// A list of attributes, serialized as `(attribute ...)` children.
pub type AttributeList = SerializableObjectList<Attribute, AttributeListTag>;

static_assertions::assert_impl_all!(AttributeList: Send, Sync);

impl AttributeList {
    /// Returns `self` extended by all attributes of `other` whose key is
    /// not contained in `self` yet (upstream `operator|`).
    pub fn merged_with(&self, other: &AttributeList) -> AttributeList {
        let mut result = self.clone();
        for attribute in other {
            if !result.contains_name(attribute.key()) {
                result.push(attribute.clone());
            }
        }
        result
    }
}
