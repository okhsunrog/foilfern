//! Port of tests/unittests/core/algorithm/airwiresbuildertest.cpp.

use foilfern_core::algorithm::{AirWire, AirWiresBuilder};
use foilfern_core::types::Point;

fn sorted(mut airwires: Vec<AirWire>) -> Vec<AirWire> {
    for (a, b) in &mut airwires {
        if b < a {
            std::mem::swap(a, b);
        }
    }
    airwires.sort();
    airwires
}

fn p(x: i64, y: i64) -> Point {
    Point::from_nm(x, y)
}

#[test]
fn test_empty() {
    let builder = AirWiresBuilder::new();
    assert!(builder.build_air_wires().is_empty());
}

#[test]
fn test_one_point() {
    let mut builder = AirWiresBuilder::new();
    builder.add_point(p(1000000, 2000000));
    assert!(builder.build_air_wires().is_empty());
}

#[test]
fn test_two_unconnected_points() {
    let mut builder = AirWiresBuilder::new();
    let id0 = builder.add_point(p(1000000, 2000000));
    let id1 = builder.add_point(p(3000000, 4000000));
    assert_eq!(sorted(builder.build_air_wires()), [(id0, id1)]);
}

#[test]
fn test_two_unconnected_overlapping_points() {
    let mut builder = AirWiresBuilder::new();
    let id0 = builder.add_point(p(100000, 200000));
    let id1 = builder.add_point(p(100000, 200000));
    assert_eq!(sorted(builder.build_air_wires()), [(id0, id1)]);
}

#[test]
fn test_two_connected_points() {
    let mut builder = AirWiresBuilder::new();
    let id0 = builder.add_point(p(100000, 200000));
    let id1 = builder.add_point(p(300000, 400000));
    builder.add_edge(id0, id1);
    assert!(builder.build_air_wires().is_empty());
}

#[test]
fn test_three_unconnected_points() {
    let mut builder = AirWiresBuilder::new();
    let id0 = builder.add_point(p(100000, 200000));
    let id1 = builder.add_point(p(300000, 400000));
    let id2 = builder.add_point(p(-50000, -50000));
    assert_eq!(sorted(builder.build_air_wires()), [(id0, id1), (id0, id2)]);
}

#[test]
fn test_three_unconnected_points_vshape() {
    let mut builder = AirWiresBuilder::new();
    let id0 = builder.add_point(p(-50000, 0));
    let id1 = builder.add_point(p(100000, 0));
    let id2 = builder.add_point(p(0, -1000000));
    assert_eq!(sorted(builder.build_air_wires()), [(id0, id1), (id0, id2)]);
}

// Test added for bug https://github.com/LibrePCB/LibrePCB/issues/588
#[test]
fn test_three_unconnected_collinear_points() {
    let mut builder = AirWiresBuilder::new();
    let id0 = builder.add_point(p(0, 0));
    let id1 = builder.add_point(p(100000, 0));
    let id2 = builder.add_point(p(-100000, 0));
    assert_eq!(sorted(builder.build_air_wires()), [(id0, id1), (id0, id2)]);
}

// Test added for bug https://github.com/LibrePCB/LibrePCB/issues/588
#[test]
fn test_three_unconnected_diagonal_collinear_points() {
    let mut builder = AirWiresBuilder::new();
    let id0 = builder.add_point(p(0, 0));
    let id1 = builder.add_point(p(1000000, 1000000));
    let id2 = builder.add_point(p(2000000, 2000000));
    assert_eq!(sorted(builder.build_air_wires()), [(id0, id1), (id1, id2)]);
}

// Test added for bug https://github.com/LibrePCB/LibrePCB/issues/588
#[test]
fn test_three_unconnected_diagonal_collinear_points2() {
    let mut builder = AirWiresBuilder::new();
    let id0 = builder.add_point(p(71437500, 78898800));
    let id1 = builder.add_point(p(70485000, 80010000));
    let id2 = builder.add_point(p(72707500, 77470000));
    assert_eq!(sorted(builder.build_air_wires()), [(id0, id1), (id0, id2)]);
}

// Test added for bug https://github.com/LibrePCB/LibrePCB/issues/588
#[test]
fn test_partly_connected_collinear_points() {
    let mut builder = AirWiresBuilder::new();
    let ids: Vec<usize> = (0..7)
        .map(|i| builder.add_point(p(i * 100000, i * 100000)))
        .collect();
    builder.add_edge(ids[1], ids[2]);
    let expected = [
        (ids[0], ids[1]),
        (ids[2], ids[3]),
        (ids[3], ids[4]),
        (ids[4], ids[5]),
        (ids[5], ids[6]),
    ];
    assert_eq!(sorted(builder.build_air_wires()), expected);
}
