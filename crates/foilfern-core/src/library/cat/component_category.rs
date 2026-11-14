//! Port of libs/librepcb/core/library/cat/componentcategory.{h,cpp}.

use super::library_category::{CategoryKind, LibraryCategory};

/// [`CategoryKind`] of [`ComponentCategory`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComponentCategoryKind;

impl CategoryKind for ComponentCategoryKind {
    const SHORT_ELEMENT_NAME: &'static str = "cmpcat";
    const LONG_ELEMENT_NAME: &'static str = "component_category";
}

/// A category of symbols, components and devices.
pub type ComponentCategory = LibraryCategory<ComponentCategoryKind>;

static_assertions::assert_impl_all!(ComponentCategory: Send, Sync);
