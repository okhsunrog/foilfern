//! Port of tests/unittests/core/types/circuitidentifiertest.cpp.

use foilfern_core::serialization::{FromSExpression, SExpression, ToSExpression};
use foilfern_core::types::CircuitIdentifier;

#[rustfmt::skip]
const DATA: &[(&str, bool)] = &[
    // valid identifiers
    ("1", true),
    ("A", true),
    ("z", true),
    ("_", true),
    ("+", true),
    ("-", true),
    ("01234567890123456789012345678901", true),
    ("._+-/!?&@#$asDF1234()", true),

    // invalid identifiers
    ("", false), // empty
    ("012345678901234567890123456789012", false), // too long
    (" ", false), // space
    ("A B", false), // space
    (";", false), // invalid character
    ("Bus[]", false), // invalid character at end
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
        let result = CircuitIdentifier::new(input);
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
        let cleaned = CircuitIdentifier::clean(input);
        if valid {
            assert_eq!(cleaned, input);
        } else if !cleaned.is_empty() {
            assert!(CircuitIdentifier::new(cleaned).is_ok(), "{input:?}");
        }
    }
}

#[test]
fn test_serialize() {
    for &(input, _) in DATA.iter().filter(|d| d.1) {
        let identifier = CircuitIdentifier::new(input).unwrap();
        assert_eq!(identifier.to_sexpression().value().unwrap(), input);
        assert_eq!(Some(identifier).to_sexpression().value().unwrap(), input);
    }
}

#[test]
fn test_deserialize() {
    for &(input, valid) in DATA {
        let sexpr = SExpression::token(input);
        if valid {
            assert_eq!(CircuitIdentifier::from_sexpression(&sexpr).unwrap(), input);
            assert_eq!(
                Option::<CircuitIdentifier>::from_sexpression(&sexpr)
                    .unwrap()
                    .unwrap(),
                input
            );
        } else {
            assert!(CircuitIdentifier::from_sexpression(&sexpr).is_err());
            if input.is_empty() {
                assert_eq!(
                    Option::<CircuitIdentifier>::from_sexpression(&sexpr),
                    Ok(None)
                );
            } else {
                assert!(Option::<CircuitIdentifier>::from_sexpression(&sexpr).is_err());
            }
        }
    }
}

#[test]
fn test_serialize_optional() {
    assert_eq!(
        None::<CircuitIdentifier>.to_sexpression().value().unwrap(),
        ""
    );
}

#[test]
fn test_deserialize_optional() {
    let sexpr = SExpression::token("");
    assert_eq!(
        Option::<CircuitIdentifier>::from_sexpression(&sexpr),
        Ok(None)
    );
}
