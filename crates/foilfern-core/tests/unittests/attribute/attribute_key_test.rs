//! Port of tests/unittests/core/attribute/attributekeytest.cpp.

use foilfern_core::attribute::AttributeKey;
use foilfern_core::serialization::{FromSExpression, Mode, SExpression, ToSExpression};

#[rustfmt::skip]
const DATA: &[(&str, bool)] = &[
    // valid keys
    ("1", true),
    ("A", true),
    ("_", true),
    ("_A_2_C_", true),
    ("0123456789012345678901234567890123456789", true),

    // invalid keys
    ("", false), // empty
    ("01234567890123456789012345678901234567890", false), // too long
    (" ", false), // space
    ("A B", false), // space
    ("z", false), // lowercase character
    (";", false), // invalid character
    (":1234", false), // invalid character at start
    ("AS:DF", false), // invalid character in the middle
    ("1234:", false), // invalid character at end
    ("\n", false), // invalid character
    ("FOO\tBAR", false), // invalid character in the middle
    ("FOO\nBAR", false), // invalid character in the middle
    ("\nFOO", false), // invalid character at start
    ("FOO\n", false), // invalid character at end
];

#[test]
fn test_constructor() {
    for &(input, valid) in DATA {
        let result = AttributeKey::new(input);
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
        let cleaned = AttributeKey::clean(input);
        if valid {
            assert_eq!(cleaned, input);
        } else if !cleaned.is_empty() {
            assert!(AttributeKey::new(cleaned).is_ok(), "{input:?}");
        }
    }
}

#[test]
fn test_serialize() {
    for &(input, _) in DATA.iter().filter(|d| d.1) {
        let key = AttributeKey::new(input).unwrap();
        let bytes = key.to_sexpression().to_byte_array(Mode::LibrePcb).unwrap();
        assert_eq!(bytes, format!("\"{input}\"\n").as_bytes());
    }
}

#[test]
fn test_deserialize() {
    for &(input, valid) in DATA {
        let result = AttributeKey::from_sexpression(&SExpression::string(input));
        if valid {
            assert_eq!(result.unwrap(), AttributeKey::new(input).unwrap());
        } else {
            assert!(result.is_err(), "{input:?}");
        }
    }
}
