//! Port of libs/librepcb/core/library/cat/librarycategorycheck.{h,cpp}.

use super::library_category_check_messages::LibraryCategoryCheckMessage;
use crate::library::{BaseMetadata, LibraryCheckMessage, run_base_element_checks};
use crate::types::Uuid;

/// Runs the category check, including the checks common to all elements
/// (upstream `LibraryCategoryCheck::runChecks()`).
pub fn run_category_checks(
    metadata: &BaseMetadata,
    parent_uuid: Option<Uuid>,
    msgs: &mut Vec<LibraryCheckMessage>,
) {
    run_base_element_checks(metadata, msgs);
    check_parent(metadata, parent_uuid, msgs);
}

fn check_parent(
    metadata: &BaseMetadata,
    parent_uuid: Option<Uuid>,
    msgs: &mut Vec<LibraryCheckMessage>,
) {
    if parent_uuid == Some(metadata.uuid()) {
        msgs.push(LibraryCategoryCheckMessage::InvalidParent.into());
    }
}
