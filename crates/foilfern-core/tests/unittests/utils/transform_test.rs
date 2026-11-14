//! Port of tests/unittests/core/utils/transformtest.cpp.

use foilfern_core::geometry::{Path, Vertex};
use foilfern_core::serialization::{List, Mode, SExpression, SerializeObject};
use foilfern_core::types::{Angle, Layer, Point};
use foilfern_core::utils::transform::Transform;

fn str_of(obj: &impl SerializeObject, name: &str) -> String {
    let mut root = List::new(name);
    obj.serialize(&mut root);
    SExpression::List(root)
        .to_string_with_mode(Mode::LibrePcb)
        .unwrap()
}

fn p(x: i64, y: i64) -> Point {
    Point::from_nm(x, y)
}

fn a(udeg: i32) -> Angle {
    Angle::new(udeg)
}

fn assert_angle(expected: Angle, actual: Angle) {
    assert_eq!(expected.to_deg_string(), actual.to_deg_string());
}

#[test]
fn test_copy() {
    let t1 = Transform::new(p(1, 2), a(3), true);
    let t2 = t1;
    assert_eq!(t1, t2);
}

#[test]
fn test_map_mirrorable_angle_non_mirrored() {
    let t = Transform::new(p(1000, 2000), a(3000), false);
    assert_angle(a(3000), t.map_mirrorable(a(0)));
    assert_angle(a(0), t.map_mirrorable(a(-3000)));
    assert_angle(a(180_003_000), t.map_mirrorable(a(180_000_000)));
    assert_angle(a(-179_997_000), t.map_mirrorable(a(-180_000_000)));
}

#[test]
fn test_map_mirrorable_angle_mirrored() {
    let t = Transform::new(p(1000, 2000), a(3000), true);
    assert_angle(a(3000), t.map_mirrorable(a(0)));
    assert_angle(a(6000), t.map_mirrorable(a(-3000)));
    assert_angle(a(-179_997_000), t.map_mirrorable(a(180_000_000)));
    assert_angle(a(180_003_000), t.map_mirrorable(a(-180_000_000)));
}

#[test]
fn test_map_non_mirrorable_angle_non_mirrored() {
    let t = Transform::new(p(1000, 2000), a(3000), false);
    assert_angle(a(3000), t.map_non_mirrorable(a(0)));
    assert_angle(a(0), t.map_non_mirrorable(a(-3000)));
    assert_angle(a(180_003_000), t.map_non_mirrorable(a(180_000_000)));
    assert_angle(a(-179_997_000), t.map_non_mirrorable(a(-180_000_000)));
}

#[test]
fn test_map_non_mirrorable_angle_mirrored() {
    let t = Transform::new(p(1000, 2000), a(3000), true);
    assert_angle(a(180_003_000), t.map_non_mirrorable(a(0)));
    assert_angle(a(180_006_000), t.map_non_mirrorable(a(-3000)));
    assert_angle(a(3000), t.map_non_mirrorable(a(180_000_000)));
    assert_angle(a(3000), t.map_non_mirrorable(a(-180_000_000)));
}

#[test]
fn test_map_point_non_mirrored() {
    let t = Transform::new(p(1000, 2000), a(30_000_000), false);
    assert_eq!(
        str_of(&t.map(&p(0, 0)), "pos"),
        str_of(&p(1000, 2000), "pos")
    );
    assert_eq!(
        str_of(&t.map(&p(4567, 9876)), "pos"),
        str_of(&p(17, 12836), "pos")
    );
}

#[test]
fn test_map_point_mirrored() {
    let t = Transform::new(p(1000, 2000), a(30_000_000), true);
    assert_eq!(
        str_of(&t.map(&p(0, 0)), "pos"),
        str_of(&p(1000, 2000), "pos")
    );
    assert_eq!(
        str_of(&t.map(&p(4567, 9876)), "pos"),
        str_of(&p(-7893, 8269), "pos")
    );
}

#[test]
fn test_map_path_non_mirrored() {
    let t = Transform::new(p(1000, 2000), a(30_000_000), false);
    let input = Path::new(vec![
        Vertex::new(p(0, 0), Angle::DEG90),
        Vertex::new(p(4567, 9876), Angle::DEG0),
    ]);
    let expected = Path::new(vec![
        Vertex::new(p(1000, 2000), Angle::DEG90),
        Vertex::new(p(17, 12836), Angle::DEG0),
    ]);
    assert_eq!(str_of(&t.map(&input), "path"), str_of(&expected, "path"));
}

#[test]
fn test_map_path_mirrored() {
    let t = Transform::new(p(1000, 2000), a(30_000_000), true);
    let input = Path::new(vec![
        Vertex::new(p(0, 0), Angle::DEG90),
        Vertex::new(p(4567, 9876), Angle::DEG0),
    ]);
    let expected = Path::new(vec![
        Vertex::new(p(1000, 2000), -Angle::DEG90),
        Vertex::new(p(-7893, 8269), Angle::DEG0),
    ]);
    assert_eq!(str_of(&t.map(&input), "path"), str_of(&expected, "path"));
}

#[test]
fn test_map_layer_non_mirrored() {
    let t = Transform::new(p(1000, 2000), a(3000), false);
    let inner3 = Layer::inner_copper(3).unwrap();
    assert_eq!(t.map(&Layer::SYMBOL_OUTLINES), Layer::SYMBOL_OUTLINES);
    assert_eq!(t.map(&Layer::TOP_COPPER), Layer::TOP_COPPER);
    assert_eq!(t.map(&inner3), inner3);
    assert_eq!(t.map(&Layer::BOT_COURTYARD), Layer::BOT_COURTYARD);
}

#[test]
fn test_map_layer_mirrored() {
    let t = Transform::new(p(1000, 2000), a(3000), true);
    let inner3 = Layer::inner_copper(3).unwrap();
    assert_eq!(t.map(&Layer::SYMBOL_OUTLINES), Layer::SYMBOL_OUTLINES);
    assert_eq!(t.map(&Layer::TOP_COPPER), Layer::BOT_COPPER);
    assert_eq!(t.map(&inner3), inner3);
    assert_eq!(t.map(&Layer::BOT_COURTYARD), Layer::TOP_COURTYARD);
}

#[test]
fn test_map_collections_and_bools() {
    let t = Transform::new(p(1000, 2000), a(0), true);
    assert_eq!(
        t.map(&vec![p(0, 0), p(1, 1)]),
        vec![p(1000, 2000), p(999, 2001)]
    );
    assert!(t.map_mirror(false));
    assert!(!t.map_mirror(true));
}
