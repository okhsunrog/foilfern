//! Bounded placement of an ordered group on an existing schematic.

use std::collections::BTreeSet;

use librepcb_core::project::{
    Mutation, NetSegmentId, NetSegmentRef, SchematicId, SchematicMutation, SymbolId,
};
use librepcb_core::types::{Length, Point, UnsignedLength};

use super::component::MoveSymbol;
use super::placement::{
    Rect, is_frame, placed_symbol_rect, schematic_obstacles_except, segment_obstacles,
    sheet_annotation_obstacles,
};
use crate::{Command, Error, Result, Transaction};

/// The direction in which to place successive members.
#[derive(Debug, Clone, Copy)]
pub enum BlockDirection {
    Row,
    Column,
}

/// Packs symbols into a single row or column inside an explicit rectangle.
/// Single-symbol wire segments move together with their symbols.
pub struct PlaceSchematicBlock {
    pub schematic: SchematicId,
    pub symbols: Vec<SymbolId>,
    pub area: Rect,
    pub direction: BlockDirection,
    pub gap: UnsignedLength,
    /// Shared Y (row) or X (column) coordinate for member origins.
    pub baseline: Option<Length>,
}

struct Member {
    symbol: SymbolId,
    origin: Point,
    bounds: Rect,
    segments: Vec<NetSegmentId>,
}

impl Command for PlaceSchematicBlock {
    type Output = Vec<(SymbolId, Point, Point)>;

    fn text(&self) -> String {
        librepcb_i18n::tr!("CmdMoveSelectedSchematicItems", "Move Schematic Items")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<Self::Output> {
        let project = tx.project();
        let sheet = project
            .schematic(self.schematic)
            .ok_or_else(|| Error::not_found("Schematic", self.schematic))?;
        let selected: BTreeSet<_> = self.symbols.iter().copied().collect();
        if selected.is_empty() || selected.len() != self.symbols.len() {
            return Err(Error::InvalidArgument(
                "A block requires distinct movable symbols.".into(),
            ));
        }
        let limit = Length::new(10_000_000_000);
        if [
            self.area.min.x,
            self.area.min.y,
            self.area.max.x,
            self.area.max.y,
        ]
        .into_iter()
        .any(|v| v < -limit || v > limit)
            || self.area.min.x >= self.area.max.x
            || self.area.min.y >= self.area.max.y
            || *self.gap > limit
            || *sheet.grid_interval() > limit
            || self.baseline.is_some_and(|v| v < -limit || v > limit)
        {
            return Err(Error::InvalidArgument(
                "Expected a nonempty block area and layout parameters within 10 m.".into(),
            ));
        }

        let mut members = Vec::new();
        let mut moving_segments = BTreeSet::new();
        for symbol in self.symbols {
            let instance = sheet
                .symbols()
                .get(&symbol)
                .ok_or_else(|| Error::not_found("Symbol", symbol))?;
            if is_frame(project, self.schematic, symbol)? {
                return Err(Error::InvalidArgument(format!(
                    "Keep schematic frame {symbol} outside the movable members."
                )));
            }
            let mut bounds = placed_symbol_rect(project, self.schematic, symbol, true)?;
            let mut segments = Vec::new();
            for segment in sheet.net_segments().values() {
                let pins = segment.connected_pins();
                if !pins.iter().any(|(owner, _)| *owner == symbol) {
                    continue;
                }
                if pins.iter().any(|(owner, _)| *owner != symbol)
                    || !segment.connected_bus_junctions().is_empty()
                {
                    return Err(Error::InvalidArgument(format!(
                        "Symbol {symbol} shares wire segment {}. Keep it fixed or choose a group with independent wire stubs.",
                        segment.id()
                    )));
                }
                for rect in segment_obstacles(project, sheet, segment) {
                    bounds = Rect::bounding([bounds.min, bounds.max, rect.min, rect.max])
                        .unwrap_or(bounds);
                }
                segments.push(segment.id());
                moving_segments.insert(segment.id());
            }
            members.push(Member {
                symbol,
                origin: instance.position(),
                bounds,
                segments,
            });
        }
        let mut obstacles = schematic_obstacles_except(
            project,
            self.schematic,
            &selected,
            &moving_segments,
            false,
        )?;
        obstacles.extend(sheet_annotation_obstacles(project, sheet));
        let mut cursor = match self.direction {
            BlockDirection::Row => self.area.min.x,
            BlockDirection::Column => -self.area.max.y,
        };
        let gap = *self.gap;
        let grid = sheet.grid_interval().to_nm();
        let floor_grid = |length: Length| Length::new(length.to_nm().div_euclid(grid) * grid);
        let ceil_grid = |length: Length| -floor_grid(-length);
        let nearest_grid = |length: Length| floor_grid(length + Length::new(grid / 2));
        let clearance = (gap / 2).max(Length::new(1));
        let mut placements = Vec::new();
        for member in &members {
            let position = loop {
                // Translate on the existing grid phase to keep wire geometry
                // and make repeated placement stable.
                let delta = match self.direction {
                    BlockDirection::Row => Point::new(
                        ceil_grid(cursor - member.bounds.min.x),
                        self.baseline.map_or_else(
                            || floor_grid(self.area.max.y - member.bounds.max.y),
                            |y| nearest_grid(y - member.origin.y),
                        ),
                    ),
                    BlockDirection::Column => Point::new(
                        self.baseline.map_or_else(
                            || ceil_grid(self.area.min.x - member.bounds.min.x),
                            |x| nearest_grid(x - member.origin.x),
                        ),
                        floor_grid(-cursor - member.bounds.max.y),
                    ),
                };
                let bounds = member.bounds.translated(delta);
                if !self.area.contains(&bounds) {
                    return Err(Error::InvalidArgument(format!(
                        "Symbol {} does not fit in the requested block area.",
                        member.symbol
                    )));
                }
                let past_obstacles = obstacles
                    .iter()
                    .filter(|obstacle| obstacle.intersects(&bounds.expanded(clearance)))
                    .map(|obstacle| match self.direction {
                        BlockDirection::Row => obstacle.max.x,
                        BlockDirection::Column => -obstacle.min.y,
                    })
                    .max();
                let Some(past_obstacles) = past_obstacles else {
                    break (member.origin + delta, bounds);
                };
                cursor = past_obstacles + clearance;
            };
            cursor = match self.direction {
                BlockDirection::Row => position.1.max.x + gap,
                BlockDirection::Column => -position.1.min.y + gap,
            };
            placements.push(position.0);
        }

        let mut moved = Vec::new();
        for (member, position) in members.into_iter().zip(placements) {
            let delta = position - member.origin;
            if delta.is_origin() {
                continue;
            }
            tx.run(MoveSymbol {
                symbol: member.symbol,
                position: Some(position),
                rotation: None,
                mirrored: None,
            })?;
            for id in member.segments {
                let segment = NetSegmentRef {
                    schematic: self.schematic,
                    segment: id,
                };
                let current = tx
                    .project()
                    .schematic(self.schematic)
                    .and_then(|sheet| sheet.net_segments().get(&id))
                    .ok_or_else(|| Error::not_found("Net segment", id))?;
                let mutations: Vec<_> = current
                    .junctions()
                    .values()
                    .map(|junction| SchematicMutation::SetNetPointPosition {
                        segment,
                        junction: junction.uuid(),
                        position: junction.position() + delta,
                    })
                    .chain(current.labels().values().cloned().map(|mut label| {
                        label.set_position(label.position() + delta);
                        SchematicMutation::UpdateNetLabel { segment, label }
                    }))
                    .map(Mutation::Schematic)
                    .collect();
                tx.apply_all(mutations)?;
            }
            moved.push((member.symbol, member.origin, position));
        }
        Ok(moved)
    }
}
