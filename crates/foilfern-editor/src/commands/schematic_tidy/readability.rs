//! Resolve layout conflicts using the same visible geometry as schematic review.
//! Native text and label mutations preserve pin anchors and circuit membership.

use std::collections::{BTreeMap, BTreeSet};

use foilfern_core::geometry::NetLineAnchor;
use foilfern_core::project::{
    Mutation, NetSegmentRef, SchematicId, SchematicMutation, SymbolArtwork, SymbolId, SymbolRef,
};
use foilfern_core::types::{Length, Point, Uuid};
use foilfern_scene::foilfern_canvas::{SelectionMode, convert};
use foilfern_scene::{
    Annotation, ColorScheme, SchematicObject, SchematicScene, SymbolTextPlacement,
    annotation_overlaps,
};

use super::super::placement::placed_symbol_rect;
use crate::{Error, Result, Transaction};

pub(super) fn polish(tx: &mut Transaction<'_>, page: SchematicId) -> Result<Vec<Uuid>> {
    let mut artwork_symbols = BTreeSet::new();
    // Every successful translation clears its full bounds against the sheet.
    // The limit also bounds repairs of malformed imported pin legends.
    for _ in 0..256 {
        let scene = SchematicScene::build(tx.project(), page, &ColorScheme::SCHEMATIC_LIGHT)?;
        let conflicts = annotation_overlaps(&scene, None);
        if conflicts.is_empty() {
            return Ok(artwork_symbols.into_iter().collect());
        }
        let mut changed = false;
        for conflict in conflicts {
            for target in [conflict.first, conflict.second] {
                changed = match target {
                    Annotation::SymbolText(symbol, text) => {
                        move_field(tx, page, &scene, symbol, text)?
                    }
                    Annotation::NetLabel(segment, label) => {
                        move_label(tx, page, &scene, segment, label)?
                            || super::super::wiring::repair_label_stub(tx, page, segment)?
                    }
                    Annotation::Wire(segment, _) => {
                        super::super::wiring::repair_label_stub(tx, page, segment)?
                    }
                    _ => false,
                };
                if changed {
                    break;
                }
            }
            if changed {
                break;
            }
            if let (Annotation::PinName(symbol, pin), Annotation::PinName(other, _)) =
                (conflict.first, conflict.second)
                && symbol == other
            {
                artwork_symbols.insert(move_pin_legend(tx, page, symbol, pin)?);
                changed = true;
                break;
            }
        }
        if !changed {
            break;
        }
    }
    let scene = SchematicScene::build(tx.project(), page, &ColorScheme::SCHEMATIC_LIGHT)?;
    let remaining = annotation_overlaps(&scene, None);
    let first = remaining
        .first()
        .map(|conflict| {
            format!(
                "{} overlaps {} at {:?} mm",
                describe(tx, page, conflict.first),
                describe(tx, page, conflict.second),
                conflict.bounds
            )
        })
        .unwrap_or_default();
    Err(Error::InvalidArgument(format!(
        "Could not clear schematic layout, {} conflicts remain. {}. The layout operation was rolled back.",
        remaining.len(),
        first
    )))
}

fn describe(tx: &Transaction<'_>, page: SchematicId, annotation: Annotation) -> String {
    let Some(sheet) = tx.project().schematic(page) else {
        return format!("{annotation:?}");
    };
    match annotation {
        Annotation::SymbolText(symbol, _)
        | Annotation::PinName(symbol, _)
        | Annotation::PinNumber(symbol, _) => {
            let name =
                super::resolve::component_name(tx.project(), sheet.symbols()[&symbol].component());
            format!("{name} annotation")
        }
        Annotation::NetLabel(segment, _) | Annotation::Wire(segment, _) => {
            let name =
                super::resolve::net_name(tx.project(), Some(sheet.net_segments()[&segment].net()));
            format!("net {name}")
        }
        Annotation::Text(_) => "page text".into(),
    }
}

fn move_field(
    tx: &mut Transaction<'_>,
    page: SchematicId,
    scene: &SchematicScene,
    symbol: SymbolId,
    uuid: Uuid,
) -> Result<bool> {
    let instance = &tx
        .project()
        .schematic(page)
        .ok_or_else(|| Error::not_found("Schematic", page))?
        .symbols()[&symbol];
    let Some(text) = instance.texts().get(&uuid) else {
        return Ok(false);
    };
    if text.locked() {
        return Ok(false);
    }
    let mut text = text.clone();
    let Some(placement) = SymbolTextPlacement::new(scene, symbol, uuid, text.position()) else {
        return Ok(false);
    };
    let area =
        placed_symbol_rect(tx.project(), page, symbol, false)?.expanded(Length::new(20_320_000));
    let step = Length::new(635_000);
    for radius in 1i32..=32 {
        for dx in -radius..=radius {
            for dy in -radius..=radius {
                if dx.abs().max(dy.abs()) != radius {
                    continue;
                }
                let position =
                    text.position() + Point::new(step * i64::from(dx), step * i64::from(dy));
                if placement.fits(position, area.min, area.max, 0.4) {
                    text.set_position(position);
                    tx.apply(Mutation::Schematic(SchematicMutation::UpdateSymbolText {
                        symbol: SymbolRef {
                            schematic: page,
                            symbol,
                        },
                        text,
                    }))?;
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn move_label(
    tx: &mut Transaction<'_>,
    page: SchematicId,
    scene: &SchematicScene,
    segment: foilfern_core::project::NetSegmentId,
    uuid: Uuid,
) -> Result<bool> {
    let sheet = tx
        .project()
        .schematic(page)
        .ok_or_else(|| Error::not_found("Schematic", page))?;
    let net = &sheet.net_segments()[&segment];
    let mut label = net.labels()[&uuid].clone();
    let target = SchematicObject::NetLabel(segment, uuid);
    let bounds = scene
        .scene()
        .items()
        .filter(|(id, _)| scene.object(*id) == Some(target))
        .map(|(_, item)| item.bounding_box())
        .reduce(|a, b| a.union(b));
    let Some(bounds) = bounds else {
        return Ok(false);
    };
    let relative = bounds - convert::point(label.position()).to_vec2();
    let horizontal = label.rotation().to_deg().rem_euclid(180.0) == 0.0;
    for line in net.lines().values() {
        let (Some(a), Some(b)) = (
            sheet.net_line_anchor_position(segment, line.p1(), tx.project().view()),
            sheet.net_line_anchor_position(segment, line.p2(), tx.project().view()),
        ) else {
            continue;
        };
        if (horizontal && a.y != b.y) || (!horizontal && a.x != b.x) {
            continue;
        }
        let length = (a.x - b.x).abs() + (a.y - b.y).abs();
        let count = (length.to_nm() / 635_000).max(1);
        for index in 1..count {
            let position = Point::new(
                a.x + (b.x - a.x) * index / count,
                a.y + (b.y - a.y) * index / count,
            );
            if position == label.position() {
                continue;
            }
            let moved = relative + convert::point(position).to_vec2();
            let clear = scene.scene()
                .items_in_rect(moved.inflate(0.3, 0.3), SelectionMode::Intersects)
                .into_iter()
                .all(|id| {
                    if scene.object(id) == Some(target) {
                        return true;
                    }
                    if matches!(scene.object(id), Some(SchematicObject::NetLine(own, _)) if own == segment) {
                        return scene.scene().item(id).is_some_and(|item| {
                            let overlap = moved.intersect(item.bounding_box());
                            overlap.width() <= 0.1 || overlap.height() <= 0.1
                        });
                    }
                    false
                });
            if clear {
                label.set_position(position);
                tx.apply(Mutation::Schematic(SchematicMutation::UpdateNetLabel {
                    segment: NetSegmentRef {
                        schematic: page,
                        segment,
                    },
                    label,
                }))?;
                return Ok(true);
            }
        }
    }
    // Lengthen a terminal stub when its original line offers no free label
    // position. Pin anchors and the segment's connected membership stay fixed.
    for junction in net.junctions().values() {
        let anchor = NetLineAnchor::Junction(junction.uuid());
        let lines: Vec<_> = net.lines_at(anchor).collect();
        let [line] = lines.as_slice() else {
            continue;
        };
        let other = if line.p1() == anchor {
            line.p2()
        } else {
            line.p1()
        };
        let Some(peer) = sheet.net_line_anchor_position(segment, other, tx.project().view()) else {
            continue;
        };
        let delta = junction.position() - peer;
        if (horizontal && delta.y != Length::ZERO) || (!horizontal && delta.x != Length::ZERO) {
            continue;
        }
        let direction = Point::new(
            Length::new(delta.x.to_nm().signum()),
            Length::new(delta.y.to_nm().signum()),
        );
        for distance in 1..=64i64 {
            let position = junction.position()
                + Point::new(
                    direction.x * distance * 1_270_000,
                    direction.y * distance * 1_270_000,
                );
            let moved = relative + convert::point(position).to_vec2();
            let label_clear = scene.scene()
                .items_in_rect(moved.inflate(0.3, 0.3), SelectionMode::Intersects)
                .into_iter()
                .all(|id| {
                    scene.object(id) == Some(target)
                        || matches!(scene.object(id), Some(SchematicObject::NetLine(own, _)) if own == segment)
                });
            if !label_clear {
                continue;
            }
            let start = peer + Point::new(direction.x * 300_000, direction.y * 300_000);
            let wire = foilfern_scene::foilfern_canvas::kurbo::Rect::from_points(
                convert::point(start),
                convert::point(position),
            )
            .inflate(0.08, 0.08);
            let wire_clear = scene.scene()
                .items_in_rect(wire, SelectionMode::Intersects)
                .into_iter()
                .all(|id| {
                    scene.object(id) == Some(target)
                        || matches!(scene.object(id), Some(SchematicObject::NetLine(own, _)) | Some(SchematicObject::NetJunction(own, _)) if own == segment)
                });
            if wire_clear {
                let junction = junction.uuid();
                let reference = NetSegmentRef {
                    schematic: page,
                    segment,
                };
                label.set_position(position);
                tx.apply(Mutation::Schematic(
                    SchematicMutation::SetNetPointPosition {
                        segment: reference,
                        junction,
                        position,
                    },
                ))?;
                tx.apply(Mutation::Schematic(SchematicMutation::UpdateNetLabel {
                    segment: reference,
                    label,
                }))?;
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn move_pin_legend(
    tx: &mut Transaction<'_>,
    page: SchematicId,
    symbol: SymbolId,
    pin: Uuid,
) -> Result<Uuid> {
    let instance = &tx
        .project()
        .schematic(page)
        .ok_or_else(|| Error::not_found("Schematic", page))?
        .symbols()[&symbol];
    let lib = instance.resolve(tx.project().view())?.lib_symbol;
    let legend = lib.pins().required_by_uuid(&pin)?;
    // Name positions are in the pin's local frame, whose X axis points into
    // the symbol. Move toward its terminal to separate opposing legends.
    let position = legend.name_position() - Point::new(Length::new(635_000), Length::ZERO);
    let artwork = SymbolArtwork {
        symbol: instance.resolve(tx.project().view())?.gate.symbol_uuid(),
        polygons: lib.polygons().clone(),
        pin_name_positions: BTreeMap::from([(pin, position)]),
    };
    let id = artwork.symbol;
    tx.apply(Mutation::SetLibrarySymbolArtwork(artwork))?;
    Ok(id)
}
