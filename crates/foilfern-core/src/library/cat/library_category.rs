//! Port of libs/librepcb/core/library/cat/librarycategory.{h,cpp}.
//!
//! The two upstream subclasses `ComponentCategory` and `PackageCategory`
//! differ only in their element names, so both are instances of the generic
//! [`LibraryCategory`], parametrized by a [`CategoryKind`].

use std::fmt;
use std::marker::PhantomData;

use super::library_category_check::run_category_checks;
use crate::fileio::TransactionalDirectory;
use crate::geometry::property;
use crate::library::{BaseMetadata, LibraryBaseElement, LibraryCheckMessage, Result};
use crate::serialization::{DeserializeObject, List, SExpression, SerializeObject};
use crate::types::Uuid;

/// Kind of a [`LibraryCategory`] (component or package category).
pub trait CategoryKind: Send + Sync + 'static {
    /// See [`LibraryBaseElement::SHORT_ELEMENT_NAME`].
    const SHORT_ELEMENT_NAME: &'static str;
    /// See [`LibraryBaseElement::LONG_ELEMENT_NAME`].
    const LONG_ELEMENT_NAME: &'static str;
}

/// A category of library elements, forming a tree via the parent category.
pub struct LibraryCategory<K: CategoryKind> {
    directory: TransactionalDirectory,
    metadata: BaseMetadata,
    parent_uuid: Option<Uuid>,
    kind: PhantomData<fn() -> K>,
}

impl<K: CategoryKind> LibraryCategory<K> {
    /// Creates a new category (without parent) in a temporary directory.
    pub fn new(metadata: BaseMetadata) -> Result<Self> {
        Ok(Self {
            directory: TransactionalDirectory::new_temporary()?,
            metadata,
            parent_uuid: None,
            kind: PhantomData,
        })
    }

    property!(
        /// Returns the UUID of the parent category (`None` for root
        /// categories).
        copy parent_uuid: Option<Uuid>, set_parent_uuid
    );

    /// Copies all attributes except the UUID from `other`, removing all
    /// files of this category (upstream `duplicateFrom()`).
    pub fn duplicate_from(&mut self, other: &Self) -> Result<()> {
        use crate::fileio::FileSystem;
        self.directory.remove_dir_recursively("")?;
        self.metadata.duplicate_from(&other.metadata);
        self.parent_uuid = other.parent_uuid;
        Ok(())
    }
}

impl<K: CategoryKind> fmt::Debug for LibraryCategory<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(K::LONG_ELEMENT_NAME)
            .field("directory", &self.directory)
            .field("metadata", &self.metadata)
            .field("parent_uuid", &self.parent_uuid)
            .finish()
    }
}

impl<K: CategoryKind> LibraryBaseElement for LibraryCategory<K> {
    const SHORT_ELEMENT_NAME: &'static str = K::SHORT_ELEMENT_NAME;
    const LONG_ELEMENT_NAME: &'static str = K::LONG_ELEMENT_NAME;

    fn metadata(&self) -> &BaseMetadata {
        &self.metadata
    }
    fn metadata_mut(&mut self) -> &mut BaseMetadata {
        &mut self.metadata
    }
    fn directory(&self) -> &TransactionalDirectory {
        &self.directory
    }
    fn directory_mut(&mut self) -> &mut TransactionalDirectory {
        &mut self.directory
    }

    fn load(directory: TransactionalDirectory, root: &SExpression) -> Result<Self> {
        Ok(Self {
            directory,
            metadata: BaseMetadata::deserialize(root)?,
            parent_uuid: root.child_value("parent/@0")?,
            kind: PhantomData,
        })
    }

    fn run_checks(&self) -> Result<Vec<LibraryCheckMessage>> {
        let mut msgs = Vec::new();
        run_category_checks(&self.metadata, self.parent_uuid, &mut msgs);
        Ok(msgs)
    }
}

impl<K: CategoryKind> SerializeObject for LibraryCategory<K> {
    fn serialize(&self, root: &mut List) {
        self.metadata.serialize(root);
        root.ensure_line_break();
        root.append_child("parent", &self.parent_uuid);
        root.ensure_line_break();
        self.metadata.serialize_message_approvals(root);
        root.ensure_line_break();
    }
}
