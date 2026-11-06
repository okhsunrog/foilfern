//! Port of width/text checks in
//! libs/librepcb/core/project/board/drc/boarddesignrulecheck{,messages}.cpp.

use super::{BoardDrcData, BoardDrcMessage, DrcMessageKind, DrcObjectRef};
use crate::rule_check::{RuleCheckMessage, Severity};
use librepcb_i18n::tr;

pub(super) fn check(data: &BoardDrcData) -> Vec<BoardDrcMessage> {
    check_impl(data, false)
}
pub(super) fn check_copper(data: &BoardDrcData) -> Vec<BoardDrcMessage> {
    check_impl(data, true)
}
fn check_impl(data: &BoardDrcData, copper_only: bool) -> Vec<BoardDrcMessage> {
    let mut result = Vec::new();
    for copper in [true, false] {
        if !copper && copper_only {
            continue;
        }
        let minimum = if copper {
            data.settings.min_copper_width()
        } else {
            data.settings.min_silkscreen_width()
        };
        for object in &data.widths {
            if !(if copper {
                data.copper_layers.contains(&object.layer)
            } else {
                data.silkscreen_layers.contains(&object.layer)
            }) {
                continue;
            }
            let minimum = minimum.max(object.net_min_width);
            if object.width >= minimum {
                continue;
            }
            let layer = object.layer.name_tr();
            let actual = object.displayed_width.to_mm_string();
            let min = minimum.to_mm_string();
            let name = &object.device_name;
            let (title, description, suggestion, critical) = match object.reference {
                DrcObjectRef::Trace { .. } => (
                    tr!(
                        "DrcMsgMinimumWidthViolation",
                        "Trace width on '{0}': {1} < {2} {3}",
                        layer,
                        actual,
                        min,
                        "mm"
                    ),
                    tr!(
                        "DrcMsgMinimumWidthViolation",
                        "The trace is thinner than the minimum copper width configured in the DRC settings."
                    ),
                    tr!(
                        "DrcMsgMinimumWidthViolation",
                        "Check the DRC settings and increase the trace width if needed."
                    ),
                    true,
                ),
                DrcObjectRef::Plane(_) => (
                    tr!(
                        "DrcMsgMinimumWidthViolation",
                        "Min. plane width on '{0}': {1} < {2} {3}",
                        layer,
                        actual,
                        min,
                        "mm"
                    ),
                    tr!(
                        "DrcMsgMinimumWidthViolation",
                        "The configured minimum width of the plane is smaller than the minimum copper width configured in the DRC settings."
                    ),
                    tr!(
                        "DrcMsgMinimumWidthViolation",
                        "Check the DRC settings and increase the minimum plane width in its properties if needed."
                    ),
                    true,
                ),
                DrcObjectRef::StrokeText { .. } => (
                    tr!(
                        "DrcMsgMinimumWidthViolation",
                        "Stroke width on '{0}': {1} < {2} {3}",
                        layer,
                        actual,
                        min,
                        "mm"
                    ),
                    tr!(
                        "DrcMsgMinimumWidthViolation",
                        "The text stroke width is smaller than the minimum width configured in the DRC settings."
                    ),
                    tr!(
                        "DrcMsgMinimumWidthViolation",
                        "Check the DRC settings and increase the text stroke width if needed."
                    ),
                    false,
                ),
                DrcObjectRef::Polygon { device, .. } => (
                    if device.is_some() {
                        tr!(
                            "DrcMsgMinimumWidthViolation",
                            "Polygon width of '{0}' on '{1}': {2} < {3} {4}",
                            name,
                            layer,
                            actual,
                            min,
                            "mm"
                        )
                    } else {
                        tr!(
                            "DrcMsgMinimumWidthViolation",
                            "Polygon width on '{0}': {1} < {2} {3}",
                            layer,
                            actual,
                            min,
                            "mm"
                        )
                    },
                    tr!(
                        "DrcMsgMinimumWidthViolation",
                        "The polygon line width is smaller than the minimum width configured in the DRC settings."
                    ),
                    tr!(
                        "DrcMsgMinimumWidthViolation",
                        "Check the DRC settings and increase the polygon line width if needed."
                    ),
                    false,
                ),
                DrcObjectRef::Circle { .. } => (
                    tr!(
                        "DrcMsgMinimumWidthViolation",
                        "Circle width of '{0}' on '{1}': {2} < {3} {4}",
                        name,
                        layer,
                        actual,
                        min,
                        "mm"
                    ),
                    tr!(
                        "DrcMsgMinimumWidthViolation",
                        "The circle line width is smaller than the minimum width configured in the DRC settings."
                    ),
                    tr!(
                        "DrcMsgMinimumWidthViolation",
                        "Check the DRC settings and increase the circle line width if needed."
                    ),
                    false,
                ),
            };
            let description = if critical {
                description
                    + " "
                    + &tr!(
                        "BoardDesignRuleCheckMessages",
                        "Depending on the capabilities of the PCB manufacturer, this could cause higher costs or even serious troubles during production, leading to a possibly non-functional PCB."
                    )
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
                "minimum_width_violation",
                object.locations.clone(),
            );
            object.reference.serialize(rule.approval_mut());
            result.push(BoardDrcMessage {
                board: data.board,
                kind: DrcMessageKind::MinimumWidth {
                    object: object.reference,
                    minimum,
                },
                rule,
            });
        }
    }
    if copper_only {
        return result;
    }
    for object in &data.widths {
        let Some(height) = object.board_text_height else {
            continue;
        };
        let minimum = data.settings.min_silkscreen_text_height();
        if !data.silkscreen_layers.contains(&object.layer) || *height >= *minimum {
            continue;
        }
        let mut rule = RuleCheckMessage::new(
            Severity::Warning,
            tr!(
                "DrcMsgMinimumTextHeightViolation",
                "Text height on '{0}': {1} < {2} {3}",
                object.layer.name_tr(),
                height.to_mm_string(),
                minimum.to_mm_string(),
                "mm"
            ),
            tr!(
                "DrcMsgMinimumTextHeightViolation",
                "The text height is smaller than the minimum height configured in the DRC settings. If the text is smaller than the minimum height specified by the PCB manufacturer, it may not be readable after production."
            ) + "\n\n"
                + &tr!(
                    "DrcMsgMinimumTextHeightViolation",
                    "Check the DRC settings and increase the text height if needed."
                ),
            "minimum_text_height_violation",
            object.locations.clone(),
        );
        object.reference.serialize(rule.approval_mut());
        result.push(BoardDrcMessage {
            board: data.board,
            kind: DrcMessageKind::MinimumTextHeight {
                object: object.reference,
                minimum,
            },
            rule,
        });
    }
    result
}
