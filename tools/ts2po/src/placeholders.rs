//! Placeholder conversion between Qt (`%1`, `%L1`, `%n`) and the brace style
//! (`{0}`, `{n}`) used by Slint and `foilfern_i18n`.

use std::collections::BTreeSet;

/// A token of a Qt format string.
#[derive(Debug, PartialEq, Eq)]
enum Token<'a> {
    Text(&'a str),
    /// `%1`..`%99` or `%L1`..`%L99`.
    Arg(u32),
    /// `%n` or `%Ln`.
    Count,
    /// `%%` (only meaningful for Slint-origin strings, see [`from_qtized_slint`]).
    Percent,
}

/// Splits a Qt format string. `%%` is only recognized as an escape if
/// `percent_escape` is set (Qt's `arg()` itself has no such escape).
fn tokenize(s: &str, percent_escape: bool) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let bytes = s.as_bytes();
    let mut text_start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        if bytes.get(j) == Some(&b'L') {
            j += 1;
        }
        let token_end;
        let token = if bytes.get(j) == Some(&b'n') {
            token_end = j + 1;
            Token::Count
        } else if bytes.get(j).is_some_and(u8::is_ascii_digit) {
            // Qt accepts one or two digits.
            let mut end = j + 1;
            if bytes.get(end).is_some_and(u8::is_ascii_digit) {
                end += 1;
            }
            let n: u32 = s[j..end].parse().unwrap_or(0);
            if n == 0 {
                i += 1;
                continue;
            }
            token_end = end;
            Token::Arg(n)
        } else if percent_escape && j == i + 1 && bytes.get(j) == Some(&b'%') {
            token_end = j + 1;
            Token::Percent
        } else {
            i += 1;
            continue;
        };
        if text_start < i {
            tokens.push(Token::Text(&s[text_start..i]));
        }
        tokens.push(token);
        i = token_end;
        text_start = i;
    }
    if text_start < s.len() {
        tokens.push(Token::Text(&s[text_start..]));
    }
    tokens
}

/// Escapes literal braces for the brace-style format (`{` → `{{`).
fn escape_braces(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '{' => out.push_str("{{"),
            '}' => out.push_str("}}"),
            c => out.push(c),
        }
    }
}

/// Numbering of Qt arguments in brace style, derived from the source string.
///
/// `QString::arg()` always replaces the *lowest-numbered* remaining `%N`, so
/// `"%n segments: %2 mm"` followed by one `.arg(x)` puts `x` into `%2`. We
/// therefore number arguments by their rank among the numbers used in the
/// source string (`%2` → `{0}` here). For the common contiguous case this is
/// simply `%N` → `{N-1}`.
#[derive(Debug)]
pub struct ArgMap {
    numbers: Vec<u32>,
}

impl ArgMap {
    /// Builds the numbering from a (C++-origin) source string.
    pub fn from_source(source: &str) -> Self {
        let set: BTreeSet<u32> = tokenize(source, false)
            .into_iter()
            .filter_map(|t| match t {
                Token::Arg(n) => Some(n),
                _ => None,
            })
            .collect();
        Self {
            numbers: set.into_iter().collect(),
        }
    }

    /// Whether the numbers used are not simply `1..=k`.
    pub fn has_gaps(&self) -> bool {
        self.numbers
            .iter()
            .enumerate()
            .any(|(i, &n)| n as usize != i + 1)
    }

    /// Brace index for Qt argument `%n`; `None` if the source doesn't use it.
    fn index(&self, n: u32) -> Option<usize> {
        self.numbers.iter().position(|&x| x == n)
    }
}

/// Result of a conversion; `unknown_args` lists `%N` placeholders that were
/// not present in the source string (they are mapped to `{N-1}`).
#[derive(Debug, PartialEq, Eq)]
pub struct Converted {
    pub text: String,
    pub unknown_args: Vec<u32>,
}

/// Converts a C++-origin (Qt `tr()`) string to brace style.
///
/// * `%N`/`%LN` → `{i}` where `i` is the rank of `N` in `args` (see [`ArgMap`]).
/// * `%n`/`%Ln` → `{n}` (only numerus messages use it; in others Qt leaves it
///   untouched, so we do too).
/// * Literal `{`/`}` → `{{`/`}}`.
/// * `%` without a following placeholder stays a literal `%` (Qt's `arg()` has
///   no `%%` escape).
pub fn from_qt_cpp(s: &str, args: &ArgMap, numerus: bool) -> Converted {
    let mut text = String::with_capacity(s.len() + 8);
    let mut unknown_args = Vec::new();
    for token in tokenize(s, false) {
        match token {
            Token::Text(t) => escape_braces(t, &mut text),
            Token::Percent => unreachable!("no percent escapes in C++ strings"),
            Token::Count if numerus => text.push_str("{n}"),
            Token::Count => text.push_str("%n"),
            Token::Arg(n) => {
                let idx = args.index(n).unwrap_or_else(|| {
                    unknown_args.push(n);
                    n as usize - 1
                });
                text.push_str(&format!("{{{idx}}}"));
            }
        }
    }
    Converted { text, unknown_args }
}

/// Reverses the transformation `dev/i18n.py` applied to Slint strings before
/// merging them into the `.ts` file (`po_str_to_qs()` on a PO-escaped string).
///
/// * `%%` → `%`, `%n` → `{n}`, `%N` → `{N-1}`.
/// * PO escape sequences (`\"`, `\\`, `\n`, `\t`) are unescaped.
/// * Braces are left untouched: the Slint source already uses brace syntax.
///
/// Note that `{}` in the Slint source became `%1` and cannot be told apart
/// from `{0}`; for msgids prefer the exact string recovered from the `.slint`
/// sources (see `slint_src`).
pub fn from_qtized_slint(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for token in tokenize(s, true) {
        match token {
            Token::Text(t) => out.push_str(t),
            Token::Percent => out.push('%'),
            Token::Count => out.push_str("{n}"),
            Token::Arg(n) => out.push_str(&format!("{{{}}}", n - 1)),
        }
    }
    po_unescape(&out)
}

/// Forward transformation of `dev/i18n.py` (`po_str_to_qs(po_escape(s))`),
/// used to match `.slint` source strings against `.ts` sources.
pub fn qtize_slint(s: &str) -> String {
    let mut s = po_escape(s).replace('%', "%%").replace("{n}", "%n");
    let mut index = 0;
    while let Some(pos) = s.find("{}") {
        s.replace_range(pos..pos + 2, &format!("{{{index}}}"));
        index += 1;
    }
    let mut index = 0;
    loop {
        let placeholder = format!("{{{index}}}");
        if !s.contains(&placeholder) {
            break;
        }
        s = s.replace(&placeholder, &format!("%{}", index + 1));
        index += 1;
    }
    s
}

/// Escapes a string the way gettext tools write it inside `"..."`.
pub fn po_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

/// Inverse of [`po_escape`]; unknown escapes are kept verbatim.
pub fn po_unescape(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpp(src: &str, numerus: bool) -> String {
        from_qt_cpp(src, &ArgMap::from_source(src), numerus).text
    }

    #[test]
    fn cpp_placeholders() {
        assert_eq!(cpp("Invalid key: '%1'", false), "Invalid key: '{0}'");
        assert_eq!(cpp("%2 of %1, %1", false), "{1} of {0}, {0}");
        assert_eq!(cpp("%L1 mm", false), "{0} mm");
        assert_eq!(cpp("%10 %9 %1", false), "{2} {1} {0}");
        assert_eq!(cpp("%n item(s)", true), "{n} item(s)");
        assert_eq!(cpp("%Ln item(s)", true), "{n} item(s)");
        assert_eq!(cpp("%n not numerus", false), "%n not numerus");
    }

    #[test]
    fn cpp_literals() {
        assert_eq!(cpp("Ratio (% of Diameter)", false), "Ratio (% of Diameter)");
        assert_eq!(cpp("100%", false), "100%");
        assert_eq!(cpp("%0 %x %", false), "%0 %x %");
        assert_eq!(cpp("'{{NAME}}' %1", false), "'{{{{NAME}}}}' {0}");
        assert_eq!(cpp("p { margin: 0; }", false), "p {{ margin: 0; }}");
    }

    #[test]
    fn cpp_gaps_use_arg_rank() {
        // QString::arg() replaces the lowest-numbered placeholder first.
        let src = "Total length of %n trace segment(s): %2 mm / %3 in";
        let args = ArgMap::from_source(src);
        assert!(args.has_gaps());
        assert_eq!(
            from_qt_cpp(src, &args, true).text,
            "Total length of {n} trace segment(s): {0} mm / {1} in"
        );
        assert_eq!(
            cpp("Useless keepout zone in '%2'", false),
            "Useless keepout zone in '{0}'"
        );
        assert!(!ArgMap::from_source("%1 %2").has_gaps());
    }

    #[test]
    fn cpp_translation_uses_source_numbering() {
        let args = ArgMap::from_source("%2 mm / %3 in");
        let t = from_qt_cpp("%3 in / %2 mm / %4", &args, false);
        assert_eq!(t.text, "{1} in / {0} mm / {3}");
        assert_eq!(t.unknown_args, vec![4]);
    }

    #[test]
    fn slint_reverse() {
        assert_eq!(from_qtized_slint("Choose %1"), "Choose {0}");
        assert_eq!(from_qtized_slint("Pad %1/%2"), "Pad {0}/{1}");
        assert_eq!(from_qtized_slint("%n warning(s)"), "{n} warning(s)");
        assert_eq!(from_qtized_slint("100%% sure"), "100% sure");
        assert_eq!(from_qtized_slint(r#"E.g. \"%1\""#), r#"E.g. "{0}""#);
        assert_eq!(from_qtized_slint("{{literal}}"), "{{literal}}");
    }

    #[test]
    fn slint_forward_matches_dev_i18n_py() {
        // Test vectors of po_str_to_qs() in LibrePCB's dev/i18n.py.
        assert_eq!(qtize_slint(""), "");
        assert_eq!(qtize_slint("Foo {}"), "Foo %1");
        assert_eq!(qtize_slint("Foo {} Bar {} {}"), "Foo %1 Bar %2 %3");
        assert_eq!(qtize_slint("Foo {2} Bar {1} {0}"), "Foo %3 Bar %2 %1");
        assert_eq!(qtize_slint("Foo {1} Bar {n} {0}"), "Foo %2 Bar %n %1");
        assert_eq!(qtize_slint("Foo {} Bar {n} {}"), "Foo %1 Bar %n %2");
        assert_eq!(qtize_slint("Foo % Bar {}"), "Foo %% Bar %1");
        assert_eq!(qtize_slint("Say \"{}\""), r#"Say \"%1\""#);
    }

    #[test]
    fn po_escaping_roundtrip() {
        let s = "a\"b\\c\nd\te";
        assert_eq!(po_escape(s), r#"a\"b\\c\nd\te"#);
        assert_eq!(po_unescape(&po_escape(s)), s);
    }
}
