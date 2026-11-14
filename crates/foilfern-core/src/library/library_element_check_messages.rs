//! Port of libs/librepcb/core/library/libraryelementcheckmessages.{h,cpp}.

use foilfern_i18n::tr;

use crate::rule_check::{RuleCheckMessage, Severity};

/// Messages of the check common to symbols, packages, components and
/// devices (upstream `Msg*` classes in libraryelementcheckmessages.h).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LibraryElementCheckMessage {
    /// No categories are set (upstream `MsgMissingCategories`).
    MissingCategories,
}

impl LibraryElementCheckMessage {
    /// Returns the generic rule check message.
    pub fn to_message(&self) -> RuleCheckMessage {
        match self {
            Self::MissingCategories => RuleCheckMessage::new(
                Severity::Error,
                tr!("MsgMissingCategories", "No categories set"),
                tr!(
                    "MsgMissingCategories",
                    "It's very important to assign every library element to at least \
                     one category. Otherwise it will be very hard to find the element \
                     in the workspace library, so it's highly recommended to fix \
                     this."
                ),
                "missing_categories",
                Vec::new(),
            ),
        }
    }
}
