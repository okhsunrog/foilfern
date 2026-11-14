//! Port of libs/librepcb/core/project/erc (electrical rule check).
//!
//! [`run_erc()`] checks a [`Project`](crate::project::Project) and returns
//! [`ErcMessage`]s; their approvals are compared with
//! [`Project::erc_approvals()`](crate::project::Project::erc_approvals)
//! (stored in `circuit/erc.lp`).

mod electrical_rule_check;
mod electrical_rule_check_messages;

pub use electrical_rule_check::run_erc;
pub use electrical_rule_check_messages::{ErcMessage, ErcMessageKind};
