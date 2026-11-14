//! Port of tests/unittests/core/utils/clipperhelperstest.cpp (plus tests
//! of the hole cut-in flattening, which upstream tests indirectly).

use clipper::{IntPoint, PolyFillType};
use foilfern_core::geometry::{Path, Vertex};
use foilfern_core::types::{Angle, Length, Point, PositiveLength};
use foilfern_core::utils::clipper_helpers;

fn pos(nm: i64) -> PositiveLength {
    PositiveLength::new(Length::new(nm)).unwrap()
}

fn ip(x: i64, y: i64) -> IntPoint {
    IntPoint::new(x, y)
}

fn format(path: &[IntPoint]) -> String {
    path.iter()
        .map(|p| format!("({}, {}) ", p.x, p.y))
        .collect()
}

// Test to reproduce https://github.com/LibrePCB/LibrePCB/issues/974
#[test]
fn test_convert_path_with_approximate() {
    let input = Path::new(vec![
        Vertex::new(Point::from_nm(30_875_000, 32_385_000), -Angle::DEG180),
        Vertex::new(Point::from_nm(26_275_000, 32_385_000), -Angle::DEG180),
        Vertex::new(Point::from_nm(30_875_000, 32_385_000), Angle::DEG0),
    ]);
    let output = clipper_helpers::path_to_clipper(&input, pos(1_000_000));
    assert_eq!(
        format(&output),
        "(30875000, 32385000) (29725000, 34376858) (27425000, 34376858) \
         (26275000, 32385000) (27425000, 30393142) (29725000, 30393142) \
         (30875000, 32385000) "
    );
}

#[test]
fn test_points_inside() {
    let square = vec![ip(0, 0), ip(10, 0), ip(10, 10), ip(0, 10)];
    assert!(clipper_helpers::all_points_inside(
        &[ip(0, 0), ip(5, 5)],
        &square
    ));
    assert!(!clipper_helpers::all_points_inside(
        &[ip(5, 5), ip(11, 5)],
        &square
    ));
    assert!(clipper_helpers::any_points_inside(
        &[ip(11, 5), ip(5, 5)],
        &square
    ));
    // Points on the outline are not "inside".
    assert!(!clipper_helpers::any_points_inside(
        &[ip(0, 0), ip(10, 5)],
        &square
    ));
    assert!(clipper_helpers::any_points_of_paths_inside(
        &[vec![ip(20, 20)], vec![ip(1, 1)]],
        &square
    ));
}

#[test]
fn test_flatten_tree_with_hole() {
    // A square with a square hole: the hole is connected to the outline by a
    // cut-in from its lowest point straight down to the outline.
    let outer = Path::new(
        [(0, 0), (100, 0), (100, 100), (0, 100), (0, 0)]
            .iter()
            .map(|(x, y)| Vertex::at(Point::from_nm(*x, *y)))
            .collect(),
    );
    let inner = Path::new(
        [(40, 40), (60, 40), (60, 60), (40, 60), (40, 40)]
            .iter()
            .map(|(x, y)| Vertex::at(Point::from_nm(*x, *y)))
            .collect(),
    );
    let paths = clipper_helpers::paths_to_clipper(&[outer, inner], pos(5000));
    let tree = clipper_helpers::unite_to_tree(&paths, PolyFillType::EvenOdd).unwrap();
    let flattened = clipper_helpers::flatten_tree(tree.root()).unwrap();
    assert_eq!(flattened.len(), 1);
    let flat = &flattened[0];
    // Outline (4) + cut-in (3 points + 4 hole points).
    assert_eq!(flat.len(), 11, "{}", format(flat));
    assert!(flat.contains(&ip(40, 0)));
    assert!(flat.contains(&ip(40, 40)));
    // The area is the outline minus the hole.
    assert_eq!(clipper::area(flat).abs(), 10000.0 - 400.0);
    let converted = clipper_helpers::paths_from_clipper(&flattened);
    assert!(converted[0].is_closed());
}

#[test]
fn test_intersect_all_needs_two_areas() {
    assert_eq!(
        clipper_helpers::intersect_all_to_tree(&[vec![vec![ip(0, 0), ip(1, 0), ip(1, 1)]]]).err(),
        Some(clipper_helpers::Error::TooFewAreas)
    );
}

#[test]
fn test_offset() {
    let mut paths = vec![vec![ip(0, 0), ip(1000, 0), ip(1000, 1000), ip(0, 1000)]];
    clipper_helpers::offset(
        &mut paths,
        Length::new(-100),
        pos(10),
        clipper::JoinType::Miter,
    )
    .unwrap();
    assert_eq!(paths.len(), 1);
    assert_eq!(clipper::area(&paths[0]).abs(), 800.0 * 800.0);
}
