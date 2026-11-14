//! Port of libs/librepcb/core/utils/mathparser.{h,cpp}.
//!
//! Upstream wraps [muparser](https://beltoforion.de/en/muparser), configured
//! with the decimal point and group separator of a locale. It is used to
//! evaluate user input in length/angle/ratio edits, e.g. `2.54*3`.
//!
//! The expressions are evaluated by [`evalexpr`] with a context providing
//! muparser's default functions (`sin`, `sqrt`, `min`, ...) and constants
//! (`_pi`, `_e`); like in muparser, `;` separates function arguments.
//!
//! Number literals are handled by hand before: no crate reproduces
//! muparser's locale-dependent number parsing (a C++ `std::num_get` with the
//! locale's decimal point and a thousands separator with groups of 3
//! digits). Each literal is validated and rewritten as plain floating
//! point literal, so `evalexpr` never sees locale-specific characters (and
//! never uses integer arithmetic). Identifiers are checked against the
//! known functions and constants, so values like `inf` are rejected like
//! upstream.
//!
//! Not supported (compared to muparser): comparison, logical and ternary
//! operators, `rnd()`, and right-associativity of `^` (`2^3^2` is 64 here,
//! 512 in muparser).

use std::f64::consts::{E, PI};

use evalexpr::{
    Context, ContextWithMutableFunctions, ContextWithMutableVariables, DefaultNumericTypes,
    EvalexprError, Function, HashMapContext, Value,
};
use foilfern_i18n::tr;

/// Error returned by [`MathParser::parse()`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}\n\n{details}", tr!("MathParser", "Failed to parse expression:"))]
pub struct ParseError {
    /// Technical details (not translated).
    pub details: String,
}

impl ParseError {
    fn new(details: impl Into<String>) -> Self {
        Self {
            details: details.into(),
        }
    }
}

type UnaryFunction = fn(f64) -> f64;
type BinaryFunction = fn(f64, f64) -> f64;
type VariadicFunction = fn(&[f64]) -> f64;

/// Unary functions of muparser (transcendental functions from [`libm`] for
/// cross-platform determinism).
const UNARY_FUNCTIONS: &[(&str, UnaryFunction)] = &[
    ("sin", libm::sin),
    ("cos", libm::cos),
    ("tan", libm::tan),
    ("asin", libm::asin),
    ("acos", libm::acos),
    ("atan", libm::atan),
    ("sinh", libm::sinh),
    ("cosh", libm::cosh),
    ("tanh", libm::tanh),
    ("asinh", libm::asinh),
    ("acosh", libm::acosh),
    ("atanh", libm::atanh),
    ("log2", libm::log2),
    ("log10", libm::log10),
    ("log", libm::log),
    ("ln", libm::log),
    ("exp", libm::exp),
    ("sqrt", f64::sqrt),
    ("sign", sign),
    ("rint", rint),
    ("abs", f64::abs),
];

/// Functions of muparser with a variable number of arguments (at least 1).
const VARIADIC_FUNCTIONS: &[(&str, VariadicFunction)] = &[
    ("sum", |v| v.iter().sum()),
    ("avg", |v| v.iter().sum::<f64>() / v.len() as f64),
    ("min", |v| v.iter().copied().fold(f64::INFINITY, f64::min)),
    ("max", |v| {
        v.iter().copied().fold(f64::NEG_INFINITY, f64::max)
    }),
];

/// Binary functions of muparser.
const BINARY_FUNCTIONS: &[(&str, BinaryFunction)] = &[("atan2", libm::atan2)];

/// Constants of muparser.
const CONSTANTS: &[(&str, f64)] = &[("_pi", PI), ("_e", E)];

fn sign(v: f64) -> f64 {
    if v < 0.0 {
        -1.0
    } else if v > 0.0 {
        1.0
    } else {
        0.0
    }
}

fn rint(v: f64) -> f64 {
    (v + 0.5).floor()
}

/// Evaluates mathematical expressions like `(1+2)/2`.
///
/// Numbers are parsed with a configurable decimal point and group
/// (thousands) separator, see [`with_separators()`](Self::with_separators).
/// Additionally, `.` is always accepted as decimal point (upstream issue
/// #1367).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MathParser {
    decimal_point: char,
    group_separator: Option<char>,
}

impl Default for MathParser {
    /// Uses `.` as decimal point and `,` as group separator (like the "C"
    /// and English locales).
    fn default() -> Self {
        Self {
            decimal_point: '.',
            group_separator: Some(','),
        }
    }
}

impl MathParser {
    /// Creates a parser for the "C" locale (see [`Default`]).
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a parser for a locale with the given decimal point and group
    /// separator (upstream `setLocale()`), e.g. `(',', Some('.'))` for
    /// German.
    pub fn with_separators(decimal_point: char, group_separator: Option<char>) -> Self {
        Self {
            decimal_point,
            group_separator,
        }
    }

    /// Evaluates `expression`.
    pub fn parse(&self, expression: &str) -> Result<f64, ParseError> {
        let normalized = self.normalize(expression)?;
        let context = context().map_err(|e| ParseError::new(e.to_string()))?;
        match evalexpr::eval_with_context(&normalized, &context) {
            Ok(Value::Float(value)) => Ok(value),
            Ok(Value::Int(value)) => Ok(value as f64),
            Ok(_) => Err(ParseError::new("Expression does not evaluate to a number.")),
            Err(err) => Err(ParseError::new(err.to_string())),
        }
    }

    /// Validates identifiers and number literals and converts the
    /// expression to the `evalexpr` syntax.
    fn normalize(&self, expression: &str) -> Result<String, ParseError> {
        // Always support '.' as decimal separator in addition to the
        // locale-dependent separator, especially for German people.
        let expression = if self.decimal_point != '.' {
            expression.replace('.', &self.decimal_point.to_string())
        } else {
            expression.to_owned()
        };
        let chars: Vec<char> = expression.chars().collect();
        let mut out = String::with_capacity(expression.len());
        let mut last_literal_end = None;
        let mut i = 0;
        while let Some(&c) = chars.get(i) {
            if c.is_ascii_alphabetic() || c == '_' {
                let end = token_end(&chars, i, is_name_char);
                let name: String = chars[i..end].iter().collect();
                if !is_known_name(&name) {
                    return Err(unexpected("token", &name, i));
                }
                out.push_str(&name);
                i = end;
            } else if c.is_ascii_digit() || c == self.decimal_point {
                if last_literal_end == Some(i) {
                    return Err(unexpected("value", &c.to_string(), i));
                }
                let (value, end) = self
                    .parse_number(&chars, i)
                    .ok_or_else(|| unexpected("token", &c.to_string(), i))?;
                if chars.get(end).copied().is_some_and(is_name_char) {
                    return Err(unexpected("token", &chars[end].to_string(), end));
                }
                // `Display` never uses an exponent and is exact.
                let literal = value.to_string();
                out.push_str(&literal);
                if !literal.contains('.') {
                    out.push_str(".0"); // Make it a float for evalexpr.
                }
                last_literal_end = Some(end);
                i = end;
            } else if c == ';' {
                out.push(','); // Argument separator.
                i += 1;
            } else if c == '+'
                && out
                    .trim_end()
                    .chars()
                    .next_back()
                    .is_none_or(|prev| "(,+-*/^".contains(prev))
            {
                i += 1; // Unary plus (not supported by evalexpr) is a no-op.
            } else if matches!(c, ',' | '"' | '=' | '!' | '&' | '|' | '<' | '>' | '%')
                || Some(c) == self.group_separator
            {
                return Err(unexpected("token", &c.to_string(), i));
            } else {
                out.push(c);
                i += 1;
            }
        }
        Ok(out)
    }

    /// Parses a number literal at `start` like `std::num_get` with
    /// muparser's locale facet (decimal point, thousands separator with a
    /// grouping of 3). Returns the value and the end index, or `None` for an
    /// invalid number.
    fn parse_number(&self, chars: &[char], start: usize) -> Option<(f64, usize)> {
        let is_separator = |c: char| Some(c) == self.group_separator;
        let mut digits = String::new(); // In Rust's `f64` syntax.
        let mut groups: Vec<usize> = Vec::new();
        let mut sep_pos = 0;
        let mut found_mantissa = false;
        let mut found_dec = false;
        let mut found_sci = false;
        let mut i = start;
        while let Some(&c) = chars.get(i) {
            if is_separator(c) {
                if found_dec || found_sci {
                    break;
                }
                if sep_pos == 0 {
                    // Separator at the beginning or two consecutive ones.
                    return None;
                }
                groups.push(sep_pos);
                sep_pos = 0;
            } else if c == self.decimal_point {
                if found_dec || found_sci {
                    break;
                }
                if !groups.is_empty() {
                    groups.push(sep_pos);
                }
                digits.push('.');
                found_dec = true;
            } else if c.is_ascii_digit() {
                digits.push(c);
                found_mantissa = true;
                sep_pos += 1;
            } else if matches!(c, 'e' | 'E') && found_mantissa && !found_sci {
                if !groups.is_empty() && !found_dec {
                    groups.push(sep_pos);
                }
                digits.push('e');
                found_sci = true;
                if let Some(&sign @ ('+' | '-')) = chars.get(i + 1)
                    && !is_separator(sign)
                    && sign != self.decimal_point
                {
                    digits.push(sign);
                    i += 1;
                }
            } else {
                break;
            }
            i += 1;
        }
        if !groups.is_empty() {
            if !found_dec && !found_sci {
                groups.push(sep_pos);
            }
            // All groups must have 3 digits, except the first one (1..=3).
            let (first, rest) = groups.split_first()?;
            if *first > 3 || rest.iter().any(|&g| g != 3) {
                return None;
            }
        }
        // Like `strtod()`, the whole literal must be valid (e.g. not "1e"),
        // and overflows are errors.
        let value: f64 = digits.parse().ok()?;
        value.is_finite().then_some((value, i))
    }
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_known_name(name: &str) -> bool {
    UNARY_FUNCTIONS.iter().any(|(n, _)| *n == name)
        || BINARY_FUNCTIONS.iter().any(|(n, _)| *n == name)
        || VARIADIC_FUNCTIONS.iter().any(|(n, _)| *n == name)
        || CONSTANTS.iter().any(|(n, _)| *n == name)
}

fn token_end(chars: &[char], start: usize, predicate: impl Fn(char) -> bool) -> usize {
    chars[start..]
        .iter()
        .position(|&c| !predicate(c))
        .map_or(chars.len(), |len| start + len)
}

fn unexpected(what: &str, token: &str, pos: usize) -> ParseError {
    ParseError::new(format!(
        "Unexpected {what} \"{token}\" found at position {pos}."
    ))
}

/// Builds the evaluation context with muparser's functions and constants
/// (evalexpr's builtin functions are disabled).
fn context() -> Result<HashMapContext<DefaultNumericTypes>, EvalexprError> {
    let mut context = HashMapContext::new();
    context.set_builtin_functions_disabled(true)?;
    for &(name, value) in CONSTANTS {
        context.set_value(name.into(), Value::Float(value))?;
    }
    for &(name, f) in UNARY_FUNCTIONS {
        let function = Function::new(move |arg| Ok(Value::Float(f(arg.as_number()?))));
        context.set_function(name.into(), function)?;
    }
    for &(name, f) in BINARY_FUNCTIONS {
        let function = Function::new(move |arg| {
            let args = arg.as_fixed_len_tuple(2)?;
            Ok(Value::Float(f(args[0].as_number()?, args[1].as_number()?)))
        });
        context.set_function(name.into(), function)?;
    }
    for &(name, f) in VARIADIC_FUNCTIONS {
        let function = Function::new(move |arg| {
            let values = match arg {
                Value::Tuple(tuple) => tuple
                    .iter()
                    .map(Value::as_number)
                    .collect::<Result<_, _>>()?,
                value => vec![value.as_number()?],
            };
            Ok(Value::Float(f(&values)))
        });
        context.set_function(name.into(), function)?;
    }
    Ok(context)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Option<f64> {
        MathParser::new().parse(input).ok()
    }

    #[test]
    fn operators() {
        assert_eq!(parse("-2^2"), Some(-4.0));
        assert_eq!(parse("(-3)^2"), Some(9.0));
        assert_eq!(parse("3--2"), Some(5.0));
        assert_eq!(parse("+1*3"), Some(3.0));
        assert_eq!(parse("3-+2"), Some(1.0));
        assert_eq!(parse("7/2"), Some(3.5));
        assert_eq!(
            parse("1+2-3*4/5^6"),
            Some(1.0 + 2.0 - 3.0 * 4.0 / 5f64.powi(6))
        );
    }

    #[test]
    fn functions_and_constants() {
        assert_eq!(parse("sqrt(4)*2"), Some(4.0));
        assert_eq!(parse("min(3;1;2)"), Some(1.0));
        assert_eq!(parse("max(3)"), Some(3.0));
        assert_eq!(parse("avg(1;2)"), Some(1.5));
        assert_eq!(parse("atan2(0;1)"), Some(0.0));
        assert_eq!(parse("-_pi"), Some(-PI));
        assert_eq!(parse("log10(100)"), Some(2.0));
        assert_eq!(parse("rint(2.5)"), Some(3.0));
        assert_eq!(parse("min(1,2)"), None);
        assert_eq!(parse("foo(1)"), None);
        assert_eq!(parse("inf"), None);
        assert_eq!(parse("nan"), None);
        assert_eq!(parse("pi"), None);
    }

    #[test]
    fn numbers() {
        assert_eq!(parse("1e3"), Some(1000.0));
        assert_eq!(parse("2.5E-1"), Some(0.25));
        assert_eq!(parse(".5"), Some(0.5));
        assert_eq!(parse("5."), Some(5.0));
        assert_eq!(parse("1,000,000.5"), Some(1_000_000.5));
        assert_eq!(parse("1e400"), None);
        assert_eq!(parse("1e"), None);
        assert_eq!(parse("1,00"), None);
        assert_eq!(parse("1234,567"), None);
        assert_eq!(parse(",123"), None);
        assert_eq!(parse("1,,123"), None);
        assert_eq!(parse("1.5.3"), None);
        assert_eq!(parse("2x"), None);
        assert_eq!(parse("0x10"), None);
        assert_eq!(parse("2 3"), None);
        assert_eq!(parse("99999999999999999999"), Some(1e20));
    }

    #[test]
    fn other_syntax() {
        assert_eq!(parse("a=1"), None);
        assert_eq!(parse("1==1"), None);
        assert_eq!(parse("\"x\""), None);
        assert_eq!(parse("()"), None);
        assert_eq!(parse("1;2"), None);
        assert_eq!(parse("1%2"), None);
    }

    #[test]
    fn error_message() {
        let err = MathParser::new().parse("(1+2").unwrap_err();
        assert!(
            err.to_string()
                .starts_with("Failed to parse expression:\n\n")
        );
        assert!(!err.details.is_empty());
    }
}
