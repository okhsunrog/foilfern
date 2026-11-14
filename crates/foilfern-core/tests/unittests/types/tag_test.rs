//! Port of tests/unittests/core/types/tagtest.cpp.

use foilfern_core::serialization::{FromSExpression, SExpression, ToSExpression};
use foilfern_core::types::Tag;

#[rustfmt::skip]
const DATA: &[(&str, bool)] = &[
    // valid tags
    ("1", true),
    ("z", true),
    ("-foo-bar-12.34-", true),
    ("ipc-density-level-a", true),
    ("01234567890123456789012345678901", true),

    // invalid tags
    ("", false), // empty
    ("012345678901234567890123456789012", false), // too long
    ("Z", false), // uppercase letter
    (" ", false), // space
    ("a b", false), // space
    ("~", false), // invalid character
    (":1234", false), // invalid character at start
    ("as:df", false), // invalid character in the middle
    ("1234:", false), // invalid character at end
    ("\n", false), // invalid character
    ("\nfoo", false), // invalid character at start
    ("foo\tbar", false), // invalid character in the middle
    ("foo\nbar", false), // invalid character in the middle
    ("foo bar", false), // invalid character in the middle
    ("foo\n", false), // invalid character at end
];

#[test]
fn test_constructor() {
    for &(input, valid) in DATA {
        let result = Tag::new(input);
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
        let cleaned = Tag::clean(input);
        if valid {
            assert_eq!(cleaned, input);
        } else if !cleaned.is_empty() {
            assert!(Tag::new(cleaned).is_ok(), "{input:?}");
        }
    }
}

#[test]
fn test_parse() {
    for &(input, valid) in DATA {
        let parsed = input.parse::<Tag>().ok();
        assert_eq!(parsed.is_some(), valid, "{input:?}");
        if let Some(tag) = parsed {
            assert_eq!(tag, input);
        }
    }
}

#[test]
fn test_serialize() {
    for &(input, _) in DATA.iter().filter(|d| d.1) {
        let tag = Tag::new(input).unwrap();
        assert_eq!(tag.to_sexpression().value().unwrap(), input);
    }
}

#[test]
fn test_deserialize() {
    for &(input, valid) in DATA {
        let result = Tag::from_sexpression(&SExpression::token(input));
        if valid {
            assert_eq!(result.unwrap(), input);
        } else {
            assert!(result.is_err());
        }
    }
}
