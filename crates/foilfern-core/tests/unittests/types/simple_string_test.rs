//! Port of tests/unittests/core/types/simplestringtest.cpp.

use foilfern_core::serialization::{FromSExpression, Mode, SExpression, ToSExpression};
use foilfern_core::types::SimpleString;

/// (input, cleaned, valid)
#[rustfmt::skip]
const DATA: &[(&str, &str, bool)] = &[
    // Valid strings.
    ("", "", true),
    ("1", "1", true),
    ("foo:_-+*ç%&/()=", "foo:_-+*ç%&/()=", true),
    ("_", "_", true),
    ("_A B  C", "_A B C", true),

    // Invalid strings.
    (" ABC", "ABC", false), // leading space
    ("ABC ", "ABC", false), // trailing space
    ("AB\n\nCD", "AB CD", false), // invalid character
    ("AB\r\rCD", "AB CD", false), // invalid character
    ("AB\t\tCD", "AB CD", false), // invalid character
];

#[test]
fn test_constructor() {
    for &(input, _, valid) in DATA {
        let result = SimpleString::new(input);
        if valid {
            assert_eq!(result.unwrap().as_str(), input);
        } else {
            assert!(result.is_err(), "{input:?}");
        }
    }
}

#[test]
fn test_clean() {
    for &(input, cleaned, _) in DATA {
        assert_eq!(SimpleString::clean(input).as_str(), cleaned, "{input:?}");
    }
}

#[test]
fn test_serialize() {
    for &(input, _, _) in DATA.iter().filter(|d| d.2) {
        let bytes = SimpleString::new(input)
            .unwrap()
            .to_sexpression()
            .to_byte_array(Mode::LibrePcb)
            .unwrap();
        assert_eq!(bytes, format!("\"{input}\"\n").as_bytes());
    }
}

#[test]
fn test_deserialize() {
    for &(input, _, valid) in DATA {
        let result = SimpleString::from_sexpression(&SExpression::string(input));
        if valid {
            assert_eq!(result, Ok(SimpleString::new(input).unwrap()));
        } else {
            assert!(result.is_err());
        }
    }
}
