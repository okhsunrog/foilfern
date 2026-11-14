//! Bounded legend searches inside the detached declarative board transaction.

use std::collections::BTreeSet;

use foilfern_core::project::board::BoardItem;
use foilfern_core::project::{BoardId, BoardMutation, Mutation};
use foilfern_editor::Transaction;
use foilfern_editor::legend_placement::LegendObstacles;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::tools::board_edit::BoardTextTarget;
use crate::units::{PointMm, angle};
use crate::{resolve, views};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct LegendPlacement {
    /// Scoped target from board_legend_list, on this layout's board.
    target: BoardTextTarget,
    /// Up to 32 poses, tried in order; first geometrically clear pose wins.
    candidates: Vec<TextPose>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TextPose {
    position: PointMm,
    /// Absolute angle; defaults to the current text rotation.
    rotation: Option<f64>,
}

fn geometry_error(error: foilfern_core::project::board::drc::Error) -> ToolError {
    ToolError::new(
        ErrorKind::Internal,
        format!("Legend geometry could not be checked: {error}"),
    )
}

pub(super) fn apply(
    requests: Vec<LegendPlacement>,
    tx: &mut Transaction<'_>,
    id: BoardId,
    diff: &mut Vec<Value>,
    checkpoint: &mut impl FnMut() -> ToolResult<()>,
) -> ToolResult<Vec<Value>> {
    if requests.len() > 16 {
        return Err(ToolError::invalid(
            "At most 16 legend searches per document.",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut reports = Vec::new();
    for request in requests {
        checkpoint()?;
        if !(1..=32).contains(&request.candidates.len()) {
            return Err(ToolError::invalid("Supply 1–32 legend candidate poses."));
        }
        let project = tx.project();
        let (_, target_board, board) = resolve::board(project, Some(&request.target.board))?;
        if target_board != id {
            return Err(ToolError::invalid(
                "Legend target belongs to a different board.",
            ));
        }
        let component = request
            .target
            .component
            .as_deref()
            .map(|key| resolve::component(project, key).map(|(id, _)| id))
            .transpose()?;
        let mut device = component
            .map(|key| {
                board
                    .device(key)
                    .cloned()
                    .ok_or_else(|| ToolError::not_found("Legend device is not placed."))
            })
            .transpose()?;
        let uuid = resolve::required_uuid(&request.target.uuid, "legend text")?;
        if !seen.insert((component, uuid)) {
            return Err(ToolError::invalid("Duplicate legend target."));
        }
        let old = device
            .as_ref()
            .map_or(board.stroke_texts(), |d| d.stroke_texts())
            .get(&uuid)
            .ok_or_else(|| ToolError::not_found("Legend text does not exist."))?;
        if old.locked() {
            return Err(ToolError::new(ErrorKind::Refused, "Legend text is locked."));
        }
        if !board
            .settings()
            .silkscreen_layers_top
            .contains(&old.layer())
            && !board
                .settings()
                .silkscreen_layers_bot
                .contains(&old.layer())
        {
            return Err(ToolError::invalid(
                "Target is not on a configured silkscreen layer.",
            ));
        }
        if board
            .settings()
            .silkscreen_layers_top
            .contains(&old.layer())
            && board
                .settings()
                .silkscreen_layers_bot
                .contains(&old.layer())
        {
            return Err(ToolError::invalid(
                "Legend layer is configured for both sides; select unambiguous silkscreen layers first.",
            ));
        }
        let obstacles =
            LegendObstacles::new(project, board, old, device.as_ref()).map_err(geometry_error)?;
        let mut attempts = Vec::new();
        let mut chosen = None;
        for pose in request.candidates {
            checkpoint()?;
            let mut text = old.clone();
            text.set_position(pose.position.to_point()?);
            if let Some(rotation) = pose.rotation {
                text.set_rotation(angle(rotation)?);
            }
            let check = obstacles
                .check(project, board, &text, device.as_ref())
                .map_err(geometry_error)?;
            let clear = check.clear() && obstacles.unknown_devices.is_empty();
            attempts.push(json!({"position":views::point(text.position()),"rotation":text.rotation().to_deg(),"clear":clear,"checks":check}));
            if clear {
                chosen = Some(text);
                break;
            }
        }
        let applicable = chosen.is_some();
        let mut report = json!({"target":{"board":id,"component":component,"uuid":uuid},
            "applicable":applicable,"attempts":attempts,"outline_fallbacks":obstacles.outline_fallbacks,
            "unknown_devices":obstacles.unknown_devices});
        if let Some(text) = chosen {
            report["text"] =
                crate::tools::board_edit::legend_json(project, board, &text, device.as_ref())?;
            if text != *old {
                diff.push(json!({"field":"board_legend","board":id,"component":component,"uuid":uuid,
                    "before":{"position":views::point(old.position()),"rotation":old.rotation().to_deg()},
                    "after":{"position":views::point(text.position()),"rotation":text.rotation().to_deg()}}));
                let mutation = if let Some(device) = device.as_mut() {
                    device.insert_stroke_text(text);
                    BoardMutation::UpdateDevice {
                        board: id,
                        device: device.clone(),
                    }
                } else {
                    BoardMutation::UpdateItem {
                        board: id,
                        item: BoardItem::StrokeText(text),
                    }
                };
                tx.apply(Mutation::Board(mutation))?;
            }
        }
        reports.push(report);
    }
    Ok(reports)
}

/// Recheck the final board after any authorized routing repair changed mask openings.
pub(super) fn review(
    project: &foilfern_core::project::Project,
    id: BoardId,
    reports: &mut [Value],
    mut checkpoint: impl FnMut() -> ToolResult<()>,
) -> ToolResult<()> {
    let board = super::board(project, id)?;
    for report in reports {
        checkpoint()?;
        if report["applicable"] != true {
            continue;
        }
        let target: BoardTextTarget = serde_json::from_value(report["target"].clone())
            .map_err(|error| ToolError::new(ErrorKind::Internal, error.to_string()))?;
        let component = target
            .component
            .as_deref()
            .map(|key| resolve::component(project, key).map(|(id, _)| id))
            .transpose()?;
        let device = component
            .map(|id| {
                board
                    .device(id)
                    .ok_or_else(|| ToolError::not_found("Reviewed legend device disappeared."))
            })
            .transpose()?;
        let uuid = resolve::required_uuid(&target.uuid, "legend text")?;
        let text = device
            .map_or(board.stroke_texts(), |d| d.stroke_texts())
            .get(&uuid)
            .ok_or_else(|| ToolError::not_found("Reviewed legend text disappeared."))?;
        let obstacles =
            LegendObstacles::new(project, board, text, device).map_err(geometry_error)?;
        let check = obstacles
            .check(project, board, text, device)
            .map_err(geometry_error)?;
        report["applicable"] = json!(check.clear() && obstacles.unknown_devices.is_empty());
        report["final_checks"] = json!(check);
    }
    Ok(())
}
