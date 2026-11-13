//! Bounding-box checks for visible schematic annotations.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_canvas::kurbo::Rect;
use librepcb_core::project::{NetSegmentId, SymbolId};
use librepcb_core::types::Uuid;

use crate::{SchematicObject, SchematicScene};

/// An annotation and its overlapping neighbor, in schematic coordinates.
#[derive(Debug)]
pub struct AnnotationOverlap {
    pub first: Annotation,
    pub second: Annotation,
    pub bounds: [f64; 4],
}

/// The visible item whose text participates in a readability finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Annotation {
    SymbolText(SymbolId, Uuid),
    Text(Uuid),
    NetLabel(NetSegmentId, Uuid),
    PinName(SymbolId, Uuid),
    PinNumber(SymbolId, Uuid),
}

/// Checks symbol fields, page texts, net labels and pin legends. Limits the
/// report to overlaps involving the supplied symbols or segments when provided.
pub fn annotation_overlaps(
    scene: &SchematicScene,
    scope: Option<(&BTreeSet<SymbolId>, &BTreeSet<NetSegmentId>)>,
) -> Vec<AnnotationOverlap> {
    let mut annotations = BTreeMap::new();
    let pin_names = SchematicScene::layer_id("schematic_pin_names");
    let pin_numbers = SchematicScene::layer_id("schematic_pin_numbers");
    for (id, item) in scene.scene().items() {
        if !scene
            .scene()
            .layer(item.layer)
            .is_some_and(|layer| layer.visible)
        {
            continue;
        }
        let Some(object) = scene.object(id) else {
            continue;
        };
        let annotation = match object {
            SchematicObject::SymbolText(symbol, uuid) => Annotation::SymbolText(symbol, uuid),
            SchematicObject::Text(uuid) => Annotation::Text(uuid),
            SchematicObject::NetLabel(net, uuid) => Annotation::NetLabel(net, uuid),
            SchematicObject::SymbolPin(symbol, pin) if Some(item.layer) == pin_names => {
                Annotation::PinName(symbol, pin)
            }
            SchematicObject::SymbolPin(symbol, pin) if Some(item.layer) == pin_numbers => {
                Annotation::PinNumber(symbol, pin)
            }
            _ => continue,
        };
        let bounds = item.bounding_box();
        annotations
            .entry(annotation)
            .and_modify(|rect: &mut Rect| *rect = rect.union(bounds))
            .or_insert(bounds);
    }
    let mut annotations: Vec<_> = annotations.into_iter().collect();
    annotations.sort_by(|(_, a), (_, b)| a.x0.total_cmp(&b.x0));
    let in_scope = |object| match (scope, object) {
        (None, _) => true,
        (
            Some((symbols, _)),
            Annotation::SymbolText(id, _)
            | Annotation::PinName(id, _)
            | Annotation::PinNumber(id, _),
        ) => symbols.contains(&id),
        (Some((_, segments)), Annotation::NetLabel(id, _)) => segments.contains(&id),
        _ => false,
    };
    let mut result = Vec::new();
    for (index, &(first, a)) in annotations.iter().enumerate() {
        for &(second, b) in annotations[index + 1..]
            .iter()
            .take_while(|(_, b)| b.x0 < a.x1)
        {
            if !in_scope(first) && !in_scope(second) {
                continue;
            }
            let overlap = a.intersect(b);
            // Ignore small contacts from stroke width and numeric rounding.
            if overlap.width() > 0.1 && overlap.height() > 0.1 {
                result.push(AnnotationOverlap {
                    first,
                    second,
                    bounds: [overlap.x0, overlap.y0, overlap.x1, overlap.y1],
                });
            }
        }
    }
    result
}
