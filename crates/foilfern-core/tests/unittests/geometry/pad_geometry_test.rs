//! Tests of libs/librepcb/core/geometry/padgeometry.{h,cpp} (no upstream
//! test exists; expected values are derived from the upstream algorithm).

use foilfern_core::geometry::{
    NonEmptyPath, PadGeometry, PadGeometryShape, PadHole, PadHoleList, Path, Vertex,
};
use foilfern_core::types::{
    Angle, Length, Point, PositiveLength, Ratio, UnsignedLength, UnsignedLimitedRatio, Uuid,
};

fn pos(nm: i64) -> PositiveLength {
    PositiveLength::new(Length::new(nm)).unwrap()
}

fn unsigned(nm: i64) -> UnsignedLength {
    UnsignedLength::new(Length::new(nm)).unwrap()
}

fn ratio(percent: i32) -> UnsignedLimitedRatio {
    UnsignedLimitedRatio::new(Ratio::from_percent(percent)).unwrap()
}

fn pt(x: i64, y: i64) -> Point {
    Point::from_nm(x, y)
}

fn polyline(points: &[(i64, i64)]) -> Path {
    Path::new(points.iter().map(|&(x, y)| Vertex::at(pt(x, y))).collect())
}

fn holes(paths: Vec<(i64, Path)>) -> PadHoleList {
    paths
        .into_iter()
        .map(|(diameter, path)| {
            PadHole::new(
                Uuid::new_random(),
                pos(diameter),
                NonEmptyPath::new(path).unwrap(),
            )
        })
        .collect()
}

/// Returns the sorted, deduplicated vertex positions of the paths.
fn positions(paths: &[Path]) -> Vec<Point> {
    let mut points: Vec<Point> = paths
        .iter()
        .flat_map(|p| p.vertices().iter().map(|v| v.pos))
        .collect();
    points.sort();
    points.dedup();
    points
}

#[test]
fn test_rounded_rect_size_and_corner_radius() {
    let g = PadGeometry::rounded_rect(
        pos(2_000_000),
        pos(1_000_000),
        ratio(50),
        PadHoleList::new(),
    );
    assert_eq!(g.shape(), PadGeometryShape::RoundedRect);
    assert_eq!(g.width(), Length::new(2_000_000));
    assert_eq!(g.height(), Length::new(1_000_000));
    // 50% of half the smaller side.
    assert_eq!(g.corner_radius(), unsigned(250_000));

    // A positive offset grows the size and the radius.
    let g2 = g.with_offset(Length::new(100_000));
    assert_eq!(g2.width(), Length::new(2_200_000));
    assert_eq!(g2.height(), Length::new(1_200_000));
    assert_eq!(g2.corner_radius(), unsigned(350_000));

    // Offsets accumulate; the radius doesn't get negative.
    let g3 = g2.with_offset(Length::new(-400_000));
    assert_eq!(g3.width(), Length::new(1_400_000));
    assert_eq!(g3.height(), Length::new(400_000));
    assert_eq!(g3.corner_radius(), UnsignedLength::ZERO);

    // Size may become negative.
    let g4 = g.with_offset(Length::new(-600_000));
    assert_eq!(g4.height(), Length::new(-200_000));
}

#[test]
fn test_corner_radius_limits() {
    let g = PadGeometry::rounded_rect(
        pos(2_000_000),
        pos(1_000_000),
        ratio(100),
        PadHoleList::new(),
    );
    assert_eq!(g.corner_radius(), unsigned(500_000));
    let g = PadGeometry::rounded_rect(pos(2_000_000), pos(1_000_000), ratio(0), PadHoleList::new());
    assert_eq!(g.corner_radius(), UnsignedLength::ZERO);
    // A positive offset rounds sharp corners.
    assert_eq!(
        g.with_offset(Length::new(100_000)).corner_radius(),
        unsigned(100_000)
    );
    // Rounding to nanometers.
    let g = PadGeometry::rounded_rect(pos(333), pos(1_000), ratio(33), PadHoleList::new());
    assert_eq!(g.corner_radius(), unsigned(55)); // 166nm * 0.33 = 54.78nm
}

#[test]
fn test_rounded_rect_outlines() {
    let g = PadGeometry::rounded_rect(
        pos(2_000_000),
        pos(1_000_000),
        ratio(50),
        PadHoleList::new(),
    );
    assert_eq!(
        g.to_outlines().unwrap(),
        [Path::centered_rect(
            pos(2_000_000),
            pos(1_000_000),
            unsigned(250_000)
        )]
    );
    assert_eq!(
        g.with_offset(Length::new(-100_000)).to_outlines().unwrap(),
        [Path::centered_rect(
            pos(1_800_000),
            pos(800_000),
            unsigned(150_000)
        )]
    );
    // Nothing left if the offset consumes the whole pad.
    assert!(
        g.with_offset(Length::new(-500_000))
            .to_outlines()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn test_rounded_octagon_outlines() {
    let g = PadGeometry::rounded_octagon(
        pos(2_000_000),
        pos(1_000_000),
        ratio(20),
        PadHoleList::new(),
    );
    assert_eq!(g.shape(), PadGeometryShape::RoundedOctagon);
    assert_eq!(g.corner_radius(), unsigned(100_000));
    assert_eq!(
        g.to_outlines().unwrap(),
        [Path::octagon(
            pos(2_000_000),
            pos(1_000_000),
            unsigned(100_000)
        )]
    );
    assert_eq!(
        g.with_offset(Length::new(50_000)).to_outlines().unwrap(),
        [Path::octagon(
            pos(2_100_000),
            pos(1_100_000),
            unsigned(150_000)
        )]
    );
    assert!(
        g.with_offset(Length::new(-600_000))
            .to_outlines()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn test_stroke_outlines() {
    // Single vertex: a circle, not united.
    let path = NonEmptyPath::from_point(pt(1_000_000, 2_000_000));
    let g = PadGeometry::stroke(pos(500_000), path, PadHoleList::new());
    assert_eq!(g.shape(), PadGeometryShape::Stroke);
    assert_eq!(g.width(), Length::new(500_000));
    assert_eq!(g.height(), Length::ZERO);
    assert_eq!(g.corner_radius(), UnsignedLength::ZERO);
    assert_eq!(
        g.to_outlines().unwrap(),
        [Path::circle(pos(500_000)).translated(pt(1_000_000, 2_000_000))]
    );
    assert_eq!(
        g.with_offset(Length::new(100_000)).to_outlines().unwrap(),
        [Path::circle(pos(700_000)).translated(pt(1_000_000, 2_000_000))]
    );
    assert!(
        g.with_offset(Length::new(-250_000))
            .to_outlines()
            .unwrap()
            .is_empty()
    );

    // One straight segment: an obround, not united.
    let path = NonEmptyPath::new(polyline(&[(0, 0), (1_000_000, 0)])).unwrap();
    let g = PadGeometry::stroke(pos(500_000), path, PadHoleList::new());
    assert_eq!(
        g.to_outlines().unwrap(),
        [Path::obround_line(pt(0, 0), pt(1_000_000, 0), pos(500_000))]
    );

    // Several segments: united to a single, flattened outline.
    let path =
        NonEmptyPath::new(polyline(&[(0, 0), (1_000_000, 0), (1_000_000, 1_000_000)])).unwrap();
    let g = PadGeometry::stroke(pos(500_000), path, PadHoleList::new());
    let outlines = g.to_outlines().unwrap();
    assert_eq!(outlines.len(), 1);
    assert!(!outlines[0].is_curved());
    let points = positions(&outlines);
    let min_x = points.iter().map(|p| p.x).min().unwrap();
    let max_x = points.iter().map(|p| p.x).max().unwrap();
    let min_y = points.iter().map(|p| p.y).min().unwrap();
    let max_y = points.iter().map(|p| p.y).max().unwrap();
    // Arcs are flattened with at most 5µm tolerance (vertices on the arc).
    assert_eq!(min_x, Length::new(-250_000));
    assert_eq!(max_x, Length::new(1_250_000));
    assert_eq!(min_y, Length::new(-250_000));
    assert_eq!(max_y, Length::new(1_250_000));

    // A single arc segment is united (flattened) as well.
    let path = NonEmptyPath::new(Path::line(pt(0, 0), pt(1_000_000, 0), Angle::DEG180)).unwrap();
    let outlines = PadGeometry::stroke(pos(200_000), path, PadHoleList::new())
        .to_outlines()
        .unwrap();
    assert_eq!(outlines.len(), 1);
    assert!(!outlines[0].is_curved());
}

#[test]
fn test_custom_outlines() {
    // The outline is closed implicitly and cleaned by Clipper.
    let triangle = polyline(&[(0, 0), (1_000_000, 0), (0, 1_000_000)]);
    let g = PadGeometry::custom(triangle.clone(), PadHoleList::new());
    assert_eq!(g.shape(), PadGeometryShape::Custom);
    assert_eq!(g.width(), Length::ZERO);
    assert_eq!(g.height(), Length::ZERO);
    let outlines = g.to_outlines().unwrap();
    assert_eq!(outlines.len(), 1);
    assert!(outlines[0].is_closed());
    assert_eq!(
        positions(&outlines),
        [pt(0, 0), pt(0, 1_000_000), pt(1_000_000, 0)]
    );

    // With offset.
    let outlines = g.with_offset(Length::new(-100_000)).to_outlines().unwrap();
    assert_eq!(outlines.len(), 1);
    let points = positions(&outlines);
    assert_eq!(points.len(), 3);
    assert_eq!(points[0], pt(100_000, 100_000));
    let outlines = g.with_offset(Length::new(100_000)).to_outlines().unwrap();
    assert_eq!(outlines.len(), 1);
    assert!(positions(&outlines).len() > 3); // Rounded corners.

    // Less than 3 vertices: nothing.
    let line = polyline(&[(0, 0), (1_000_000, 0)]);
    assert!(
        PadGeometry::custom(line, PadHoleList::new())
            .to_outlines()
            .unwrap()
            .is_empty()
    );
    assert!(
        PadGeometry::custom(Path::default(), PadHoleList::new())
            .to_outlines()
            .unwrap()
            .is_empty()
    );

    // Arcs are flattened.
    let mut circle = Path::circle(pos(1_000_000));
    circle.open();
    let outlines = PadGeometry::custom(circle, PadHoleList::new())
        .to_outlines()
        .unwrap();
    assert_eq!(outlines.len(), 1);
    assert!(!outlines[0].is_curved());
    assert!(outlines[0].vertices().len() > 10);
}

#[test]
fn test_custom_outline_validity() {
    let triangle = polyline(&[(0, 0), (1_000_000, 0), (0, 1_000_000)]);
    assert!(PadGeometry::is_valid_custom_outline(&triangle));
    // Closing the path is optional.
    let mut closed = triangle.clone();
    closed.close();
    assert!(PadGeometry::is_valid_custom_outline(&closed));
    // Zero area.
    let collinear = polyline(&[(0, 0), (1_000_000, 0), (2_000_000, 0)]);
    assert!(!PadGeometry::is_valid_custom_outline(&collinear));
    assert!(!PadGeometry::is_valid_custom_outline(&Path::default()));
    // Area must be greater than 1nm².
    let tiny = polyline(&[(0, 0), (1, 0), (0, 2)]);
    assert!(!PadGeometry::is_valid_custom_outline(&tiny));
    let small = polyline(&[(0, 0), (2, 0), (0, 2)]);
    assert!(PadGeometry::is_valid_custom_outline(&small));
}

#[test]
fn test_holes() {
    let slot = polyline(&[(0, 0), (1_000_000, 0), (1_000_000, 1_000_000)]);
    let hole_list = holes(vec![
        (300_000, Path::new(vec![Vertex::at(pt(-1_000_000, 0))])),
        (400_000, slot),
    ]);
    let g = PadGeometry::rounded_rect(pos(3_000_000), pos(3_000_000), ratio(0), hole_list.clone());
    assert_eq!(g.holes(), &hole_list);
    assert_eq!(
        g.to_hole_outlines(),
        [
            Path::circle(pos(300_000)).translated(pt(-1_000_000, 0)),
            Path::obround_line(pt(0, 0), pt(1_000_000, 0), pos(400_000)),
            Path::obround_line(pt(1_000_000, 0), pt(1_000_000, 1_000_000), pos(400_000)),
        ]
    );
    // Offsets don't affect holes.
    assert_eq!(
        g.with_offset(Length::new(100_000)).to_hole_outlines(),
        g.to_hole_outlines()
    );

    let without = g.without_holes();
    assert!(without.holes().is_empty());
    assert!(without.to_hole_outlines().is_empty());
    assert_eq!(without.to_outlines().unwrap(), g.to_outlines().unwrap());
    assert_ne!(without, g);
}
