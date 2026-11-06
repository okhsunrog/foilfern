//! Port of copper clearance and annular-ring checks in
//! libs/librepcb/core/project/board/drc/boarddesignrulecheck.cpp.

use super::drill_clearances::{board_clearance_area, intersections, tolerance};
use super::{
    BoardDesignRuleCheck, BoardDrcData, BoardDrcMessage, DrcCopperRef, DrcHoleRef, DrcPadRef,
    copper_messages, paths,
};
use crate::geometry::Path;
use crate::project::{NetSignalId, Result};
use crate::types::{Layer, Length, PositiveLength, UnsignedLength};
use crate::utils::clipper_helpers::{self as ch, ClipperPaths};
use clipper::PolyFillType::EvenOdd;
use std::collections::BTreeSet;

struct Item {
    reference: DrcCopperRef,
    net: Option<NetSignalId>,
    net_name: String,
    start: Layer,
    end: Layer,
    clearance: UnsignedLength,
    area: ClipperPaths,
    expanded: ClipperPaths,
}
fn items(data: &BoardDrcData) -> Result<Vec<Item>> {
    let p = &data.physical;
    let mut result = Vec::new();
    let expansion =
        |clearance: UnsignedLength| (*clearance - *tolerance() - Length::new(1)).max(Length::ZERO);
    for pad in &p.pads {
        for &layer in &data.copper_layers {
            if pad.geometries.get(&layer).is_none_or(Vec::is_empty) {
                continue;
            }
            result.push(Item {
                reference: DrcCopperRef::Pad(pad.reference),
                net: pad.net,
                net_name: pad.net_name.clone(),
                start: layer,
                end: layer,
                clearance: pad.clearance,
                area: paths::pad(pad, layer, Length::ZERO, true)?,
                expanded: paths::pad(pad, layer, expansion(pad.clearance), true)?,
            });
        }
    }
    for via in &p.vias {
        result.push(Item {
            reference: via.reference,
            net: via.net,
            net_name: via.net_name.clone(),
            start: via.via.start_layer(),
            end: via.via.end_layer(),
            clearance: via.clearance,
            area: paths::via(via, Length::ZERO)?,
            expanded: paths::via(via, expansion(via.clearance))?,
        });
    }
    for trace in &p.traces {
        result.push(Item {
            reference: DrcCopperRef::Graphic(trace.reference),
            net: trace.net,
            net_name: trace.net_name.clone(),
            start: trace.layer,
            end: trace.layer,
            clearance: trace.clearance,
            area: paths::trace(trace, Length::ZERO)?,
            expanded: paths::trace(trace, expansion(trace.clearance))?,
        });
    }
    for plane in p
        .planes
        .iter()
        .filter(|_| data.mode == super::DrcMode::Full)
    {
        result.push(Item {
            reference: DrcCopperRef::Graphic(plane.reference),
            net: plane.net,
            net_name: plane.net_name.clone(),
            start: plane.layer,
            end: plane.layer,
            clearance: plane.clearance,
            area: paths::plane(plane, Length::ZERO)?,
            expanded: paths::plane(plane, expansion(plane.clearance))?,
        });
    }
    let clearance = data.settings.min_copper_copper_clearance();
    for drawing in p
        .drawings
        .iter()
        .filter(|d| data.copper_layers.contains(&d.layer))
    {
        result.push(Item {
            reference: DrcCopperRef::Graphic(drawing.reference),
            net: None,
            net_name: String::new(),
            start: drawing.layer,
            end: drawing.layer,
            clearance,
            area: paths::drawing(drawing, Length::ZERO)?,
            expanded: {
                let mut area = paths::drawing(drawing, Length::ZERO)?;
                ch::offset(
                    &mut area,
                    expansion(clearance),
                    tolerance(),
                    clipper::JoinType::Round,
                )?;
                area
            },
        });
    }
    for text in p
        .texts
        .iter()
        .filter(|t| data.copper_layers.contains(&t.layer))
    {
        result.push(Item {
            reference: DrcCopperRef::Graphic(text.reference),
            net: None,
            net_name: String::new(),
            start: text.layer,
            end: text.layer,
            clearance,
            area: paths::text(text, Length::ZERO)?,
            expanded: paths::text(text, expansion(clearance))?,
        });
    }
    Ok(result)
}
impl BoardDesignRuleCheck {
    /// Checks copper-to-copper, board and NPTH clearances and plated annular rings.
    pub fn run_copper_rules(data: &BoardDrcData) -> Result<Vec<BoardDrcMessage>> {
        let objects = items(data)?;
        let mut result = Vec::new();
        struct Violation {
            a: usize,
            b: usize,
            layers: BTreeSet<Layer>,
            minimum: UnsignedLength,
            locations: Vec<Path>,
        }
        let mut violations: Vec<Violation> = Vec::new();
        if data.physical.clearance_enabled {
            for (i, a) in objects.iter().enumerate() {
                for (j, b) in objects.iter().enumerate().skip(i + 1) {
                    if (a.clearance == UnsignedLength::ZERO && b.clearance == UnsignedLength::ZERO)
                        || (a.net.is_some() && a.net == b.net)
                    {
                        continue;
                    }
                    let layers: BTreeSet<_> = data
                        .copper_layers
                        .iter()
                        .filter(|l| {
                            l.copper_number()
                                >= a.start.copper_number().max(b.start.copper_number())
                                && l.copper_number()
                                    <= a.end.copper_number().min(b.end.copper_number())
                        })
                        .copied()
                        .collect();
                    if layers.is_empty() {
                        continue;
                    }
                    let mut locations = intersections(&a.area, &b.expanded)?;
                    if a.clearance != b.clearance || !locations.is_empty() {
                        locations.extend(intersections(&b.area, &a.expanded)?);
                    }
                    if locations.is_empty() {
                        continue;
                    }
                    let minimum = a.clearance.max(b.clearance);
                    if let Some(v) = violations.iter_mut().find(|v| {
                        (objects[v.a].reference == a.reference
                            && objects[v.b].reference == b.reference)
                            || (objects[v.a].reference == b.reference
                                && objects[v.b].reference == a.reference)
                    }) {
                        v.layers.extend(layers);
                        v.minimum = v.minimum.max(minimum);
                        v.locations.extend(locations);
                    } else {
                        violations.push(Violation {
                            a: i,
                            b: j,
                            layers,
                            minimum,
                            locations,
                        });
                    }
                }
            }
        }
        for violation in violations {
            let a = &objects[violation.a];
            let b = &objects[violation.b];
            result.push(copper_messages::pair(
                data,
                [a.reference, b.reference],
                [&a.net_name, &b.net_name],
                violation.layers,
                violation.minimum,
                violation.locations,
            ));
        }
        let minimum = data.settings.min_copper_board_clearance();
        if minimum > UnsignedLength::ZERO {
            let restricted = board_clearance_area(data, *minimum)?;
            let mut reported = Vec::new();
            for object in &objects {
                if reported.contains(&object.reference) {
                    continue;
                }
                let locations = intersections(&restricted, &object.area)?;
                if locations.is_empty() {
                    continue;
                }
                reported.push(object.reference);
                result.push(copper_messages::board(
                    data,
                    object.reference,
                    minimum,
                    locations,
                ));
            }
        }
        let copper: Vec<_> = data
            .copper_layers
            .iter()
            .map(|l| paths::copper(data, *l))
            .collect::<Result<_>>()?;
        let minimum = data.settings.min_copper_npth_clearance();
        if minimum > UnsignedLength::ZERO {
            let mut any = Vec::new();
            for area in &copper {
                paths::unite(&mut any, area)?;
            }
            for hole in data.holes.iter().filter(|h| !h.reference.is_plated()) {
                let area = paths::hole(
                    &hole.path,
                    hole.diameter,
                    *minimum - *tolerance() - Length::new(1),
                )?;
                let locations = intersections(&any, &area)?;
                if !locations.is_empty() {
                    result.push(copper_messages::hole(
                        data,
                        hole.reference,
                        minimum,
                        locations,
                    ));
                }
            }
        }
        let minimum = data.settings.min_pth_annular_ring();
        if minimum > UnsignedLength::ZERO && data.mode == super::DrcMode::Full {
            let tree = ch::intersect_all_to_tree(&copper)?;
            let all = ch::tree_to_paths(&tree);
            for pad in &data.physical.pads {
                let mut required = Vec::new();
                for hole in data
                    .holes
                    .iter()
                    .filter(|h| match (h.reference, pad.reference) {
                        (
                            DrcHoleRef::PadHole { segment, pad, .. },
                            DrcPadRef::Board { segment: s, pad: p },
                        ) => segment == s && pad == p,
                        (
                            DrcHoleRef::FootprintPadHole { device, pad, .. },
                            DrcPadRef::Footprint { device: d, pad: p },
                        ) => device == d && pad == p,
                        _ => false,
                    })
                {
                    let width =
                        PositiveLength::new(*hole.diameter + *minimum * 2 - Length::new(1))?;
                    paths::unite(
                        &mut required,
                        &ch::paths_to_clipper(&hole.path.to_outline_strokes(width), tolerance()),
                    )?;
                }
                let tree = ch::subtract_to_tree(&required, &all, EvenOdd, EvenOdd, true)?;
                let locations = ch::paths_from_clipper(&ch::flatten_tree(tree.root())?);
                if !locations.is_empty() {
                    result.push(copper_messages::annular(
                        data,
                        DrcCopperRef::Pad(pad.reference),
                        minimum,
                        locations,
                    ));
                }
            }
            for via in &data.physical.vias {
                if (*via.size - *via.drill) / 2 < *minimum {
                    result.push(copper_messages::annular(
                        data,
                        via.reference,
                        minimum,
                        vec![Path::circle(via.size).translated(via.via.position())],
                    ));
                }
            }
        }
        Ok(result)
    }
}
