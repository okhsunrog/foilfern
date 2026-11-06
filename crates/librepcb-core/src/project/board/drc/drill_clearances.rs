//! Port of drill clearance checks in
//! libs/librepcb/core/project/board/drc/boarddesignrulecheck{,messages}.cpp.
//!
//! Uses the existing faithful Clipper implementation and upstream's arc
//! tolerance, expansion and fill rules; those define accepted clearances.

use super::{BoardDesignRuleCheck, BoardDrcData, BoardDrcMessage, DrcHoleRef, DrcMessageKind};
use crate::geometry::Path;
use crate::project::Result;
use crate::rule_check::{RuleCheckMessage, Severity};
use crate::serialization::{List, SExpression};
use crate::types::{Layer, Length, PositiveLength};
use crate::utils::clipper_helpers::{self as ch, ClipperPaths};
use clipper::PolyFillType::EvenOdd;
use librepcb_i18n::tr;

pub(super) fn tolerance() -> PositiveLength {
    PositiveLength::new(Length::new(5_000)).expect("positive constant")
}
pub(super) fn intersections(a: &ClipperPaths, b: &ClipperPaths) -> Result<Vec<Path>> {
    let tree = ch::intersect_to_tree(a, b, EvenOdd, EvenOdd, true)?;
    Ok(ch::paths_from_clipper(&ch::flatten_tree(tree.root())?))
}
pub(super) fn board_clearance_area(data: &BoardDrcData, clearance: Length) -> Result<ClipperPaths> {
    let width = positive_nm(
        (i128::from(clearance.to_nm()) * 2 - i128::from(tolerance().to_nm()) * 2).max(1),
    )?;
    let mut result = Vec::new();
    for (_, layer, path) in &data.outlines {
        if !matches!(*layer, Layer::BOARD_OUTLINES | Layer::BOARD_CUTOUTS) || !path.is_closed() {
            continue;
        }
        let paths = ch::paths_to_clipper(&path.to_outline_strokes(width), tolerance());
        ch::unite_with(&mut result, &paths, EvenOdd, EvenOdd)?;
    }
    Ok(result)
}
fn serious() -> String {
    tr!(
        "BoardDesignRuleCheckMessages",
        "Depending on the capabilities of the PCB manufacturer, this could cause higher costs or even serious troubles during production, leading to a possibly non-functional PCB."
    )
}

impl BoardDesignRuleCheck {
    /// Runs drill-to-drill and drill-to-board-outline clearance checks.
    pub fn run_drill_clearances(data: &BoardDrcData) -> Result<Vec<BoardDrcMessage>> {
        let mut result = Vec::new();
        let clearance = data.settings.min_drill_drill_clearance();
        if clearance > crate::types::UnsignedLength::ZERO {
            let expansion = (*clearance - *tolerance() - Length::new(1)).max(Length::ZERO);
            let areas: Vec<_> = data
                .holes
                .iter()
                .map(|h| -> Result<_> {
                    let width = positive_nm(
                        i128::from(h.diameter.to_nm()) + i128::from(expansion.to_nm()),
                    )?;
                    Ok(ch::paths_to_clipper(
                        &h.path.to_outline_strokes(width),
                        tolerance(),
                    ))
                })
                .collect::<Result<_>>()?;
            for (i, first) in data.holes.iter().enumerate() {
                for (j, second) in data.holes.iter().enumerate().skip(i + 1) {
                    let locations = intersections(&areas[i], &areas[j])?;
                    if locations.is_empty() {
                        continue;
                    }
                    let mut rule = RuleCheckMessage::new(
                        Severity::Error,
                        tr!(
                            "DrcMsgDrillDrillClearanceViolation",
                            "Clearance drill ↔ drill < {0} {1}",
                            clearance.to_mm_string(),
                            "mm"
                        ),
                        tr!(
                            "DrcMsgDrillDrillClearanceViolation",
                            "The clearance between two drills is smaller than the drill clearance configured in the DRC settings."
                        ) + " "
                            + &serious()
                            + "\n\n"
                            + &tr!(
                                "DrcMsgDrillDrillClearanceViolation",
                                "Check the DRC settings and move the drills to increase their distance if needed."
                            ),
                        "drill_clearance_violation",
                        locations,
                    );
                    let mut nodes = [drill_node(first.reference), drill_node(second.reference)];
                    nodes.sort();
                    let approval = rule.approval_mut();
                    for node in nodes {
                        approval.ensure_line_break();
                        approval.push(node);
                    }
                    approval.ensure_line_break();
                    result.push(BoardDrcMessage {
                        board: data.board,
                        kind: DrcMessageKind::DrillClearance {
                            holes: [first.reference, second.reference],
                            minimum: clearance,
                        },
                        rule,
                    });
                }
            }
        }
        let clearance = data.settings.min_drill_board_clearance();
        if clearance > crate::types::UnsignedLength::ZERO {
            let restricted = board_clearance_area(data, *clearance)?;
            for hole in &data.holes {
                let paths =
                    ch::paths_to_clipper(&hole.path.to_outline_strokes(hole.diameter), tolerance());
                let locations = intersections(&restricted, &paths)?;
                if locations.is_empty() {
                    continue;
                }
                let mut rule = RuleCheckMessage::new(
                    Severity::Error,
                    tr!(
                        "DrcMsgDrillBoardClearanceViolation",
                        "Clearance drill ↔ board outline < {0} {1}",
                        clearance.to_mm_string(),
                        "mm"
                    ),
                    tr!(
                        "DrcMsgDrillBoardClearanceViolation",
                        "The clearance between a drill and the board outline is smaller than the drill clearance configured in the DRC settings."
                    ) + " "
                        + &serious()
                        + "\n\n"
                        + &tr!(
                            "DrcMsgDrillBoardClearanceViolation",
                            "Check the DRC settings and move the drill away from the board outline if needed."
                        ),
                    "drill_board_clearance_violation",
                    locations,
                );
                hole.reference.serialize(rule.approval_mut());
                result.push(BoardDrcMessage {
                    board: data.board,
                    kind: DrcMessageKind::DrillBoardClearance {
                        hole: hole.reference,
                        minimum: clearance,
                    },
                    rule,
                });
            }
        }
        Ok(result)
    }
}
fn drill_node(reference: DrcHoleRef) -> SExpression {
    let mut node = List::new("drill");
    if let DrcHoleRef::BoardHole(hole) = reference {
        node.append_child("hole", &hole);
    } else {
        reference.serialize(&mut node);
    }
    SExpression::List(node)
}

fn positive_nm(value: i128) -> Result<PositiveLength> {
    let nm = i64::try_from(value).map_err(|_| crate::types::Error::Range {
        value: value as f64,
        min: 1,
        max: i64::MAX,
    })?;
    Ok(PositiveLength::new(Length::new(nm))?)
}
