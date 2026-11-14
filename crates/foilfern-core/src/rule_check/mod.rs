//! Port of libs/librepcb/core/rulecheck (messages of the library element
//! checks and electrical rule checks, and later of design rule checks).

mod rule_check_message;

pub use rule_check_message::{RuleCheckMessage, Severity, all_approvals};
