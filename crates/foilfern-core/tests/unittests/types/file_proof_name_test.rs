//! Port of tests/unittests/core/types/fileproofnametest.cpp.

use foilfern_core::serialization::{FromSExpression, Mode, SExpression, ToSExpression};
use foilfern_core::types::FileProofName;

/// (input, cleaned, valid)
#[rustfmt::skip]
const DATA: &[(&str, &str, bool)] = &[
    // Valid strings.
    ("1", "1", true),
    (".1", ".1", true),
    ("..1", "..1", true),
    (".1.", ".1.", true),
    ("foo-bar_+().", "foo-bar_+().", true),

    // Invalid strings.
    ("", "", false), // too short
    (".", "", false), // only dots (invalid filename!)
    ("..", "", false), // only dots (invalid filename!)
    ("...", "", false), // only dots (invalid filename!)
    ("../", "", false), // invalid characters, only dots
    ("123456789012345678901", "12345678901234567890", false), // too long
    (" ", "", false), // space
    ("äöü", "aou", false), // invalid characters
    (" ABC", "ABC", false), // leading space
    ("ABC ", "ABC", false), // trailing space
    ("AB CD", "AB-CD", false), // invalid character
    ("AB\nCD", "ABCD", false), // invalid character
    ("AB/CD", "ABCD", false), // invalid character
    ("AB:CD", "ABCD", false), // invalid character
];

#[test]
fn test_constructor() {
    for &(input, _, valid) in DATA {
        let result = FileProofName::new(input);
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
        assert_eq!(FileProofName::clean(input), cleaned, "{input:?}");
    }
}

#[test]
fn test_serialize() {
    for &(input, _, _) in DATA.iter().filter(|d| d.2) {
        let bytes = FileProofName::new(input)
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
        let result = FileProofName::from_sexpression(&SExpression::string(input));
        if valid {
            assert_eq!(result, Ok(FileProofName::new(input).unwrap()));
        } else {
            assert!(result.is_err());
        }
    }
}
