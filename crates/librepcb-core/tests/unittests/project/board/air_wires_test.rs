//! Air wires (upstream `BoardAirWiresBuilder`, `Board::triggerAirWiresRebuild()`)
//! on a small board and on the upstream test boards.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path as StdPath;
use std::sync::Arc;

use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::geometry::Path;
use librepcb_core::project::board::{Board, BoardChange, BoardPlane};
use librepcb_core::project::{BoardMutation, BoardNetSegmentRef, Change, ChangesSince, Mutation};
use petgraph::unionfind::UnionFind;

use super::*;

fn air_wire_pairs(board: &Board) -> BTreeMap<NetSignalId, BTreeSet<[TraceAnchor; 2]>> {
    let mut result: BTreeMap<NetSignalId, BTreeSet<[TraceAnchor; 2]>> = BTreeMap::new();
    for (net, wires) in board.derived().air_wires() {
        for wire in wires {
            let mut pair = [wire.p1(), wire.p2()];
            pair.sort();
            result.entry(net.unwrap()).or_default().insert(pair);
        }
    }
    result
}

#[test]
fn small_board() {
    let mut f = Fixture::with_devices();
    let board = f.board;
    f.p.rebuild_air_wires(board).unwrap();
    assert!(f.board().derived().dirty_air_wire_nets().is_empty());
    let mut expected = [f.pad_anchor(f.r1, 0), f.pad_anchor(f.r2, 0)];
    expected.sort();
    assert_eq!(
        air_wire_pairs(f.board()),
        BTreeMap::from([(f.gnd, BTreeSet::from([expected]))])
    );
    let wire = &f.board().derived().air_wires()[&Some(f.gnd)][0];
    assert!(!wire.is_vertical());
    let positions = BTreeSet::from([wire.p1_position(), wire.p2_position()]);
    assert_eq!(positions, BTreeSet::from([pt(-1.0, 0.0), pt(9.0, 0.0)]));

    // Connecting the pads removes the air wire.
    let (segment, _) = gnd_segment(&f);
    let seg_ref = BoardNetSegmentRef {
        board,
        segment: segment.id(),
    };
    f.p.add_board_net_segment(board, segment).unwrap();
    assert!(
        f.board()
            .derived()
            .dirty_air_wire_nets()
            .contains(&Some(f.gnd))
    );
    let revision = f.p.revision();
    f.p.rebuild_air_wires(board).unwrap();
    assert!(air_wire_pairs(f.board()).is_empty());
    let ChangesSince::Changes(changes) = f.p.changes_since(revision) else {
        panic!("resync");
    };
    assert_eq!(
        changes,
        &[Change::Board {
            id: board,
            change: BoardChange::AirWires(vec![Some(f.gnd)])
        }]
    );

    // A plane with fragments connects the pads as well.
    f.p.apply(Mutation::Board(BoardMutation::RemoveNetSegment(seg_ref)))
        .unwrap();
    let plane = BoardPlane::new(
        Uuid::new_random(),
        Layer::TOP_COPPER,
        Some(f.gnd),
        Path::rect(pt(-5.0, -5.0), pt(15.0, 5.0)),
    );
    let plane_ref = f.p.add_board_plane(board, plane.clone()).unwrap();
    f.p.rebuild_air_wires(board).unwrap();
    assert_eq!(air_wire_pairs(f.board()).len(), 1);
    f.p.apply_plane_fragments(
        board,
        BTreeMap::from([(plane_ref.plane, vec![plane.outline().clone()])]),
    )
    .unwrap();
    assert_eq!(
        f.board().derived().fragments_of(plane_ref.plane),
        &[plane.outline().clone()]
    );
    f.p.rebuild_air_wires(board).unwrap();
    assert!(air_wire_pairs(f.board()).is_empty());

    // A fragment on the other side doesn't connect SMT top pads.
    let mut bottom = plane.clone();
    bottom.set_layer(Layer::BOT_COPPER);
    f.p.apply(Mutation::Board(BoardMutation::UpdatePlane {
        board,
        plane: bottom,
    }))
    .unwrap();
    f.p.force_air_wires_rebuild(board).unwrap();
    assert_eq!(air_wire_pairs(f.board()).len(), 1);

    // Changing the net of a component signal schedules both nets.
    let signal = gnd_signal(&f, f.r2);
    f.p.set_component_signal_net(
        librepcb_core::project::ComponentSignalRef {
            component: f.r2,
            signal,
        },
        Some(f.vcc),
    )
    .unwrap();
    assert!(
        f.board()
            .derived()
            .dirty_air_wire_nets()
            .is_superset(&BTreeSet::from([Some(f.gnd), Some(f.vcc)]))
    );
    f.p.rebuild_air_wires(board).unwrap();
    let mut expected = [f.pad_anchor(f.r1, 1), f.pad_anchor(f.r2, 0)];
    expected.sort();
    assert_eq!(
        air_wire_pairs(f.board()),
        BTreeMap::from([(f.vcc, BTreeSet::from([expected]))])
    );
}

fn open_test_project(dir: &str, file: &str) -> Project {
    let path = StdPath::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/projects")
        .join(dir);
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(&path).unwrap()).unwrap();
    Project::open(TransactionalDirectory::new(Arc::new(fs), ""), file).unwrap()
}

/// Upstream DRC anchor of an air wire end: the device pad or the net
/// segment containing the anchor.
fn drc_anchor(board: &Board, anchor: TraceAnchor) -> String {
    match anchor {
        TraceAnchor::FootprintPad { device, pad } => format!("device {device} pad {pad}"),
        _ => {
            let segment = board
                .net_segments()
                .values()
                .find(|s| s.contains_anchor(&anchor))
                .unwrap();
            format!("netsegment {}", segment.uuid())
        }
    }
}

fn drc_node_anchor(node: &librepcb_core::serialization::SExpression) -> String {
    match node.child("netsegment") {
        Some(segment) => format!(
            "netsegment {}",
            segment.child("@0").unwrap().value().unwrap()
        ),
        None => format!(
            "device {} pad {}",
            node.child("device/@0").unwrap().value().unwrap(),
            node.child("pad/@0").unwrap().value().unwrap()
        ),
    }
}

/// The positions of the air wire points of a DRC anchor (a footprint pad or
/// all points of a net segment).
fn drc_anchor_points(p: &Project, board: &Board, anchor: &str) -> Vec<Point> {
    let words: Vec<&str> = anchor.split(' ').collect();
    match words[..] {
        ["netsegment", uuid] => {
            let segment = board.net_segment(uuid.parse().unwrap()).unwrap();
            let pads = segment.pads().values().map(|p| p.pad().position());
            let vias = segment.vias().values().map(|v| v.position());
            let junctions = segment
                .junctions()
                .values()
                .filter(|j| segment.junction_layer(&j.uuid()).is_some())
                .map(|j| j.position());
            pads.chain(vias).chain(junctions).collect()
        }
        ["device", device, "pad", pad] => {
            let device = board.device(device.parse().unwrap()).unwrap();
            let pad = device
                .pad(&pad.parse().unwrap(), p.library(), p.circuit())
                .unwrap()
                .unwrap();
            vec![pad.position()]
        }
        _ => unreachable!(),
    }
}

/// The shortest distance between the points of two DRC anchors (nm).
fn drc_pair_length(p: &Project, board: &Board, pair: &BTreeSet<String>) -> i64 {
    let mut anchors = pair.iter();
    let a = drc_anchor_points(p, board, anchors.next().unwrap());
    let b = drc_anchor_points(p, board, anchors.next().unwrap());
    a.iter()
        .flat_map(|pa| b.iter().map(move |pb| (*pa - *pb).length().to_nm()))
        .min()
        .unwrap()
}

/// The air wires of the DRC test board for missing connections equal the
/// approved `missing_connection` messages of the upstream DRC (including
/// the choice among equally long air wires).
#[test]
fn drc_missing_connections() {
    let p = open_test_project("DRC", "project.lpp");
    let board = p
        .boards()
        .iter()
        .find(|b| b.directory_name() == "checkformissingconnections")
        .unwrap();
    let actual: BTreeSet<BTreeSet<String>> = board
        .derived()
        .all_air_wires()
        .map(|w| BTreeSet::from([drc_anchor(board, w.p1()), drc_anchor(board, w.p2())]))
        .collect();
    let actual_length: i64 = board
        .derived()
        .all_air_wires()
        .map(|w| (w.p1_position() - w.p2_position()).length().to_nm())
        .sum();
    let expected: BTreeSet<BTreeSet<String>> = board
        .drc_approvals()
        .iter()
        .filter(|a| a.child("@0").and_then(|n| n.value().ok()) == Some("missing_connection"))
        .map(|a| {
            BTreeSet::from([
                drc_node_anchor(a.child("from").unwrap()),
                drc_node_anchor(a.child("to").unwrap()),
            ])
        })
        .collect();
    assert_eq!(expected.len(), 10);
    assert_eq!(actual.len(), expected.len());
    let expected_length: i64 = expected
        .iter()
        .map(|pair| drc_pair_length(&p, board, pair))
        .sum();
    assert_eq!(actual_length, expected_length);
    assert_eq!(actual, expected);
}

/// On every test board, the air wires of each net connect exactly the
/// connected groups of its anchors (count = groups - 1, and together with
/// the traces they connect everything).
#[test]
fn test_boards_air_wires_connect_all_groups() {
    for (dir, file) in [
        ("DRC", "project.lpp"),
        ("Gerber Test", "project.lpp"),
        ("Nested Planes", "project.lpp"),
        ("Project With Two Boards", "Project With Two Boards.lpp"),
    ] {
        let p = open_test_project(dir, file);
        let mut wires_total = 0;
        for board in p.boards() {
            let mut anchors: BTreeMap<NetSignalId, BTreeMap<TraceAnchor, usize>> = BTreeMap::new();
            for device in board.devices().values() {
                for pad in device.pads(p.library(), p.circuit()).unwrap() {
                    if let Some(net) = pad.net() {
                        let map = anchors.entry(net).or_default();
                        let n = map.len();
                        map.insert(pad.trace_anchor(), n);
                    }
                }
            }
            let mut edges: BTreeMap<NetSignalId, Vec<(TraceAnchor, TraceAnchor)>> = BTreeMap::new();
            for segment in board.net_segments().values() {
                let Some(net) = segment.net() else { continue };
                let map = anchors.entry(net).or_default();
                let mut add = |a: TraceAnchor| {
                    let n = map.len();
                    map.entry(a).or_insert(n);
                };
                segment
                    .pads()
                    .keys()
                    .for_each(|u| add(TraceAnchor::Pad(*u)));
                segment
                    .vias()
                    .keys()
                    .for_each(|u| add(TraceAnchor::Via(*u)));
                segment
                    .junctions()
                    .keys()
                    .filter(|u| segment.junction_layer(u).is_some())
                    .for_each(|u| add(TraceAnchor::Junction(*u)));
                edges
                    .entry(net)
                    .or_default()
                    .extend(segment.traces().values().map(|t| (t.p1(), t.p2())));
            }
            for (net, map) in &anchors {
                let mut uf = UnionFind::<usize>::new(map.len());
                for (a, b) in edges.get(net).into_iter().flatten() {
                    uf.union(map[a], map[b]);
                }
                let groups = (0..map.len())
                    .map(|i| uf.find(i))
                    .collect::<BTreeSet<_>>()
                    .len();
                let wires = board
                    .derived()
                    .air_wires()
                    .get(&Some(*net))
                    .map_or(&[][..], Vec::as_slice);
                assert_eq!(wires.len(), groups - 1, "{dir}/{}", board.name());
                for wire in wires {
                    uf.union(map[&wire.p1()], map[&wire.p2()]);
                }
                let groups = (0..map.len())
                    .map(|i| uf.find(i))
                    .collect::<BTreeSet<_>>()
                    .len();
                assert_eq!(groups, 1, "{dir}/{}", board.name());
                wires_total += wires.len();
            }
        }
        println!("{dir}: {wires_total} air wires");
    }
}
