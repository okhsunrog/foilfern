//! Port of libs/librepcb/core/library/cat (component and package
//! categories).

mod component_category;
mod library_category;
mod library_category_check;
mod library_category_check_messages;
mod package_category;

pub use component_category::{ComponentCategory, ComponentCategoryKind};
pub use library_category::{CategoryKind, LibraryCategory};
pub use library_category_check::run_category_checks;
pub use library_category_check_messages::LibraryCategoryCheckMessage;
pub use package_category::{PackageCategory, PackageCategoryKind};
