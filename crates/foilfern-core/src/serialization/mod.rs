//! Port of libs/librepcb/core/serialization (S-expression file format).
//!
//! # Serialization traits
//!
//! Upstream uses two mechanisms, which map to two pairs of traits here:
//!
//! - *Values* (lengths, UUIDs, enums, strings, ...) are converted from/to
//!   exactly one node with the free function templates `serialize<T>()` and
//!   `deserialize<T>()`. These are [`ToSExpression`] and [`FromSExpression`].
//! - *Objects* (points, alignments, library elements, ...) write their content
//!   into a list node created by the parent (`void serialize(SExpression&
//!   root) const`) and are constructed from such a node (`explicit
//!   T(const SExpression& node)`). These are [`SerializeObject`] and
//!   [`DeserializeObject`].
//!
//! Deserialization always operates on the *current* file format. Older file
//! formats are upgraded beforehand by the file format migrations
//! ([`FileFormatMigration`], [`file_format_migrations()`]), which operate on
//! the raw [`SExpression`] tree (see the mutating API of [`SExpression`] and
//! [`List`]).
//!
//! Not ported: `serialize<QUrl>()`/`deserialize<QUrl>()` (URL handling is
//! deferred to the workspace/library port) and the Qt signal/slot based
//! change notifications of the containers.

mod error;
mod file_format_migration;
mod primitives;
mod serializable_key_value_map;
mod serializable_object_list;
mod sexpression;

pub use error::{Error, ParseError, Result};
pub use file_format_migration::{
    FileFormatMigration, MigrationError, MigrationMessage, MigrationResult, MigrationSeverity,
    UnstableMigration, V01Migration, V1Migration, file_format_migrations,
};
pub use serializable_key_value_map::{
    KeyValueMapPolicy, LocalizedDescriptionMap, LocalizedDescriptionMapPolicy,
    LocalizedKeywordsMap, LocalizedKeywordsMapPolicy, LocalizedNameMap, LocalizedNameMapPolicy,
    SerializableKeyValueMap,
};
pub use serializable_object_list::{HasName, HasUuid, ListTagName, SerializableObjectList};
pub use sexpression::{List, Mode, SExpression};

/// Conversion of a value into a single S-expression node (upstream
/// `serialize<T>()` specializations).
pub trait ToSExpression {
    /// Returns the node representing `self`.
    fn to_sexpression(&self) -> SExpression;
}

/// Conversion of a single S-expression node into a value (upstream
/// `deserialize<T>()` specializations).
pub trait FromSExpression: Sized {
    /// Parses the value from `node` (typically a token or string).
    fn from_sexpression(node: &SExpression) -> Result<Self>;
}

/// Objects which serialize their content into a list node (upstream member
/// function `void serialize(SExpression& root) const`).
pub trait SerializeObject {
    /// Appends the content of `self` to `root`.
    fn serialize(&self, root: &mut List);
}

/// Objects which are constructed from a list node (upstream constructor
/// `explicit T(const SExpression& node)`).
pub trait DeserializeObject: Sized {
    /// Loads the object from `node`.
    fn deserialize(node: &SExpression) -> Result<Self>;
}

impl<T: ToSExpression + ?Sized> ToSExpression for &T {
    fn to_sexpression(&self) -> SExpression {
        (**self).to_sexpression()
    }
}

impl ToSExpression for SExpression {
    fn to_sexpression(&self) -> SExpression {
        self.clone()
    }
}

impl FromSExpression for SExpression {
    fn from_sexpression(node: &SExpression) -> Result<Self> {
        Ok(node.clone())
    }
}
