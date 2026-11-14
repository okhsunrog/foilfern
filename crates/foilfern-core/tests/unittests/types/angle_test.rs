//! Port of tests/unittests/core/types/angletest.cpp.

use foilfern_core::serialization::{FromSExpression, Mode, SExpression, ToSExpression};
use foilfern_core::types::Angle;

fn a(udeg: i32) -> Angle {
    Angle::new(udeg)
}

#[test]
fn test_inverted() {
    assert_eq!(a(0).inverted(), a(0));
    assert_eq!(a(10000000).inverted(), a(-350000000));
    assert_eq!(a(-350000000).inverted(), a(10000000));
    assert_eq!(a(180000000).inverted(), a(-180000000));
}

#[test]
fn test_rounded() {
    assert_eq!(a(54).rounded(a(-1)), a(54)); // Invalid -> ignored
    assert_eq!(a(54).rounded(a(0)), a(54)); // Invalid -> ignored
    assert_eq!(a(54).rounded(a(1)), a(54)); // already OK
    assert_eq!(a(1000000).rounded(a(10)), a(1000000)); // already OK
    assert_eq!(a(54).rounded(a(10)), a(50)); // rounded down
    assert_eq!(a(55).rounded(a(10)), a(60)); // rounded up
    assert_eq!(a(56).rounded(a(10)), a(60)); // rounded up
    assert_eq!(a(-54).rounded(a(10)), a(-50)); // rounded down
    assert_eq!(a(-55).rounded(a(10)), a(-60)); // rounded up
    assert_eq!(a(-56).rounded(a(10)), a(-60)); // rounded up
    assert_eq!(a(359999990).rounded(a(100)), a(0)); // overflow
    assert_eq!(a(-359999990).rounded(a(100)), a(0)); // underflow
}

struct AngleTestData {
    valid: bool,
    orig_str: &'static str,
    value: Angle,
    gen_str: &'static str,
}

const fn d(valid: bool, orig_str: &'static str, udeg: i32, gen_str: &'static str) -> AngleTestData {
    AngleTestData {
        valid,
        orig_str,
        value: Angle::new(udeg),
        gen_str,
    }
}

#[rustfmt::skip]
const DATA: &[AngleTestData] = &[
    // from/to deg
    d(true,  "0",          0,         "0.0"       ),
    d(true,  "90",         90000000,  "90.0"      ),
    d(true,  "-90",        -90000000, "-90.0"     ),
    d(true,  "90.000001",  90000001,  "90.000001" ),
    d(true,  "-90.000001", -90000001, "-90.000001"),
    d(true,  "1e3",        280000000, "280.0"     ),
    d(true,  "0.1",        100000,    "0.1"       ),

    // invalid cases
    d(false, "",           0,         ""          ),
    d(false, ".",          0,         ""          ),
    d(false, "0e",         0,         ""          ),
    d(false, "0e+",        0,         ""          ),
    d(false, "0e-",        0,         ""          ),
    d(false, "0.0000001",  0,         ""          ),
    d(false, "1e-7",       0,         ""          ),
    d(false, "1e1000",     0,         ""          ),
];

#[test]
fn test_from_deg() {
    for data in DATA {
        let result = Angle::from_deg_str(data.orig_str);
        if data.valid {
            assert_eq!(result, Ok(data.value), "{}", data.orig_str);
        } else {
            assert!(result.is_err(), "{}", data.orig_str);
        }
    }
}

#[test]
fn test_to_deg_string() {
    for data in DATA.iter().filter(|d| d.valid) {
        assert_eq!(data.value.to_deg_string(), data.gen_str);
    }
}

#[test]
fn test_serialize() {
    for data in DATA.iter().filter(|d| d.valid) {
        let bytes = data
            .value
            .to_sexpression()
            .to_byte_array(Mode::LibrePcb)
            .unwrap();
        assert_eq!(bytes, format!("{}\n", data.gen_str).as_bytes());
    }
}

#[test]
fn test_deserialize() {
    for data in DATA {
        let sexpr = SExpression::string(data.orig_str);
        let result = Angle::from_sexpression(&sexpr);
        if data.valid {
            assert_eq!(result, Ok(data.value));
        } else {
            assert!(result.is_err());
        }
    }
}

/// (degrees, radians, microdegrees, overflow)
#[rustfmt::skip]
#[allow(clippy::approx_constant)] // Upstream test vectors.
const FLOAT_DATA: &[(f64, f64, i32, bool)] = &[
    (0.0, 0.0, 0, false),
    (-0.0, 0.0, 0, false),
    (180.123456, 3.143747367, 180123456, false),
    (-180.123456, -3.143747367, -180123456, false),
    (359.999999, 6.28318529, 359999999, false),
    (-359.999999, -6.28318529, -359999999, false),
    (360.0, 6.2831853072, 0, true), // overflow
    (-360.0, -6.2831853072, 0, true), // underflow
    (360.1, 6.2849306364, 100000, true), // overflow
    (-360.1, -6.2849306364, -100000, true), // underflow
    (359.9999999, 6.2831853054, 0, true), // round -> overflow
    (-359.9999999, -6.2831853054, 0, true), // round -> overflow
    (360.0000006, 6.2831853177, 1, true), // round -> overflow
    (-360.0000006, -6.2831853177, -1, true), // round -> underflow
    (0.1000004, 0.0017453362, 100000, true), // round
    (-0.1000004, -0.0017453362, -100000, true), // round
    (0.1000006, 0.0017453397, 100001, true), // round
    (-0.1000006, -0.0017453397, -100001, true), // round
];

#[test]
fn test_from_deg_float() {
    for &(degrees, _, output, overflow) in FLOAT_DATA {
        let angle = Angle::from_deg(degrees).unwrap();
        assert_eq!(angle.to_micro_deg(), output, "{degrees}");
        if !overflow {
            assert!((degrees - angle.to_deg()).abs() <= 1e-6);
        }
    }
}

#[test]
fn test_from_rad() {
    for &(_, radians, output, overflow) in FLOAT_DATA {
        let angle = Angle::from_rad(radians).unwrap();
        assert_eq!(angle.to_micro_deg(), output, "{radians}");
        if !overflow {
            assert!((radians - angle.to_rad()).abs() <= 1e-7);
        }
    }
    assert!(Angle::from_rad(f64::NAN).is_err());
    assert!(Angle::from_rad(f64::INFINITY).is_err());
}

/// Ported from the upstream Rust unit tests (rust-core/src/types/angle.rs).
#[test]
fn test_mapping() {
    assert_eq!(a(-90_000_000).mapped_to_0_360deg(), a(270_000_000));
    assert_eq!(a(-180_000_000).mapped_to_0_360deg(), a(180_000_000));
    assert_eq!(a(180_000_000).mapped_to_0_360deg(), a(180_000_000));
    assert_eq!(a(-180_000_000).mapped_to_180deg(), a(-180_000_000));
    assert_eq!(a(-270_000_000).mapped_to_180deg(), a(90_000_000));
    assert_eq!(a(180_000_000).mapped_to_180deg(), a(-180_000_000));
    assert_eq!(a(270_000_000).mapped_to_180deg(), a(-90_000_000));
    assert_eq!(a(-359_999_999).abs(), a(359_999_999));
    assert_eq!(a(361_500_000).to_micro_deg(), 1_500_000);
    assert_eq!(a(-361_500_000).to_micro_deg(), -1_500_000);
    assert_eq!(a(270_000_000) + a(180_000_000), a(90_000_000));
}
