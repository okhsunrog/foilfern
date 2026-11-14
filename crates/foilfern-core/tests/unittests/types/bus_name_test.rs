//! Port of tests/unittests/core/types/busnametest.cpp.

use foilfern_core::serialization::{FromSExpression, SExpression, ToSExpression};
use foilfern_core::types::BusName;

#[rustfmt::skip]
const DATA: &[(&str, bool)] = &[
    // valid identifiers
    ("1", true),
    ("A", true),
    ("z", true),
    ("_", true),
    ("+", true),
    ("-", true),
    ("Bus[]", true),
    ("DATA[0..7]", true),
    ("01234567890123456789012345678901", true),
    ("._+-/!?&@#$asDF1234()", true),

    // invalid identifiers
    ("", false), // empty
    ("012345678901234567890123456789012", false), // too long
    (" ", false), // space
    ("A B", false), // space
    (";", false), // invalid character
    (":1234", false), // invalid character at start
    ("AS:df", false), // invalid character in the middle
    ("1234:", false), // invalid character at end
    ("\n", false), // invalid character
    ("Foo\tBar", false), // invalid character in the middle
    ("Foo\nBar", false), // invalid character in the middle
    ("\nFoo", false), // invalid character at start
    ("Foo\n", false), // invalid character at end
];

#[test]
fn test_constructor() {
    for &(input, valid) in DATA {
        let result = BusName::new(input);
        if valid {
            assert_eq!(result.unwrap().as_str(), input);
        } else {
            assert!(result.is_err(), "{input:?}");
        }
    }
}

#[test]
fn test_clean() {
    for &(input, valid) in DATA {
        let cleaned = BusName::clean(input);
        if valid {
            assert_eq!(cleaned, input);
        } else if !cleaned.is_empty() {
            assert!(BusName::new(cleaned).is_ok(), "{input:?}");
        }
    }
}

#[test]
fn test_serialize() {
    for &(input, _) in DATA.iter().filter(|d| d.1) {
        let name = BusName::new(input).unwrap();
        assert_eq!(name.to_sexpression().value().unwrap(), input);
    }
}

#[test]
fn test_deserialize() {
    for &(input, valid) in DATA {
        let result = BusName::from_sexpression(&SExpression::token(input));
        if valid {
            assert_eq!(result.unwrap(), input);
        } else {
            assert!(result.is_err());
        }
    }
}
