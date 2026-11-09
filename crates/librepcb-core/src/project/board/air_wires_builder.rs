//! Port of libs/librepcb/core/project/board/boardairwiresbuilder.{h,cpp}.
//!
//! Differences to upstream: the builder collects the anchors of all nets
//! of a board in one pass ([`BoardAirWiresBuilder::new()`]) and then builds
//! the air wires of single nets from it; the anchors are identified by
//! [`TraceAnchor`]s. The points are added in the order of upstream's
//! registration lists after loading a project (footprint pads by component,
//! component signal and pad UUID, then per net segment the standalone
//! pads, vias and junctions by UUID), since the order decides between
//! equally long air wires. Point-in-fragment tests use the emulated
//! `QPainterPath::contains()` like upstream.
//!
//! Also contains the project level entry points [`Project::rebuild_air_wires()`]
//! and [`Project::force_air_wires_rebuild()`] (upstream
//! `Board::triggerAirWiresRebuild()` and `forceAirWiresRebuild()`).

use std::collections::{BTreeMap, BTreeSet};

use super::pad_data::PadOnBoard;
use super::{AirWire, Board, BoardChange};
use crate::algorithm::AirWiresBuilder;
use crate::geometry::{Path, TraceAnchor};
use crate::project::Project;
use crate::project::change::Change;
use crate::project::circuit::Circuit;
use crate::project::error::{EntityKind, Error, Result};
use crate::project::id::{BoardId, NetSignalId, PlaneId};
use crate::project::library::ProjectLibrary;
use crate::types::{Layer, Point};
use crate::utils::painter_path::PainterPathPx;

/// An anchor with its position and copper layer span (copper numbers).
#[derive(Debug, Clone, Copy)]
struct AnchorPoint {
    anchor: TraceAnchor,
    position: Point,
    start_layer: usize,
    end_layer: usize,
}

/// The anchors and connections of one net on a board.
#[derive(Debug, Clone, Default)]
struct NetItems {
    points: Vec<AnchorPoint>,
    traces: Vec<(TraceAnchor, TraceAnchor)>,
    planes: Vec<(Layer, PlaneId)>,
}

/// Builds the air wires of the nets of a board (upstream
/// `BoardAirWiresBuilder`).
#[derive(Debug, Clone)]
pub struct BoardAirWiresBuilder<'a> {
    board: &'a Board,
    nets: BTreeMap<NetSignalId, NetItems>,
}

impl<'a> BoardAirWiresBuilder<'a> {
    /// Collects the anchors of all nets of `board`. Devices whose library
    /// elements are missing are skipped (they cannot be in a validated
    /// project).
    pub fn new(board: &'a Board, library: &ProjectLibrary, circuit: &Circuit) -> Self {
        let mut nets: BTreeMap<NetSignalId, NetItems> = BTreeMap::new();
        // Footprint pads, in upstream's registration order: component
        // signals of the net (by component, then signal), then the pads of
        // each signal (by pad UUID).
        let mut footprint_pads = Vec::new();
        for device in board.devices().values() {
            let Ok(pads) = device.pads(library, circuit) else {
                continue;
            };
            for pad in pads {
                let (Some(net), Some(signal)) = (pad.net(), pad.component_signal()) else {
                    continue;
                };
                let (start_layer, end_layer) = if pad.properties().is_tht() {
                    (
                        Layer::TOP_COPPER.copper_number(),
                        Layer::BOT_COPPER.copper_number(),
                    )
                } else {
                    let layer = pad.solder_layer().copper_number();
                    (layer, layer)
                };
                let point = AnchorPoint {
                    anchor: pad.trace_anchor(),
                    position: pad.position(),
                    start_layer,
                    end_layer,
                };
                footprint_pads.push(((signal, pad.uuid()), net, point));
            }
        }
        footprint_pads.sort_by_key(|(key, _, _)| *key);
        for (_, net, point) in footprint_pads {
            nets.entry(net).or_default().points.push(point);
        }
        // Standalone pads, vias, junctions and traces.
        for segment in board.net_segments().values() {
            let Some(net) = segment.net() else {
                continue;
            };
            let items = nets.entry(net).or_default();
            for pad in segment.pads().values() {
                let on_board = PadOnBoard {
                    pad: pad.pad(),
                    mirrored: false,
                };
                let (start_layer, end_layer) = if pad.pad().is_tht() {
                    (
                        Layer::TOP_COPPER.copper_number(),
                        Layer::BOT_COPPER.copper_number(),
                    )
                } else {
                    let layer = on_board.solder_layer().copper_number();
                    (layer, layer)
                };
                items.points.push(AnchorPoint {
                    anchor: TraceAnchor::Pad(pad.uuid()),
                    position: pad.pad().position(),
                    start_layer,
                    end_layer,
                });
            }
            for via in segment.vias().values() {
                items.points.push(AnchorPoint {
                    anchor: TraceAnchor::Via(via.uuid()),
                    position: via.position(),
                    start_layer: via.start_layer().copper_number(),
                    end_layer: via.end_layer().copper_number(),
                });
            }
            for junction in segment.junctions().values() {
                if let Some(layer) = segment.junction_layer(&junction.uuid()) {
                    items.points.push(AnchorPoint {
                        anchor: TraceAnchor::Junction(junction.uuid()),
                        position: junction.position(),
                        start_layer: layer.copper_number(),
                        end_layer: layer.copper_number(),
                    });
                }
            }
            items
                .traces
                .extend(segment.traces().values().map(|t| (t.p1(), t.p2())));
        }
        // Planes.
        for plane in board.planes().values() {
            if let Some(net) = plane.net() {
                nets.entry(net)
                    .or_default()
                    .planes
                    .push((plane.layer(), plane.id()));
            }
        }
        Self { board, nets }
    }

    /// Returns the air wires of a net (upstream `buildAirWires()`).
    pub fn build_air_wires(&self, net: NetSignalId) -> Vec<AirWire> {
        let Some(items) = self.nets.get(&net) else {
            return Vec::new();
        };
        let mut builder = AirWiresBuilder::new();
        let mut ids: BTreeMap<TraceAnchor, usize> = BTreeMap::new();
        for point in &items.points {
            let id = builder.add_point(point.position);
            ids.insert(point.anchor, id);
        }
        for (p1, p2) in &items.traces {
            match (ids.get(p1), ids.get(p2)) {
                (Some(&a), Some(&b)) => builder.add_edge(a, b),
                // Unreachable for a validated board (every anchor of a
                // trace of the net is a point of the net).
                _ => debug_assert!(false, "trace anchor without air wire point"),
            }
        }
        // Connections made by planes.
        for (layer, plane) in &items.planes {
            let plane_layer = layer.copper_number();
            for fragment in self.board.derived().fragments_of(*plane) {
                let path = PainterPathPx::from_paths([fragment as &Path]);
                let mut last_id = None;
                for (id, point) in items.points.iter().enumerate() {
                    if (plane_layer >= point.start_layer)
                        && (plane_layer <= point.end_layer)
                        && path.contains_point(point.position.to_px())
                    {
                        if let Some(last) = last_id {
                            builder.add_edge(last, id);
                        }
                        last_id = Some(id);
                    }
                }
            }
        }
        builder
            .build_air_wires()
            .into_iter()
            .map(|(a, b)| {
                let (p1, p2) = (items.points[a], items.points[b]);
                AirWire::new(net, (p1.anchor, p1.position), (p2.anchor, p2.position))
            })
            .collect()
    }
}

impl Project {
    /// Rebuilds the air wires of the nets scheduled for rebuild on a board
    /// (upstream `Board::triggerAirWiresRebuild()`); journals
    /// [`BoardChange::AirWires`] if any net was rebuilt.
    pub fn rebuild_air_wires(&mut self, board: BoardId) -> Result<()> {
        let index = self.board_index(board).ok_or(Error::NotFound {
            kind: EntityKind::Board,
            uuid: board.0,
        })?;
        let b = &self.boards[index];
        if b.derived.dirty_air_wire_nets.is_empty() {
            return Ok(());
        }
        let nets: Vec<Option<NetSignalId>> =
            b.derived.dirty_air_wire_nets.iter().copied().collect();
        let builder = BoardAirWiresBuilder::new(b, &self.library, &self.circuit);
        let mut results = Vec::with_capacity(nets.len());
        for net in &nets {
            let wires = match net {
                Some(id) if self.circuit.net_signals.contains_key(id) => {
                    builder.build_air_wires(*id)
                }
                _ => Vec::new(),
            };
            results.push((*net, wires));
        }
        let derived = &mut self.boards[index].derived;
        for (net, wires) in results {
            if wires.is_empty() {
                derived.air_wires.remove(&net);
            } else {
                derived.air_wires.insert(net, wires);
            }
        }
        derived.dirty_air_wire_nets.clear();
        self.record(Change::Board {
            id: board,
            change: BoardChange::AirWires(nets),
        });
        Ok(())
    }

    /// Schedules all nets of the circuit (and those with air wires) for
    /// rebuild and rebuilds the air wires of a board (upstream
    /// `Board::forceAirWiresRebuild()`).
    pub fn force_air_wires_rebuild(&mut self, board: BoardId) -> Result<()> {
        let index = self.board_index(board).ok_or(Error::NotFound {
            kind: EntityKind::Board,
            uuid: board.0,
        })?;
        let nets: BTreeSet<Option<NetSignalId>> = self
            .circuit
            .net_signals
            .keys()
            .map(|id| Some(*id))
            .chain(self.boards[index].derived.air_wires.keys().copied())
            .collect();
        self.boards[index].derived.dirty_air_wire_nets.extend(nets);
        self.rebuild_air_wires(board)
    }

    /// Replaces the calculated plane fragments of a board (the result of
    /// the plane fragments builder, upstream
    /// `BI_Plane::setCalculatedFragments()`). Not undoable; journals
    /// [`BoardChange::PlaneFragments`] with the planes whose fragments
    /// changed and schedules their nets for an air wire rebuild. Fragments
    /// of unknown planes are ignored.
    pub fn apply_plane_fragments(
        &mut self,
        board: BoardId,
        fragments: BTreeMap<PlaneId, Vec<Path>>,
    ) -> Result<()> {
        let index = self.board_index(board).ok_or(Error::NotFound {
            kind: EntityKind::Board,
            uuid: board.0,
        })?;
        let b = &mut self.boards[index];
        let mut changed = Vec::new();
        for (plane, paths) in fragments {
            let Some(net) = b.planes.get(&plane).map(|p| p.net()) else {
                continue;
            };
            if b.derived.plane_fragments.get(&plane) != Some(&paths) {
                b.derived.plane_fragments.insert(plane, paths);
                if net.is_some() {
                    b.derived.schedule_air_wires(net);
                }
                changed.push(plane);
            }
        }
        if !changed.is_empty() {
            self.record(Change::Board {
                id: board,
                change: BoardChange::PlaneFragments(changed),
            });
        }
        Ok(())
    }

    /// Returns and forgets the dirty plane layers among `layers` of a board
    /// (upstream `Board::takeScheduledLayersForPlanesRebuild()`).
    pub fn take_dirty_plane_layers(
        &mut self,
        board: BoardId,
        layers: &BTreeSet<Layer>,
    ) -> Result<BTreeSet<Layer>> {
        let index = self.board_index(board).ok_or(Error::NotFound {
            kind: EntityKind::Board,
            uuid: board.0,
        })?;
        let dirty = &mut self.boards[index].derived.dirty_plane_layers;
        let taken: BTreeSet<Layer> = dirty.intersection(layers).copied().collect();
        dirty.retain(|l| !taken.contains(l));
        Ok(taken)
    }
}
