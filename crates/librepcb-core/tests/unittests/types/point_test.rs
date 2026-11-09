//! Port of tests/unittests/core/types/pointtest.cpp.

// The comparison tests negate operators on purpose (`!(a < b)`), as upstream.
#![allow(clippy::nonminimal_bool)]

use librepcb_core::types::{Angle, Length, Point, UnsignedLength};

fn p(x: i64, y: i64) -> Point {
    Point::from_nm(x, y)
}

fn mm(x: f64, y: f64) -> Point {
    Point::from_mm(x, y).unwrap()
}

#[test]
fn test_default_constructor() {
    let point = Point::default();
    assert_eq!(point.x.to_nm(), 0);
    assert_eq!(point.y.to_nm(), 0);
    assert!(point.is_origin());
}

#[test]
fn test_operator_less_than() {
    assert!(!(p(0, 0) < p(0, 0)));
    assert!(!(p(10, 20) < p(9, 19)));
    assert!(!(p(10, 20) < p(9, 21)));
    assert!(!(p(10, 20) < p(10, 19)));
    assert!(!(p(-10, 20) < p(-11, 0)));
    assert!(p(0, 0) < p(0, 1));
    assert!(p(0, 0) < p(1, 0));
    assert!(p(10, 20) < p(11, 19));
    assert!(p(10, 20) < p(11, 21));
    assert!(p(10, 20) < p(10, 21));
    assert!(p(-1, -2) < p(-1, -1));
}

#[test]
fn test_operator_less_equal() {
    assert!(!(p(10, 20) <= p(9, 19)));
    assert!(!(p(10, 20) <= p(9, 21)));
    assert!(!(p(10, 20) <= p(10, 19)));
    assert!(!(p(-10, 20) <= p(-11, 0)));
    assert!(p(0, 0) <= p(0, 0));
    assert!(p(0, 0) <= p(0, 1));
    assert!(p(0, 0) <= p(1, 0));
    assert!(p(10, 20) <= p(11, 19));
    assert!(p(10, 20) <= p(11, 21));
    assert!(p(10, 20) <= p(10, 21));
    assert!(p(-1, -2) <= p(-1, -1));
}

#[test]
fn test_operator_greater_than() {
    assert!(!(p(0, 0) > p(0, 0)));
    assert!(!(p(0, 0) > p(0, 1)));
    assert!(!(p(0, 0) > p(1, 0)));
    assert!(!(p(10, 20) > p(11, 19)));
    assert!(!(p(10, 20) > p(11, 21)));
    assert!(!(p(10, 20) > p(10, 21)));
    assert!(!(p(-1, -2) > p(-1, -1)));
    assert!(p(10, 20) > p(9, 19));
    assert!(p(10, 20) > p(9, 21));
    assert!(p(10, 20) > p(10, 19));
    assert!(p(-10, 20) > p(-11, 0));
}

#[test]
fn test_operator_greater_equal() {
    assert!(!(p(0, 0) >= p(0, 1)));
    assert!(!(p(0, 0) >= p(1, 0)));
    assert!(!(p(10, 20) >= p(11, 19)));
    assert!(!(p(10, 20) >= p(11, 21)));
    assert!(!(p(10, 20) >= p(10, 21)));
    assert!(!(p(-1, -2) >= p(-1, -1)));
    assert!(p(0, 0) >= p(0, 0));
    assert!(p(10, 20) >= p(9, 19));
    assert!(p(10, 20) >= p(9, 21));
    assert!(p(10, 20) >= p(10, 19));
    assert!(p(-10, 20) >= p(-11, 0));
}

#[test]
fn test_rotate() {
    let deg = |d: f64| Angle::from_deg(d).unwrap();
    #[rustfmt::skip]
    let data: Vec<(Point, Angle, Point, Point)> = vec![
        (mm(10.0, 0.0),   deg(0.0),   mm(0.0, 0.0),    mm(10.0, 0.0)  ),
        (mm(10.0, 0.0),   deg(180.0), mm(0.0, 0.0),    mm(-10.0, 0.0) ),
        (mm(10.0, 0.0),   deg(270.0), mm(0.0, 0.0),    mm(0.0, -10.0) ),
        (mm(10.0, 0.0),   deg(0.0),   mm(0.0, 0.0),    mm(10.0, 0.0)  ),

        (mm(0.0, 0.0),    deg(90.0),  mm(0.0, 0.0),    mm(0.0, 0.0)   ),
        (mm(10.0, 0.0),   deg(90.0),  mm(0.0, 0.0),    mm(0.0, 10.0)  ),
        (mm(0.0, 10.0),   deg(90.0),  mm(0.0, 0.0),    mm(-10.0, 0.0) ),
        (mm(-10.0, 0.0),  deg(90.0),  mm(0.0, 0.0),    mm(0.0, -10.0) ),
        (mm(0.0, -10.0),  deg(90.0),  mm(0.0, 0.0),    mm(10.0, 0.0)  ),

        (mm(100.0, 50.0), deg(90.0),  mm(100.0, 50.0), mm(100.0, 50.0)),
        (mm(110.0, 50.0), deg(90.0),  mm(100.0, 50.0), mm(100.0, 60.0)),
        (mm(100.0, 60.0), deg(90.0),  mm(100.0, 50.0), mm(90.0, 50.0) ),
        (mm(90.0, 50.0),  deg(90.0),  mm(100.0, 50.0), mm(100.0, 40.0)),
        (mm(100.0, 40.0), deg(90.0),  mm(100.0, 50.0), mm(110.0, 50.0)),

        (p(10, 0),        deg(1.0),   p(0, 0),         p(10, 0)       ),
        (p(10, 0),        deg(2.0),   p(0, 0),         p(10, 0)       ),
        (p(10, 0),        deg(3.0),   p(0, 0),         p(10, 1)       ),
        (p(10, 0),        deg(4.0),   p(0, 0),         p(10, 1)       ),
        (p(10, 0),        deg(18.0),  p(0, 0),         p(10, 3)       ),
        (p(10, 0),        deg(19.0),  p(0, 0),         p(9, 3)        ),
    ];
    for (input, angle, center, output) in data {
        assert_eq!(input.rotated(angle, center), output, "{input:?} {angle:?}");
    }
}

#[test]
fn test_length_precision() {
    const MM: i64 = 1_000_000;
    const M: i64 = 1_000_000_000;
    const KM: i64 = 1_000_000_000_000;
    let ul = |nm: i64| UnsignedLength::new(Length::new(nm)).unwrap();
    // sqrt2 according wikipedia
    // 1.41421356237309504880168872420969807856967187537694807317667973799
    #[rustfmt::skip]
    let data: &[(i64, i64, i64)] = &[
        (0,        0,        0),
        // keep precision on nanometer scale
        (10,       10,       14),
        // keep precision on millimeter scale
        (MM,       MM,       1414213),
        // keep precision on meter scale
        (M,        M,        1414213562),
        (2 * M,    2 * M,    2828427124),
        (3 * M,    3 * M,    4242640687),
        // keep precision on kilometer scale
        (KM,       KM,       1414213562373),
        (10 * KM,  10 * KM,  14142135623730),
        (100 * KM, 100 * KM, 141421356237309),
        // keep precision on small planet's scale
        (1000 * KM, 1000 * KM, 1414213562373095),
    ];
    for &(range, axis_length, squared_length) in data {
        assert_eq!(p(range, 0).length(), ul(axis_length));
        assert_eq!(p(0, range).length(), ul(axis_length));
        assert_eq!(p(range, range).length(), ul(squared_length));
    }
}
