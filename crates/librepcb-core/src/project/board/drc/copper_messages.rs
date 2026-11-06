//! Port of copper/annular-ring findings in
//! libs/librepcb/core/project/board/drc/boarddesignrulecheckmessages.cpp.

use super::messages::net_with_fallback;
use super::{
    BoardDrcData, BoardDrcMessage, DrcCopperRef, DrcHoleRef, DrcMessageKind, DrcObjectRef, paths,
};
use crate::geometry::Path;
use crate::rule_check::{RuleCheckMessage, Severity};
use crate::serialization::{List, SExpression};
use crate::types::{Layer, UnsignedLength};
use librepcb_i18n::tr;
use std::collections::BTreeSet;

pub(super) fn serious() -> String {
    tr!(
        "BoardDesignRuleCheckMessages",
        "Depending on the capabilities of the PCB manufacturer, this could cause higher costs or even serious troubles during production, leading to a possibly non-functional PCB."
    )
}
fn record(data: &BoardDrcData, kind: DrcMessageKind, rule: RuleCheckMessage) -> BoardDrcMessage {
    BoardDrcMessage {
        board: data.board,
        kind,
        rule,
    }
}
pub(super) fn pair(
    data: &BoardDrcData,
    objects: [DrcCopperRef; 2],
    nets: [&str; 2],
    layers: BTreeSet<Layer>,
    minimum: UnsignedLength,
    locations: Vec<Path>,
) -> BoardDrcMessage {
    let layer = if layers.len() == 1 {
        layers
            .iter()
            .next()
            .map(|l| format!("'{}'", l.name_tr()))
            .unwrap_or_default()
    } else {
        tr!(
            "DrcMsgCopperCopperClearanceViolation",
            "{0} layers",
            layers.len()
        )
    };
    let mut rule = RuleCheckMessage::new(
        Severity::Error,
        tr!(
            "DrcMsgCopperCopperClearanceViolation",
            "Clearance on {0}: {1} ↔ {2} < {3} {4}",
            layer,
            paths::copper_name(data, objects[0], nets[0]),
            paths::copper_name(data, objects[1], nets[1]),
            minimum.to_mm_string(),
            "mm"
        ),
        tr!(
            "DrcMsgCopperCopperClearanceViolation",
            "The clearance between two copper objects of different nets is smaller than the minimum copper clearance configured in the DRC settings."
        ) + " "
            + &serious()
            + "\n\n"
            + &tr!(
                "DrcMsgCopperCopperClearanceViolation",
                "Check the DRC settings and move the objects to increase their clearance if needed."
            ),
        "copper_clearance_violation",
        locations,
    );
    let node = |reference: DrcCopperRef| {
        let mut node = List::new("object");
        reference.serialize(&mut node, true);
        SExpression::List(node)
    };
    let mut nodes = [node(objects[0]), node(objects[1])];
    nodes.sort();
    let approval = rule.approval_mut();
    for node in nodes {
        approval.ensure_line_break();
        approval.push(node);
    }
    approval.ensure_line_break();
    record(
        data,
        DrcMessageKind::CopperClearance {
            objects,
            layers,
            minimum,
        },
        rule,
    )
}
pub(super) fn board(
    data: &BoardDrcData,
    object: DrcCopperRef,
    minimum: UnsignedLength,
    locations: Vec<Path>,
) -> BoardDrcMessage {
    let min = minimum.to_mm_string();
    let (title, description, suggestion, critical) = match object {
        DrcCopperRef::Pad(_) => (
            tr!(
                "DrcMsgCopperBoardClearanceViolation",
                "Clearance pad ↔ board outline < {0} {1}",
                min,
                "mm"
            ),
            tr!(
                "DrcMsgCopperBoardClearanceViolation",
                "The clearance between a pad and the board outline is smaller than the board outline clearance configured in the DRC settings."
            ),
            tr!(
                "DrcMsgCopperBoardClearanceViolation",
                "Check the DRC settings and move the pad away from the board outline if needed."
            ),
            true,
        ),
        DrcCopperRef::Via { .. } => (
            tr!(
                "DrcMsgCopperBoardClearanceViolation",
                "Clearance via ↔ board outline < {0} {1}",
                min,
                "mm"
            ),
            tr!(
                "DrcMsgCopperBoardClearanceViolation",
                "The clearance between a via and the board outline is smaller than the board outline clearance configured in the DRC settings."
            ),
            tr!(
                "DrcMsgCopperBoardClearanceViolation",
                "Check the DRC settings and move the via away from the board outline if needed."
            ),
            true,
        ),
        DrcCopperRef::Graphic(reference) => match reference {
            DrcObjectRef::Trace { .. } => (
                tr!(
                    "DrcMsgCopperBoardClearanceViolation",
                    "Clearance trace ↔ board outline < {0} {1}",
                    min,
                    "mm"
                ),
                tr!(
                    "DrcMsgCopperBoardClearanceViolation",
                    "The clearance between a trace and the board outline is smaller than the board outline clearance configured in the DRC settings."
                ),
                tr!(
                    "DrcMsgCopperBoardClearanceViolation",
                    "Check the DRC settings and move the trace away from the board outline if needed."
                ),
                true,
            ),
            DrcObjectRef::Plane(_) => (
                tr!(
                    "DrcMsgCopperBoardClearanceViolation",
                    "Clearance plane ↔ board outline < {0} {1}",
                    min,
                    "mm"
                ),
                tr!(
                    "DrcMsgCopperBoardClearanceViolation",
                    "The clearance between a plane and the board outline is smaller than the board outline clearance configured in the DRC settings."
                ),
                tr!(
                    "DrcMsgCopperBoardClearanceViolation",
                    "Check the DRC settings and increase the configured plane clearance if needed."
                ),
                true,
            ),
            DrcObjectRef::Polygon { .. } => (
                tr!(
                    "DrcMsgCopperBoardClearanceViolation",
                    "Clearance copper polygon ↔ board outline < {0} {1}",
                    min,
                    "mm"
                ),
                tr!(
                    "DrcMsgCopperBoardClearanceViolation",
                    "The clearance between a polygon and the board outline is smaller than the board outline clearance configured in the DRC settings."
                ),
                tr!(
                    "DrcMsgCopperBoardClearanceViolation",
                    "Check the DRC settings and move the polygon away from the board outline if needed."
                ),
                false,
            ),
            DrcObjectRef::Circle { .. } => (
                tr!(
                    "DrcMsgCopperBoardClearanceViolation",
                    "Clearance copper circle ↔ board outline < {0} {1}",
                    min,
                    "mm"
                ),
                tr!(
                    "DrcMsgCopperBoardClearanceViolation",
                    "The clearance between a circle and the board outline is smaller than the board outline clearance configured in the DRC settings."
                ),
                tr!(
                    "DrcMsgCopperBoardClearanceViolation",
                    "Check the DRC settings and move the circle away from the board outline if needed."
                ),
                false,
            ),
            DrcObjectRef::StrokeText { .. } => (
                tr!(
                    "DrcMsgCopperBoardClearanceViolation",
                    "Clearance copper text ↔ board outline < {0} {1}",
                    min,
                    "mm"
                ),
                tr!(
                    "DrcMsgCopperBoardClearanceViolation",
                    "The clearance between a stroke text and the board outline is smaller than the board outline clearance configured in the DRC settings."
                ),
                tr!(
                    "DrcMsgCopperBoardClearanceViolation",
                    "Check the DRC settings and move the stroke text away from the board outline if needed."
                ),
                false,
            ),
        },
    };
    let description = if critical {
        description + " " + &serious()
    } else {
        description
    };
    let mut rule = RuleCheckMessage::new(
        if critical {
            Severity::Error
        } else {
            Severity::Warning
        },
        title,
        format!("{description}\n\n{suggestion}"),
        "copper_board_clearance_violation",
        locations,
    );
    object.serialize(rule.approval_mut(), false);
    record(
        data,
        DrcMessageKind::CopperBoardClearance { object, minimum },
        rule,
    )
}
pub(super) fn hole(
    data: &BoardDrcData,
    hole: DrcHoleRef,
    minimum: UnsignedLength,
    locations: Vec<Path>,
) -> BoardDrcMessage {
    let mut rule = RuleCheckMessage::new(
        Severity::Error,
        tr!(
            "DrcMsgCopperHoleClearanceViolation",
            "Clearance copper ↔ hole < {0} {1}",
            minimum.to_mm_string(),
            "mm"
        ),
        tr!(
            "DrcMsgCopperHoleClearanceViolation",
            "The clearance between a non-plated hole and copper objects is smaller than the hole clearance configured in the DRC settings."
        ) + " "
            + &serious()
            + "\n\n"
            + &tr!(
                "DrcMsgCopperHoleClearanceViolation",
                "Check the DRC settings and move the copper objects away from the hole if needed."
            ),
        "copper_hole_clearance_violation",
        locations,
    );
    hole.serialize(rule.approval_mut());
    record(
        data,
        DrcMessageKind::CopperHoleClearance { hole, minimum },
        rule,
    )
}
pub(super) fn annular(
    data: &BoardDrcData,
    object: DrcCopperRef,
    minimum: UnsignedLength,
    locations: Vec<Path>,
) -> BoardDrcMessage {
    let (title, description, suggestion) = if let DrcCopperRef::Pad(reference) = object {
        let pad = data.physical.pads.iter().find(|p| p.reference == reference);
        let name = if let Some(device) = reference.device() {
            data.physical
                .devices
                .get(&device)
                .map(|d| {
                    format!(
                        "{}:{}",
                        d.name,
                        pad.map(|p| p.name.as_str()).unwrap_or_default()
                    )
                })
                .unwrap_or_default()
        } else {
            net_with_fallback(pad.map(|p| p.net_name.as_str()).unwrap_or_default())
        };
        (
            tr!(
                "DrcMsgMinimumAnnularRingViolation",
                "Pad annular ring of '{0}' < {1} {2}",
                name,
                minimum.to_mm_string(),
                "mm"
            ),
            tr!(
                "DrcMsgMinimumAnnularRingViolation",
                "The through-hole pad annular ring width (i.e. the copper around the hole) is smaller than the minimum annular width configured in the DRC settings."
            ),
            tr!(
                "DrcMsgMinimumAnnularRingViolation",
                "Check the DRC settings and increase the pad size if needed."
            ),
        )
    } else {
        let name = data
            .physical
            .vias
            .iter()
            .find(|v| v.reference == object)
            .map(|v| v.net_name.as_str())
            .unwrap_or_default();
        (
            tr!(
                "DrcMsgMinimumAnnularRingViolation",
                "Via annular ring of '{0}' < {1} {2}",
                net_with_fallback(name),
                minimum.to_mm_string(),
                "mm"
            ),
            tr!(
                "DrcMsgMinimumAnnularRingViolation",
                "The via annular ring width (i.e. the copper around the hole) is smaller than the minimum annular width configured in the DRC settings."
            ),
            tr!(
                "DrcMsgMinimumAnnularRingViolation",
                "Check the DRC settings and increase the via size if needed."
            ),
        )
    };
    let mut rule = RuleCheckMessage::new(
        Severity::Error,
        title,
        description + " " + &serious() + "\n\n" + &suggestion,
        "minimum_annular_ring_violation",
        locations,
    );
    object.serialize(rule.approval_mut(), false);
    record(data, DrcMessageKind::AnnularRing { object, minimum }, rule)
}
