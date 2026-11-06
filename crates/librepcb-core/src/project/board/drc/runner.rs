//! Port of BoardDesignRuleCheck execution and Board::updateDrcMessageApprovals
//! in libs/librepcb/core/project/board/{drc/boarddesignrulecheck,board}.cpp.
//!
//! A synchronous callback replaces Qt's worker lifecycle. Errors/cancellation
//! return no partial result, so approval cleanup cannot consume a failed run.

use super::{BoardDesignRuleCheck, BoardDrcData, BoardDrcMessage};
use crate::project::{BoardId, BoardMutation, Error, Mutation, Project, Result};
use std::collections::BTreeSet;
use std::ops::ControlFlow;

/// Scope of a board DRC run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DrcMode {
    /// All rules, with fresh planes and air wires.
    #[default]
    Full,
    /// Interactive copper clearance/width checks, omitting planes and the
    /// other manufacturing, placement and connectivity rules like upstream.
    Quick,
}
/// Successful DRC output with its board and completeness for approval cleanup.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardDrcResult {
    board: BoardId,
    mode: DrcMode,
    messages: Vec<BoardDrcMessage>,
}
static_assertions::assert_impl_all!(BoardDrcResult:Send,Sync);
impl BoardDrcResult {
    /// Board the findings belong to.
    pub fn board(&self) -> BoardId {
        self.board
    }
    /// Full or quick run.
    pub fn mode(&self) -> DrcMode {
        self.mode
    }
    /// Whether rules were omitted; these results must not remove approvals.
    pub fn is_partial(&self) -> bool {
        self.mode == DrcMode::Quick
    }
    /// Findings, including those already approved in the project.
    pub fn messages(&self) -> &[BoardDrcMessage] {
        &self.messages
    }
    /// Consumes the result and returns its findings.
    pub fn into_messages(self) -> Vec<BoardDrcMessage> {
        self.messages
    }
}
type Check = fn(&BoardDrcData) -> Result<Vec<BoardDrcMessage>>;
impl BoardDesignRuleCheck {
    /// Runs the captured mode. The snapshot stays unchanged and can be reused
    /// on several caller-provided threads.
    pub fn run(data: &BoardDrcData) -> Result<BoardDrcResult> {
        Self::run_with_progress(data, |_, _| ControlFlow::Continue(()))
    }
    /// Reports completed/total rule groups, including initial/final progress.
    /// Cancellation or any rule error returns no partially applicable result.
    pub fn run_with_progress(
        data: &BoardDrcData,
        mut progress: impl FnMut(usize, usize) -> ControlFlow<()>,
    ) -> Result<BoardDrcResult> {
        let checks: &[Check] = if data.mode == DrcMode::Quick {
            &[Self::run_copper_rules, |data| {
                Ok(super::widths::check_copper(data))
            }]
        } else {
            &[
                |data| Ok(Self::run_manufacturing_rules(data)),
                Self::run_drill_clearances,
                Self::run_outline_rules,
                Self::run_structural_rules,
                Self::run_copper_rules,
                Self::run_placement_rules,
                Self::run_keepout_rules,
                Self::run_cutout_rules,
            ]
        };
        let mut messages = Vec::new();
        for (index, check) in checks.iter().enumerate() {
            if progress(index, checks.len()).is_break() {
                return Err(Error::DrcCancelled);
            }
            messages.extend(check(data)?);
        }
        if progress(checks.len(), checks.len()).is_break() {
            return Err(Error::DrcCancelled);
        }
        Ok(BoardDrcResult {
            board: data.board,
            mode: data.mode,
            messages,
        })
    }
}
impl Project {
    /// Updates the session's supported approvals and removes obsolete approvals
    /// after a successful run. Quick runs never delete or upgrade approvals.
    ///
    /// After a format upgrade all obsolete approvals are removed. Otherwise
    /// only approvals previously emitted this session can be removed, keeping
    /// unknown findings introduced by newer application versions. Persistent
    /// changes go through the mutation API and change journal.
    pub fn update_drc_approvals(&mut self, result: &BoardDrcResult) -> Result<bool> {
        let index = self.board_index(result.board).ok_or(Error::NotFound {
            kind: crate::project::EntityKind::Board,
            uuid: result.board.0,
        })?;
        let current: BTreeSet<_> = result
            .messages
            .iter()
            .map(|m| m.rule_message().approval().clone())
            .collect();
        let board = &mut self.boards[index];
        board
            .derived
            .supported_drc_approvals
            .extend(current.iter().cloned());
        if result.is_partial() {
            return Ok(false);
        }
        let format = crate::application::file_format_version();
        let upgraded = board.drc_approvals_version < format;
        let version = if upgraded {
            format
        } else {
            board.drc_approvals_version.clone()
        };
        let approvals = board
            .drc_approvals
            .iter()
            .filter(|a| {
                current.contains(*a)
                    || !upgraded && !board.derived.supported_drc_approvals.contains(*a)
            })
            .cloned()
            .collect();
        if board.drc_approvals_version == version && board.drc_approvals == approvals {
            return Ok(false);
        }
        self.apply(Mutation::Board(BoardMutation::SetDrcApprovals {
            board: result.board,
            version,
            approvals,
        }))?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::super::DrcMessageKind;
    use super::*;
    use crate::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
    use crate::project::ProjectLoader;
    use crate::rule_check::{RuleCheckMessage, Severity};
    use crate::serialization::SExpression;
    use crate::types::Version;
    use std::{path::Path, sync::Arc};

    fn project() -> Project {
        let root = Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data/projects/Gerber Test");
        let fs = TransactionalFileSystem::open_ro(&FilePath::new(root).unwrap()).unwrap();
        ProjectLoader::new()
            .open(TransactionalDirectory::new(Arc::new(fs), ""), "project.lpp")
            .unwrap()
    }
    fn approval(name: &str) -> SExpression {
        RuleCheckMessage::new(Severity::Warning, "", "", name, Vec::new())
            .approval()
            .clone()
    }
    fn output(board: BoardId, mode: DrcMode, known: bool) -> BoardDrcResult {
        BoardDrcResult {
            board,
            mode,
            messages: if known {
                vec![BoardDrcMessage {
                    board,
                    kind: DrcMessageKind::MissingBoardOutline,
                    rule: RuleCheckMessage::new(
                        Severity::Error,
                        "Missing board outline",
                        "",
                        "missing_board_outline",
                        Vec::new(),
                    ),
                }]
            } else {
                Vec::new()
            },
        }
    }
    fn set_approvals(project: &mut Project, board: BoardId, version: Version) {
        project
            .apply(Mutation::Board(BoardMutation::SetDrcApprovals {
                board,
                version,
                approvals: BTreeSet::from([
                    approval("missing_board_outline"),
                    approval("future_rule"),
                ]),
            }))
            .unwrap();
    }
    #[test]
    fn cleanup_preserves_unknown_approvals_and_quick_runs() {
        let mut project = project();
        let board = project.boards()[0].id();
        set_approvals(
            &mut project,
            board,
            crate::application::file_format_version(),
        );
        let revision = project.revision();
        assert!(
            !project
                .update_drc_approvals(&output(board, DrcMode::Full, true))
                .unwrap()
        );
        assert!(
            !project
                .update_drc_approvals(&output(board, DrcMode::Quick, false))
                .unwrap()
        );
        assert_eq!(project.revision(), revision);
        assert_eq!(project.board(board).unwrap().drc_approvals().len(), 2);
        assert!(
            project
                .update_drc_approvals(&output(board, DrcMode::Full, false))
                .unwrap()
        );
        assert_eq!(project.revision(), revision + 1);
        assert_eq!(
            project.board(board).unwrap().drc_approvals(),
            &BTreeSet::from([approval("future_rule")])
        );
        assert!(
            !project
                .update_drc_approvals(&output(board, DrcMode::Full, false))
                .unwrap()
        );
        assert_eq!(project.revision(), revision + 1);
    }
    #[test]
    fn format_upgrade_cleanup_waits_for_a_full_run() {
        let mut project = project();
        let board = project.boards()[0].id();
        let old: Version = "1".parse().unwrap();
        set_approvals(&mut project, board, old.clone());
        let revision = project.revision();
        assert!(
            !project
                .update_drc_approvals(&output(board, DrcMode::Quick, false))
                .unwrap()
        );
        assert_eq!(project.board(board).unwrap().drc_approvals_version(), &old);
        assert_eq!(project.revision(), revision);
        assert!(
            project
                .update_drc_approvals(&output(board, DrcMode::Full, true))
                .unwrap()
        );
        assert_eq!(
            project.board(board).unwrap().drc_approvals_version(),
            &crate::application::file_format_version()
        );
        assert_eq!(
            project.board(board).unwrap().drc_approvals(),
            &BTreeSet::from([approval("missing_board_outline")])
        );
        assert_eq!(project.revision(), revision + 1);
    }
    #[test]
    fn invalid_hole_mask_fails_full_check_without_breaking_quick_mode() {
        use crate::geometry::NonEmptyPath;
        use crate::project::board::BoardHoleData;
        use crate::types::{
            Layer, Length, MaskConfig, Point, PositiveLength, UnsignedLength, Uuid,
        };
        let project = project();
        let mut board = project.boards()[0].clone();
        let mut setup = board.settings().clone();
        setup.silkscreen_layers_top = vec![Layer::TOP_LEGEND];
        board.set_settings(setup);
        let diameter = PositiveLength::new(Length::new(1_000_000)).unwrap();
        let hole = BoardHoleData::new(
            Uuid::new_random(),
            diameter,
            NonEmptyPath::from_point(Point::ORIGIN),
            MaskConfig::Manual(Length::new(-500_000)),
            false,
        );
        board.holes.insert(hole.uuid(), hole);
        let mut settings = board.settings().drc_settings.clone();
        settings.set_min_silkscreen_stopmask_clearance(
            UnsignedLength::new(Length::new(100_000)).unwrap(),
        );
        let quick =
            BoardDrcData::extract_with_mode(&project, &board, &settings, DrcMode::Quick).unwrap();
        assert!(BoardDesignRuleCheck::run(&quick).unwrap().is_partial());
        let full = BoardDrcData::extract(&project, &board, &settings).unwrap();
        assert!(matches!(
            BoardDesignRuleCheck::run(&full),
            Err(Error::Types(_))
        ));
    }
}
