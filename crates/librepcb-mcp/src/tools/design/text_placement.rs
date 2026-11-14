//! Bounded placement of one existing schematic symbol field.

use std::collections::BTreeMap;

use librepcb_core::project::{ComponentInstanceId, SchematicId, SymbolRef};
use librepcb_core::types::{Color, Point};
use librepcb_editor::Transaction;
use librepcb_editor::design_preview::DesignEdit;
use librepcb_graphics::{GraphicsSettings, schematic_field_geometry};
use librepcb_scene::{ColorScheme, SchematicScene, SymbolTextPlacement};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{ToolError, ToolResult};
use crate::units::{PointMm, length};
use crate::{resolve, views};

use super::ExistingComponent;

const MAX_TRIALS: i128 = 4096;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct PlaceText {
    /// Component key from the design document.
    component: String,
    /// `name` or `value` on a single symbol, or the placed field UUID.
    text: String,
    /// Bottom-left corner of the area that must contain the entire field, in mm.
    min: PointMm,
    /// Top-right corner of the area, in mm.
    max: PointMm,
    /// Spacing of trial positions in mm. Defaults to 0.635.
    step_mm: Option<f64>,
    /// Minimum clearance around the field in mm. Defaults to 0.5.
    clearance_mm: Option<f64>,
}

impl PlaceText {
    pub(super) fn component(&self) -> &str {
        &self.component
    }

    pub(super) fn place(
        &self,
        tx: &mut Transaction<'_>,
        page: SchematicId,
        bindings: &BTreeMap<&str, ComponentInstanceId>,
        existing: &BTreeMap<String, ExistingComponent>,
        checkpoint: impl Fn() -> ToolResult<()>,
        diff: &mut Vec<Value>,
    ) -> ToolResult<Value> {
        let component = *bindings.get(self.component.as_str()).ok_or_else(|| {
            ToolError::invalid(format!(
                "Unknown component key {:?} in place_text.",
                self.component
            ))
        })?;
        let min = self.min.to_point()?;
        let max = self.max.to_point()?;
        if min.x >= max.x || min.y >= max.y {
            return Err(ToolError::invalid(
                "place_text needs a rectangle with positive width and height.",
            ));
        }
        let step = i128::from(length(self.step_mm.unwrap_or(0.635))?.to_nm());
        let clearance = length(self.clearance_mm.unwrap_or(0.5))?;
        if step <= 0 || clearance.to_nm() < 0 {
            return Err(ToolError::invalid(
                "place_text needs a positive step_mm and a nonnegative clearance_mm.",
            ));
        }

        let project = tx.project();
        let sheet = project
            .schematic(page)
            .ok_or_else(|| ToolError::not_found("Schematic not found."))?;
        let mut symbols = sheet
            .symbols()
            .values()
            .filter(|symbol| symbol.component() == component);
        let (symbol, text) = if matches!(self.text.as_str(), "name" | "value") {
            let symbol = symbols.next().filter(|_| symbols.next().is_none()).ok_or_else(|| {
                ToolError::invalid("place_text role needs one symbol on this sheet. Pass the field UUID for a multi-gate component.")
            })?;
            (symbol.id(), resolve::symbol_text(symbol, &self.text)?)
        } else {
            let uuid = resolve::required_uuid(&self.text, "symbol text")?;
            symbols
                .find_map(|symbol| Some((symbol.id(), symbol.texts().get(&uuid)?)))
                .ok_or_else(|| {
                    ToolError::not_found(format!(
                        "Text {uuid} was not found on {}.",
                        self.component
                    ))
                })?
        };
        let uuid = text.uuid();
        if existing.get(&self.component).is_some_and(|spec| {
            spec.text_positions
                .keys()
                .any(|key| resolve::parse_uuid(key) == Some(uuid))
        }) {
            return Err(ToolError::invalid(
                "place_text and text_positions cannot target the same field.",
            ));
        }

        let current = text.position();
        let candidates = candidates(min, max, current, step)?;
        let mut evidence = json!({
            "component":self.component, "symbol":symbol, "text":uuid,
            "position":views::point(current), "trials":0,
        });
        if text.locked() {
            evidence["status"] = json!("locked");
            return Ok(evidence);
        }
        checkpoint()?;
        let scene = SchematicScene::build(project, page, &ColorScheme::SCHEMATIC_LIGHT)?;
        checkpoint()?;
        let Some(placement) = SymbolTextPlacement::new(&scene, symbol, uuid, current) else {
            evidence["status"] = json!("not_visible");
            return Ok(evidence);
        };
        let symbol_ref = SymbolRef {
            schematic: page,
            symbol,
        };
        let graphics = if sheet.images().is_empty() {
            None
        } else {
            let settings = GraphicsSettings {
                background: Color::WHITE,
                ..Default::default()
            };
            let Some(geometry) =
                schematic_field_geometry(project, sheet, symbol_ref, uuid, &settings)
                    .map_err(|error| ToolError::internal(error.to_string()))?
            else {
                evidence["status"] = json!("not_visible");
                return Ok(evidence);
            };
            checkpoint()?;
            Some(geometry)
        };
        let fits = |candidate: Point| {
            placement.fits(candidate, min, max, clearance.to_mm())
                && graphics.as_ref().is_none_or(|geometry| {
                    geometry.fits(current, candidate, min, max, clearance.to_mm())
                })
        };
        let mut trials = 1;
        if fits(current) {
            evidence["trials"] = json!(trials);
            evidence["status"] = json!("unchanged");
            return Ok(evidence);
        }
        for candidate in candidates {
            checkpoint()?;
            trials += 1;
            if fits(candidate) {
                let mut text = text.clone();
                text.set_position(candidate);
                DesignEdit::SymbolText {
                    symbol: symbol_ref,
                    text,
                }
                .execute(tx)?;
                diff.push(
                    json!({"component":self.component,"field":"text_position","text":uuid,
                    "before":views::point(current),"after":views::point(candidate)}),
                );
                evidence["position"] = views::point(candidate);
                evidence["status"] = json!("moved");
                evidence["trials"] = json!(trials);
                return Ok(evidence);
            }
        }
        evidence["status"] = json!("no_fit");
        evidence["trials"] = json!(trials);
        Ok(evidence)
    }
}

fn candidates(min: Point, max: Point, current: Point, step: i128) -> ToolResult<Vec<Point>> {
    let width = i128::from(max.x.to_nm()) - i128::from(min.x.to_nm());
    let height = i128::from(max.y.to_nm()) - i128::from(min.y.to_nm());
    let columns = (width + step - 1) / step + 1;
    let rows = (height + step - 1) / step + 1;
    if columns > MAX_TRIALS || rows > MAX_TRIALS || columns * rows > MAX_TRIALS {
        return Err(ToolError::invalid(
            "place_text exceeds 4096 trial positions. Use a smaller area or increase step_mm.",
        ));
    }
    let mut points = Vec::with_capacity((columns * rows) as usize);
    for column in 0..columns {
        let x = i128::from(min.x.to_nm()) + (column * step).min(width);
        for row in 0..rows {
            let y = i128::from(min.y.to_nm()) + (row * step).min(height);
            let candidate = Point::from_nm(x as i64, y as i64);
            if candidate != current {
                points.push(candidate);
            }
        }
    }
    let x = i128::from(current.x.to_nm());
    let y = i128::from(current.y.to_nm());
    let distance = |point: Point| {
        let dx = (i128::from(point.x.to_nm()) - x) as f64;
        let dy = (i128::from(point.y.to_nm()) - y) as f64;
        dx.hypot(dy)
    };
    points.sort_unstable_by(|&a, &b| distance(a).total_cmp(&distance(b)).then_with(|| a.cmp(&b)));
    Ok(points)
}
