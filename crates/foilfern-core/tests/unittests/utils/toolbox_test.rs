//! Port of tests/unittests/core/utils/toolboxtest.cpp (the geometry and
//! string range parts; `shapeFromPath()` and `floatToString()` are not
//! ported) and of the `increment_number_in_string()` tests of
//! libs/librepcb/rust-core/src/toolbox.rs.

use foilfern_core::types::{Angle, Point};
use foilfern_core::utils::toolbox;

fn p(x: i64, y: i64) -> Point {
    Point::from_nm(x, y)
}

#[test]
fn test_is_text_upside_down() {
    let data = [
        (-360_000_000, false), // 0°
        (-315_000_000, false), // 45°
        (-270_000_000, false), // 90°
        (-225_000_000, true),  // 135°
        (-180_000_000, true),  // 180°
        (-135_000_000, true),  // 225°
        (-90_000_000, true),   // 270°
        (-45_000_000, false),  // 315°
        (0, false),            // 0°
        (45_000_000, false),   // 45°
        (90_000_000, false),   // 90°
        (135_000_000, true),   // 135°
        (180_000_000, true),   // 180°
        (225_000_000, true),   // 225°
        (270_000_000, true),   // 270°
        (315_000_000, false),  // 315°
        (360_000_000, false),  // 0°
    ];
    for (udeg, expected) in data {
        assert_eq!(
            toolbox::is_text_upside_down(Angle::new(udeg)),
            expected,
            "{udeg}"
        );
    }
}

#[test]
fn test_increment_number_in_string() {
    let data = [
        ("", "1"),
        ("  ", "  1"),
        ("0", "1"),
        ("1", "2"),
        (" 123 ", " 124 "),
        ("X", "X1"),
        ("X-1", "X-2"),
        ("GND 41", "GND 42"),
        ("FOO1.2", "FOO1.3"),
        ("12 foo 34", "12 foo 35"),
        ("12 foo 34 bar 56 ", "12 foo 34 bar 57 "),
        ("99A", "100A"),
        ("9999999999", "99999999991"), // cover i32 overflow
        ("U1", "U2"),
    ];
    for (input, expected) in data {
        assert_eq!(toolbox::increment_number_in_string(input), expected);
    }
}

#[test]
fn test_arc_center() {
    #[rustfmt::skip]
    let data = [
        (p(0, 0),                   p(0, 0),                   Angle::DEG0,                     None),
        (p(0, 0),                   p(0, 0),                   Angle::from_deg(20.0).unwrap(), None),
        (p(1000, 2000),             p(5000, 4000),             Angle::DEG0,                     None),
        (p(47_744_137, 37_820_591), p(55_364_137, 24_622_364), -Angle::DEG90,                   Some(p(44_955_024, 27_411_478))),
        // Test to reproduce https://github.com/LibrePCB/LibrePCB/issues/974
        (p(30_875_000, 32_385_000), p(26_275_000, 32_385_000), -Angle::DEG180,                  Some(p(28_575_000, 32_385_000))),
        // Test to reproduce another case where small deviations were observed
        (p(-21_401_446, 16_018_901), p(-23_214_523, 17_264_994), -Angle::DEG180,                Some(p(-22_307_985, 16_641_948))),
    ];
    for (p1, p2, angle, center) in data {
        assert_eq!(toolbox::arc_center(p1, p2, angle), center);
    }
}

#[test]
fn test_arc_angle() {
    #[rustfmt::skip]
    let data = [
        (p(0, 0),                 p(0, 0),                 p(0, 0),                 Angle::DEG0),
        (p(2_000_000, 0),         p(1_000_000, 0),         p(0, 0),                 Angle::DEG0),
        (p(2_000_000, 0),         p(-1_000_000, 0),        p(0, 0),                 Angle::DEG180),
        (p(2_000_000, 3_000_000), p(-1_000_000, 2_000_000), p(1_000_000, 1_000_000), Angle::DEG90),
        (p(-1_000_000, 2_000_000), p(2_000_000, 3_000_000), p(1_000_000, 1_000_000), Angle::DEG270),
        (p(2_000_000, 3_000_000), p(3_000_000, 0),         p(1_000_000, 1_000_000), Angle::DEG270),
        (p(3_000_000, 0),         p(2_000_000, 3_000_000), p(1_000_000, 1_000_000), Angle::DEG90),
    ];
    for (p1, p2, center, angle) in data {
        assert_eq!(
            toolbox::arc_angle(p1, p2, center).to_deg_string(),
            angle.to_deg_string()
        );
    }
}

#[test]
fn test_arc_angle_from_3_points() {
    #[rustfmt::skip]
    let data = [
        (p(0, 0),                  p(0, 0),                  p(0, 0),                  Angle::DEG0),
        (p(2_000_000, 0),          p(1_500_000, 0),          p(1_000_000, 0),          Angle::DEG0),
        (p(2_000_000, 0),          p(500_000, 1_500_000),    p(-1_000_000, 0),         Angle::DEG180),
        (p(2_000_000, 0),          p(500_000, -1_500_000),   p(-1_000_000, 0),         -Angle::DEG180),
        (p(2_000_000, 3_000_000),  p(292_893, 3_121_320),    p(-1_000_000, 2_000_000), Angle::DEG90),
        (p(2_000_000, 3_000_000),  p(707_107, 1_878_680),    p(-1_000_000, 2_000_000), -Angle::DEG90),
        (p(-1_000_000, 2_000_000), p(1_707_107, -1_121_320), p(2_000_000, 3_000_000),  Angle::DEG270),
        (p(2_000_000, 3_000_000),  p(-1_121_320, 292_893),   p(3_000_000, 0),          Angle::DEG270),
        (p(2_000_000, 3_000_000),  p(6_121_320, 2_707_107),  p(3_000_000, 0),          -Angle::DEG270),
        (p(3_000_000, 0),          p(3_121_320, 1_707_107),  p(2_000_000, 3_000_000),  Angle::DEG90),
        (p(3_000_000, 0),          p(1_878_680, 1_292_893),  p(2_000_000, 3_000_000),  -Angle::DEG90),
    ];
    for (start, mid, end, angle) in data {
        // Currently the function under test is not needed for important
        // things, thus we allow quite some tolerance.
        let actual = toolbox::arc_angle_from_3_points(start, mid, end);
        assert!(
            (actual.to_deg() - angle.to_deg()).abs() <= 0.01,
            "{} != {}",
            actual.to_deg(),
            angle.to_deg()
        );
    }
}

#[test]
fn test_angle_between_points() {
    #[rustfmt::skip]
    let data = [
        (p(0, 0),                 p(0, 0),                  Angle::DEG0),
        (p(2_000_000, 0),         p(3_000_000, 0),          Angle::DEG0),
        (p(2_000_000, 0),         p(-1_000_000, 0),         Angle::DEG180),
        (p(2_000_000, 0),         p(2_000_000, 5_000_000),  Angle::DEG90),
        (p(2_000_000, 0),         p(2_000_000, -5_000_000), Angle::DEG270),
        (p(2_000_000, 2_000_000), p(1_000_000, 1_000_000),  Angle::DEG225),
    ];
    for (p1, p2, angle) in data {
        assert_eq!(
            toolbox::angle_between_points(p1, p2).to_deg_string(),
            angle.to_deg_string()
        );
    }
}

#[test]
fn test_expand_ranges_in_string() {
    let data: &[(&str, &[&str])] = &[
        ("", &[""]),
        ("  ", &["  "]),
        ("..", &[".."]),
        ("1", &["1"]),
        ("A..A", &["A"]),
        ("1..5", &["1", "2", "3", "4", "5"]),
        ("X-2..2", &["X-2"]),
        (
            "X-5..11",
            &["X-5", "X-6", "X-7", "X-8", "X-9", "X-10", "X-11"],
        ),
        ("X3..-1Y", &["X3..-1Y"]),
        ("0..1_X..Y", &["0_X", "0_Y", "1_X", "1_Y"]),
        (
            "-1..3_z..y",
            &["-1_z", "-1_y", "-2_z", "-2_y", "-3_z", "-3_y"],
        ),
        (
            "2..3B..A0..1",
            &["2B0", "2B1", "2A0", "2A1", "3B0", "3B1", "3A0", "3A1"],
        ),
    ];
    for (input, expected) in data {
        assert_eq!(
            toolbox::expand_ranges_in_string(input),
            *expected,
            "{input}"
        );
    }
}

#[test]
fn test_nearest_point_on_line() {
    let (l1, l2) = (p(0, 0), p(1_000_000, 0));
    assert_eq!(toolbox::nearest_point_on_line(p(-5, 5), l1, l2), l1);
    assert_eq!(toolbox::nearest_point_on_line(p(2_000_000, 5), l1, l2), l2);
    assert_eq!(
        toolbox::nearest_point_on_line(p(500_000, 700), l1, l2),
        p(500_000, 0)
    );
    let (distance, nearest) =
        toolbox::shortest_distance_between_point_and_line(p(500_000, 700), l1, l2);
    assert_eq!(*distance, foilfern_core::types::Length::new(700));
    assert_eq!(nearest, p(500_000, 0));
}
