//! Port of libs/librepcb/core/project/erc (electrical rule checks).
//!
//! The checker reads the project tree and reverse index. It has no cached
//! state, callbacks or model mutations. Message approvals preserve upstream
//! tokens, UUID fields, order and line breaks.

mod electrical_rule_check;
mod messages;

pub use electrical_rule_check::ElectricalRuleCheck;
pub use messages::{ElectricalRuleCheckMessage, ErcMessageKind};
