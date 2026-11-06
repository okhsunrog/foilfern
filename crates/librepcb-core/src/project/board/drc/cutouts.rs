//! Port of plated/non-plated cutout checks in
//! libs/librepcb/core/project/board/drc/boarddesignrulecheck{,messages}.cpp.

use super::drill_clearances::tolerance;
use super::{
    BoardDesignRuleCheck, BoardDrcData, BoardDrcMessage, DrcMessageKind, DrcObjectRef, paths,
};
use crate::geometry::Path;
use crate::project::Result;
use crate::rule_check::{RuleCheckMessage, Severity};
use crate::types::{Layer, Length, PositiveLength, UnsignedLength};
use crate::utils::clipper_helpers as ch;
use clipper::PolyFillType::{EvenOdd, NonZero};
use librepcb_i18n::tr;

fn locations(paths: Vec<Path>, width: UnsignedLength) -> Result<Vec<Path>> {
    let width = PositiveLength::new((*width).max(Length::new(100_000)))?;
    Ok(paths
        .iter()
        .flat_map(|p| p.to_outline_strokes(width))
        .collect())
}
impl BoardDesignRuleCheck {
    /// Checks the copper along plated and non-plated cutout paths, and emits
    /// the upstream manufacturing notice for plated cutouts.
    pub fn run_cutout_rules(data: &BoardDrcData) -> Result<Vec<BoardDrcMessage>> {
        let mut result = Vec::new();
        let mut any = paths::copper(data, Layer::TOP_COPPER)?;
        let bottom = paths::copper(data, Layer::BOT_COPPER)?;
        let mut both = any.clone();
        ch::unite_with(&mut any, &bottom, EvenOdd, NonZero)?;
        ch::intersect(&mut both, &bottom, EvenOdd, NonZero)?;
        let mut plated_locations = Vec::new();
        for drawing in data
            .physical
            .drawings
            .iter()
            .filter(|d| matches!(d.layer, Layer::BOARD_CUTOUTS | Layer::BOARD_PLATED_CUTOUTS))
        {
            // Upstream's cutout check uses the device origin for footprint
            // circles, unlike its board-clearance and Gerber implementations.
            let outline = if let (Some((_, diameter)), DrcObjectRef::Circle { device, .. }) =
                (drawing.circle, drawing.reference)
            {
                Path::circle(diameter).translated(data.physical.devices[&device].position)
            } else {
                drawing.path.clone()
            };
            if outline.is_closed() {
                let subject = ch::paths_to_clipper(std::slice::from_ref(&outline), tolerance());
                let plated = drawing.layer == Layer::BOARD_PLATED_CUTOUTS;
                let tree = if plated {
                    ch::subtract_to_tree(&subject, &both, EvenOdd, EvenOdd, false)?
                } else {
                    ch::intersect_to_tree(&subject, &any, EvenOdd, EvenOdd, false)?
                };
                let highlights = locations(
                    ch::paths_from_clipper(&ch::tree_to_paths(&tree)),
                    UnsignedLength::ZERO,
                )?;
                if !highlights.is_empty() {
                    let (mut rule, kind) = if plated {
                        (
                            RuleCheckMessage::new(
                                Severity::Warning,
                                tr!(
                                    "DrcMsgPlatedCutoutWithoutCopper",
                                    "Plated cutout not surrounded by copper"
                                ),
                                tr!(
                                    "DrcMsgPlatedCutoutWithoutCopper",
                                    "Plated- and non-plated cutouts are exported to the same Gerber file because there is no standardized way to send plated cutouts to manufacturers. Some manufacturers just detect plated vs. non-plated cutouts by the existence of copper along their outline.\n\nThis cutout is on the \"{0}\" layer but does not have copper on both top & bottom layers along its complete outline, therefore this detection might fail, possibly leading to wrong manufacturing. It is recommended to draw copper along the complete cutout outline.",
                                    Layer::BOARD_PLATED_CUTOUTS.name_tr()
                                ),
                                "plated_cutout_without_copper",
                                highlights,
                            ),
                            DrcMessageKind::PlatedCutoutWithoutCopper(drawing.reference),
                        )
                    } else {
                        (
                            RuleCheckMessage::new(
                                Severity::Warning,
                                tr!(
                                    "DrcMsgNonPlatedCutoutWithCopper",
                                    "Non-plated cutout intersects with copper"
                                ),
                                tr!(
                                    "DrcMsgNonPlatedCutoutWithCopper",
                                    "Plated- and non-plated cutouts are exported to the same Gerber file because there is no standardized way to send plated cutouts to manufacturers. Some manufacturers just detect plated vs. non-plated cutouts by the existence of copper along their outline.\n\nThis cutout is on the \"{0}\" layer but intersects with copper layers along its outline, therefore this detection might fail, possibly leading to wrong manufacturing. It is recommended to remove any copper along the cutouts outline.",
                                    Layer::BOARD_CUTOUTS.name_tr()
                                ),
                                "nonplated_cutout_with_copper",
                                highlights,
                            ),
                            DrcMessageKind::NonPlatedCutoutWithCopper(drawing.reference),
                        )
                    };
                    drawing.reference.serialize(rule.approval_mut());
                    result.push(BoardDrcMessage {
                        board: data.board,
                        kind,
                        rule,
                    });
                }
            }
            if drawing.layer == Layer::BOARD_PLATED_CUTOUTS {
                plated_locations.extend(locations(vec![outline], drawing.width)?);
            }
        }
        if !plated_locations.is_empty() {
            result.push(BoardDrcMessage{board:data.board,kind:DrcMessageKind::PlatedCutouts,rule:RuleCheckMessage::new(Severity::Hint,tr!("DrcMsgPlatedCutouts","Plated cutouts detected"),tr!("DrcMsgPlatedCutouts","The board contains plated cutouts with arbitrary shape (polygons or circles). Those are currently considered EXPERIMENTAL because of the issues described below. In future releases, they might behave differently.\n\n1. Unfortunately, there is no standardized way to let PCB manufacturers know that those cutouts need to be plated. The Gerber export just includes those cutouts in the normal board outlines layer (together with the non-plated cutouts), therefore you need to ensure that the manufacturer will recognize them correctly. Some manufacturers automatically detect plated vs. non-plated cutouts by the existence of copper on the top & bottom layers along the cutout path. The DRC will raise a separate warning if this is not the case, but you should check if your manufacturer will follow this convention.\n\n2. The DRC does not yet detect every possible problem with such cutouts, you have to be careful not to accidentally create short-circuits with plated cutouts as the DRC won't warn you about this."),"plated_cutouts",plated_locations)});
        }
        Ok(result)
    }
}
