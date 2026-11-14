//! Port of tests/unittests/core/library/cmp/componentprefixtest.cpp.

use foilfern_core::library::cmp::ComponentPrefix;

#[rustfmt::skip]
const DATA: &[(&str, bool)] = &[
    // valid keys
    ("", true),
    ("A", true),
    ("z", true),
    ("_", true),
    ("_a_B_C_", true),
    ("abcdefghijklmnop", true),

    // invalid keys
    ("abcdefghijklmnopq", false), // too long
    (" ", false), // space
    ("A1", false), // digit
    ("A B", false), // space
    (";", false), // invalid character
    (":abcd", false), // invalid character at start
    ("AS:df", false), // invalid character in the middle
    ("abcd:", false), // invalid character at end
    ("\n", false), // invalid character
    ("Foo\tBar", false), // invalid character in the middle
    ("Foo\nBar", false), // invalid character in the middle
    ("\nFoo", false), // invalid character at start
    ("Foo\n", false), // invalid character at end
];

#[test]
fn test_constructor() {
    for &(input, valid) in DATA {
        let result = ComponentPrefix::new(input);
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
        let cleaned = ComponentPrefix::clean(input);
        if valid {
            assert_eq!(cleaned, input);
        } else {
            ComponentPrefix::new(cleaned).unwrap(); // must not fail
        }
    }
}
