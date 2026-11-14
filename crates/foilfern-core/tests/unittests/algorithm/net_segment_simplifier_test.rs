//! Port of tests/unittests/core/algorithm/netsegmentsimplifiertest.cpp.

use foilfern_core::algorithm::{AnchorType, NetSegmentLine, NetSegmentSimplifier, SimplifyResult};
use foilfern_core::types::{Length, Point};

/// Upstream `Line{id, p1, p2, layer, width, modified}` without layer.
fn line(id: usize, p1: usize, p2: usize, width: i64, modified: bool) -> NetSegmentLine {
    NetSegmentLine {
        id,
        p1,
        p2,
        layer: None,
        width: Length::new(width),
        modified,
    }
}

fn result(
    lines: Vec<NetSegmentLine>,
    new_junctions: &[(usize, Point)],
    disconnected: &[usize],
    modified: bool,
) -> SimplifyResult {
    SimplifyResult {
        lines,
        new_junctions: new_junctions.iter().copied().collect(),
        disconnected_fixed_anchors: disconnected.iter().copied().collect(),
        modified,
    }
}

/// String representation for readable failure messages (like upstream).
fn str(result: &SimplifyResult) -> String {
    let mut s = Vec::new();
    for line in &result.lines {
        s.push(format!(
            "line id={} p1={} p2={} layer={} width={} modified={}",
            line.id,
            line.p1,
            line.p2,
            line.layer.map_or("nullptr", |l| l.id()),
            line.width.to_mm_string(),
            line.modified
        ));
    }
    for (id, pos) in &result.new_junctions {
        s.push(format!(
            "new junction id={id} x={} y={}",
            pos.x.to_mm_string(),
            pos.y.to_mm_string()
        ));
    }
    for id in &result.disconnected_fixed_anchors {
        s.push(format!("disconnected pin/pad id={id}"));
    }
    s.push(format!("modified={}", result.modified));
    s.join("\n")
}

fn anchor(obj: &mut NetSegmentSimplifier, anchor_type: AnchorType, x: i64, y: i64) -> usize {
    obj.add_anchor(anchor_type, Point::from_nm(x, y), None, None)
}

fn add_line(obj: &mut NetSegmentSimplifier, p1: usize, p2: usize, width: i64) -> usize {
    obj.add_line(p1, p2, None, Length::new(width))
}

use AnchorType::{Fixed, Junction, Via};

#[test]
fn test_empty() {
    let mut obj = NetSegmentSimplifier::new();
    let actual = obj.simplify();
    let expected = result(vec![], &[], &[], false);
    assert_eq!(str(&expected), str(&actual));
}

#[test]
fn test_incrementing_ids_and_reset_state() {
    let mut obj = NetSegmentSimplifier::new();
    for _ in 0..2 {
        let p0 = anchor(&mut obj, Junction, 0, 0);
        assert_eq!(p0, 0);
        let p1 = anchor(&mut obj, Via, 1000, 1000);
        assert_eq!(p1, 1);
        let p2 = anchor(&mut obj, Via, 1000, 1000);
        assert_eq!(p2, 2);
        let l0 = add_line(&mut obj, p0, p1, 1);
        assert_eq!(l0, 0);
        let l1 = add_line(&mut obj, p1, p2, 1);
        assert_eq!(l1, 1);
        obj.simplify(); // Must reset the state, i.e. reuse IDs.
    }
}

#[test]
fn test_only_anchors() {
    let mut obj = NetSegmentSimplifier::new();
    anchor(&mut obj, Junction, 0, 0);
    anchor(&mut obj, Via, 1000, 1000);
    let actual = obj.simplify();
    let expected = result(vec![], &[], &[], false);
    assert_eq!(str(&expected), str(&actual));
}

#[test]
fn test_one_line() {
    let mut obj = NetSegmentSimplifier::new();
    anchor(&mut obj, Junction, 0, 0);
    anchor(&mut obj, Via, 1000, 1000);
    add_line(&mut obj, 0, 1, 1);
    let actual = obj.simplify();
    let expected = result(vec![line(0, 0, 1, 1, false)], &[], &[], false);
    assert_eq!(str(&expected), str(&actual));
}

#[test]
fn test_duplicate_junctions() {
    let mut obj = NetSegmentSimplifier::new();
    anchor(&mut obj, Junction, 0, 0);
    anchor(&mut obj, Junction, 10, 0);
    anchor(&mut obj, Junction, 10, 10);
    anchor(&mut obj, Junction, 0, 0);
    anchor(&mut obj, Junction, -10, 0);
    add_line(&mut obj, 0, 1, 1);
    add_line(&mut obj, 1, 2, 2);
    add_line(&mut obj, 2, 3, 3);
    add_line(&mut obj, 3, 4, 4);
    let actual = obj.simplify();
    let expected = result(
        vec![
            line(0, 0, 1, 1, false),
            line(1, 1, 2, 2, false),
            line(2, 2, 0, 3, true),
            line(3, 0, 4, 4, true),
        ],
        &[],
        &[],
        true,
    );
    assert_eq!(str(&expected), str(&actual));
}

#[test]
fn test_two_redundant_lines() {
    let mut obj = NetSegmentSimplifier::new();
    anchor(&mut obj, Junction, 0, 0);
    anchor(&mut obj, Junction, 1000, 1000);
    add_line(&mut obj, 0, 1, 1);
    add_line(&mut obj, 1, 0, 2);
    let actual = obj.simplify();
    let expected = result(vec![line(1, 1, 0, 2, false)], &[], &[], true);
    assert_eq!(str(&expected), str(&actual));
}

#[test]
fn test_one_zero_length_line_between_junctions() {
    let mut obj = NetSegmentSimplifier::new();
    anchor(&mut obj, Junction, 0, 0);
    anchor(&mut obj, Junction, 0, 0);
    add_line(&mut obj, 0, 1, 1);
    let actual = obj.simplify();
    let expected = result(vec![], &[], &[], true);
    assert_eq!(str(&expected), str(&actual));
}

#[test]
fn test_keep_zero_length_line_between_pins() {
    let mut obj = NetSegmentSimplifier::new();
    anchor(&mut obj, Fixed, 0, 0);
    anchor(&mut obj, Fixed, 0, 0);
    add_line(&mut obj, 0, 1, 1);
    let actual = obj.simplify();
    let expected = result(vec![line(0, 0, 1, 1, false)], &[], &[], false);
    assert_eq!(str(&expected), str(&actual));
}

#[test]
fn test_merge_straight_lines() {
    let mut obj = NetSegmentSimplifier::new();
    anchor(&mut obj, Junction, 0, 0);
    anchor(&mut obj, Junction, 1000, 0);
    anchor(&mut obj, Junction, 2000, 0);
    anchor(&mut obj, Junction, 3000, 100);
    add_line(&mut obj, 0, 1, 1);
    add_line(&mut obj, 1, 2, 1);
    add_line(&mut obj, 2, 3, 3);
    let actual = obj.simplify();
    let expected = result(
        vec![line(0, 0, 2, 1, true), line(2, 2, 3, 3, false)],
        &[],
        &[],
        true,
    );
    assert_eq!(str(&expected), str(&actual));
}

#[test]
fn test_keep_straight_lines_with_different_width() {
    let mut obj = NetSegmentSimplifier::new();
    anchor(&mut obj, Junction, 0, 0);
    anchor(&mut obj, Junction, 1000, 0);
    anchor(&mut obj, Junction, 2000, 0);
    anchor(&mut obj, Junction, 3000, 100);
    add_line(&mut obj, 0, 1, 1);
    add_line(&mut obj, 1, 2, 2); // different width
    add_line(&mut obj, 2, 3, 3);
    let actual = obj.simplify();
    let expected = result(
        vec![
            line(0, 0, 1, 1, false),
            line(1, 1, 2, 2, false),
            line(2, 2, 3, 3, false),
        ],
        &[],
        &[],
        false,
    );
    assert_eq!(str(&expected), str(&actual));
}

#[test]
fn test_split_line_at_existing_anchor() {
    let mut obj = NetSegmentSimplifier::new();
    anchor(&mut obj, Junction, 0, 0);
    anchor(&mut obj, Junction, 1000, 0);
    anchor(&mut obj, Junction, 1000, 1000);
    anchor(&mut obj, Junction, 200, 0);
    add_line(&mut obj, 0, 1, 1);
    add_line(&mut obj, 1, 2, 2);
    add_line(&mut obj, 2, 3, 3);
    let actual = obj.simplify();
    let expected = result(
        vec![
            line(0, 0, 3, 1, true),
            line(1, 1, 2, 2, false),
            line(2, 2, 3, 3, false),
            line(3, 3, 1, 1, true), // new
        ],
        &[],
        &[],
        true,
    );
    assert_eq!(str(&expected), str(&actual));
}

#[test]
fn test_split_intersecting_lines() {
    let mut obj = NetSegmentSimplifier::new();
    anchor(&mut obj, Junction, 0, 0);
    anchor(&mut obj, Junction, 1000, 0);
    anchor(&mut obj, Junction, 700, 1000);
    anchor(&mut obj, Junction, 700, -1000);
    add_line(&mut obj, 0, 1, 1);
    add_line(&mut obj, 1, 2, 2);
    add_line(&mut obj, 2, 3, 3);
    let actual = obj.simplify();
    let expected = result(
        vec![
            line(0, 0, 4, 1, true), // split
            line(1, 1, 2, 2, false),
            line(2, 2, 4, 3, true), // split
            line(3, 4, 1, 1, true), // new
            line(4, 4, 3, 3, true), // new
        ],
        &[(4, Point::from_nm(700, 0))],
        &[],
        true,
    );
    assert_eq!(str(&expected), str(&actual));
}

#[test]
fn test_split_multiple_intersecting_lines() {
    let mut obj = NetSegmentSimplifier::new();
    anchor(&mut obj, Junction, 0, 0);
    anchor(&mut obj, Junction, 1000, 0);
    anchor(&mut obj, Junction, 1000, 1000);
    anchor(&mut obj, Junction, 800, 1000);
    anchor(&mut obj, Junction, 800, -1000);
    anchor(&mut obj, Junction, 600, -1000);
    anchor(&mut obj, Junction, 600, 1000);
    anchor(&mut obj, Junction, 400, 1000);
    anchor(&mut obj, Junction, 400, -1000);
    for i in 0..8 {
        add_line(&mut obj, i, i + 1, 1);
    }
    let actual = obj.simplify();
    let expected = result(
        vec![
            line(0, 0, 11, 1, true), // split
            line(1, 1, 2, 1, false),
            line(2, 2, 3, 1, false),
            line(3, 3, 9, 1, true), // split
            line(4, 4, 5, 1, false),
            line(5, 5, 10, 1, true), // split
            line(6, 6, 7, 1, false),
            line(7, 7, 11, 1, true),   // split
            line(8, 9, 1, 1, true),    // new
            line(9, 10, 9, 1, true),   // new
            line(10, 11, 10, 1, true), // new
            line(11, 9, 4, 1, true),   // new
            line(12, 10, 6, 1, true),  // new
            line(13, 11, 8, 1, true),  // new
        ],
        &[
            (9, Point::from_nm(800, 0)),
            (10, Point::from_nm(600, 0)),
            (11, Point::from_nm(400, 0)),
        ],
        &[],
        true,
    );
    assert_eq!(str(&expected), str(&actual));
}

#[test]
fn test_disconnected_pins_or_pads() {
    let mut obj = NetSegmentSimplifier::new();
    anchor(&mut obj, Junction, 0, 0);
    anchor(&mut obj, Fixed, 0, 0);
    add_line(&mut obj, 0, 1, 1);
    let actual = obj.simplify();
    let expected = result(
        vec![], // Line removed
        &[],    // No new junctions
        &[1],   // Pin 1 disconnected
        true,
    );
    assert_eq!(str(&expected), str(&actual));
}
