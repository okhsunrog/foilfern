//! Port of libs/librepcb/core/library/libraryelement.{h,cpp}.
//!
//! Upstream `LibraryElement` extends `LibraryBaseElement` by the attributes
//! common to symbols, packages, components and devices (but not categories,
//! organizations and libraries). Here these are [`ElementMetadata`], which
//! embeds the [`BaseMetadata`], and the [`LibraryElement`] trait.

use std::collections::BTreeSet;

use super::library_base_element::{BaseMetadata, LibraryBaseElement};
use super::resource::ResourceList;
use crate::geometry::property;
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::Uuid;

/// Attributes common to symbols, packages, components and devices (upstream
/// `LibraryElement` members, plus the embedded [`BaseMetadata`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ElementMetadata {
    base: BaseMetadata,
    generated_by: String,
    categories: BTreeSet<Uuid>,
    resources: ResourceList,
}

impl ElementMetadata {
    /// Creates the metadata of a new element (not generated, without
    /// categories and resources).
    pub fn new(base: BaseMetadata) -> Self {
        Self {
            base,
            generated_by: String::new(),
            categories: BTreeSet::new(),
            resources: ResourceList::new(),
        }
    }

    /// Returns the common attributes.
    pub fn base(&self) -> &BaseMetadata {
        &self.base
    }

    /// Returns the common attributes for modification.
    pub fn base_mut(&mut self) -> &mut BaseMetadata {
        &mut self.base
    }

    property!(
        /// Returns the generator of the element (empty if the element is not
        /// generated).
        ref generated_by: String, set_generated_by
    );
    property!(
        /// Returns the UUIDs of the categories the element belongs to.
        ref categories: BTreeSet<Uuid>, set_categories
    );
    property!(
        /// Returns the resources (e.g. datasheets).
        ref resources: ResourceList, set_resources
    );

    /// Copies all attributes except the UUID from `other` (upstream part of
    /// `duplicateFrom()`).
    pub fn duplicate_from(&mut self, other: &ElementMetadata) {
        self.base.duplicate_from(&other.base);
        self.generated_by = other.generated_by.clone();
        self.categories = other.categories.clone();
        self.resources = other.resources.clone();
    }
}

impl SerializeObject for ElementMetadata {
    /// Serializes the attributes (without message approvals, see
    /// [`BaseMetadata::serialize_message_approvals()`]).
    fn serialize(&self, root: &mut List) {
        self.base.serialize(root);
        root.ensure_line_break();
        root.append_child("generated_by", &self.generated_by);
        for uuid in &self.categories {
            root.ensure_line_break();
            root.append_child("category", uuid);
        }
        root.ensure_line_break();
        self.resources.serialize(root);
        root.ensure_line_break();
    }
}

impl DeserializeObject for ElementMetadata {
    fn deserialize(root: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            base: BaseMetadata::deserialize(root)?,
            generated_by: root.child_value("generated_by/@0")?,
            categories: root
                .children_named("category")
                .map(|node| node.child_value("@0"))
                .collect::<serialization::Result<_>>()?,
            resources: ResourceList::deserialize(root)?,
        })
    }
}

/// Library elements with categories and resources (symbols, packages,
/// components, devices).
pub trait LibraryElement: LibraryBaseElement {
    /// Returns the element attributes.
    fn element_metadata(&self) -> &ElementMetadata;
    /// Returns the element attributes for modification.
    fn element_metadata_mut(&mut self) -> &mut ElementMetadata;
}

/// Implements [`LibraryBaseElement`]'s accessors and [`LibraryElement`] for
/// an element type with the fields `metadata: ElementMetadata` and
/// `directory: TransactionalDirectory`.
macro_rules! impl_library_element {
    ($ty:ty) => {
        impl $crate::library::LibraryElement for $ty {
            fn element_metadata(&self) -> &$crate::library::ElementMetadata {
                &self.metadata
            }
            fn element_metadata_mut(&mut self) -> &mut $crate::library::ElementMetadata {
                &mut self.metadata
            }
        }
    };
}

/// Implements the accessors of [`LibraryBaseElement`] (to be used inside
/// the `impl LibraryBaseElement for ...` block) for element types with the
/// fields `metadata: ElementMetadata` and `directory: TransactionalDirectory`.
macro_rules! element_accessors {
    () => {
        fn metadata(&self) -> &$crate::library::BaseMetadata {
            self.metadata.base()
        }
        fn metadata_mut(&mut self) -> &mut $crate::library::BaseMetadata {
            self.metadata.base_mut()
        }
        fn directory(&self) -> &$crate::fileio::TransactionalDirectory {
            &self.directory
        }
        fn directory_mut(&mut self) -> &mut $crate::fileio::TransactionalDirectory {
            &mut self.directory
        }
    };
}

pub(crate) use element_accessors;
pub(crate) use impl_library_element;
