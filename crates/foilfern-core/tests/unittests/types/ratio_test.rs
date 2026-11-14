//! Port of tests/unittests/core/types/ratiotest.cpp.

use foilfern_core::serialization::{FromSExpression, Mode, SExpression, ToSExpression};
use foilfern_core::types::Ratio;

struct RatioTestData {
    ratio: Ratio,
    ppm: i32,
    percent: f64,
    normalized: f64,
    string: &'static str,
}

const fn d(ppm: i32, percent: f64, normalized: f64, string: &'static str) -> RatioTestData {
    RatioTestData {
        ratio: Ratio::new(ppm),
        ppm,
        percent,
        normalized,
        string,
    }
}

#[rustfmt::skip]
const DATA: &[RatioTestData] = &[
    //       ppm,     percent,  normalized,        string
    d(         0,         0.0,         0.0,         "0.0"),
    d(    500000,        50.0,         0.5,         "0.5"),
    d(   1000000,       100.0,         1.0,         "1.0"),
    d( 123456789,  12345.6789,  123.456789,  "123.456789"),
    d(-987654321, -98765.4321, -987.654321, "-987.654321"),
];

fn assert_near(expected: f64, actual: f64, tolerance: f64) {
    assert!(
        (expected - actual).abs() <= tolerance,
        "{expected} != {actual}"
    );
}

#[test]
fn test_default_constructor() {
    assert_eq!(Ratio::default().to_ppm(), 0);
}

#[test]
fn test_ppm_constructor() {
    for data in DATA {
        assert_eq!(Ratio::new(data.ppm).to_ppm(), data.ppm);
        assert_eq!(data.ratio.to_ppm(), data.ppm);
    }
}

#[test]
fn test_from_percent_int() {
    assert_eq!(Ratio::from_percent(42).to_ppm(), 420000);
    assert_eq!(Ratio::from_percent(0).to_ppm(), 0);
    assert_eq!(Ratio::from_percent(50).to_ppm(), 500000);
    assert_eq!(Ratio::from_percent(100).to_ppm(), 1000000);
}

#[test]
fn test_from_percent_float() {
    for data in DATA {
        assert_near(
            f64::from(data.ppm),
            f64::from(Ratio::from_percent_f64(data.percent).to_ppm()),
            2.0,
        );
    }
    assert_near(0.0, Ratio::from_percent_f64(0.0).to_percent(), 0.0002);
    assert_near(50.0, Ratio::from_percent_f64(50.0).to_percent(), 0.0002);
    assert_near(100.0, Ratio::from_percent_f64(100.0).to_percent(), 0.0002);
    assert_near(42.42, Ratio::from_percent_f64(42.42).to_percent(), 0.0002);
}

#[test]
fn test_from_normalized_float() {
    for data in DATA {
        assert_near(
            f64::from(data.ppm),
            f64::from(Ratio::from_normalized(data.normalized).to_ppm()),
            2.0,
        );
    }
}

#[test]
fn test_from_normalized_string() {
    for data in DATA {
        assert_eq!(
            Ratio::from_normalized_str(data.string).unwrap().to_ppm(),
            data.ppm
        );
    }
}

#[test]
fn test_conversions() {
    for data in DATA {
        assert_near(data.percent, data.ratio.to_percent(), 0.0002);
        assert_near(data.normalized, data.ratio.to_normalized(), 0.000002);
        assert_eq!(data.ratio.to_normalized_string(), data.string);
    }
}

#[test]
fn test_operator_equal() {
    assert!(Ratio::default() == Ratio::default());
    assert!(Ratio::default() == Ratio::new(0));
    assert!(Ratio::new(1234) == Ratio::new(1234));
    assert!(Ratio::new(-987654321) == Ratio::new(-987654321));
    assert!(Ratio::new(0) != Ratio::new(1));
    assert!(Ratio::new(5) != Ratio::new(-6));
    assert!(Ratio::new(-987654321) != Ratio::new(-987654322));
}

#[test]
fn test_serialize() {
    for data in DATA {
        let bytes = data
            .ratio
            .to_sexpression()
            .to_byte_array(Mode::LibrePcb)
            .unwrap();
        assert_eq!(bytes, format!("{}\n", data.string).as_bytes());
    }
}

#[test]
fn test_deserialize() {
    for data in DATA {
        let sexpr = SExpression::string(data.string);
        assert_eq!(Ratio::from_sexpression(&sexpr), Ok(data.ratio));
    }
    assert!(Ratio::from_sexpression(&SExpression::string("")).is_err());
    assert!(Ratio::from_sexpression(&SExpression::string("foo")).is_err());
}
