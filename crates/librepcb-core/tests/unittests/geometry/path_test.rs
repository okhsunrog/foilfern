//! Port of tests/unittests/core/geometry/pathtest.cpp.

// The comparison tests negate operators on purpose (`!(a < b)`), as upstream.
#![allow(clippy::nonminimal_bool)]

use librepcb_core::geometry::{Path, Vertex};
use librepcb_core::types::{Angle, Length, Point, PositiveLength, UnsignedLength};

use super::serialize_str;

fn str(path: &Path) -> String {
    serialize_str(path, "path")
}

fn p(x: i64, y: i64) -> Point {
    Point::from_nm(x, y)
}

fn v(x: i64, y: i64) -> Vertex {
    Vertex::at(p(x, y))
}

fn va(x: i64, y: i64, angle: Angle) -> Vertex {
    Vertex::new(p(x, y), angle)
}

fn path(vertices: &[Vertex]) -> Path {
    Path::new(vertices.to_vec())
}

fn pos(nm: i64) -> PositiveLength {
    PositiveLength::new(Length::new(nm)).unwrap()
}

fn unsigned(nm: i64) -> UnsignedLength {
    UnsignedLength::new(Length::new(nm)).unwrap()
}

const A0: Angle = Angle::DEG0;
const A90: Angle = Angle::DEG90;
const A180: Angle = Angle::DEG180;
const A270: Angle = Angle::DEG270;

#[test]
fn test_default_constructor_creates_empty_path() {
    assert_eq!(Path::default().vertices().len(), 0);
}

#[test]
fn test_is_curved_false() {
    assert!(!Path::default().is_curved());
    assert!(!path(&[v(0, 0)]).is_curved());
    assert!(!path(&[v(0, 0), v(1, 1)]).is_curved());
}

// Ensure that the angle of the last vertex is not relevant.
#[test]
fn test_is_curved_last_vertex_false() {
    assert!(!path(&[va(1, 1, A90)]).is_curved());
    assert!(!path(&[v(0, 0), va(1, 1, A90)]).is_curved());
}

#[test]
fn test_is_curved_true() {
    assert!(path(&[va(0, 0, A90), v(1, 1)]).is_curved());
}

#[test]
fn test_is_zero_length_true() {
    assert!(Path::default().is_zero_length());
    assert!(path(&[va(0, 0, A90)]).is_zero_length());
    assert!(path(&[va(0, 0, A90), v(0, 0)]).is_zero_length());
    assert!(path(&[v(0, 0), v(0, 0), v(0, 0)]).is_zero_length());
}

#[test]
fn test_is_zero_length_false() {
    assert!(!path(&[v(0, 0), v(0, 1)]).is_zero_length());
    assert!(!path(&[v(0, 0), v(0, 0), v(1, 0)]).is_zero_length());
}

#[test]
fn test_is_on_grid() {
    let grid = pos(1_000_000);
    assert!(Path::default().is_on_grid(grid)); // Usually this is the good case.
    assert!(path(&[va(0, 0, A90), v(1_000_000, 2_000_000)]).is_on_grid(grid));
    assert!(!path(&[va(1, 0, A90), v(1_000_000, 2_000_000)]).is_on_grid(grid));
    assert!(!path(&[va(0, 0, A90), v(1_000_000, 2_000_001)]).is_on_grid(grid));
}

#[test]
fn test_get_total_straight_length() {
    let mut vertices = Vec::new();
    assert_eq!(path(&vertices).total_straight_length(), unsigned(0));
    vertices.push(v(10, 0));
    assert_eq!(path(&vertices).total_straight_length(), unsigned(0));
    vertices.push(v(10, 10));
    assert_eq!(path(&vertices).total_straight_length(), unsigned(10));
    vertices.push(v(10, 0));
    assert_eq!(path(&vertices).total_straight_length(), unsigned(20));
}

#[test]
fn test_to_svg_path_empty() {
    assert_eq!(Path::default().to_svg_path_mm(), "");
}

#[test]
fn test_to_svg_path_one_vertex() {
    let input = path(&[va(1_000_000, 1_234_567, Angle::DEG45)]);
    assert_eq!(input.to_svg_path_mm(), "M 1 -1.234567");
}

#[test]
fn test_to_svg_path() {
    let input = path(&[v(1_000_000, 1_234_567), v(0, 0), v(1_000_000, 1_234_567)]);
    assert_eq!(input.to_svg_path_mm(), "M 1 -1.234567 L 0 0 L 1 -1.234567");
}

#[test]
fn test_to_svg_path_arc() {
    let input = path(&[va(0, 0, -A180), v(2_000_000, 0)]);
    assert_eq!(input.to_svg_path_mm(), "M 0 0 A 1 1 0 1 1 2 0");
}

#[test]
fn test_reverse_empty_path() {
    assert_eq!(str(&Path::default().reversed()), str(&Path::default()));
}

#[test]
fn test_reverse_one_vertex() {
    let input = path(&[va(1, 2, A90)]);
    let expected = path(&[v(1, 2)]);
    assert_eq!(str(&input.reversed()), str(&expected));
}

#[test]
fn test_reverse_multiple_vertices() {
    let input = path(&[va(1, 2, A90), va(3, 4, A180), va(5, 6, A270), v(7, 8)]);
    let expected = path(&[va(7, 8, -A270), va(5, 6, -A180), va(3, 4, -A90), v(1, 2)]);
    let actual = input.reversed();
    assert_eq!(str(&actual), str(&expected));
    // Sanity check that reversing again restores the original path.
    assert_eq!(str(&actual.reversed()), str(&input));
}

#[test]
fn test_flatten_arcs_empty_path() {
    let actual = Path::default().flattened_arcs(pos(1));
    assert_eq!(str(&actual), str(&Path::default()));
}

#[test]
fn test_flatten_arcs_one_vertex() {
    let actual = path(&[va(10, 20, A180)]).flattened_arcs(pos(1));
    assert_eq!(str(&actual), str(&path(&[v(10, 20)])));
}

#[test]
fn test_flatten_arcs_two_vertices_arc() {
    let input = path(&[va(1000, 2000, A180), va(1000, 3000, A180)]);
    let expected = path(&[v(1000, 2000), v(1433, 2250), v(1433, 2750), v(1000, 3000)]);
    assert_eq!(str(&input.flattened_arcs(pos(600))), str(&expected));
}

#[test]
fn test_flatten_arcs_multiple_vertices() {
    let input = path(&[
        va(1000, 1000, A180),
        va(1000, 2000, A0),
        va(1000, 3000, A180),
        va(1000, 4000, A0),
        va(1000, 5000, A180),
        va(1000, 6000, A180),
        va(1000, 7000, A180),
        va(1000, 8000, A180),
    ]);
    let expected = path(&[
        v(1000, 1000),
        v(1433, 1250),
        v(1433, 1750),
        v(1000, 2000),
        v(1000, 3000),
        v(1433, 3250),
        v(1433, 3750),
        v(1000, 4000),
        v(1000, 5000),
        v(1433, 5250),
        v(1433, 5750),
        v(1000, 6000),
        v(1433, 6250),
        v(1433, 6750),
        v(1000, 7000),
        v(1433, 7250),
        v(1433, 7750),
        v(1000, 8000),
    ]);
    assert_eq!(str(&input.flattened_arcs(pos(600))), str(&expected));
}

#[test]
fn test_clean_empty_path() {
    let mut actual = Path::default();
    assert!(!actual.clean());
    assert_eq!(str(&actual), str(&Path::default()));
}

#[test]
fn test_clean_one_vertex() {
    let mut actual = path(&[va(1, 2, A90)]);
    assert!(!actual.clean());
    assert_eq!(str(&actual), str(&path(&[va(1, 2, A90)])));
}

#[test]
fn test_clean_multiple_vertices() {
    let mut actual = path(&[
        va(1, 2, Angle::DEG45),
        va(1, 2, A90), // duplicate
        va(3, 4, A0),
        va(5, 6, A0),
        va(5, 6, A180), // duplicate
        va(5, 6, A270), // duplicate
        va(7, 8, A0),
        va(9, 9, A180),
        va(9, 9, A270), // duplicate
    ]);
    let expected = path(&[
        va(1, 2, A90),
        va(3, 4, A0),
        va(5, 6, A270),
        va(7, 8, A0),
        va(9, 9, A270),
    ]);
    assert!(actual.clean());
    assert_eq!(str(&actual), str(&expected));
}

#[test]
fn test_open_empty_path() {
    let mut actual = Path::default();
    assert!(!actual.open());
    assert_eq!(str(&actual), str(&Path::default()));
}

#[test]
fn test_open_two_vertices() {
    let mut actual = path(&[va(1, 2, A180), va(1, 2, A180)]);
    assert!(!actual.open());
    assert_eq!(str(&actual), str(&path(&[va(1, 2, A180), va(1, 2, A180)])));
}

#[test]
fn test_open_multiple_vertices_closed() {
    let mut actual = path(&[va(1, 2, Angle::DEG45), va(3, 4, A90), va(1, 2, A180)]);
    assert!(actual.open());
    assert_eq!(
        str(&actual),
        str(&path(&[va(1, 2, Angle::DEG45), va(3, 4, A90)]))
    );
}

#[test]
fn test_open_multiple_vertices_open() {
    let input = path(&[va(1, 2, Angle::DEG45), va(3, 4, A90), va(5, 6, A180)]);
    let mut actual = input.clone();
    assert!(!actual.open());
    assert_eq!(str(&actual), str(&input));
}

#[test]
fn test_operator_compare_less() {
    assert!(!(Path::default() < Path::default()));
    assert!(!(path(&[v(1, 2)]) < Path::default()));
    assert!(!(path(&[v(1, 2)]) < path(&[v(1, 2)])));
    assert!(!(path(&[v(2, 2)]) < path(&[v(1, 2)])));
    assert!(!(path(&[va(0, 0, A90)]) < path(&[va(0, 0, A0)])));
    assert!(Path::default() < path(&[v(1, 2)]));
    assert!(path(&[v(1, 2)]) < path(&[v(2, 2)]));
    assert!(path(&[va(0, 0, A0)]) < path(&[va(0, 0, A90)]));
}

#[test]
fn test_line() {
    let (p1, p2, angle) = (p(12, 34), p(56, 78), Angle::new(1234));
    let path = Path::line(p1, p2, angle);
    assert_eq!(path.vertices(), &[Vertex::new(p1, angle), Vertex::at(p2)]);
    assert!(!path.is_closed());
}

#[test]
fn test_circle() {
    let path = Path::circle(pos(1000));
    assert_eq!(
        path.vertices(),
        &[va(500, 0, -A180), va(-500, 0, -A180), va(500, 0, A0)]
    );
    assert!(path.is_closed());
}

#[test]
fn test_donut() {
    let expected = path(&[
        va(0, 500, -A180),
        va(0, -500, A0),
        va(0, -250, A180),
        va(0, 250, A180),
        va(0, -250, A0),
        va(0, -500, -A180),
        va(0, 500, A0),
    ]);
    let actual = Path::donut(pos(1000), pos(500));
    assert_eq!(str(&actual), str(&expected));
    assert!(actual.is_closed());
}

#[test]
fn test_donut_invalid() {
    let actual = Path::donut(pos(1000), pos(1000));
    assert_eq!(str(&actual), str(&Path::default()));
}

// If the line has zero length and angle, the filled area is zero, so no path
// should be returned.
#[test]
fn test_flat_cap_line_zero_length() {
    let actual = Path::flat_cap_line(Point::ORIGIN, Point::ORIGIN, A0, pos(1_000_000));
    assert!(actual.is_none());
}

// If the line has zero length but angle, we don't know the start- and end
// angles, so there's nothing we can return.
#[test]
fn test_flat_cap_line_zero_length_arc() {
    let actual = Path::flat_cap_line(Point::ORIGIN, Point::ORIGIN, A90, pos(1_000_000));
    assert!(actual.is_none());
}

#[test]
fn test_flat_cap_line_straight_horizontal() {
    let expected = path(&[
        v(1_000_000, 2_500_000),
        v(3_000_000, 2_500_000),
        v(3_000_000, 1_500_000),
        v(1_000_000, 1_500_000),
        v(1_000_000, 2_500_000),
    ]);
    let actual = Path::flat_cap_line(
        p(1_000_000, 2_000_000),
        p(3_000_000, 2_000_000),
        A0,
        pos(1_000_000),
    )
    .unwrap();
    assert_eq!(str(&actual), str(&expected));
}

#[test]
fn test_flat_cap_line_straight_horizontal_reverse() {
    let expected = path(&[
        v(3_000_000, 1_500_000),
        v(1_000_000, 1_500_000),
        v(1_000_000, 2_500_000),
        v(3_000_000, 2_500_000),
        v(3_000_000, 1_500_000),
    ]);
    let actual = Path::flat_cap_line(
        p(3_000_000, 2_000_000),
        p(1_000_000, 2_000_000),
        A0,
        pos(1_000_000),
    )
    .unwrap();
    assert_eq!(str(&actual), str(&expected));
}

#[test]
fn test_flat_cap_line_arc_90() {
    let expected = path(&[
        va(2_000_000, 1_000_000, A90),
        v(1_000_000, 2_000_000),
        va(1_000_000, 4_000_000, -A90),
        v(4_000_000, 1_000_000),
        v(2_000_000, 1_000_000),
    ]);
    let actual = Path::flat_cap_line(
        p(3_000_000, 1_000_000),
        p(1_000_000, 3_000_000),
        A90,
        pos(2_000_000),
    )
    .unwrap();
    assert_eq!(str(&actual), str(&expected));
}

#[test]
fn test_centered_rect_rounded_corners() {
    let expected = path(&[
        v(-30000, 75000),
        va(30000, 75000, -A90),
        v(50000, 55000),
        va(50000, -55000, -A90),
        v(30000, -75000),
        va(-30000, -75000, -A90),
        v(-50000, -55000),
        va(-50000, 55000, -A90),
        v(-30000, 75000),
    ]);
    let actual = Path::centered_rect(pos(100_000), pos(150_000), unsigned(20000));
    assert_eq!(str(&actual), str(&expected));
}

#[test]
fn test_centered_rect_rounded_corners_saturation() {
    let expected = Path::obround(pos(100_000), pos(150_000));
    let actual = Path::centered_rect(pos(100_000), pos(150_000), unsigned(60000));
    assert_eq!(str(&actual), str(&expected));
}

#[test]
fn test_chamfered_rect() {
    let expected = path(&[
        v(-500, 150),
        v(-400, 250),
        v(400, 250),
        v(500, 150),
        v(500, -150),
        v(400, -250),
        v(-400, -250),
        v(-500, -150),
        v(-500, 150),
    ]);
    let actual = Path::chamfered_rect(pos(1000), pos(500), unsigned(100), true, true, true, true);
    assert_eq!(str(&actual), str(&expected));
    assert!(actual.is_closed());
}

#[test]
fn test_chamfered_rect_top_right() {
    let expected = path(&[
        v(-500, 250),
        v(400, 250),
        v(500, 150),
        v(500, -250),
        v(-500, -250),
        v(-500, 250),
    ]);
    let actual = Path::chamfered_rect(
        pos(1000),
        pos(500),
        unsigned(100),
        false,
        true,
        false,
        false,
    );
    assert_eq!(str(&actual), str(&expected));
    assert!(actual.is_closed());
}

#[test]
fn test_chamfered_rect_saturated() {
    let expected = path(&[
        v(-500, 0),
        v(-250, 250),
        v(250, 250),
        v(500, 0),
        v(250, -250),
        v(-250, -250),
        v(-500, 0),
    ]);
    // Chamfer must be clipped to 250.
    let actual = Path::chamfered_rect(pos(1000), pos(500), unsigned(300), true, true, true, true);
    assert_eq!(str(&actual), str(&expected));
    assert!(actual.is_closed());
}

#[test]
fn test_trapezoid() {
    let expected = path(&[
        v(-600, 350),
        v(600, 450),
        v(400, -450),
        v(-400, -350),
        v(-600, 350),
    ]);
    let actual = Path::trapezoid(pos(1000), pos(800), Length::new(200), Length::new(100));
    assert_eq!(str(&actual), str(&expected));
    assert!(actual.is_closed());
}

// Note: This is actually a very strange case, in real world we'll probably
// never create such trapezoids which do not look like a trapezoid anymore.
#[test]
fn test_trapezoid_saturated() {
    let expected = path(&[
        v(-1000, 800),
        v(1000, 0),
        v(0, 0),
        v(0, -800),
        v(-1000, 800),
    ]);
    // dw must be clipped to 1000, dh to -800.
    let actual = Path::trapezoid(pos(1000), pos(800), Length::new(1200), Length::new(-1200));
    assert_eq!(str(&actual), str(&expected));
    assert!(actual.is_closed());
}

#[test]
fn test_trapezoid_dw() {
    let expected = path(&[
        v(-600, 400),
        v(600, 400),
        v(400, -400),
        v(-400, -400),
        v(-600, 400),
    ]);
    let actual = Path::trapezoid(pos(1000), pos(800), Length::new(200), Length::new(0));
    assert_eq!(str(&actual), str(&expected));
    assert!(actual.is_closed());
}

#[test]
fn test_trapezoid_dh() {
    let expected = path(&[
        v(-500, 500),
        v(500, 300),
        v(500, -300),
        v(-500, -500),
        v(-500, 500),
    ]);
    let actual = Path::trapezoid(pos(1000), pos(800), Length::new(0), Length::new(-200));
    assert_eq!(str(&actual), str(&expected));
    assert!(actual.is_closed());
}

#[test]
fn test_octagon_rounded_corners_saturation() {
    let expected = Path::obround(pos(100_000), pos(150_000));
    let actual = Path::octagon(pos(100_000), pos(150_000), unsigned(60000));
    assert_eq!(str(&actual), str(&expected));
}

// Test to reproduce https://github.com/LibrePCB/LibrePCB/issues/974
#[test]
fn test_flat_arc() {
    let expected = path(&[
        v(30_875_000, 32_385_000),
        v(29_725_000, 30_393_142),
        v(27_425_000, 30_393_142),
        v(26_275_000, 32_385_000),
    ]);
    let actual = Path::flat_arc(
        p(30_875_000, 32_385_000),
        p(26_275_000, 32_385_000),
        -A180,
        pos(1_000_000),
    );
    assert_eq!(str(&actual), str(&expected));
}

// Test to reproduce another case where small deviations were observed
#[test]
fn test_flat_arc2() {
    let expected = path(&[
        v(-21_401_446, 16_018_901),
        v(-22_394_290, 15_545_339),
        v(-23_300_829, 16_168_386),
        v(-23_214_523, 17_264_994),
    ]);
    let actual = Path::flat_arc(
        p(-21_401_446, 16_018_901),
        p(-23_214_523, 17_264_994),
        -A180,
        pos(2_000_000),
    );
    assert_eq!(str(&actual), str(&expected));
}

#[test]
fn test_obround_width_height() {
    let data: &[(i64, i64, &[Vertex])] = &[
        // width == height
        (10, 10, &[va(5, 0, -A180), va(-5, 0, -A180), va(5, 0, A0)]),
        // width > height
        (
            30,
            10,
            &[
                va(-10, 5, A0),
                va(10, 5, -A180),
                va(10, -5, A0),
                va(-10, -5, -A180),
                va(-10, 5, A0),
            ],
        ),
        // width < height
        (
            10,
            30,
            &[
                va(5, 10, A0),
                va(5, -10, -A180),
                va(-5, -10, A0),
                va(-5, 10, -A180),
                va(5, 10, A0),
            ],
        ),
    ];
    for (width, height, vertices) in data {
        let path = Path::obround(pos(*width), pos(*height));
        assert_eq!(path.vertices(), *vertices);
        assert!(path.is_closed());
    }
}

#[test]
fn test_obround_p1_p2_width() {
    let data: &[(Point, Point, i64, &[Vertex])] = &[
        // on x-axis from negative to positive
        (
            p(-10, 0),
            p(10, 0),
            20,
            &[
                va(-10, 10, A0),
                va(10, 10, -A180),
                va(10, -10, A0),
                va(-10, -10, -A180),
                va(-10, 10, A0),
            ],
        ),
        // horizontal from positive to negative
        (
            p(10, 55),
            p(-10, 55),
            2,
            &[
                va(10, 54, A0),
                va(-10, 54, -A180),
                va(-10, 56, A0),
                va(10, 56, -A180),
                va(10, 54, A0),
            ],
        ),
        // on y-axis from negative to positive
        (
            p(0, -20),
            p(0, -10),
            2,
            &[
                va(-1, -20, A0),
                va(-1, -10, -A180),
                va(1, -10, A0),
                va(1, -20, -A180),
                va(-1, -20, A0),
            ],
        ),
        // vertical from positive to negative
        (
            p(-5, -10),
            p(-5, -20),
            2,
            &[
                va(-4, -10, A0),
                va(-4, -20, -A180),
                va(-6, -20, A0),
                va(-6, -10, -A180),
                va(-4, -10, A0),
            ],
        ),
    ];
    for (p1, p2, width, vertices) in data {
        let path = Path::obround_line(*p1, *p2, pos(*width));
        assert_eq!(path.vertices(), *vertices);
        assert!(path.is_closed());
    }
}
