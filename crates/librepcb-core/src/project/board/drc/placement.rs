//! Port of device and silkscreen clearance checks in
//! libs/librepcb/core/project/board/drc/boarddesignrulecheck{,messages}.cpp.

use super::drill_clearances::{board_clearance_area, intersections, tolerance};
use super::{BoardDesignRuleCheck, BoardDrcData, BoardDrcMessage, DrcMessageKind, paths};
use crate::project::{ComponentInstanceId, Result};
use crate::rule_check::{RuleCheckMessage, Severity};
use crate::types::{Layer, Length, UnsignedLength};
use crate::utils::clipper_helpers::{self as ch, ClipperPaths};
use clipper::PolyFillType::{EvenOdd, NonZero};
use librepcb_i18n::tr;

pub(super) fn device_outline(
    data: &BoardDrcData,
    device: ComponentInstanceId,
    layer: Layer,
) -> Result<ClipperPaths> {
    let mut result = Vec::new();
    for drawing in data
        .physical
        .drawings
        .iter()
        .filter(|d| d.reference.device() == Some(device) && d.layer == layer)
    {
        ch::unite_with(
            &mut result,
            &ch::paths_to_clipper(std::slice::from_ref(&drawing.path), tolerance()),
            NonZero,
            NonZero,
        )?;
    }
    Ok(result)
}
impl BoardDesignRuleCheck {
    /// Checks body/body and body/courtyard overlaps and silkscreen texts near
    /// solder-mask openings and board edges.
    pub fn run_placement_rules(data: &BoardDrcData) -> Result<Vec<BoardDrcMessage>> {
        let mut result = Vec::new();
        let devices: Vec<_> = data.physical.devices.iter().collect();
        for (body, courtyard) in [
            (Layer::TOP_PACKAGE_OUTLINES, Layer::TOP_COURTYARD),
            (Layer::BOT_PACKAGE_OUTLINES, Layer::BOT_COURTYARD),
        ] {
            let outlines: Vec<_> = devices
                .iter()
                .map(|(id, _)| device_outline(data, **id, body))
                .collect::<Result<_>>()?;
            let courts: Vec<_> = devices
                .iter()
                .map(|(id, _)| device_outline(data, **id, courtyard))
                .collect::<Result<_>>()?;
            for (i, (id1, d1)) in devices.iter().enumerate() {
                for (j, (id2, d2)) in devices.iter().enumerate().skip(i + 1) {
                    let mut locations = intersections(&outlines[i], &outlines[j])?;
                    let overlap = !locations.is_empty();
                    if !overlap {
                        locations = intersections(&outlines[i], &courts[j])?;
                        if locations.is_empty() {
                            locations = intersections(&outlines[j], &courts[i])?;
                        }
                    }
                    if locations.is_empty() {
                        continue;
                    }
                    let name1 = d1.name.as_str().min(d2.name.as_str());
                    let name2 = d1.name.as_str().max(d2.name.as_str());
                    let (mut rule, kind) = if overlap {
                        (
                            RuleCheckMessage::new(
                                Severity::Error,
                                tr!(
                                    "DrcMsgOverlappingDevices",
                                    "Device overlap: '{0}' ↔ '{1}'",
                                    name1,
                                    name2
                                ),
                                tr!(
                                    "DrcMsgOverlappingDevices",
                                    "Two devices are overlapping and thus probably cannot be assembled both at the same time."
                                ) + "\n\n"
                                    + &tr!(
                                        "DrcMsgOverlappingDevices",
                                        "Either move the devices to increase their clearance or approve this message if you're sure they can be assembled without problems (or only one of them gets assembled)."
                                    ),
                                "overlapping_devices",
                                locations,
                            ),
                            DrcMessageKind::OverlappingDevices([**id1, **id2]),
                        )
                    } else {
                        (
                            RuleCheckMessage::new(
                                Severity::Warning,
                                tr!(
                                    "DrcMsgDeviceInCourtyard",
                                    "Device in courtyard: '{0}' ↔ '{1}'",
                                    name1,
                                    name2
                                ),
                                tr!(
                                    "DrcMsgDeviceInCourtyard",
                                    "A device is placed within the courtyard of another device, which might cause troubles during assembly of these parts."
                                ) + "\n\n"
                                    + &tr!(
                                        "DrcMsgDeviceInCourtyard",
                                        "Either move the devices to increase their clearance or approve this message if you're sure they can be assembled without problems."
                                    ),
                                "device_in_courtyard",
                                locations,
                            ),
                            DrcMessageKind::DeviceInCourtyard([**id1, **id2]),
                        )
                    };
                    super::structural::fields(
                        &mut rule,
                        &[("device", id1.0.min(id2.0)), ("device", id1.0.max(id2.0))],
                    );
                    result.push(BoardDrcMessage {
                        board: data.board,
                        kind,
                        rule,
                    });
                }
            }
        }
        let minimum = data.settings.min_silkscreen_stopmask_clearance();
        if minimum == UnsignedLength::ZERO {
            return Ok(result);
        }
        let outlines: Vec<_> = data
            .outlines
            .iter()
            .filter(|(_, l, p)| *l == Layer::BOARD_OUTLINES && p.is_closed())
            .map(|(_, _, p)| p.clone())
            .collect();
        let cutouts: Vec<_> = data
            .outlines
            .iter()
            .filter(|(_, l, p)| *l == Layer::BOARD_CUTOUTS && p.is_closed())
            .map(|(_, _, p)| p.clone())
            .collect();
        let mut board = ch::paths_to_clipper(&outlines, tolerance());
        ch::subtract(
            &mut board,
            &ch::paths_to_clipper(&cutouts, tolerance()),
            NonZero,
            NonZero,
        )?;
        let edges = board_clearance_area(data, *minimum)?;
        for (side, layers) in data.silkscreen_sides.iter().enumerate() {
            if layers.is_empty() {
                continue;
            }
            let layer = if side == 0 {
                Layer::TOP_STOP_MASK
            } else {
                Layer::BOT_STOP_MASK
            };
            let mut restricted = paths::stop_mask(data, layer, *minimum)?;
            paths::unite(&mut restricted, &edges)?;
            ch::intersect(&mut restricted, &board, EvenOdd, EvenOdd)?;
            for text in data
                .physical
                .texts
                .iter()
                .filter(|t| layers.contains(&t.layer))
            {
                let locations = intersections(&restricted, &paths::text(text, Length::ZERO)?)?;
                if locations.is_empty() {
                    continue;
                }
                let mut rule = RuleCheckMessage::new(
                    Severity::Warning,
                    tr!(
                        "DrcMsgSilkscreenClearanceViolation",
                        "Clearance silkscreen text ↔ stop mask < {0} {1}",
                        minimum.to_mm_string(),
                        "mm"
                    ),
                    tr!(
                        "DrcMsgSilkscreenClearanceViolation",
                        "The clearance between a silkscreen text and a solder resist opening is smaller than the minimum clearance configured in the DRC settings. This could lead to clipped silkscreen during production."
                    ) + "\n\n"
                        + &tr!(
                            "DrcMsgSilkscreenClearanceViolation",
                            "Check the DRC settings and move the text away from the solder resist opening if needed."
                        ),
                    "silkscreen_clearance_violation",
                    locations,
                );
                text.reference.serialize(rule.approval_mut());
                result.push(BoardDrcMessage {
                    board: data.board,
                    kind: DrcMessageKind::SilkscreenClearance {
                        object: text.reference,
                        minimum,
                    },
                    rule,
                });
            }
        }
        Ok(result)
    }
}
