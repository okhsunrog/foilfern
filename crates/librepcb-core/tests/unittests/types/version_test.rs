//! Port of tests/unittests/core/types/versiontest.cpp.

// The comparison tests negate operators on purpose (`!(a < b)`), as upstream.
#![allow(clippy::nonminimal_bool)]

use librepcb_core::serialization::{FromSExpression, Mode, SExpression, ToSExpression};
use librepcb_core::types::Version;

fn v(s: &str) -> Version {
    s.parse().unwrap()
}

#[test]
fn test_is_valid() {
    // valid
    assert!(Version::is_valid("0"));
    assert!(Version::is_valid("05.00000040"));
    assert!(Version::is_valid(
        "00000.00001.00002.00003.00007.00000.00600.00000.08000.20000"
    ));

    // invalid
    assert!(!Version::is_valid(""));
    assert!(!Version::is_valid("-1"));
    assert!(!Version::is_valid("1-0"));
    assert!(!Version::is_valid("100000.55"));
    assert!(!Version::is_valid("77.-11.9"));
    assert!(!Version::is_valid("4.8."));
    assert!(!Version::is_valid(".4.8"));
    assert!(!Version::is_valid(
        "00000.00001.00002.00003.00007.00000.00600.00000.08000.20000.00030"
    ));
    assert!(!Version::is_valid(
        "00000.00001.00002.00003.500007.00000.00600.00000.08000.20000"
    ));
}

#[test]
fn test_from_string_fail() {
    assert!("".parse::<Version>().is_err());
    assert!(".1.2".parse::<Version>().is_err());
    assert!("1.".parse::<Version>().is_err());
    assert!("1.-2.3".parse::<Version>().is_err());
}

#[test]
fn test_from_string_valid() {
    let mut s = String::new();
    let mut numbers = Vec::new();
    for i in 0..10 {
        numbers.push(i * 10);
        s.push_str(&(i * 10).to_string());
        let version = v(&s);
        assert_eq!(version.numbers(), numbers.as_slice());
        assert_eq!(version.to_string(), s);
        s.push('.');
    }
    assert_eq!(v("1.2").to_string(), "1.2");
}

#[test]
fn test_is_prefix_of() {
    assert!(v("0").is_prefix_of(&v("0")));
    assert!(v("0.1").is_prefix_of(&v("0.1.0")));
    assert!(v("1.2").is_prefix_of(&v("1.2.0.0.0.1")));
    assert!(v("5.5.5.4").is_prefix_of(&v("5.5.5.4.1")));

    assert!(!v("1.2").is_prefix_of(&v("1")));
    assert!(!v("0.1").is_prefix_of(&v("0.2")));
    assert!(!v("5.5").is_prefix_of(&v("5.4.5")));
}

#[test]
fn test_get_numbers() {
    assert_eq!(v("0").numbers(), [0]);
    assert_eq!(v("5.4.3").numbers(), [5, 4, 3]);
    assert_eq!(v("005.440.00.080.000").numbers(), [5, 440, 0, 80]);
}

#[test]
fn test_to_str() {
    assert_eq!(v("0").to_string(), "0");
    assert_eq!(v("5.4.3").to_string(), "5.4.3");
    assert_eq!(v("0.00.6.003.20.0.0").to_string(), "0.0.6.3.20");
    assert_eq!(v("005.440.00.080.000").to_string(), "5.440.0.80");
    assert_eq!(
        v("00000.00001.00002.00003.00007.00000.00600.00000.08000.00000").to_string(),
        "0.1.2.3.7.0.600.0.8000"
    );
}

#[test]
fn test_to_pretty_str() {
    assert_eq!(v("0").to_pretty_str(0, 4), "0");
    assert_eq!(v("5").to_pretty_str(2, 3), "5.0");
    assert_eq!(v("5.04.3.6.7").to_pretty_str(2, 3), "5.4.3");
    assert_eq!(v("0").to_pretty_str(4, 4), "0.0.0.0");
}

#[test]
fn test_to_comparable_str() {
    assert_eq!(
        v("0").to_comparable_str(),
        "00000.00000.00000.00000.00000.00000.00000.00000.00000.00000"
    );
    assert_eq!(
        v("1").to_comparable_str(),
        "00001.00000.00000.00000.00000.00000.00000.00000.00000.00000"
    );
    assert_eq!(
        v("0.0.3.0.600.0").to_comparable_str(),
        "00000.00000.00003.00000.00600.00000.00000.00000.00000.00000"
    );
}

#[test]
fn test_operator_greater() {
    assert!(v("0.1") > v("0.0.9"));
    assert!(v("5.4") > v("0.500.0"));
    assert!(v("10.0.0.1") > v("10"));

    assert!(!(v("10") > v("10.0.1")));
    assert!(!(v("0.0.1") > v("0.1.0")));
}

#[test]
fn test_operator_less() {
    assert!(v("0.0.9") < v("0.1"));
    assert!(v("0.500.0") < v("5.4"));
    assert!(v("10") < v("10.0.0.1"));

    assert!(!(v("10.0.1") < v("10")));
    assert!(!(v("0.1.0") < v("0.0.1")));
}

#[test]
fn test_operator_greater_equal() {
    assert!(v("0.1") >= v("0.0.9"));
    assert!(v("5.4") >= v("0.500.0"));
    assert!(v("10.0.0.1") >= v("10"));
    assert!(v("10.0.0.1") >= v("10.0.0.1"));
    assert!(v("5.0.0.5") >= v("5.0.0.5.0"));

    assert!(!(v("10") >= v("10.0.1")));
    assert!(!(v("0.0.1") >= v("0.1.0")));
}

#[test]
fn test_operator_less_equal() {
    assert!(v("0.0.9") <= v("0.1"));
    assert!(v("0.500.0") <= v("5.4"));
    assert!(v("10") <= v("10.0.0.1"));
    assert!(v("10.0.0.1") <= v("10.0.0.1"));
    assert!(v("5.0.0.5") <= v("5.0.0.5.0"));

    assert!(!(v("10.0.1") <= v("10")));
    assert!(!(v("0.1.0") <= v("0.0.1")));
}

#[test]
fn test_operator_equal() {
    assert!(v("10.0.0.1") == v("10.0.0.1"));
    assert!(v("5.0.0.5") == v("5.0.0.5.0"));

    assert!(!(v("10.0.1") == v("10")));
    assert!(!(v("0.1.0") == v("0.0.1")));
}

#[test]
fn test_operator_not_equal() {
    assert!(v("10.0.0.1") != v("10.0.1"));
    assert!(v("5.0.5") != v("0.5.0.5"));

    assert!(!(v("10.0.1") != v("10.0.1")));
    assert!(!(v("0.1.0") != v("0.001.0.0.0")));
}

#[test]
fn test_serialize() {
    let bytes = v("0.1.0")
        .to_sexpression()
        .to_byte_array(Mode::LibrePcb)
        .unwrap();
    assert_eq!(bytes, b"\"0.1\"\n");
}

#[test]
fn test_deserialize() {
    let sexpr = SExpression::string("0.1");
    assert_eq!(Version::from_sexpression(&sexpr), Ok(v("0.1")));
    assert!(Version::from_sexpression(&SExpression::string("")).is_err());
    assert!(Version::from_sexpression(&SExpression::string("foo")).is_err());
}
