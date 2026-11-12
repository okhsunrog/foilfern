//! Bounding-box checks for visible schematic annotations.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_canvas::kurbo::Rect;
use librepcb_core::project::{NetSegmentId, SymbolId};

use crate::{SchematicObject, SchematicScene};

/// An annotation and its overlapping neighbor, in schematic coordinates.
#[derive(Debug)]
pub struct AnnotationOverlap {
    pub first: SchematicObject,
    pub second: SchematicObject,
    pub bounds: [f64; 4],
}

/// Checks symbol fields, page texts and net labels. Limits the report to
/// overlaps involving the supplied symbols or net segments when provided.
pub fn annotation_overlaps(
    scene: &SchematicScene,
    scope: Option<(&BTreeSet<SymbolId>, &BTreeSet<NetSegmentId>)>,
) -> Vec<AnnotationOverlap> {
    let mut annotations = BTreeMap::new();
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
        let (order, owner, uuid) = match object {
            SchematicObject::SymbolText(symbol, uuid) => (0, Some(symbol.0), uuid),
            SchematicObject::Text(uuid) => (1, None, uuid),
            SchematicObject::NetLabel(net, uuid) => (2, Some(net.0), uuid),
            _ => continue,
        };
        let bounds = item.bounding_box();
        annotations
            .entry((order, owner, uuid))
            .and_modify(|(_, rect): &mut (_, Rect)| *rect = rect.union(bounds))
            .or_insert((object, bounds));
    }
    let mut annotations: Vec<_> = annotations.into_values().collect();
    annotations.sort_by(|(_, a), (_, b)| a.x0.total_cmp(&b.x0));
    let in_scope = |object| match (scope, object) {
        (None, _) => true,
        (Some((symbols, _)), SchematicObject::SymbolText(id, _)) => symbols.contains(&id),
        (Some((_, segments)), SchematicObject::NetLabel(id, _)) => segments.contains(&id),
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
