//! Placement of a symbol beside a pin of another placed symbol (original
//! code, no upstream counterpart).
//!
//! Typical use is support circuitry around an IC: a decoupling capacitor at
//! a supply pin, a crystal load capacitor at an oscillator pin, a series
//! resistor in a signal line. The placed symbol is rotated so that one of
//! its pins faces the target pin and moved onto the target pin's outward
//! axis, so a straight wire connects the two. The target stays fixed.

use std::collections::BTreeSet;

use foilfern_core::project::{
    Mutation, NetSegmentId, NetSegmentRef, SchematicId, SchematicMutation, SymbolId,
};
use foilfern_core::types::{Angle, Length, Point, Uuid};

use super::component::MoveSymbol;
use super::placement::{
    Rect, describe_blockers, named_obstacles, placed_symbol_obstacles, placed_symbol_rect,
    schematic_obstacles_except, sheet_annotation_obstacles,
};
use super::schematic_tidy::arrange_symbol_fields;
use crate::{Command, Error, Result, Transaction};

/// Places `symbol` so that its pin `pin` faces the placed pin `target` at
/// `distance` along the target pin's outward direction. If the symbol, its
/// fields or the straight connection would touch other schematic items, the
/// distance grows in page grid steps, at most `max_steps` times. A wire
/// stub of the target pin that connects nothing else (for example a net
/// label stub) is removed to make room; other wiring at the target pin is
/// an error. Name and value fields are arranged upright beside the body.
#[derive(Debug, Clone)]
pub struct PlaceAtPin {
    pub schematic: SchematicId,
    /// The symbol to place. It must have no wires yet.
    pub symbol: SymbolId,
    /// Library pin UUID of `symbol` which faces the target.
    pub pin: Uuid,
    /// The fixed target: placed symbol and library pin UUID.
    pub target: (SymbolId, Uuid),
    /// Initial distance between the two pins.
    pub distance: Length,
    /// Additional grid steps tried when the initial distance is blocked.
    pub max_steps: usize,
}

/// Where [`PlaceAtPin`] put the symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct PinPlacement {
    pub position: Point,
    pub rotation: Angle,
    /// The distance used between the two pins.
    pub distance: Length,
    /// Positions of the facing pin and the target pin.
    pub pin: Point,
    pub target: Point,
    /// A removed single-pin stub segment of the target pin.
    pub removed_stub: Option<NetSegmentId>,
}

/// Shrinks the straight connection at both ends so the touching pins do
/// not count as obstacles.
const CORRIDOR_END: Length = Length::new(100_000);

impl Command for PlaceAtPin {
    type Output = PinPlacement;

    fn text(&self) -> String {
        foilfern_i18n::tr!("CmdMoveSelectedSchematicItems", "Move Schematic Items")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<PinPlacement> {
        if self.symbol == self.target.0 {
            return Err(Error::InvalidArgument(
                "A symbol cannot be placed beside its own pin.".into(),
            ));
        }
        let sheet = tx
            .project()
            .schematic(self.schematic)
            .ok_or_else(|| Error::not_found("Schematic", self.schematic))?;
        let grid = *sheet.grid_interval();
        if self.distance <= Length::ZERO || self.distance > Length::new(1_000_000_000) {
            return Err(Error::InvalidArgument(
                "The pin distance must be positive and at most 1 m.".into(),
            ));
        }
        if sheet.net_segments().values().any(|segment| {
            segment
                .connected_pins()
                .iter()
                .any(|(owner, _)| *owner == self.symbol)
        }) {
            return Err(Error::InvalidArgument(format!(
                "Symbol {} already has wires; place it before wiring.",
                self.symbol
            )));
        }
        let target_symbol = sheet
            .symbols()
            .get(&self.target.0)
            .ok_or_else(|| Error::not_found("Symbol", self.target.0))?;
        let target = target_symbol
            .pins(tx.project().view())?
            .into_iter()
            .find(|pin| pin.uuid() == self.target.1)
            .ok_or_else(|| Error::not_found("Pin", self.target.1))?;
        let (target_position, target_rotation) = (target.position(), target.rotation());
        let outward = direction(target_rotation + Angle::DEG180).ok_or_else(|| {
            Error::InvalidArgument("The target pin is not horizontal or vertical.".into())
        })?;

        // Make room at the target pin: drop a stub which connects only it.
        let mut removed_stub = None;
        if let Some(segment) = sheet.pin_net_segment(self.target.0, self.target.1) {
            let pins = sheet.net_segments()[&segment].connected_pins();
            if pins.len() > 1
                || !sheet.net_segments()[&segment]
                    .connected_bus_junctions()
                    .is_empty()
            {
                return Err(Error::InvalidArgument(
                    "The target pin is already wired to other pins. Place the part before wiring the pin, or wire it explicitly.".into(),
                ));
            }
            tx.apply(Mutation::Schematic(SchematicMutation::RemoveNetSegment(
                NetSegmentRef {
                    schematic: self.schematic,
                    segment,
                },
            )))?;
            removed_stub = Some(segment);
        }

        // Orient the facing pin towards the target: its outward direction is
        // the target pin's rotation.
        tx.run(MoveSymbol {
            symbol: self.symbol,
            position: None,
            rotation: Some(Angle::DEG0),
            mirrored: Some(false),
        })?;
        let (_, unrotated) = self.facing_pin(tx)?;
        let rotation = (target_rotation + Angle::DEG180 - unrotated)
            .rounded(Angle::DEG90)
            .mapped_to_0_360deg();
        tx.run(MoveSymbol {
            symbol: self.symbol,
            position: None,
            rotation: Some(rotation),
            mirrored: None,
        })?;
        arrange_symbol_fields(tx, self.schematic, self.symbol)?;

        let excluded = BTreeSet::from([self.symbol, self.target.0]);
        let mut obstacles = schematic_obstacles_except(
            tx.project(),
            self.schematic,
            &excluded,
            &BTreeSet::new(),
            false,
        )?;
        let sheet = tx
            .project()
            .schematic(self.schematic)
            .ok_or_else(|| Error::not_found("Schematic", self.schematic))?;
        obstacles.extend(sheet_annotation_obstacles(tx.project(), sheet));
        // The target body (graphics and pins) only blocks the part; the
        // connection leaves from its pin. Its fields block both.
        let (target_body, target_fields) =
            placed_symbol_obstacles(tx.project(), self.schematic, self.target.0, true)?;
        obstacles.extend(target_fields);
        let mut blockers = Vec::new();
        for step in 0..=self.max_steps {
            let distance = self.distance + grid * step as i64;
            let wanted = Point::new(
                target_position.x + distance * outward.0,
                target_position.y + distance * outward.1,
            );
            let (current, _) = self.facing_pin(tx)?;
            let position = self.symbol_position(tx)? + (wanted - current);
            tx.run(MoveSymbol {
                symbol: self.symbol,
                position: Some(position),
                rotation: None,
                mirrored: None,
            })?;
            let bounds = placed_symbol_rect(tx.project(), self.schematic, self.symbol, true)?;
            let corridor = corridor(target_position, wanted, outward);
            let blocked = target_body.intersects(&bounds)
                || obstacles
                    .iter()
                    .any(|obstacle| obstacle.intersects(&bounds) || obstacle.intersects(&corridor));
            if blocked {
                if step == 0 || step == self.max_steps {
                    let mut named = named_obstacles(
                        tx.project(),
                        self.schematic,
                        &BTreeSet::from([self.symbol]),
                        &BTreeSet::new(),
                    )?;
                    named.retain(|(rect, _)| *rect != target_body);
                    named.push((
                        target_body,
                        format!("{} body", target_name(tx, self.target.0)),
                    ));
                    for name in describe_blockers(&named, &[bounds, corridor]) {
                        if !blockers.contains(&name) {
                            blockers.push(name);
                        }
                    }
                }
                continue;
            }
            return Ok(PinPlacement {
                position,
                rotation,
                distance,
                pin: wanted,
                target: target_position,
                removed_stub,
            });
        }
        let last = self.distance + grid * self.max_steps as i64;
        Err(Error::InvalidArgument(format!(
            "No clear position from {} to {} mm beside the target pin; the part, its fields or its straight connection touch {}. Free that space, move the target, or choose another distance.",
            self.distance.to_mm(),
            last.to_mm(),
            if blockers.is_empty() {
                "other items".to_owned()
            } else {
                blockers.join(", ")
            }
        )))
    }
}

impl PlaceAtPin {
    fn facing_pin(&self, tx: &Transaction<'_>) -> Result<(Point, Angle)> {
        let sheet = tx
            .project()
            .schematic(self.schematic)
            .ok_or_else(|| Error::not_found("Schematic", self.schematic))?;
        let symbol = sheet
            .symbols()
            .get(&self.symbol)
            .ok_or_else(|| Error::not_found("Symbol", self.symbol))?;
        symbol
            .pins(tx.project().view())?
            .into_iter()
            .find(|pin| pin.uuid() == self.pin)
            .map(|pin| (pin.position(), pin.rotation()))
            .ok_or_else(|| Error::not_found("Pin", self.pin))
    }

    fn symbol_position(&self, tx: &Transaction<'_>) -> Result<Point> {
        tx.project()
            .schematic(self.schematic)
            .and_then(|sheet| sheet.symbols().get(&self.symbol))
            .map(|symbol| symbol.position())
            .ok_or_else(|| Error::not_found("Symbol", self.symbol))
    }
}

/// Unit direction of an orthogonal angle, `None` otherwise.
fn direction(angle: Angle) -> Option<(i64, i64)> {
    match angle.mapped_to_0_360deg() {
        a if a == Angle::DEG0 => Some((1, 0)),
        a if a == Angle::DEG90 => Some((0, 1)),
        a if a == Angle::DEG180 => Some((-1, 0)),
        a if a == Angle::DEG270 => Some((0, -1)),
        _ => None,
    }
}

/// The straight connection between two pins on one axis, without its ends.
fn corridor(from: Point, to: Point, (dx, dy): (i64, i64)) -> Rect {
    let shrink = Point::new(CORRIDOR_END * dx, CORRIDOR_END * dy);
    Rect::bounding([from + shrink, to - shrink]).unwrap_or(Rect { min: from, max: to })
}

fn target_name(tx: &Transaction<'_>, symbol: SymbolId) -> String {
    tx.project()
        .schematics()
        .iter()
        .find_map(|sheet| sheet.symbols().get(&symbol))
        .map_or_else(
            || symbol.to_string(),
            |symbol| super::resolve::component_name(tx.project(), symbol.component()),
        )
}
