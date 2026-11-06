//! Port of zone checks and findings in
//! libs/librepcb/core/project/board/drc/boarddesignrulecheck{,messages}.cpp.
//!
//! Native painter-path predicates retain the upstream boundary/curve rules
//! because these decisions produce approvals persisted in board files.

use super::messages::net_with_fallback;
use super::physical_data::{Drawing, Zone};
use super::{
    BoardDesignRuleCheck, BoardDrcData, BoardDrcMessage, DrcCopperRef, DrcMessageKind, DrcObjectRef,
};
use crate::geometry::{Path, ZoneRules};
use crate::project::Result;
use crate::rule_check::{RuleCheckMessage, Severity};
use crate::serialization::{List, SExpression};
use crate::types::{Layer, PositiveLength, UnsignedLength};
use crate::utils::painter_path::PainterPathPx;
use librepcb_i18n::tr;
use std::collections::BTreeSet;

fn intersects(zone: &PainterPathPx, paths: &[Path]) -> bool {
    paths
        .iter()
        .any(|p| zone.intersects(&PainterPathPx::from_paths([p])))
}
fn drawing_locations(drawing: &Drawing, device: bool) -> Result<Vec<Path>> {
    let mut path = drawing.path.clone();
    let area = device && drawing.layer.polygons_represent_areas();
    if area {
        path.close();
    }
    let mut locations = Vec::new();
    if drawing.width > UnsignedLength::ZERO {
        locations.extend(path.to_outline_strokes(PositiveLength::new(*drawing.width)?));
    }
    if (drawing.filled || area) && path.is_closed() {
        locations.push(path);
    }
    Ok(locations)
}
impl BoardDesignRuleCheck {
    /// Checks copper, exposure and device keepout zones, including footprint
    /// zones and the upstream exemption for items in their own device.
    pub fn run_keepout_rules(data: &BoardDrcData) -> Result<Vec<BoardDrcMessage>> {
        let mut result = Vec::new();
        let p = &data.physical;
        for zone in &p.zones {
            if zone.device.is_none()
                && (zone.layers.is_disjoint(&data.copper_layers) || zone.rules.is_empty())
            {
                let mut outline = zone.outline.clone();
                outline.close();
                let mut rule = RuleCheckMessage::new(
                    Severity::Warning,
                    tr!("DrcMsgUselessZone", "Useless zone"),
                    tr!(
                        "DrcMsgUselessZone",
                        "The zone has no layer or rule enabled so it is useless."
                    ),
                    "useless_zone",
                    vec![outline],
                );
                super::structural::fields(&mut rule, &[("zone", zone.uuid)]);
                result.push(BoardDrcMessage {
                    board: data.board,
                    kind: DrcMessageKind::UselessZone(zone.uuid),
                    rule,
                });
            }
            let area = PainterPathPx::from_paths([&zone.outline]);
            let copper = if zone.rules.contains(ZoneRules::NO_COPPER) {
                zone.layers.clone()
            } else {
                BTreeSet::new()
            };
            let mut masks = BTreeSet::new();
            let mut devices = BTreeSet::new();
            for (layer, mask, outline, documentation) in [
                (
                    Layer::TOP_COPPER,
                    Layer::TOP_STOP_MASK,
                    Layer::TOP_PACKAGE_OUTLINES,
                    Layer::TOP_DOCUMENTATION,
                ),
                (
                    Layer::BOT_COPPER,
                    Layer::BOT_STOP_MASK,
                    Layer::BOT_PACKAGE_OUTLINES,
                    Layer::BOT_DOCUMENTATION,
                ),
            ] {
                if !zone.layers.contains(&layer) {
                    continue;
                }
                if zone.rules.contains(ZoneRules::NO_EXPOSURE) {
                    masks.insert(mask);
                }
                if zone.rules.contains(ZoneRules::NO_DEVICES) {
                    devices.extend([outline, documentation]);
                }
            }
            for pad in &p.pads {
                if pad.reference.device().is_some() && pad.reference.device() == zone.device {
                    continue;
                }
                for (layers, exposure) in [(&copper, false), (&masks, true)] {
                    let mut outlines = Vec::new();
                    for layer in layers {
                        for geometry in pad.geometries.get(layer).into_iter().flatten() {
                            for path in geometry.to_outlines()? {
                                let path = pad.transform.map(&path);
                                if !outlines.contains(&path) {
                                    outlines.push(path);
                                }
                            }
                        }
                    }
                    if intersects(&area, &outlines) {
                        result.push(violation(
                            data,
                            zone,
                            DrcCopperRef::Pad(pad.reference),
                            exposure,
                            outlines,
                        ));
                    }
                }
            }
            let mut device_violations = BTreeSet::new();
            for drawing in &p.drawings {
                let device = drawing.reference.device();
                if device.is_some() && device == zone.device {
                    continue;
                }
                if !(copper.contains(&drawing.layer)
                    || masks.contains(&drawing.layer)
                    || device.is_some() && devices.contains(&drawing.layer))
                {
                    continue;
                }
                let locations = drawing_locations(drawing, device.is_some())?;
                if !intersects(&area, &locations) {
                    continue;
                }
                if copper.contains(&drawing.layer) {
                    result.push(violation(
                        data,
                        zone,
                        DrcCopperRef::Graphic(drawing.reference),
                        false,
                        locations,
                    ));
                } else if masks.contains(&drawing.layer) {
                    result.push(violation(
                        data,
                        zone,
                        DrcCopperRef::Graphic(drawing.reference),
                        true,
                        locations,
                    ));
                } else if let Some(device) = device {
                    device_violations.insert(device);
                }
            }
            for device in device_violations {
                let d = &p.devices[&device];
                let mut rule = RuleCheckMessage::new(
                    Severity::Error,
                    tr!(
                        "DrcMsgDeviceInKeepoutZone",
                        "Device in keepout zone: '{0}'",
                        d.name
                    ),
                    tr!(
                        "DrcMsgDeviceInKeepoutZone",
                        "There is a device within a keepout zone."
                    ) + "\n\n"
                        + &tr!(
                            "DrcMsgDeviceInKeepoutZone",
                            "Move the device to outside the keepout zone."
                        ),
                    "device_in_keepout_zone",
                    d.locations.clone(),
                );
                rule.approval_mut().append_child("device", &device.0);
                zone_fields(&mut rule, zone);
                result.push(BoardDrcMessage {
                    board: data.board,
                    kind: DrcMessageKind::DeviceKeepout {
                        zone: zone.uuid,
                        zone_device: zone.device,
                        device,
                    },
                    rule,
                });
            }
            for via in &p.vias {
                let location = Path::circle(via.size).translated(via.via.position());
                if copper.iter().any(|l| via.via.is_on_layer(*l))
                    && intersects(&area, std::slice::from_ref(&location))
                {
                    result.push(violation(
                        data,
                        zone,
                        via.reference,
                        false,
                        vec![location.clone()],
                    ));
                }
                for (layer, diameter) in [
                    (Layer::TOP_STOP_MASK, via.mask_top),
                    (Layer::BOT_STOP_MASK, via.mask_bottom),
                ] {
                    if !masks.contains(&layer) {
                        continue;
                    }
                    if let Some(diameter) = diameter
                        && intersects(
                            &area,
                            &[Path::circle(diameter).translated(via.via.position())],
                        )
                    {
                        result.push(violation(
                            data,
                            zone,
                            via.reference,
                            true,
                            vec![location.clone()],
                        ));
                        break;
                    }
                }
            }
            for trace in &p.traces {
                if copper.contains(&trace.layer) {
                    let location = Path::obround_line(trace.p1, trace.p2, trace.width);
                    if intersects(&area, std::slice::from_ref(&location)) {
                        result.push(violation(
                            data,
                            zone,
                            DrcCopperRef::Graphic(trace.reference),
                            false,
                            vec![location],
                        ));
                    }
                }
            }
        }
        Ok(result)
    }
}
fn zone_fields(rule: &mut RuleCheckMessage, zone: &Zone) {
    let node = rule.approval_mut();
    node.ensure_line_break();
    node.append_child("zone", &zone.uuid);
    if let Some(device) = zone.device {
        node.ensure_line_break();
        node.append_child("from_device", &device.0);
    }
    node.ensure_line_break();
}
fn violation(
    data: &BoardDrcData,
    zone: &Zone,
    object: DrcCopperRef,
    exposure: bool,
    locations: Vec<Path>,
) -> BoardDrcMessage {
    let name = match object {
        DrcCopperRef::Pad(reference) => {
            let pad = data.physical.pads.iter().find(|p| p.reference == reference);
            if let Some(device) = reference.device() {
                format!(
                    "{}:{}",
                    data.physical.devices[&device].name,
                    pad.map(|p| p.name.as_str()).unwrap_or_default()
                )
            } else {
                net_with_fallback(pad.map(|p| p.net_name.as_str()).unwrap_or_default())
            }
        }
        DrcCopperRef::Via { .. } => net_with_fallback(
            data.physical
                .vias
                .iter()
                .find(|v| v.reference == object)
                .map(|v| v.net_name.as_str())
                .unwrap_or_default(),
        ),
        DrcCopperRef::Graphic(reference) => {
            if let Some(device) = reference.device() {
                data.physical.devices[&device].name.clone()
            } else {
                net_with_fallback(
                    data.physical
                        .traces
                        .iter()
                        .find(|t| t.reference == reference)
                        .map(|t| t.net_name.as_str())
                        .unwrap_or_default(),
                )
            }
        }
    };
    let (title, description, token) = if exposure {
        let title = match object {
            DrcCopperRef::Pad(_) => tr!(
                "DrcMsgExposureInKeepoutZone",
                "Pad in exposure keepout zone: '{0}'",
                name
            ),
            DrcCopperRef::Via { .. } => tr!(
                "DrcMsgExposureInKeepoutZone",
                "Via in exposure keepout zone: '{0}'",
                name
            ),
            DrcCopperRef::Graphic(DrcObjectRef::Polygon { device: None, .. }) => tr!(
                "DrcMsgExposureInKeepoutZone",
                "Polygon in exposure keepout zone"
            ),
            DrcCopperRef::Graphic(DrcObjectRef::Polygon { .. }) => tr!(
                "DrcMsgExposureInKeepoutZone",
                "Polygon in exposure keepout zone: '{0}'",
                name
            ),
            DrcCopperRef::Graphic(DrcObjectRef::Circle { .. }) => tr!(
                "DrcMsgExposureInKeepoutZone",
                "Circle in exposure keepout zone: '{0}'",
                name
            ),
            _ => unreachable!("exposure checks only receive pads, vias, polygons or circles"),
        };
        (
            title,
            tr!(
                "DrcMsgExposureInKeepoutZone",
                "There is a solder resist opening within an exposure keepout zone."
            ) + "\n\n"
                + &tr!(
                    "DrcMsgExposureInKeepoutZone",
                    "Move the object to outside the keepout zone."
                ),
            "exposure_in_keepout_zone",
        )
    } else {
        let title = match object {
            DrcCopperRef::Pad(_) => tr!(
                "DrcMsgCopperInKeepoutZone",
                "Pad in copper keepout zone: '{0}'",
                name
            ),
            DrcCopperRef::Via { .. } => tr!(
                "DrcMsgCopperInKeepoutZone",
                "Via in copper keepout zone: '{0}'",
                name
            ),
            DrcCopperRef::Graphic(DrcObjectRef::Trace { .. }) => tr!(
                "DrcMsgCopperInKeepoutZone",
                "Trace in copper keepout zone: '{0}'",
                name
            ),
            DrcCopperRef::Graphic(DrcObjectRef::Polygon { device: None, .. }) => tr!(
                "DrcMsgCopperInKeepoutZone",
                "Polygon in copper keepout zone"
            ),
            DrcCopperRef::Graphic(DrcObjectRef::Polygon { .. }) => tr!(
                "DrcMsgCopperInKeepoutZone",
                "Polygon in copper keepout zone: '{0}'",
                name
            ),
            DrcCopperRef::Graphic(DrcObjectRef::Circle { .. }) => tr!(
                "DrcMsgCopperInKeepoutZone",
                "Circle in copper keepout zone: '{0}'",
                name
            ),
            _ => unreachable!(
                "copper keepout checks only receive pads, vias, traces, polygons or circles"
            ),
        };
        (
            title,
            tr!(
                "DrcMsgCopperInKeepoutZone",
                "There is a copper object within a copper keepout zone."
            ) + "\n\n"
                + &tr!(
                    "DrcMsgCopperInKeepoutZone",
                    "Move the object to outside the keepout zone."
                ),
            "copper_in_keepout_zone",
        )
    };
    let mut rule = RuleCheckMessage::new(Severity::Error, title, description, token, locations);
    let mut node = List::new("object");
    object.serialize(&mut node, false);
    // These constructors put the first object field on the approval's first
    // line; preserve it rather than using the generic multiline layout.
    if matches!(node.children().first(), Some(SExpression::LineBreak)) {
        node.children_mut().remove(0);
    }
    for child in node.children_mut().drain(..) {
        rule.approval_mut().push(child);
    }
    zone_fields(&mut rule, zone);
    BoardDrcMessage {
        board: data.board,
        kind: DrcMessageKind::Keepout {
            zone: zone.uuid,
            zone_device: zone.device,
            object,
            exposure,
        },
        rule,
    }
}
