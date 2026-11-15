//! Initial-device legend placement, extending the native footprint defaults.
//! Exact checks reuse LegendObstacles; only the supplied new device texts move.

use foilfern_core::project::board::{BoardDevice, BoardStrokeTextData};
use foilfern_core::project::{BoardId, BoardMutation, ComponentInstanceId, Mutation, Project};
use foilfern_core::types::{Length, Point, Uuid};
use serde::Serialize;

use super::{LegendCheck, LegendObstacles};
use crate::{Command, Error, Result, Transaction};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InitialLegendStatus {
    Unchanged,
    Moved,
    NoFit,
    Locked,
    Unverified,
    BudgetExhausted,
}

#[derive(Debug)]
pub struct InitialLegendReport {
    pub board: BoardId,
    pub component: ComponentInstanceId,
    pub text: Uuid,
    pub before: BoardStrokeTextData,
    pub status: InitialLegendStatus,
    pub attempts: usize,
    pub before_checks: Option<LegendCheck>,
    pub final_checks: Option<LegendCheck>,
    pub outline_fallbacks: Vec<Uuid>,
    pub unknown_devices: Vec<Uuid>,
    pub error: Option<String>,
}

/// Run only at the final pose of a newly added device, inside its undo group.
/// Existing placements and explicit legend edits must not invoke this command.
pub struct PlaceInitialDeviceLegend {
    pub board: BoardId,
    pub components: Vec<ComponentInstanceId>,
}

impl Command for PlaceInitialDeviceLegend {
    type Output = Vec<InitialLegendReport>;
    fn text(&self) -> String {
        foilfern_i18n::tr!(
            "librepcb::editor::CmdAddDeviceToBoard",
            "Place device legend"
        )
    }
    fn execute(self, tx: &mut Transaction<'_>) -> Result<Self::Output> {
        place(tx, self.board, self.components, || Ok(()))
    }
}

fn candidate(origin: Point, dx: i64, dy: i64) -> Option<Point> {
    Some(Point::new(
        Length::new(origin.x.to_nm().checked_add(dx)?),
        Length::new(origin.y.to_nm().checked_add(dy)?),
    ))
}

/// At most 16 configured legend texts and 32 nearby translations per device.
/// The rotation, side, layer, font, size and every other device remain intact.
pub fn place<E: From<Error>>(
    tx: &mut Transaction<'_>,
    board_id: BoardId,
    components: impl IntoIterator<Item = ComponentInstanceId>,
    mut checkpoint: impl FnMut() -> std::result::Result<(), E>,
) -> std::result::Result<Vec<InitialLegendReport>, E> {
    let mut reports = Vec::new();
    for component in components {
        let board = tx
            .project()
            .board(board_id)
            .ok_or_else(|| Error::not_found("Board", board_id))?;
        let device = board
            .device(component)
            .ok_or_else(|| Error::not_found("Device", component))?;
        let texts: Vec<_> = device
            .stroke_texts()
            .values()
            .filter(|text| {
                board
                    .settings()
                    .silkscreen_layers_top
                    .contains(&text.layer())
                    || board
                        .settings()
                        .silkscreen_layers_bot
                        .contains(&text.layer())
            })
            .cloned()
            .collect();
        for (index, text) in texts.into_iter().enumerate() {
            checkpoint()?;
            let project = tx.project();
            let board = project
                .board(board_id)
                .ok_or_else(|| Error::not_found("Board", board_id))?;
            let device = board
                .device(component)
                .ok_or_else(|| Error::not_found("Device", component))?;
            let mut report = InitialLegendReport {
                board: board_id,
                component,
                text: text.uuid(),
                before: text.clone(),
                status: InitialLegendStatus::Unverified,
                attempts: 0,
                before_checks: None,
                final_checks: None,
                outline_fallbacks: Vec::new(),
                unknown_devices: Vec::new(),
                error: None,
            };
            let top = board
                .settings()
                .silkscreen_layers_top
                .contains(&text.layer());
            let bottom = board
                .settings()
                .silkscreen_layers_bot
                .contains(&text.layer());
            if top && bottom {
                report.error = Some("ambiguous_silkscreen_side".into());
            } else if index >= 16 {
                report.status = InitialLegendStatus::BudgetExhausted;
            } else {
                let result = (|| {
                    let obstacles = LegendObstacles::new(project, board, &text, Some(device))?;
                    report
                        .outline_fallbacks
                        .clone_from(&obstacles.outline_fallbacks);
                    report
                        .unknown_devices
                        .clone_from(&obstacles.unknown_devices);
                    let check = obstacles.check(project, board, &text, Some(device))?;
                    let clear = check.clear() && obstacles.unknown_devices.is_empty();
                    report.before_checks = Some(check);
                    if clear {
                        report.status = InitialLegendStatus::Unchanged;
                        return Ok(None);
                    }
                    if text.locked() {
                        report.status = InitialLegendStatus::Locked;
                        return Ok(None);
                    }
                    if !obstacles.unknown_devices.is_empty() {
                        return Ok(None);
                    }
                    let step = (text.height().to_nm() / 2).clamp(500_000, 2_000_000);
                    // Prefer translations along board Y, then X; symmetric
                    // alternatives handle all rotations and both board sides.
                    for ring in [1, 2, 4, 8] {
                        for (x, y) in [
                            (0, 1),
                            (0, -1),
                            (1, 0),
                            (-1, 0),
                            (1, 1),
                            (-1, 1),
                            (1, -1),
                            (-1, -1),
                        ] {
                            let Some(position) =
                                candidate(text.position(), x * ring * step, y * ring * step)
                            else {
                                continue;
                            };
                            report.attempts += 1;
                            let mut trial = text.clone();
                            trial.set_position(position);
                            if obstacles
                                .check(project, board, &trial, Some(device))?
                                .clear()
                            {
                                report.status = InitialLegendStatus::Moved;
                                return Ok(Some(trial));
                            }
                        }
                    }
                    report.status = InitialLegendStatus::NoFit;
                    Ok(None)
                })();
                let chosen: std::result::Result<
                    Option<BoardStrokeTextData>,
                    foilfern_core::project::board::drc::Error,
                > = result;
                match chosen {
                    Ok(Some(text)) => {
                        let mut device: BoardDevice = device.clone();
                        device.insert_stroke_text(text);
                        tx.apply(Mutation::Board(BoardMutation::UpdateDevice {
                            board: board_id,
                            device,
                        }))?;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        report.error = Some(error.to_string());
                        report.status = InitialLegendStatus::Unverified;
                    }
                }
            }
            reports.push(report);
        }
    }
    review(tx.project(), &mut reports);
    Ok(reports)
}

/// Recheck final text geometry, including after a declarative routing repair.
/// Never calls a finding resolved when geometry cannot be checked.
pub fn review(project: &Project, reports: &mut [InitialLegendReport]) {
    for report in reports {
        if report.status == InitialLegendStatus::BudgetExhausted
            || report.error.as_deref() == Some("ambiguous_silkscreen_side")
        {
            continue;
        }
        let check = (|| {
            let board = project
                .board(report.board)
                .ok_or_else(|| Error::not_found("Board", report.board))?;
            let device = board
                .device(report.component)
                .ok_or_else(|| Error::not_found("Device", report.component))?;
            let text = device
                .stroke_texts()
                .get(&report.text)
                .ok_or_else(|| Error::not_found("Text", report.text))?;
            let obstacles = LegendObstacles::new(project, board, text, Some(device))
                .map_err(|e| Error::InvalidArgument(e.to_string()))?;
            report
                .unknown_devices
                .clone_from(&obstacles.unknown_devices);
            obstacles
                .check(project, board, text, Some(device))
                .map_err(|e| Error::InvalidArgument(e.to_string()))
        })();
        match check {
            Ok(check) => {
                if (!check.clear() || !report.unknown_devices.is_empty())
                    && matches!(
                        report.status,
                        InitialLegendStatus::Moved | InitialLegendStatus::Unchanged
                    )
                {
                    report.status = InitialLegendStatus::Unverified;
                }
                report.final_checks = Some(check);
            }
            Err(error) => {
                report.error = Some(error.to_string());
                report.status = InitialLegendStatus::Unverified;
            }
        }
    }
}

impl InitialLegendReport {
    /// An actual conflict or an unavailable mandatory geometric check. Missing
    /// optional courtyard coverage alone does not invalidate a placed device.
    pub fn has_unresolved(&self) -> bool {
        self.error.is_some()
            || self
                .final_checks
                .as_ref()
                .is_none_or(|check| !check.clear())
    }
    pub fn verified_clear(&self) -> bool {
        self.error.is_none()
            && self.unknown_devices.is_empty()
            && self.final_checks.as_ref().is_some_and(LegendCheck::clear)
    }
}

static_assertions::assert_impl_all!(InitialLegendReport: Send, Sync);
