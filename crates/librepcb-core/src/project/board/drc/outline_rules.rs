//! Port of board-outline checks in
//! libs/librepcb/core/project/board/drc/boarddesignrulecheck{,messages}.cpp.

use super::drill_clearances::tolerance;
use super::{BoardDesignRuleCheck, BoardDrcData, BoardDrcMessage, DrcMessageKind, DrcObjectRef};
use crate::geometry::Path;
use crate::project::Result;
use crate::rule_check::{RuleCheckMessage, Severity};
use crate::serialization::{List, SExpression};
use crate::types::{Layer, Length, PositiveLength, UnsignedLength};
use crate::utils::clipper_helpers as ch;
use clipper::{
    JoinType,
    PolyFillType::{EvenOdd, NonZero},
};
use librepcb_i18n::tr;

impl BoardDesignRuleCheck {
    /// Checks board outline closure, counts, intersections, cutout containment
    /// and manufacturability with the configured minimum milling tool.
    pub fn run_outline_rules(data: &BoardDrcData) -> Result<Vec<BoardDrcMessage>> {
        let mut result = Vec::new();
        let mut add = |kind, rule| {
            result.push(BoardDrcMessage {
                board: data.board,
                kind,
                rule,
            })
        };
        let polygons: Vec<_> = data
            .outlines
            .iter()
            .filter(|(reference, _, _)| matches!(reference, DrcObjectRef::Polygon { .. }))
            .collect();
        for (reference, _, path) in &polygons {
            if path.is_closed() {
                continue;
            }
            let width = data
                .widths
                .iter()
                .find(|w| w.reference == *reference)
                .map(|w| *w.width)
                .unwrap_or(Length::ZERO)
                .max(Length::new(100_000));
            let mut rule = RuleCheckMessage::new(
                Severity::Error,
                tr!("DrcMsgOpenBoardOutlinePolygon", "Non-closed board outline"),
                tr!(
                    "DrcMsgOpenBoardOutlinePolygon",
                    "The board outline polygon is not closed, i.e. the last vertex is not at the same coordinate as the first vertex."
                ) + " "
                    + &tr!(
                        "BoardDesignRuleCheckMessages",
                        "Depending on the capabilities of the PCB manufacturer, this could cause higher costs or even serious troubles during production, leading to a possibly non-functional PCB."
                    )
                    + "\n\n"
                    + &tr!(
                        "DrcMsgOpenBoardOutlinePolygon",
                        "Replace multiple coincident polygons with a single, connected polygon and append an explicit last vertex to make the polygon closed."
                    ),
                "open_board_outline",
                path.to_outline_strokes(PositiveLength::new(width)?),
            );
            reference.serialize(rule.approval_mut());
            add(DrcMessageKind::OpenBoardOutline(*reference), rule);
        }
        let outlines: Vec<Path> = data
            .outlines
            .iter()
            .filter(|(_, layer, path)| *layer == Layer::BOARD_OUTLINES && path.is_closed())
            .map(|(_, _, p)| p.clone())
            .collect();
        if outlines.is_empty() {
            add(
                DrcMessageKind::MissingBoardOutline,
                RuleCheckMessage::new(
                    Severity::Error,
                    tr!("DrcMsgMissingBoardOutline", "Missing board outline"),
                    tr!(
                        "DrcMsgMissingBoardOutline",
                        "There's no board outline defined at all, so the board cannot be manufactured."
                    ) + "\n\n"
                        + &tr!(
                            "DrcMsgMissingBoardOutline",
                            "Add a closed, zero-width polygon on the layer '{0}' to draw the board outline.",
                            Layer::BOARD_OUTLINES.name_tr()
                        ),
                    "missing_board_outline",
                    Vec::new(),
                ),
            );
        } else if outlines.len() > 1 {
            add(
                DrcMessageKind::MultipleBoardOutlines,
                RuleCheckMessage::new(
                    Severity::Warning,
                    tr!("DrcMsgMultipleBoardOutlines", "Multiple board outlines"),
                    tr!(
                        "DrcMsgMultipleBoardOutlines",
                        "There are multiple, independent board outlines defined."
                    ) + "\n\n"
                        + &tr!(
                            "DrcMsgMultipleBoardOutlines",
                            "Either add only a single board outline or make sure the PCB manufacturer can handle production data containing multiple PCBs."
                        ),
                    "multiple_board_outlines",
                    outlines.clone(),
                ),
            );
        }
        for (i, (first, layer1, path1)) in polygons.iter().enumerate() {
            if *layer1 != Layer::BOARD_OUTLINES || !path1.is_closed() {
                continue;
            }
            for (second, layer2, path2) in polygons.iter().skip(i + 1) {
                if *layer2 != Layer::BOARD_OUTLINES || !path2.is_closed() {
                    continue;
                }
                let mut paths = ch::paths_to_clipper(std::slice::from_ref(path1), tolerance());
                ch::intersect(
                    &mut paths,
                    &ch::paths_to_clipper(std::slice::from_ref(path2), tolerance()),
                    EvenOdd,
                    EvenOdd,
                )?;
                if paths.is_empty() {
                    continue;
                }
                let mut rule = RuleCheckMessage::new(
                    Severity::Error,
                    tr!(
                        "DrcMsgIntersectingBoardOutlines",
                        "Intersecting board outlines"
                    ),
                    tr!(
                        "DrcMsgIntersectingBoardOutlines",
                        "Two board outline polygons are intersecting each other, which will lead to invalid production data."
                    ) + "\n\n"
                        + &tr!(
                            "DrcMsgIntersectingBoardOutlines",
                            "Make sure there is exactly one board outline polygon. Cutouts at the board edge need to be part of the board outline polygon, not separate polygons. Cutouts inside the board need to be drawn on the '{0}' layer.",
                            Layer::BOARD_CUTOUTS.name_tr()
                        ),
                    "intersecting_board_outlines",
                    ch::paths_from_clipper(&paths),
                );
                let mut nodes = [object_node(*first), object_node(*second)];
                nodes.sort();
                let approval = rule.approval_mut();
                for node in nodes {
                    approval.ensure_line_break();
                    approval.push(node);
                }
                approval.ensure_line_break();
                add(
                    DrcMessageKind::IntersectingBoardOutlines([*first, *second]),
                    rule,
                );
            }
        }
        let mut area = ch::paths_to_clipper(&outlines, tolerance());
        for (reference, layer, path) in &polygons {
            if *layer == Layer::BOARD_OUTLINES || !path.is_closed() {
                continue;
            }
            let mut paths = ch::paths_to_clipper(std::slice::from_ref(path), tolerance());
            ch::subtract(&mut paths, &area, NonZero, NonZero)?;
            if paths.is_empty() {
                continue;
            }
            let mut rule = RuleCheckMessage::new(
                Severity::Error,
                tr!(
                    "DrcMsgCutoutOutsideBoardArea",
                    "Cutout outside of board area"
                ),
                tr!(
                    "DrcMsgCutoutOutsideBoardArea",
                    "A cutout polygon is outside the board area (either partially or fully), which will lead to invalid production data."
                ) + "\n\n"
                    + &tr!(
                        "DrcMsgCutoutOutsideBoardArea",
                        "Make sure all cutouts are fully inside the board area. Cutouts at the board edge need to be part of the board outlines polygon on the '{0}' layer, not separate polygons.",
                        Layer::BOARD_OUTLINES.name_tr()
                    ),
                "cutout_outside_board_area",
                ch::paths_from_clipper(&paths),
            );
            reference.serialize(rule.approval_mut());
            add(DrcMessageKind::CutoutOutsideBoardArea(*reference), rule);
        }
        let radius = UnsignedLength::new(data.settings.min_outline_tool_diameter() / 2)?;
        if radius > UnsignedLength::ZERO {
            let cutouts: Vec<_> = data
                .outlines
                .iter()
                .filter(|(_, layer, p)| *layer != Layer::BOARD_OUTLINES && p.is_closed())
                .map(|(_, _, p)| p.clone())
                .collect();
            ch::subtract(
                &mut area,
                &ch::paths_to_clipper(&cutouts, tolerance()),
                NonZero,
                NonZero,
            )?;
            let mut grown = area.clone();
            ch::offset(
                &mut grown,
                (*radius - Length::new(10_000)).max(Length::ZERO),
                tolerance(),
                JoinType::Round,
            )?;
            ch::offset(&mut grown, -*radius, tolerance(), JoinType::Round)?;
            let tree = ch::subtract_to_tree(&grown, &area, EvenOdd, EvenOdd, true)?;
            let paths = ch::flatten_tree(tree.root())?;
            if !paths.is_empty() {
                add(
                    DrcMessageKind::MinimumBoardInnerRadius(radius),
                    RuleCheckMessage::new(
                        Severity::Warning,
                        tr!(
                            "DrcMsgMinimumBoardOutlineInnerRadiusViolation",
                            "Board outline inner radius < {0} {1}",
                            radius.to_mm_string(),
                            "mm"
                        ),
                        tr!(
                            "DrcMsgMinimumBoardOutlineInnerRadiusViolation",
                            "The board outline polygon is not manufacturable with the minimum tool diameter configured in the DRC settings due to edges with a smaller radius. Thus the actually produced board outline might contain larger edge radii and too small cutouts might even be missing completely."
                        ) + "\n\n"
                            + &tr!(
                                "DrcMsgMinimumBoardOutlineInnerRadiusViolation",
                                "Check the DRC settings and add/increase the radius of inner board edges if needed."
                            ),
                        "minimum_board_inner_radius_violation",
                        ch::paths_from_clipper(&paths),
                    ),
                );
            }
        }
        Ok(result)
    }
}
fn object_node(reference: DrcObjectRef) -> SExpression {
    let mut node = List::new("object");
    reference.serialize(&mut node);
    SExpression::List(node)
}
