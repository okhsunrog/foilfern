//! Design rule check of boards for `open-project --drc` (upstream
//! `BoardDesignRuleCheck`, called from `CommandLineInterface::openProject()`).

use foilfern_core::library::org::BoardDesignRuleCheckSettings;
use foilfern_core::project::board::drc;
use foilfern_core::project::{BoardId, Project};
use foilfern_core::rule_check::RuleCheckMessage;

/// Result of the DRC of one board (upstream
/// `BoardDesignRuleCheck::Result`).
#[derive(Debug, Default)]
pub struct DrcResult {
    /// Fatal errors (the check could not be completed).
    pub errors: Vec<String>,
    /// The check messages.
    pub messages: Vec<RuleCheckMessage>,
}

/// Runs the full DRC of a board with the given settings (upstream
/// `BoardDesignRuleCheck::start()` + `waitForFinished()`, without the
/// quick check).
pub fn run_drc(
    project: &mut Project,
    board: BoardId,
    settings: &BoardDesignRuleCheckSettings,
) -> Result<DrcResult, drc::Error> {
    let result = project.run_drc(board, Some(settings), false, &|_| {})?;
    Ok(DrcResult {
        errors: result.errors,
        messages: result
            .messages
            .into_iter()
            .map(|m| m.message().clone())
            .collect(),
    })
}
