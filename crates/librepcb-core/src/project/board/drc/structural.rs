//! Port of connectivity, stale-object, via and pad-origin checks in
//! libs/librepcb/core/project/board/drc/boarddesignrulecheck{,messages}.cpp.

use super::drill_clearances::tolerance;
use super::physical_data::{Anchor, OwnedVia, Stale};
use super::{
    BoardDesignRuleCheck, BoardDrcData, BoardDrcMessage, DrcMessageKind, DrcPadRef,
    messages::net_with_fallback,
};
use crate::geometry::{Path, TraceAnchor};
use crate::project::{Error, Result};
use crate::rule_check::{RuleCheckMessage, Severity};
use crate::serialization::{List, SExpression};
use crate::types::{Length, PositiveLength, Uuid};
use crate::utils::{clipper_helpers as ch, painter_path::PainterPathPx};
use librepcb_i18n::tr;

impl BoardDesignRuleCheck {
    /// Checks missing devices/connections, stale items, via validity/use and
    /// the origins of pads connected to traces.
    pub fn run_structural_rules(data: &BoardDrcData) -> Result<Vec<BoardDrcMessage>> {
        let physical = &data.physical;
        let mut result = Vec::new();
        let mut add = |kind, rule| {
            result.push(BoardDrcMessage {
                board: data.board,
                kind,
                rule,
            })
        };
        for (device, name) in &physical.unplaced {
            let mut rule = RuleCheckMessage::new(
                Severity::Error,
                tr!("DrcMsgMissingDevice", "Missing device: '{0}'", name),
                tr!(
                    "DrcMsgMissingDevice",
                    "There's a component in the schematics without a corresponding device in the board, so the circuit of the PCB is not complete.\n\nUse the \"Place Devices\" dock to add the device."
                ),
                "missing_device",
                Vec::new(),
            );
            fields(&mut rule, &[("device", device.0)]);
            add(DrcMessageKind::MissingDevice(*device), rule);
        }
        for wire in &physical.air_wires {
            let mut rule = RuleCheckMessage::new(
                Severity::Error,
                tr!(
                    "DrcMsgMissingConnection",
                    "Missing connection in '{0}': {1} ↔ {2}",
                    wire.net,
                    wire.from.name,
                    wire.to.name
                ),
                tr!(
                    "DrcMsgMissingConnection",
                    "There is a missing connection in the net, i.e. not all net items are connected together.\n\nAdd traces and/or planes to create the missing connections.\n\nNote that traces need to be snapped to the origin of footprint pads to make the airwire and this message disappearing."
                ),
                "missing_connection",
                wire.locations.clone(),
            );
            let mut nodes = [anchor_node(&wire.from)?, anchor_node(&wire.to)?];
            nodes.sort_by_key(|node| SExpression::List(node.clone()));
            nodes[0].set_name("from");
            nodes[1].set_name("to");
            let approval = rule.approval_mut();
            for node in nodes {
                approval.ensure_line_break();
                approval.push(SExpression::List(node));
            }
            approval.ensure_line_break();
            add(
                DrcMessageKind::MissingConnection([wire.from.anchor, wire.to.anchor]),
                rule,
            );
        }
        for (id, device) in &physical.devices {
            for (signal, name, net) in &device.impossible {
                let mut rule = RuleCheckMessage::new(
                    Severity::Error,
                    tr!(
                        "DrcMsgImpossibleConnection",
                        "Impossible connection in '{0}': '{1}:{2}'",
                        net,
                        device.name,
                        name
                    ),
                    tr!(
                        "DrcMsgImpossibleConnection",
                        "The pin of this device is connected to a net in the schematics, but the footprint doesn't expose a pad for it. Therefore it is impossible to make the electrical connection in the board. Check if another footprint or another device exposes a corresponding pad."
                    ),
                    "impossible_connection",
                    device.locations.clone(),
                );
                fields(&mut rule, &[("device", id.0), ("signal", *signal)]);
                add(
                    DrcMessageKind::ImpossibleConnection {
                        device: *id,
                        signal: *signal,
                    },
                    rule,
                );
            }
        }
        for stale in &physical.stale {
            match stale {
                Stale::Segment { segment, net } => {
                    let mut rule = RuleCheckMessage::new(
                        Severity::Hint,
                        tr!(
                            "DrcMsgEmptyNetSegment",
                            "Empty segment of net '{0}': '{1}'",
                            net_with_fallback(net),
                            segment
                        ),
                        "There's a net segment in the board without any pad, via or trace. This should not happen, please report it as a bug. But no worries, this issue is not harmful at all and you can just trigger the automatic fix to remove the empty net segment.",
                        "empty_netsegment",
                        Vec::new(),
                    );
                    fields(&mut rule, &[("netsegment", segment.0)]);
                    add(DrcMessageKind::EmptyNetSegment(*segment), rule);
                }
                Stale::Junction {
                    segment,
                    junction,
                    position,
                    net,
                } => {
                    let mut rule = RuleCheckMessage::new(
                        Severity::Hint,
                        tr!(
                            "DrcMsgUnconnectedJunction",
                            "Unconnected junction in net: '{0}'",
                            net_with_fallback(net)
                        ),
                        "There's an invisible junction in the board without any trace attached. This should not happen, please report it as a bug. But no worries, this issue is not harmful at all so you can safely ignore this message.",
                        "unconnected_junction",
                        vec![
                            Path::circle(PositiveLength::new(Length::new(300_000))?)
                                .translated(*position),
                        ],
                    );
                    fields(
                        &mut rule,
                        &[("netsegment", segment.0), ("junction", *junction)],
                    );
                    add(
                        DrcMessageKind::UnconnectedJunction {
                            segment: *segment,
                            junction: *junction,
                        },
                        rule,
                    );
                }
            }
        }
        for via in &physical.vias {
            let net = net_with_fallback(&via.net_name);
            let locations = vec![Path::circle(via.size).translated(via.via.position())];
            if (via.via.is_blind() && !data.settings.blind_vias_allowed())
                || (via.via.is_buried() && !data.settings.buried_vias_allowed())
            {
                let (title, description) = if via.via.is_blind() {
                    (
                        tr!("DrcMsgForbiddenVia", "Blind via in net '{0}'", net),
                        tr!(
                            "DrcMsgForbiddenVia",
                            "Blind vias are expensive to manufacture and not every PCB manufacturer is able to create them."
                        ),
                    )
                } else {
                    (
                        tr!("DrcMsgForbiddenVia", "Buried via in net '{0}'", net),
                        tr!(
                            "DrcMsgForbiddenVia",
                            "Buried vias are expensive to manufacture and not every PCB manufacturer is able to create them."
                        ),
                    )
                };
                let mut rule = RuleCheckMessage::new(
                    Severity::Error,
                    title,
                    description
                        + "\n"
                        + &tr!(
                            "DrcMsgForbiddenVia",
                            "Either avoid them or check if your PCB manufacturer supports them and adjust the DRC settings accordingly."
                        ),
                    "forbidden_via",
                    locations.clone(),
                );
                via.reference.serialize(rule.approval_mut(), false);
                add(DrcMessageKind::ForbiddenVia(via.reference), rule);
            }
            if via.span.is_none() {
                let mut rule = RuleCheckMessage::new(
                    Severity::Warning,
                    tr!("DrcMsgInvalidVia", "Invalid via in net '{0}'", net),
                    tr!(
                        "DrcMsgInvalidVia",
                        "The via is only drilled between one layer and is therefore invalid."
                    ),
                    "invalid_via",
                    locations,
                );
                via.reference.serialize(rule.approval_mut(), false);
                add(DrcMessageKind::InvalidVia(via.reference), rule);
                continue;
            }
            if useless_via(data, via)? {
                let mut rule = RuleCheckMessage::new(
                    Severity::Warning,
                    tr!("DrcMsgUselessVia", "Useless via in net '{0}'", net),
                    tr!(
                        "DrcMsgUselessVia",
                        "The via is connected on less than two layers, thus it seems to be useless."
                    ),
                    "useless_via",
                    locations,
                );
                via.reference.serialize(rule.approval_mut(), false);
                add(DrcMessageKind::UselessVia(via.reference), rule);
            }
        }
        for pad in &physical.pads {
            for layer in &pad.connected {
                let mut contains = false;
                for geometry in pad.geometries.get(layer).into_iter().flatten() {
                    let outlines = geometry.to_outlines()?;
                    if PainterPathPx::from_paths(&outlines).contains_point((0.0, 0.0)) {
                        contains = true;
                        break;
                    }
                }
                if contains {
                    continue;
                }
                let name = match pad.reference {
                    DrcPadRef::Footprint { device, .. } => format!(
                        "{}:{}",
                        physical
                            .devices
                            .get(&device)
                            .ok_or(Error::InexistentComponent(device.0))?
                            .name,
                        pad.name
                    ),
                    _ => net_with_fallback(&pad.net_name),
                };
                // Upstream supplies only the first placeholder; retain the literal
                // second placeholder in the title while using its translation key.
                let mut rule = RuleCheckMessage::new(
                    Severity::Error,
                    tr!(
                        "DrcMsgInvalidPadConnection",
                        "Invalid connection of pad '{0}' on '{1}'",
                        name,
                        "%2"
                    ),
                    tr!(
                        "DrcMsgInvalidPadConnection",
                        "The pad origin must be located within the pads copper area, or for THT pads within a hole. Otherwise traces might not beconnected fully. This issue needs to be fixed in the library."
                    ),
                    "invalid_pad_connection",
                    vec![
                        Path::circle(PositiveLength::new(Length::new(500_000))?)
                            .translated(pad.transform.position),
                    ],
                );
                rule.approval_mut().append_child("layer", layer);
                pad.reference.serialize(rule.approval_mut());
                add(
                    DrcMessageKind::InvalidPadConnection {
                        pad: pad.reference,
                        layer: *layer,
                    },
                    rule,
                );
            }
        }
        Ok(result)
    }
}
pub(super) fn fields(rule: &mut RuleCheckMessage, fields: &[(&str, Uuid)]) {
    let node = rule.approval_mut();
    for (name, id) in fields {
        node.ensure_line_break();
        node.append_child(*name, id);
    }
    node.ensure_line_break();
}
fn anchor_node(anchor: &Anchor) -> Result<List> {
    let mut node = List::new("tmp");
    if let (Some(device), TraceAnchor::FootprintPad { pad, .. }) = (anchor.device, anchor.anchor) {
        node.ensure_line_break();
        node.append_child("device", &device.0);
        node.ensure_line_break();
        node.append_child("pad", &pad);
        node.ensure_line_break();
    } else {
        let segment = anchor.segment.ok_or(Error::InexistentTraceAnchor {
            anchor: anchor.anchor,
            device_missing: false,
        })?;
        node.append_child("netsegment", &segment.0);
    }
    Ok(node)
}
fn useless_via(data: &BoardDrcData, via: &OwnedVia) -> Result<bool> {
    let mut layers = via.connected.clone();
    if layers.len() >= 2 {
        return Ok(false);
    }
    let position = ch::point_to_clipper(via.via.position());
    for plane in &data.physical.planes {
        if !via.via.is_on_layer(plane.layer)
            || plane.net.is_none()
            || plane.net != via.net
            || layers.contains(&plane.layer)
        {
            continue;
        }
        if clipper::point_in_polygon(position, &ch::path_to_clipper(&plane.outline, tolerance()))
            != 0
        {
            layers.insert(plane.layer);
            if layers.len() >= 2 {
                return Ok(false);
            }
        }
    }
    // Upstream considers footprint pad outlines regardless of pad net here,
    // and tests the plane outline rather than rebuilt plane fragments.
    for pad in data
        .physical
        .pads
        .iter()
        .filter(|p| p.reference.device().is_some())
    {
        for (layer, geometries) in &pad.geometries {
            if !layer.is_copper() || !via.via.is_on_layer(*layer) || layers.contains(layer) {
                continue;
            }
            for geometry in geometries {
                for outline in geometry.to_outlines()? {
                    if clipper::point_in_polygon(
                        position,
                        &ch::path_to_clipper(&pad.transform.map(&outline), tolerance()),
                    ) != 0
                    {
                        layers.insert(*layer);
                        if layers.len() >= 2 {
                            return Ok(false);
                        }
                    }
                }
            }
        }
    }
    Ok(true)
}
