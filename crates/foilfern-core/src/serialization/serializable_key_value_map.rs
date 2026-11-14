//! Port of libs/librepcb/core/serialization/serializablekeyvaluemap.h.
//!
//! The map always contains a default value (with the empty key), which is
//! used as fallback by [`SerializableKeyValueMap::value()`].
//!
//! Differences to upstream: the `onEdited` signal is not ported, and keys are
//! ordered by Rust string comparison (identical to `QString` ordering for
//! the locale codes used as keys).

use std::collections::BTreeMap;
use std::fmt;

use super::{Error, FromSExpression, List, Result, SExpression, SerializeObject, ToSExpression};
use crate::types::ElementName;

/// Configuration of a [`SerializableKeyValueMap`] (upstream template
/// parameter `T`).
pub trait KeyValueMapPolicy {
    /// Type of the values.
    type Value: ToSExpression + FromSExpression + Clone + PartialEq;
    /// Name of the child lists holding the entries, e.g. `"name"`.
    const TAG_NAME: &'static str;
    /// Name of the child list holding the key, e.g. `"locale"`.
    const KEY_NAME: &'static str;
}

/// An ordered key-value map which always contains a default value (empty
/// key), serialized e.g. as `(name "Foo") (name (locale "de_DE") "Bar")`.
pub struct SerializableKeyValueMap<P: KeyValueMapPolicy> {
    values: BTreeMap<String, P::Value>,
}

impl<P: KeyValueMapPolicy> SerializableKeyValueMap<P> {
    /// Creates a map containing only the default value.
    pub fn new(default_value: P::Value) -> Self {
        Self {
            values: BTreeMap::from([(String::new(), default_value)]),
        }
    }

    /// Loads the map from all children of `node` named
    /// [`P::TAG_NAME`](KeyValueMapPolicy::TAG_NAME).
    ///
    /// Fails if a key is defined multiple times or no default value exists.
    pub fn deserialize(node: &SExpression) -> Result<Self> {
        let mut values = BTreeMap::new();
        for child in node.children_named(P::TAG_NAME) {
            let (key, value) = if child.required_child("@0")?.is_list() {
                let key = child
                    .required_child(&format!("{}/@0", P::KEY_NAME))?
                    .value()?;
                (key.to_owned(), child.required_child("@1")?)
            } else {
                (String::new(), child.required_child("@0")?)
            };
            if values.contains_key(&key) {
                return Err(Error::DuplicateKey(key));
            }
            values.insert(key, P::Value::from_sexpression(value)?);
        }
        if !values.contains_key("") {
            return Err(Error::NoDefaultValue(P::TAG_NAME));
        }
        Ok(Self { values })
    }

    /// Returns all keys in ascending order (the default key `""` first).
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.values.keys().map(String::as_str)
    }

    /// Returns all entries in ascending key order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &P::Value)> {
        self.values.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Returns the default value.
    pub fn default_value(&self) -> &P::Value {
        // Invariant: the default value always exists (guaranteed by all
        // constructors, and no method removes entries).
        &self.values[""]
    }

    /// Returns whether a value for `key` exists.
    pub fn contains(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    /// Returns the value for `key`, if any.
    pub fn get(&self, key: &str) -> Option<&P::Value> {
        self.values.get(key)
    }

    /// Returns the value of the first key in `key_order` which exists, or the
    /// default value as fallback.
    pub fn value<S: AsRef<str>>(&self, key_order: &[S]) -> &P::Value {
        self.value_with_key(key_order).1
    }

    /// Same as [`value()`](Self::value), but also returns the key which was
    /// used (`""` for the default value).
    pub fn value_with_key<S: AsRef<str>>(&self, key_order: &[S]) -> (&str, &P::Value) {
        key_order
            .iter()
            .find_map(|key| self.values.get_key_value(key.as_ref()))
            .map(|(k, v)| (k.as_str(), v))
            .unwrap_or(("", self.default_value()))
    }

    /// Sets the default value.
    pub fn set_default_value(&mut self, value: P::Value) {
        self.insert("", value);
    }

    /// Inserts or replaces the value for `key`. Returns whether the map was
    /// modified.
    pub fn insert(&mut self, key: impl Into<String>, value: P::Value) -> bool {
        let key = key.into();
        if self.values.get(&key) == Some(&value) {
            return false;
        }
        self.values.insert(key, value);
        true
    }
}

impl<P: KeyValueMapPolicy> SerializeObject for SerializableKeyValueMap<P> {
    fn serialize(&self, root: &mut List) {
        for (key, value) in &self.values {
            root.ensure_line_break();
            let child = root.append_list(P::TAG_NAME);
            if !key.is_empty() {
                child.append_child(P::KEY_NAME, key);
            }
            child.append_value(value);
        }
        root.ensure_line_break();
    }
}

impl<P: KeyValueMapPolicy> Clone for SerializableKeyValueMap<P> {
    fn clone(&self) -> Self {
        Self {
            values: self.values.clone(),
        }
    }
}

impl<P: KeyValueMapPolicy> PartialEq for SerializableKeyValueMap<P> {
    fn eq(&self, other: &Self) -> bool {
        self.values == other.values
    }
}

impl<P: KeyValueMapPolicy> fmt::Debug for SerializableKeyValueMap<P>
where
    P::Value: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.values.iter()).finish()
    }
}

/// Policy of [`LocalizedNameMap`].
#[derive(Debug, Clone, Copy)]
pub struct LocalizedNameMapPolicy;

impl KeyValueMapPolicy for LocalizedNameMapPolicy {
    type Value = ElementName;
    const TAG_NAME: &'static str = "name";
    const KEY_NAME: &'static str = "locale";
}

/// Localized names of library elements.
pub type LocalizedNameMap = SerializableKeyValueMap<LocalizedNameMapPolicy>;

/// Policy of [`LocalizedDescriptionMap`].
#[derive(Debug, Clone, Copy)]
pub struct LocalizedDescriptionMapPolicy;

impl KeyValueMapPolicy for LocalizedDescriptionMapPolicy {
    type Value = String;
    const TAG_NAME: &'static str = "description";
    const KEY_NAME: &'static str = "locale";
}

/// Localized descriptions of library elements.
pub type LocalizedDescriptionMap = SerializableKeyValueMap<LocalizedDescriptionMapPolicy>;

/// Policy of [`LocalizedKeywordsMap`].
#[derive(Debug, Clone, Copy)]
pub struct LocalizedKeywordsMapPolicy;

impl KeyValueMapPolicy for LocalizedKeywordsMapPolicy {
    type Value = String;
    const TAG_NAME: &'static str = "keywords";
    const KEY_NAME: &'static str = "locale";
}

/// Localized keywords of library elements.
pub type LocalizedKeywordsMap = SerializableKeyValueMap<LocalizedKeywordsMapPolicy>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialization::Mode;

    #[test]
    fn roundtrip() {
        let input = b"(root\n (name \"Foo\")\n (name (locale \"de_DE\") \"Bar\")\n)\n";
        let node = SExpression::parse(input, None, Mode::LibrePcb).unwrap();
        let map = LocalizedNameMap::deserialize(&node).unwrap();
        assert_eq!(map.default_value().as_str(), "Foo");
        assert_eq!(map.value(&["de_CH", "de_DE"]).as_str(), "Bar");
        assert_eq!(map.value_with_key(&["fr"]), ("", map.default_value()));
        let mut root = List::new("root");
        map.serialize(&mut root);
        let out = SExpression::from(root)
            .to_byte_array(Mode::LibrePcb)
            .unwrap();
        assert_eq!(out, input);
    }

    #[test]
    fn errors() {
        let parse = |s: &str| SExpression::parse(s.as_bytes(), None, Mode::LibrePcb).unwrap();
        assert!(matches!(
            LocalizedDescriptionMap::deserialize(&parse("(root)")),
            Err(Error::NoDefaultValue("description"))
        ));
        assert!(matches!(
            LocalizedDescriptionMap::deserialize(&parse(
                "(root (description \"a\") (description \"b\"))"
            )),
            Err(Error::DuplicateKey(key)) if key.is_empty()
        ));
    }

    #[test]
    fn insert() {
        let mut map = LocalizedKeywordsMap::new("a".into());
        assert!(!map.insert("", "a".into()));
        assert!(map.insert("de", "b".into()));
        assert_eq!(map.keys().collect::<Vec<_>>(), ["", "de"]);
    }
}
