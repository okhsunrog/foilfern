//! Port of
//! tests/unittests/core/library/cmp/componentsymbolvariantitemsuffixtest.cpp.

use foilfern_core::library::cmp::ComponentSymbolVariantItemSuffix;

#[rustfmt::skip]
const DATA: &[(&str, bool)] = &[
    // valid suffixes
    ("", true),
    ("1", true),
    ("A", true),
    ("z", true),
    ("_", true),
    ("_a_B_C_", true),
    ("0123456789012345", true),

    // invalid suffixes
    ("01234567890123456", false), // too long
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
        let result = ComponentSymbolVariantItemSuffix::new(input);
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
        let cleaned = ComponentSymbolVariantItemSuffix::clean(input);
        if valid {
            assert_eq!(cleaned, input);
        } else {
            ComponentSymbolVariantItemSuffix::new(cleaned).unwrap(); // must not fail
        }
    }
}
