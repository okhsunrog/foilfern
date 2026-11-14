//! Port of libs/librepcb/core/library/libraryelementcheck.{h,cpp}.

use super::library_base_element_check::run_base_element_checks;
use super::library_check_message::LibraryCheckMessage;
use super::library_element::ElementMetadata;
use super::library_element_check_messages::LibraryElementCheckMessage;

/// Runs the checks common to symbols, packages, components and devices,
/// including [`run_base_element_checks()`] (upstream
/// `LibraryElementCheck::runChecks()`).
pub fn run_element_checks(metadata: &ElementMetadata, msgs: &mut Vec<LibraryCheckMessage>) {
    run_base_element_checks(metadata.base(), msgs);
    check_missing_categories(metadata, msgs);
}

fn check_missing_categories(metadata: &ElementMetadata, msgs: &mut Vec<LibraryCheckMessage>) {
    if metadata.categories().is_empty() {
        msgs.push(LibraryElementCheckMessage::MissingCategories.into());
    }
}
