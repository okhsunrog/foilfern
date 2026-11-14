//! Port of libs/librepcb/core/library (libraries and library elements).
//!
//! # Element design
//!
//! Upstream uses a class hierarchy (`LibraryBaseElement` → `LibraryElement`
//! → `Symbol`, ..., and `LibraryBaseElement` → `LibraryCategory`,
//! `Organization`, `Library`). Here the common attributes are plain data
//! structs embedded in each element, and the common behavior are traits:
//!
//! - [`BaseMetadata`] (UUID, version, author, names, message approvals, ...)
//!   and the trait [`LibraryBaseElement`], implemented by every element and
//!   by [`Library`]. The trait provides [`open()`](LibraryBaseElement::open),
//!   [`save()`](LibraryBaseElement::save), `save_to()`/`move_to()` etc. on
//!   top of the element specific hooks
//!   [`load()`](LibraryBaseElement::load),
//!   [`SerializeObject`](crate::serialization::SerializeObject) (the file
//!   content) and [`run_checks()`](LibraryBaseElement::run_checks).
//! - [`ElementMetadata`] (embeds [`BaseMetadata`], adds `generated_by`,
//!   categories and resources) and the trait [`LibraryElement`] for
//!   symbols, packages, components and devices.
//!
//! A new element type (e.g. a package) embeds `metadata: ElementMetadata`
//! and `directory: TransactionalDirectory`, implements
//! [`LibraryBaseElement`] (using `element_accessors!()` for the accessors),
//! [`LibraryElement`] (`impl_library_element!`) and `SerializeObject`, and
//! runs [`run_element_checks()`] before its own checks, adding its messages
//! as a new [`LibraryCheckMessage`] variant.
//!
//! # Checks
//!
//! The upstream check classes are functions (`run_*_checks()`), and the
//! message classes are enum variants (see [`LibraryCheckMessage`]).
//!
//! # Not ported
//!
//! - Change signals (`onEdited`, Qt signals), see the `// upstream: emits`
//!   notes.
//! - UI helpers (`getIconAsPixmap()`, `SymbolPainter`, ...).

pub mod cat;
pub mod cmp;
pub mod dev;
mod error;
#[allow(clippy::module_inception)] // Upstream file name.
mod library;
mod library_base_element;
mod library_base_element_check;
mod library_base_element_check_messages;
mod library_check_message;
mod library_element;
mod library_element_check;
mod library_element_check_messages;
pub mod org;
pub mod pkg;
mod resource;
pub mod sym;

pub use error::{Error, Result};
pub use library::Library;
pub use library_base_element::{
    BaseMetadata, LibraryBaseElement, read_file_format, save_element_files,
};
pub use library_base_element_check::run_base_element_checks;
pub use library_base_element_check_messages::{
    LibraryBaseElementCheckMessage, is_title_case, title_case_fixed_name,
};
pub use library_check_message::LibraryCheckMessage;
pub use library_element::{ElementMetadata, LibraryElement};
pub(crate) use library_element::{element_accessors, impl_library_element};
pub use library_element_check::run_element_checks;
pub use library_element_check_messages::LibraryElementCheckMessage;
pub use resource::{Resource, ResourceList, ResourceListTag};

/// Compares strings like `QCollator` in numeric mode, case insensitive
/// (upstream `Toolbox::sortNumeric()`): digit sequences are compared by
/// their numeric value ("A2" < "A10").
///
/// Hand-written on top of the `alphanumeric-sort` crate; unlike ICU, other
/// characters are compared by code point (see COMPAT.md).
pub(crate) fn natural_cmp_case_insensitive(a: &str, b: &str) -> std::cmp::Ordering {
    alphanumeric_sort::compare_str(a.to_lowercase(), b.to_lowercase())
}
