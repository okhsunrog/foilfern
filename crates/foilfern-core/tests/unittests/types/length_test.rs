//! Port of tests/unittests/core/types/lengthtest.cpp.

use foilfern_core::types::Length;

struct LengthTestData {
    valid: bool,
    orig_str: &'static str,
    value: Length,
    gen_str: &'static str,
}

const fn d(valid: bool, orig_str: &'static str, nm: i64, gen_str: &'static str) -> LengthTestData {
    LengthTestData {
        valid,
        orig_str,
        value: Length::new(nm),
        gen_str,
    }
}

#[rustfmt::skip]
const DATA: &[LengthTestData] = &[
    // from/to mm
    d(true,  "0",              0,           "0.0"         ),
    d(true,  "1",              1000000,     "1.0"         ),
    d(true,  "-1",             -1000000,    "-1.0"        ),
    d(true,  "0.000001",       1,           "0.000001"    ),
    d(true,  "-0.000001",      -1,          "-0.000001"   ),
    d(true,  "1e-6",           1,           "0.000001"    ),
    d(true,  "-1e-6",          -1,          "-0.000001"   ),
    d(true,  "1.000001",       1000001,     "1.000001"    ),
    d(true,  "-1.000001",      -1000001,    "-1.000001"   ),
    d(true,  "1e3",            1000000000,  "1000.0"      ),
    d(true,  "-1e3",           -1000000000, "-1000.0"     ),
    d(true,  ".1",             100000,      "0.1"         ),
    d(true,  "1.",             1000000,     "1.0"         ),
    d(true,  "2147483647e-6",  2147483647,  "2147.483647" ),
    d(true,  "-2147483648e-6", -2147483648, "-2147.483648"),
    d(true,  "9",              9000000,     "9.0"         ),
    d(true,  "9.9",            9900000,     "9.9"         ),
    d(true,  "0.9",            900000,      "0.9"         ),
    d(true,  "0.99",           990000,      "0.99"        ),
    d(true,  "0.09",           90000,       "0.09"        ),
    d(true,  "0.099",          99000,       "0.099"       ),
    d(true,  "0.009",          9000,        "0.009"       ),
    d(true,  "0.0099",         9900,        "0.0099"      ),
    d(true,  "0.0009",         900,         "0.0009"      ),
    d(true,  "0.00099",        990,         "0.00099"     ),
    d(true,  "0.00009",        90,          "0.00009"     ),
    d(true,  "0.000099",       99,          "0.000099"    ),
    d(true,  "0.000009",       9,           "0.000009"    ),

    // invalid cases
    d(false, "",               0,           ""            ),
    d(false, ".",              0,           ""            ),
    d(false, "0e",             0,           ""            ),
    d(false, "0e+",            0,           ""            ),
    d(false, "0e-",            0,           ""            ),
    d(false, "0.0000001",      0,           ""            ),
    d(false, "1e-7",           0,           ""            ),
    d(false, "1e1000",         0,           ""            ),
];

#[test]
fn test_from_mm() {
    for data in DATA {
        let result = Length::from_mm_str(data.orig_str);
        if data.valid {
            assert_eq!(result, Ok(data.value), "{}", data.orig_str);
        } else {
            assert!(result.is_err(), "{}", data.orig_str);
        }
    }
}

#[test]
fn test_to_mm_string() {
    for data in DATA.iter().filter(|d| d.valid) {
        assert_eq!(data.value.to_mm_string(), data.gen_str);
        assert_eq!(data.value.to_string(), data.gen_str);
    }
}

#[test]
fn test_mapped_to_grid() {
    #[rustfmt::skip]
    let data: &[(i64, i64, i64)] = &[
        (0,   10, 0  ),
        (10,  0,  10 ),
        (-10, 0,  -10),
        (10,  1,  10 ),
        (-10, 1,  -10),
        (8,   10, 10 ),
        (2,   10, 0  ),
        (-8,  10, -10),
        (-2,  10, 0  ),
        (18,  10, 20 ),
        (12,  10, 10 ),
        (-18, 10, -20),
        (-12, 10, -10),
        (10,  10, 10 ),
        (-10, 10, -10),
        (20,  10, 20 ),
        (-20, 10, -20),
    ];
    for &(value, grid, mapped) in data {
        assert_eq!(
            Length::new(value).mapped_to_grid(Length::new(grid)),
            Length::new(mapped),
            "{value} {grid}"
        );
    }
}

#[test]
fn test_from_mm_float() {
    let data: &[(f64, i64)] = &[(0.0, 0), (-22.3079845, -22307985), (16.6419475, 16641948)];
    for &(input, output) in data {
        assert_eq!(Length::from_mm(input).unwrap().to_nm(), output);
    }
}

/// Ported from the upstream Rust unit tests (rust-core/src/types/length.rs),
/// which define the rounding semantics used through the C++ FFI.
#[test]
fn test_rounded_down_up() {
    #[rustfmt::skip]
    let data: &[(i64, i64, i64, i64)] = &[
        (  0, 10,   0,   0),
        ( 10,  1,  10,  10),
        (-10,  1, -10, -10),
        (  8, 10,   0,  10),
        (  2, 10,   0,  10),
        ( -8, 10, -10,   0),
        ( -2, 10, -10,   0),
        ( 18, 10,  10,  20),
        ( 12, 10,  10,  20),
        (-18, 10, -20, -10),
        (-12, 10, -20, -10),
        ( 10, 10,  10,  10),
        (-10, 10, -10, -10),
        ( 20, 10,  20,  20),
        (-20, 10, -20, -20),
        ( 15, 10,  10,  20),
        (-15, 10, -20, -10),
    ];
    for &(input, multiple, lower, upper) in data {
        let l = Length::new(input);
        assert_eq!(l.rounded_down_to(Length::new(multiple)), Length::new(lower));
        assert_eq!(l.rounded_up_to(Length::new(multiple)), Length::new(upper));
    }
    assert_eq!(
        Length::new(15).mapped_to_grid(Length::new(10)),
        Length::new(20)
    );
    assert_eq!(
        Length::new(-15).mapped_to_grid(Length::new(10)),
        Length::new(-20)
    );
}

#[test]
fn test_from_nm_float() {
    assert!(Length::from_nm_f64(f64::NAN).is_err());
    assert!(Length::from_nm_f64(f64::INFINITY).is_err());
    assert!(Length::from_nm_f64(f64::NEG_INFINITY).is_err());
    assert_eq!(Length::from_nm_f64(100.3).unwrap().to_nm(), 100);
    assert_eq!(Length::from_nm_f64(100.7).unwrap().to_nm(), 101);
    assert_eq!(Length::from_nm_f64(-100.3).unwrap().to_nm(), -100);
    assert_eq!(Length::from_nm_f64(-100.7).unwrap().to_nm(), -101);
    assert_eq!(Length::from_inch(-42.5).unwrap().to_nm(), -1079500000);
    assert_eq!(Length::from_mil(-42.5).unwrap().to_nm(), -1079500);
    assert_eq!(Length::from_px(-4.5).unwrap().to_nm(), -1587500);
    assert_eq!(Length::new(-1587500).to_px(), -4.5);
}
