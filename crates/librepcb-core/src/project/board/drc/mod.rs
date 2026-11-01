//! Port of libs/librepcb/core/project/board/drc (board design rule checks).
//!
//! Manufacturing checks run synchronously on owned, thread-safe snapshots.
//! Rule predicates and approvals follow the upstream source; geometry uses
//! the existing core path operations. Clearance/connectivity checks remain
//! unported, so the partial entry point is explicitly named accordingly.

mod data;
mod manufacturing;
mod messages;

pub use data::BoardDrcData;
pub use messages::{BoardDrcMessage, DrcHoleRef, DrcMessageKind};

/// Board rule checker, independent of editor state and worker scheduling.
#[derive(Debug, Clone, Copy)]
pub struct BoardDesignRuleCheck;
