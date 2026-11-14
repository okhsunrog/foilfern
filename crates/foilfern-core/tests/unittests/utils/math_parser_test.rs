//! Port of tests/unittests/core/utils/mathparsertest.cpp.
//!
//! The locales are given by their decimal point and group separator.

use foilfern_core::utils::math_parser::MathParser;

/// Decimal point and group separator.
type Locale = (char, Option<char>);

const EN_US: Locale = ('.', Some(','));
const DE_DE: Locale = (',', Some('.'));

#[rustfmt::skip]
const DATA: &[(Locale, &str, Option<f64>)] = &[
    // valid cases
    (EN_US, "0", Some(0.0)),
    (EN_US, "0.1234", Some(0.1234)),
    (EN_US, "+0.1234", Some(0.1234)),
    (EN_US, "-0.1234", Some(-0.1234)),
    (EN_US, "2+3", Some(5.0)),
    (EN_US, "(1+2)/2", Some(1.5)),
    (EN_US, " 2 * (1.1 + 2.2) / 3.3 ", Some(2.0 * (1.1 + 2.2) / 3.3)),
    (EN_US, "5,123", Some(5123.0)), // thousand separator
    (EN_US, "5,123.456", Some(5123.456)), // both separators

    // https://github.com/LibrePCB/LibrePCB/issues/1367
    (DE_DE, "5,123", Some(5.123)), // decimal point
    (DE_DE, "5.123", Some(5.123)), // support '.' in any locale
    (DE_DE, "5.123,456", None), // downside :-/

    // invalid cases
    (EN_US, "", None),
    (EN_US, " ", None),
    (EN_US, ".", None),
    (EN_US, "/", None),
    (EN_US, "(1+2", None),
];

#[test]
fn test() {
    for &((decimal_point, group_separator), input, output) in DATA {
        let parser = MathParser::with_separators(decimal_point, group_separator);
        let result = parser.parse(input);
        match output {
            Some(value) => assert_eq!(result, Ok(value), "{input:?}"),
            None => {
                let err = result.expect_err(input);
                assert!(!err.details.is_empty());
                assert!(!err.to_string().is_empty());
            }
        }
    }
}
