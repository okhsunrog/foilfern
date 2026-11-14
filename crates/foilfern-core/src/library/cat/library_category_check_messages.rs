//! Port of libs/librepcb/core/library/cat/librarycategorycheckmessages.{h,cpp}.

use foilfern_i18n::tr;

use crate::rule_check::{RuleCheckMessage, Severity};

/// Messages of the category check (upstream `Msg*` classes in
/// librarycategorycheckmessages.h).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LibraryCategoryCheckMessage {
    /// The category is its own parent (upstream `MsgInvalidParent`).
    InvalidParent,
}

impl LibraryCategoryCheckMessage {
    /// Returns the generic rule check message.
    pub fn to_message(&self) -> RuleCheckMessage {
        match self {
            Self::InvalidParent => RuleCheckMessage::new(
                Severity::Error,
                tr!("MsgInvalidParent", "Invalid parent category"),
                tr!(
                    "MsgInvalidParent",
                    "The category has assigned itself as its parent category, which \
                     leads to an endless recursion and is thus invalid. Assign a \
                     different parent category."
                ),
                "invalid_parent",
                Vec::new(),
            ),
        }
    }
}
