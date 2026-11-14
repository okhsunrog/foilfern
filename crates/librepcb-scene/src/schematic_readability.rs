//! Bounding-box checks for visible schematic annotations.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_canvas::kurbo::Rect;
use librepcb_canvas::{SelectionMode, convert};
use librepcb_core::project::{NetSegmentId, SymbolId};
use librepcb_core::types::{Point, Uuid};

use crate::{SchematicObject, SchematicScene};

/// Checks translations of a placed symbol field against the visible sheet.
pub struct SymbolTextPlacement<'a> {
    scene: &'a SchematicScene,
    target: SchematicObject,
    relative_bounds: Rect,
}

impl<'a> SymbolTextPlacement<'a> {
    /// Returns `None` if the field has no visible text in the scene.
    pub fn new(
        scene: &'a SchematicScene,
        symbol: SymbolId,
        text: Uuid,
        position: Point,
    ) -> Option<Self> {
        let target = SchematicObject::SymbolText(symbol, text);
        let bounds = scene
            .scene()
            .items()
            .filter(|(id, item)| {
                scene.object(*id) == Some(target)
                    && scene
                        .scene()
                        .layer(item.layer)
                        .is_some_and(|layer| layer.visible)
            })
            .map(|(_, item)| item.bounding_box())
            .reduce(|a, b| a.union(b))?;
        Some(Self {
            scene,
            target,
            relative_bounds: bounds - convert::point(position).to_vec2(),
        })
    }

    /// Tests the field's full bounds and a margin around its visible geometry.
    pub fn fits(&self, position: Point, min: Point, max: Point, clearance_mm: f64) -> bool {
        let bounds = self.relative_bounds + convert::point(position).to_vec2();
        let region = Rect::from_points(convert::point(min), convert::point(max));
        if !region.contains_rect(bounds) {
            return false;
        }
        let clearance = bounds.inflate(clearance_mm, clearance_mm);
        self.scene
            .scene()
            .items_in_rect(clearance, SelectionMode::Intersects)
            .into_iter()
            .all(|id| self.scene.object(id) == Some(self.target))
    }
}

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
    Wire(NetSegmentId, Uuid),
}

/// Checks visible annotations against each other and against schematic wires.
/// Limits the report to overlaps involving the supplied symbols or their labels.
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
    for (annotation, bounds) in annotations {
        for id in scene
            .scene()
            .items_in_rect(bounds, SelectionMode::Intersects)
        {
            let Some(SchematicObject::NetLine(segment, wire)) = scene.object(id) else {
                continue;
            };
            if !in_scope(annotation)
                && !scope.is_some_and(|(_, segments)| segments.contains(&segment))
            {
                continue;
            }
            let Some(item) = scene.scene().item(id) else {
                continue;
            };
            let overlap = bounds.intersect(item.bounding_box());
            // A net label intentionally touches its own wire at the anchor.
            if matches!(annotation, Annotation::NetLabel(label_segment, _) if label_segment == segment)
                && overlap.width() <= 0.2
                && overlap.height() <= 0.2
            {
                continue;
            }
            if overlap.width() > 0.1 && overlap.height() > 0.1 {
                result.push(AnnotationOverlap {
                    first: annotation,
                    second: Annotation::Wire(segment, wire),
                    bounds: [overlap.x0, overlap.y0, overlap.x1, overlap.y1],
                });
            }
        }
    }
    result
}
