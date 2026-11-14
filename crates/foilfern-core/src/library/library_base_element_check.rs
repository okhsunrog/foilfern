//! Port of libs/librepcb/core/library/librarybaseelementcheck.{h,cpp}.
//!
//! The upstream check class hierarchy (`LibraryBaseElementCheck` →
//! `LibraryElementCheck` → `SymbolCheck`, ...) is replaced by functions
//! appending their messages to a list; each element's
//! [`run_checks()`](super::LibraryBaseElement::run_checks) calls the
//! functions of its "base classes" first.

use super::library_base_element::BaseMetadata;
use super::library_base_element_check_messages::{
    LibraryBaseElementCheckMessage as Msg, is_title_case,
};
use super::library_check_message::LibraryCheckMessage;

/// Runs the checks common to all library elements (upstream
/// `LibraryBaseElementCheck::runChecks()`).
pub fn run_base_element_checks(metadata: &BaseMetadata, msgs: &mut Vec<LibraryCheckMessage>) {
    check_default_name_title_case(metadata, msgs);
    check_missing_author(metadata, msgs);
}

fn check_default_name_title_case(metadata: &BaseMetadata, msgs: &mut Vec<LibraryCheckMessage>) {
    let name = metadata.name();
    if !is_title_case(name) {
        msgs.push(Msg::NameNotTitleCase { name: name.clone() }.into());
    }
}

fn check_missing_author(metadata: &BaseMetadata, msgs: &mut Vec<LibraryCheckMessage>) {
    if metadata.author().trim().is_empty() {
        msgs.push(Msg::MissingAuthor.into());
    }
}
