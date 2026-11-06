//! Port of libs/librepcb/core/project/board/drc (board design rule checks).
//!
//! Rule groups run synchronously on owned, thread-safe snapshots. Geometry,
//! messages and approvals follow the upstream source; the existing native
//! path/Clipper implementations preserve file-relevant geometric decisions.

mod copper_clearances;
mod copper_messages;
mod cutouts;
mod data;
mod drill_clearances;
mod keepout;
mod manufacturing;
mod messages;
mod outline_rules;
mod paths;
mod physical_data;
mod placement;
mod structural;
mod widths;

pub use data::BoardDrcData;
pub use messages::{
    BoardDrcMessage, DrcCopperRef, DrcHoleRef, DrcMessageKind, DrcObjectRef, DrcPadRef,
};

/// Board rule checker, independent of editor state and worker scheduling.
#[derive(Debug, Clone, Copy)]
pub struct BoardDesignRuleCheck;
