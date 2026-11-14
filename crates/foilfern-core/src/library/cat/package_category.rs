//! Port of libs/librepcb/core/library/cat/packagecategory.{h,cpp}.

use super::library_category::{CategoryKind, LibraryCategory};

/// [`CategoryKind`] of [`PackageCategory`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackageCategoryKind;

impl CategoryKind for PackageCategoryKind {
    const SHORT_ELEMENT_NAME: &'static str = "pkgcat";
    const LONG_ELEMENT_NAME: &'static str = "package_category";
}

/// A category of packages.
pub type PackageCategory = LibraryCategory<PackageCategoryKind>;

static_assertions::assert_impl_all!(PackageCategory: Send, Sync);
