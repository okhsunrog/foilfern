//! Snapshot preparation for the DRC, extracted from upstream
//! libs/librepcb/core/project/board/drc/boarddesignrulecheck.cpp (`start`).
//!
//! Plane calculation runs without borrowing the project. Applying its result,
//! rebuilding air wires and extracting check data form one exclusive operation;
//! a changed project rejects the entire preparation before any derived writes.

use super::{BoardDrcData, Error, Result};
use crate::library::org::BoardDesignRuleCheckSettings;
use crate::project::board::{PlaneFragments, PlaneJob};
use crate::project::{BoardId, EntityKind, Project};
use crate::types::Uuid;

/// Identity of the model from which a check was extracted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DrcSnapshot {
    project: Uuid,
    board: BoardId,
    revision: u64,
}

impl DrcSnapshot {
    pub(super) fn new(project: &Project, board: BoardId) -> Self {
        Self {
            project: project.uuid(),
            board,
            revision: project.revision(),
        }
    }

    pub(super) fn matches(self, project: &Project, board: BoardId) -> bool {
        self == Self::new(project, board)
    }
}

/// Owned preparation job; calculate planes off-thread with [`Self::run`].
#[derive(Debug)]
pub struct DrcJob {
    snapshot: DrcSnapshot,
    settings: BoardDesignRuleCheckSettings,
    quick: bool,
    planes: Option<PlaneJob>,
}

/// Calculated preparation, awaiting exclusive access to the original project.
#[derive(Debug)]
pub struct PreparedDrc {
    snapshot: DrcSnapshot,
    settings: BoardDesignRuleCheckSettings,
    quick: bool,
    planes: Option<PlaneFragments>,
}

static_assertions::assert_impl_all!(DrcJob: Send, Sync);
static_assertions::assert_impl_all!(PreparedDrc: Send, Sync);

impl Project {
    /// Captures a DRC preparation job. A full check always rebuilds all planes.
    pub fn drc_job(
        &self,
        board: BoardId,
        settings: Option<&BoardDesignRuleCheckSettings>,
        quick: bool,
    ) -> Result<DrcJob> {
        let b = self.board(board).ok_or(crate::project::Error::NotFound {
            kind: EntityKind::Board,
            uuid: board.0,
        })?;
        Ok(DrcJob {
            snapshot: DrcSnapshot::new(self, board),
            settings: settings.unwrap_or(&b.settings().drc_settings).clone(),
            quick,
            planes: if quick {
                None
            } else {
                self.plane_job(board, None)?
            },
        })
    }
}

impl DrcJob {
    /// Calculates plane fragments without accessing the project.
    pub fn run(self) -> PreparedDrc {
        PreparedDrc {
            snapshot: self.snapshot,
            settings: self.settings,
            quick: self.quick,
            planes: self.planes.map(PlaneJob::run),
        }
    }
}

impl PreparedDrc {
    /// Applies successful, current fragments and extracts a consistent snapshot.
    /// Stale, failed or incomplete plane jobs leave the project unchanged.
    /// Callers sharing a project must hold its lock for this whole operation.
    pub fn apply(self, project: &mut Project) -> Result<BoardDrcData> {
        let board = self.snapshot.board;
        if !self.snapshot.matches(project, board) {
            return Err(Error::StaleSnapshot);
        }
        if let Some(planes) = self.planes {
            planes.check()?;
            if !planes.finished {
                return Err(Error::IncompletePlaneRebuild);
            }
            project.apply_plane_job_result(planes)?;
        }
        if !self.quick {
            project.force_air_wires_rebuild(board)?;
        }
        BoardDrcData::new(project, board, &self.settings, self.quick)
    }
}
