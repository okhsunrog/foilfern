//! Port of libs/librepcb/core/project/board/drc (the board design rule
//! check).
//!
//! - [`BoardDrcData`]: the input data, a snapshot extracted from a project
//!   and one of its boards (upstream `BoardDesignRuleCheckData`), so the
//!   check can run in other threads without access to the project.
//! - [`run_drc()`]: runs all checks on the data and returns a [`DrcResult`]
//!   (upstream `BoardDesignRuleCheck`);
//!   [`Project::run_drc()`](crate::project::Project::run_drc) rebuilds the
//!   air wires, extracts the data and runs the checks.
//! - [`DrcMessage`]: the messages (upstream `DrcMsg*` classes), with the
//!   upstream texts and approvals.
//! - [`BoardClipperPathGenerator`]: the copper and stop mask areas used by
//!   the checks.
//!
//! The DRC settings (upstream `BoardDesignRuleCheckSettings`) live in the
//! library module because organizations provide them too
//! ([`BoardDesignRuleCheckSettings`](crate::library::org::BoardDesignRuleCheckSettings)).

mod check;
mod clipper_path_generator;
mod data;
mod error;
mod messages;

pub use check::{DrcProgress, DrcProgressCallback, DrcResult, run_drc};
pub use clipper_path_generator::BoardClipperPathGenerator;
pub use data::{
    BoardDrcData, DrcAirWire, DrcAirWireAnchor, DrcAirWireObject, DrcCircle, DrcDevice, DrcHole,
    DrcImpossibleConnection, DrcJunction, DrcNetClass, DrcPad, DrcPlane, DrcPolygon, DrcSegment,
    DrcStrokeText, DrcTrace, DrcVia, DrcZone,
};
pub use error::{Error, Result};
pub use messages::{DrcMessage, DrcMessageKind};
