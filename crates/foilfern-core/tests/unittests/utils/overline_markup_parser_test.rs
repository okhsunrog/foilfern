//! Port of tests/unittests/core/utils/overlinemarkupparsertest.cpp.
//!
//! Spans are `(start, length)` pairs like upstream (the test data is ASCII,
//! so byte and UTF-16 offsets are the same).

use foilfern_core::utils::overline_markup_parser::extract_overlines;

/// `(start, length)` pairs.
type Spans = &'static [(usize, usize)];

#[rustfmt::skip]
const DATA: &[(&str, &str, Spans)] = &[
    // Without modification at all.
    ("", "", &[]),
    ("!", "!", &[]),
    ("!!", "!!", &[]),
    ("!!!", "!!!", &[]),
    ("A", "A", &[]),
    ("A/B/C", "A/B/C", &[]),
    ("AB_CD!", "AB_CD!", &[]),
    // With substitutions, but without overlines.
    ("!!A", "!A", &[]),
    ("!!!!A", "!!A", &[]),
    ("A!!B", "A!B", &[]),
    ("A!!!!B", "A!!B", &[]),
    ("!!/!!A", "!/!A", &[]),
    ("!!!!/A", "!!/A", &[]),
    // Only with overlines.
    ("!ABCD", "ABCD", &[(0, 4)]),
    ("AB!CD", "ABCD", &[(2, 2)]),
    ("AB!CD!EF", "ABCDEF", &[(2, 2)]),
    ("!AB/CD", "AB/CD", &[(0, 2)]),
    ("!AB!/CD", "AB/CD", &[(0, 5)]),
    ("AB!/CD", "AB/CD", &[(2, 3)]),
    ("!AB/!CD", "AB/CD", &[(0, 2), (3, 2)]),
    ("!AB/CD/!EF", "AB/CD/EF", &[(0, 2), (6, 2)]),
    ("AB/!CD/EF", "AB/CD/EF", &[(3, 2)]),
    ("!AB!/CD/EF", "AB/CD/EF", &[(0, 5)]),
    // Overlines mixed with substitutions.
    ("!!!AB!!CD", "!AB!CD", &[(1, 5)]),
    ("AB!!!!!CD", "AB!!CD", &[(4, 2)]),
    ("AB!CD!!EF!", "ABCD!EF!", &[(2, 6)]),
    ("!AB!CD!!EF!", "ABCD!EF!", &[(0, 2)]),
    ("!AB/CD!!EF!", "AB/CD!EF!", &[(0, 2)]),
    ("!AB!!/CD", "AB!/CD", &[(0, 3)]),
    ("!AB!!!/CD", "AB!/CD", &[(0, 6)]),
];

#[test]
fn test() {
    for &(input, output, spans) in DATA {
        let (actual_output, actual_spans) = extract_overlines(input);
        assert_eq!(actual_output, output, "{input:?}");
        let actual_spans: Vec<(usize, usize)> =
            actual_spans.iter().map(|s| (s.start, s.len())).collect();
        assert_eq!(actual_spans, spans, "{input:?}");
    }
}
