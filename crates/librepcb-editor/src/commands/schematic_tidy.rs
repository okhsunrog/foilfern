//! Re-layout of a schematic page by connectivity (no upstream counterpart:
//! upstream users arrange symbols by hand). Used by agents after adding
//! and connecting parts without coordinates.
//!
//! [`TidySchematic`] removes the wiring of the page, places the symbols
//! again with a greedy placement (hand-written, deterministic): the
//! symbol with the most connections first, then repeatedly the symbol most
//! strongly connected to the placed ones at the free grid position which
//! minimizes the weighted distance of its pins to the pins of the same nets
//! (every symbol keeps room for pin labels, see
//! [`symbol_room_rect()`](super::placement)). Supply symbols are not placed
//! but moved next to a pin of their net when the nets are drawn again with
//! [`ConnectNet`] (shortest nets first, direct wires up to 38.1 mm). No
//! crate places schematic symbols against LibrePCB geometry.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::project::{
    ComponentSignalRef, Mutation, NetSegmentRef, NetSignalId, SchematicId, SchematicMutation,
    SymbolId,
};
use librepcb_core::types::{Length, Point, UnsignedLength};
use librepcb_i18n::tr;

use super::component::MoveSymbol;
use super::placement::{AutoPlaceSymbols, Rect, SCHEMATIC_AREA, is_frame, symbol_room_rect};
use super::resolve;
use super::wiring::{ConnectNet, supply_signal};
use crate::editor::{Command, Transaction};
use crate::error::{Error, Result};

/// Placement grid of the symbols.
const GRID: Length = Length::new(2_540_000);
/// Space between the rooms of neighboring symbols.
const MARGIN: Length = Length::new(2_540_000);
/// Maximum length of direct wires when the nets are drawn again.
const MAX_WIRE_LENGTH: Length = Length::new(38_100_000);

/// Re-arranges the symbols of a schematic page by connectivity and draws
/// the nets again (see the [module docs](self)). Nets, net classes and the
/// boards are not changed.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TidySchematic {
    /// The page (default: the first one).
    #[serde(default)]
    pub schematic: Option<SchematicId>,
}

/// Result of [`TidySchematic`].
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TidiedSchematic {
    /// The page.
    pub schematic: Option<SchematicId>,
    /// Number of symbols placed by connectivity.
    pub placed: usize,
    /// Number of supply symbols (placed next to their pins).
    pub supplies: usize,
    /// Number of nets drawn again.
    pub nets: usize,
    /// Number of direct wires between pins.
    pub wires: usize,
    /// Number of net labels.
    pub labels: usize,
    /// Hints from drawing the nets.
    pub warnings: Vec<String>,
}

/// A symbol to place: its room relative to its origin and its pins.
struct Item {
    symbol: SymbolId,
    room: Rect,
    /// Pins as (net, offset from the symbol origin).
    pins: Vec<(NetSignalId, Point)>,
}

impl Command for TidySchematic {
    type Output = TidiedSchematic;

    fn text(&self) -> String {
        tr!("CmdMoveSelectedSchematicItems", "Move Schematic Items")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<TidiedSchematic> {
        let p = tx.project();
        let schematic = resolve::schematic(p, self.schematic)?.id();
        let s = p
            .schematic(schematic)
            .ok_or_else(|| Error::not_found("Schematic", schematic))?;

        // Symbols and the nets of their pins.
        let mut items = Vec::new();
        let mut supplies = Vec::new();
        let mut nets: BTreeMap<NetSignalId, BTreeSet<ComponentSignalRef>> = BTreeMap::new();
        let mut symbols: Vec<(String, SymbolId)> = s
            .symbols()
            .iter()
            .map(|(id, sym)| (resolve::component_name(p, sym.component()), *id))
            .collect();
        symbols.sort_by(|a, b| super::placement::natural_cmp(&a.0, &b.0));
        for (_, id) in symbols {
            if is_frame(p, schematic, id)? {
                continue;
            }
            let sym = &s.symbols()[&id];
            let mut pins = Vec::new();
            for pin in sym.pins(p.view())? {
                if let Some(net) = pin.net() {
                    nets.entry(net).or_default().insert(pin.signal());
                    pins.push((net, pin.position() - sym.position()));
                }
            }
            if supply_signal(p, sym.component()).is_some() {
                supplies.push(id);
            } else {
                let room =
                    symbol_room_rect(p, schematic, id)?.translated(Point::ORIGIN - sym.position());
                items.push(Item {
                    symbol: id,
                    room,
                    pins,
                });
            }
        }
        let supply_nets: BTreeSet<NetSignalId> = supplies
            .iter()
            .filter_map(|id| {
                let sym = s.symbols().get(id)?;
                let (signal, _) = supply_signal(p, sym.component())?;
                p.circuit()
                    .component_instance(signal.component)?
                    .signal(&signal.signal)?
                    .net()
            })
            .collect();

        // Remove the wiring (the nets stay: the component signals keep
        // them).
        let segments: Vec<_> = s.net_segments().keys().copied().collect();
        for segment in segments {
            tx.apply(Mutation::Schematic(SchematicMutation::RemoveNetSegment(
                NetSegmentRef { schematic, segment },
            )))?;
        }

        // Place by connectivity, then move the group to the top left.
        let positions = place(&items, &supply_nets);
        if let Some(group) = Rect::bounding(items.iter().zip(&positions).flat_map(|(item, pos)| {
            let r = item.room.translated(*pos);
            [r.min, r.max]
        })) {
            let (left, top, _) = SCHEMATIC_AREA;
            let shift = Point::new(snap(left - group.min.x), snap(top - group.max.y));
            for (item, pos) in items.iter().zip(&positions) {
                tx.run(MoveSymbol {
                    symbol: item.symbol,
                    position: Some(*pos + shift),
                    rotation: None,
                    mirrored: None,
                })?;
            }
        }
        // Supply symbols out of the way first; drawing their net moves them
        // next to a pin.
        if !supplies.is_empty() {
            tx.run(AutoPlaceSymbols {
                symbols: supplies.clone(),
                margin: None,
            })?;
        }

        // Draw the nets again, shortest first.
        let mut order: Vec<(usize, String, NetSignalId)> = nets
            .iter()
            .map(|(net, signals)| {
                (
                    signals.len(),
                    resolve::net_name(tx.project(), Some(*net)),
                    *net,
                )
            })
            .collect();
        order.sort();
        let mut result = TidiedSchematic {
            schematic: Some(schematic),
            placed: items.len(),
            supplies: supplies.len(),
            ..Default::default()
        };
        let max_wire_length = UnsignedLength::new(MAX_WIRE_LENGTH).ok();
        for (_, _, net) in order {
            let Some(name) = tx
                .project()
                .circuit()
                .net_signal(net)
                .map(|n| n.name().clone())
            else {
                continue;
            };
            let signals: Vec<ComponentSignalRef> = nets[&net].iter().copied().collect();
            let drawn = tx.run(ConnectNet {
                net: Some(name),
                signals,
                max_wire_length,
            })?;
            result.nets += 1;
            result.wires += drawn.wires;
            result.labels += drawn.labels;
            result.warnings.extend(drawn.warnings);
        }
        Ok(result)
    }
}

/// Rounds to the placement grid.
fn snap(v: Length) -> Length {
    let g = GRID.to_nm();
    Length::new((v.to_nm() as f64 / g as f64).round() as i64 * g)
}

fn manhattan(a: Point, b: Point) -> i64 {
    (a.x - b.x).abs().to_nm() + (a.y - b.y).abs().to_nm()
}

/// Greedy placement (see the [module docs](self)); returns the origin of
/// each item, relative to the first one.
fn place(items: &[Item], supply_nets: &BTreeSet<NetSignalId>) -> Vec<Point> {
    let n = items.len();
    // Connection weights: every net contributes 1 / (symbols - 1) to each
    // pair of its symbols (less for supply nets, which are drawn with
    // supply symbols anyway).
    let mut net_items: BTreeMap<NetSignalId, BTreeSet<usize>> = BTreeMap::new();
    for (i, item) in items.iter().enumerate() {
        for (net, _) in &item.pins {
            net_items.entry(*net).or_default().insert(i);
        }
    }
    let net_weight = |net: &NetSignalId, count: usize| {
        let base = if supply_nets.contains(net) { 0.3 } else { 1.0 };
        base / (count.max(2) - 1) as f64
    };
    let mut weight = vec![vec![0.0; n]; n];
    for (net, members) in &net_items {
        let w = net_weight(net, members.len());
        for a in members {
            for b in members {
                if a != b {
                    weight[*a][*b] += w;
                }
            }
        }
    }
    let total: Vec<f64> = weight.iter().map(|row| row.iter().sum()).collect();

    let mut positions: Vec<Option<Point>> = vec![None; n];
    let mut rects: Vec<Rect> = Vec::new();
    for step in 0..n {
        // The next item: most strongly connected to the placed ones (first:
        // the most connected one), ties in designator order.
        let next = (0..n).filter(|i| positions[*i].is_none()).max_by(|a, b| {
            let key = |i: usize| {
                let placed: f64 = (0..n)
                    .filter(|j| positions[*j].is_some())
                    .map(|j| weight[i][j])
                    .sum();
                (placed, total[i])
            };
            let (ka, kb) = (key(*a), key(*b));
            ka.partial_cmp(&kb)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.cmp(a))
        });
        let Some(i) = next else {
            break;
        };
        let item = &items[i];
        if step == 0 {
            positions[i] = Some(Point::ORIGIN);
            rects.push(item.room.expanded(MARGIN / 2));
            continue;
        }
        // Pins of the placed items on the nets of this item.
        let placed_pins: Vec<(NetSignalId, Point)> = item
            .pins
            .iter()
            .flat_map(|(net, _)| net_items.get(net).into_iter().flatten())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter_map(|j| Some((positions[*j]?, &items[*j].pins)))
            .flat_map(|(pos, pins)| pins.iter().map(move |(net, offset)| (*net, pos + *offset)))
            .collect();
        // Search around the centroid of the connected placed items (or of
        // all placed items).
        let neighbors: Vec<Point> = (0..n)
            .filter(|j| weight[i][*j] > 0.0)
            .filter_map(|j| positions[j])
            .collect();
        let all: Vec<Point> = positions.iter().flatten().copied().collect();
        let base = if neighbors.is_empty() {
            &all
        } else {
            &neighbors
        };
        let count = base.len().max(1) as i64;
        let center = Point::new(
            snap(Length::new(
                base.iter().map(|p| p.x.to_nm()).sum::<i64>() / count,
            )),
            snap(Length::new(
                base.iter().map(|p| p.y.to_nm()).sum::<i64>() / count,
            )),
        );
        let all_center = Point::new(
            Length::new(all.iter().map(|p| p.x.to_nm()).sum::<i64>() / all.len().max(1) as i64),
            Length::new(all.iter().map(|p| p.y.to_nm()).sum::<i64>() / all.len().max(1) as i64),
        );
        let cost = |origin: Point| -> f64 {
            let mut c = 0.0;
            for (net, offset) in &item.pins {
                let pin = origin + *offset;
                let w = net_weight(net, net_items.get(net).map_or(2, |m| m.len()));
                let nearest = placed_pins
                    .iter()
                    .filter(|(n, _)| n == net)
                    .map(|(_, q)| manhattan(pin, *q))
                    .min();
                if let Some(d) = nearest {
                    c += w * d as f64;
                }
            }
            // Keep the layout compact (and wide rather than tall).
            let dx = (origin.x - all_center.x).abs().to_nm() as f64;
            let dy = (origin.y - all_center.y).abs().to_nm() as f64;
            c + 0.05 * dx + 0.1 * dy
        };
        let mut best: Option<(f64, Point)> = None;
        let mut radius = 20;
        while best.is_none() && radius <= 320 {
            for gx in -radius..=radius {
                for gy in -radius..=radius {
                    let origin = Point::new(center.x + GRID * gx, center.y + GRID * gy);
                    let r = item.room.translated(origin).expanded(MARGIN / 2);
                    if rects.iter().any(|o| o.intersects(&r)) {
                        continue;
                    }
                    let c = cost(origin);
                    if best.is_none_or(|(bc, _)| c < bc) {
                        best = Some((c, origin));
                    }
                }
            }
            radius *= 2;
        }
        // The search area always contains free positions eventually; as a
        // fallback, place right of everything.
        let origin = best.map(|(_, o)| o).unwrap_or_else(|| {
            let right = rects.iter().map(|r| r.max.x).max().unwrap_or(Length::ZERO);
            Point::new(snap(right - item.room.min.x + MARGIN), Length::ZERO)
        });
        positions[i] = Some(origin);
        rects.push(item.room.translated(origin).expanded(MARGIN / 2));
    }
    positions
        .into_iter()
        .map(|p| p.unwrap_or(Point::ORIGIN))
        .collect()
}
