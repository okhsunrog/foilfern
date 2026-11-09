//! Seeded random edit sequences on the upstream test projects: moves,
//! removals, additions, renames, raw mutations, undo and redo; the scenes
//! are checked after every step.

use std::collections::BTreeMap;

use fastrand::Rng;
use librepcb_core::project::board::BoardItem;
use librepcb_core::project::board::BoardSegmentElements;
use librepcb_core::project::{
    BoardId, BoardMutation, BoardNetSegmentRef, Mutation, Project, SchematicId,
};
use librepcb_core::types::{Angle, CircuitIdentifier, Point};
use librepcb_editor::commands::*;

use super::helpers::{Harness, mm, projects, upstream_editor};

fn pick<T: Clone>(rng: &mut Rng, items: &[T]) -> Option<T> {
    if items.is_empty() {
        None
    } else {
        Some(items[rng.usize(..items.len())].clone())
    }
}

fn offset(rng: &mut Rng) -> Point {
    mm(
        f64::from(rng.i32(-8..=8)) * 1.27,
        f64::from(rng.i32(-8..=8)) * 1.27,
    )
}

fn angle(rng: &mut Rng) -> Angle {
    pick(
        rng,
        &[Angle::DEG0, Angle::DEG90, Angle::DEG180, Angle::DEG270],
    )
    .unwrap()
}

fn primary_schematic(p: &Project) -> Option<SchematicId> {
    p.schematics().first().map(|s| s.id())
}

fn primary_board(p: &Project) -> Option<BoardId> {
    p.boards().first().map(|b| b.id())
}

/// Performs one random step; returns its name.
fn random_step(h: &mut Harness, rng: &mut Rng, counter: &mut usize) -> &'static str {
    let p = h.project();
    let schematic = primary_schematic(p);
    let board = primary_board(p);
    *counter += 1;
    match rng.u32(..18) {
        0 | 1 => {
            let Some(b) = board.and_then(|b| p.board(b)) else {
                return "skip";
            };
            let devices: Vec<_> = b.devices().values().collect();
            let Some(device) = pick(rng, &devices) else {
                return "skip";
            };
            let cmd = MoveDevice {
                component: device.component().into(),
                board,
                position: Some(device.position() + offset(rng)),
                rotation: rng.bool().then(|| angle(rng)),
                mirrored: (rng.u32(..4) == 0).then(|| !device.mirrored()),
                locked: None,
            };
            h.try_run(cmd);
            "move device"
        }
        2 | 3 => {
            let Some(s) = schematic.and_then(|s| p.schematic(s)) else {
                return "skip";
            };
            let symbols: Vec<_> = s.symbols().values().collect();
            let Some(symbol) = pick(rng, &symbols) else {
                return "skip";
            };
            let cmd = MoveSymbol {
                symbol: symbol.id(),
                position: Some(symbol.position() + offset(rng)),
                rotation: rng.bool().then(|| angle(rng)),
                mirrored: (rng.u32(..4) == 0).then(|| !symbol.mirrored()),
            };
            h.try_run(cmd);
            "move symbol"
        }
        4 => {
            // Move a via or junction with a raw mutation.
            let Some(b) = board.and_then(|b| p.board(b)) else {
                return "skip";
            };
            let segments: Vec<_> = b.net_segments().values().collect();
            let Some(segment) = pick(rng, &segments) else {
                return "skip";
            };
            let mut elements = BoardSegmentElements::default();
            let vias: Vec<_> = segment.vias().values().cloned().collect();
            let junctions: Vec<_> = segment.junctions().values().cloned().collect();
            if let Some(mut via) = pick(rng, &vias) {
                via.set_position(via.position() + offset(rng));
                elements.vias.push(via);
            } else if let Some(mut junction) = pick(rng, &junctions) {
                junction.set_position(junction.position() + offset(rng));
                elements.junctions.push(junction);
            } else {
                return "skip";
            }
            let m = Mutation::Board(BoardMutation::UpdateNetSegmentElements {
                segment: BoardNetSegmentRef {
                    board: b.id(),
                    segment: segment.id(),
                },
                elements,
            });
            h.try_run(ApplyMutations {
                text: None,
                mutations: vec![m],
            });
            "move via/junction"
        }
        5 => {
            // Move a board item (zone, polygon, text, hole) or a plane.
            let Some(b) = board.and_then(|b| p.board(b)) else {
                return "skip";
            };
            let d = offset(rng);
            let mut items: Vec<BoardItem> = Vec::new();
            items.extend(b.stroke_texts().values().map(|t| {
                let mut t = t.clone();
                t.set_position(t.position() + d);
                BoardItem::StrokeText(t)
            }));
            items.extend(b.holes().values().map(|h| {
                let mut h = h.clone();
                if let Ok(path) =
                    librepcb_core::geometry::NonEmptyPath::new(h.path().get().translated(d))
                {
                    h.set_path(path);
                }
                BoardItem::Hole(h)
            }));
            let planes: Vec<_> = b.planes().values().collect();
            if rng.bool()
                && let Some(plane) = pick(rng, &planes)
            {
                let cmd = EditPlane {
                    board,
                    plane: plane.id(),
                    net: None,
                    layer: None,
                    outline: Some(plane.outline().translated(d)),
                    settings: PlaneSettings::default(),
                };
                h.try_run(cmd);
                return "move plane";
            }
            let Some(item) = pick(rng, &items) else {
                return "skip";
            };
            h.try_run(UpdateBoardItem { board, item });
            "move board item"
        }
        6 | 7 => {
            // Remove random board items.
            let Some(b) = board.and_then(|b| p.board(b)) else {
                return "skip";
            };
            let mut selection = BoardSelection::default();
            let seg_items =
                |f: &dyn Fn(&librepcb_core::project::board::BoardNetSegment) -> Vec<_>| {
                    b.net_segments().values().flat_map(f).collect::<Vec<_>>()
                };
            match rng.u32(..6) {
                0 => {
                    let traces =
                        seg_items(&|s| s.traces().keys().map(|t| (s.id(), *t)).collect::<Vec<_>>());
                    selection.traces.extend(pick(rng, &traces));
                }
                1 => {
                    let vias =
                        seg_items(&|s| s.vias().keys().map(|t| (s.id(), *t)).collect::<Vec<_>>());
                    selection.vias.extend(pick(rng, &vias));
                }
                2 => {
                    let junctions = seg_items(&|s| {
                        s.junctions()
                            .keys()
                            .map(|t| (s.id(), *t))
                            .collect::<Vec<_>>()
                    });
                    selection.junctions.extend(pick(rng, &junctions));
                }
                3 => {
                    let planes: Vec<_> = b.planes().keys().copied().collect();
                    selection.planes.extend(pick(rng, &planes));
                }
                4 => {
                    let devices: Vec<_> = b.devices().keys().copied().collect();
                    selection.devices.extend(pick(rng, &devices));
                }
                _ => {
                    let polygons: Vec<_> = b.polygons().keys().copied().collect();
                    selection.polygons.extend(pick(rng, &polygons));
                    let zones: Vec<_> = b.zones().keys().copied().collect();
                    selection.zones.extend(pick(rng, &zones));
                }
            }
            h.try_run(RemoveBoardItems { board, selection });
            "remove board items"
        }
        8 => {
            // Remove random schematic items.
            let Some(s) = schematic.and_then(|s| p.schematic(s)) else {
                return "skip";
            };
            let mut selection = SchematicSelection::default();
            let lines: Vec<_> = s
                .net_segments()
                .values()
                .flat_map(|seg| seg.lines().keys().map(|l| (seg.id(), *l)))
                .collect();
            let labels: Vec<_> = s
                .net_segments()
                .values()
                .flat_map(|seg| seg.labels().keys().map(|l| (seg.id(), *l)))
                .collect();
            let symbols: Vec<_> = s.symbols().keys().copied().collect();
            match rng.u32(..4) {
                0 | 1 => selection.net_lines.extend(pick(rng, &lines)),
                2 => selection.net_labels.extend(pick(rng, &labels)),
                _ => selection.symbols.extend(pick(rng, &symbols)),
            }
            h.try_run(RemoveSchematicItems {
                schematic: schematic.unwrap(),
                selection,
            });
            "remove schematic items"
        }
        9 => {
            // Rename a net.
            let nets: Vec<_> = p.circuit().net_signals().keys().copied().collect();
            let Some(net) = pick(rng, &nets) else {
                return "skip";
            };
            h.try_run(EditNet {
                net: net.into(),
                name: Some(CircuitIdentifier::new(format!("RND{counter}")).unwrap()),
                net_class: None,
                merge: false,
            });
            "rename net"
        }
        10 => {
            // Change a component value.
            let components: Vec<_> = p.circuit().component_instances().keys().copied().collect();
            let Some(component) = pick(rng, &components) else {
                return "skip";
            };
            h.try_run(EditComponent {
                component: component.into(),
                name: None,
                value: Some(format!("{counter}k")),
                attributes: None,
                assembly_options: None,
                lock_assembly: None,
            });
            "edit component"
        }
        11 => {
            // Add a via on a random net.
            let Some(b) = board.and_then(|b| p.board(b)) else {
                return "skip";
            };
            let nets: Vec<_> = p.circuit().net_signals().keys().copied().collect();
            let position = b
                .devices()
                .values()
                .next()
                .map_or(mm(10.0, 10.0), |d| d.position())
                + offset(rng);
            h.try_run(AddVia {
                board,
                position,
                net: pick(rng, &nets).map(NetRef::from),
                start_layer: None,
                end_layer: None,
                drill_diameter: None,
                size: None,
                exposure: None,
            });
            "add via"
        }
        12 => {
            // Add a trace from a random footprint pad to a random point.
            let Some(b) = board.and_then(|b| p.board(b)) else {
                return "skip";
            };
            let pads: Vec<(librepcb_core::project::ComponentInstanceId, String, Point)> = b
                .devices()
                .values()
                .flat_map(|d| {
                    d.pads(p.library(), p.circuit())
                        .unwrap_or_default()
                        .into_iter()
                        .map(|pad| (d.component(), pad.uuid().to_string(), pad.position()))
                        .collect::<Vec<_>>()
                })
                .collect();
            let Some((component, pad, position)) = pick(rng, &pads) else {
                return "skip";
            };
            h.try_run(AddTrace {
                board,
                start: TraceEndpoint::Pad(PadRef {
                    component: component.into(),
                    pad,
                }),
                end: TraceEndpoint::Point(position + offset(rng)),
                points: vec![],
                layer: None,
                width: None,
                net: None,
            });
            "add trace"
        }
        13 => {
            // Draw a wire from a random symbol pin.
            let Some(s) = schematic.and_then(|s| p.schematic(s)) else {
                return "skip";
            };
            let pins: Vec<(
                librepcb_core::project::SymbolId,
                librepcb_core::types::Uuid,
                Point,
            )> = s
                .symbols()
                .values()
                .flat_map(|sym| {
                    sym.pins(p.view())
                        .unwrap_or_default()
                        .iter()
                        .map(|pin| (sym.id(), pin.uuid(), pin.position()))
                        .collect::<Vec<_>>()
                })
                .collect();
            let Some((symbol, pin, position)) = pick(rng, &pins) else {
                return "skip";
            };
            h.try_run(DrawWire {
                schematic,
                start: WireAnchor::SymbolPin { symbol, pin },
                end: WireAnchor::Point(position + offset(rng)),
                points: vec![],
                net: None,
            });
            "draw wire"
        }
        14 => {
            // Toggle the visibility of a layer.
            let Some(b) = board.and_then(|b| p.board(b)) else {
                return "skip";
            };
            let mut visibility: BTreeMap<String, bool> = b.layers_visibility().clone();
            let layers = librepcb_core::types::Layer::all();
            let layer = layers[rng.usize(..layers.len())];
            let visible = visibility.get(layer.id()).copied().unwrap_or(true);
            visibility.insert(layer.id().to_owned(), !visible);
            h.try_run(ApplyMutations {
                text: None,
                mutations: vec![Mutation::Board(BoardMutation::SetLayersVisibility {
                    board: b.id(),
                    visibility,
                })],
            });
            "toggle layer"
        }
        15 | 16 => {
            h.undo();
            "undo"
        }
        _ => {
            h.redo();
            "redo"
        }
    }
}

fn run_random(seed: u64, steps: usize, filter: impl Fn(&str) -> bool) {
    for dir in projects() {
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        if !filter(&name) {
            continue;
        }
        let mut h = Harness::new(upstream_editor(&dir));
        let mut rng = Rng::with_seed(seed);
        let mut counter = 0;
        let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
        for _ in 0..steps {
            *kinds
                .entry(random_step(&mut h, &mut rng, &mut counter))
                .or_default() += 1;
        }
        // Undo everything back to the loaded state.
        while h.undo() {}
        println!(
            "{name} (seed {seed}): {:?}; {} checks, {} incremental, {} full rebuilds",
            kinds, h.stats.steps, h.stats.incremental, h.stats.rebuilds
        );
    }
}

#[test]
fn test_random_edits_seed_1() {
    run_random(1, 60, |_| true);
}

#[test]
fn test_random_edits_seed_2() {
    // The DRC project (many boards) is covered by seed 1.
    run_random(2, 60, |n| n != "DRC");
}

#[test]
fn test_random_edits_many_seeds() {
    for seed in 3..8 {
        run_random(seed, 40, |n| n == "Gerber Test" || n == "v1");
    }
}
