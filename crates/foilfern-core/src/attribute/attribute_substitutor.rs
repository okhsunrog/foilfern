//! Port of libs/librepcb/core/attribute/attributesubstitutor.{h,cpp}.
//!
//! Variables are found with the [`regex`] crate (same pattern as upstream).
//! Keys are trimmed and adjacent whitespace is detected with Rust's
//! whitespace definition.
//!
//! Difference to upstream (see COMPAT.md): if the filter changes the length
//! of a substituted value, upstream continues with stale positions (and
//! corrupts the text); here the search restarts behind the filtered value.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

/// Matches a variable like `{{KEY}}` or `{{KEY or 'fallback'}}` (not
/// spanning lines, like upstream's `QRegularExpression`).
static VARIABLE: LazyLock<Regex> = LazyLock::new(|| {
    // The pattern is a constant, so it is known to be valid.
    Regex::new(r"\{\{(.*?)\}\}").expect("valid regex")
});

/// Special variable to insert a literal `}}` (does not work with the regex).
const ESCAPED_CLOSING_BRACES: &str = "{{ '}}' }}";

/// Substitutes all variables like `{{NAME}}` in `text` by their values
/// (e.g. by the name of a component, `U42`).
///
/// - `lookup` returns the value of an attribute key; `None` or an empty
///   string means the attribute does not exist.
/// - `filter`, if given, is applied to each substituted value (including
///   the values of nested variables, and surrounding text of the same outer
///   variable). This allows e.g. removing characters invalid in file names.
///
/// Variable syntax:
/// - `{{KEY}}`: the value of `KEY`. Values may contain variables too, which
///   are substituted recursively (with endless recursion detection).
/// - `{{KEY or OTHER or 'literal'}}`: the value of the first existing key,
///   or the literal text in quotes.
/// - Variables resolving to nothing are removed, together with one adjacent
///   whitespace character.
///
/// Known limitations (same as upstream): the recursion detection also
/// suppresses the second occurrence of the same key (`{{FOO}} {{FOO}}`),
/// and `{{FOO or BAR}}` takes `FOO` even if it evaluates to an empty string.
pub fn substitute(
    text: &str,
    mut lookup: impl FnMut(&str) -> Option<String>,
    mut filter: Option<&mut dyn FnMut(&str) -> String>,
) -> String {
    let mut s = text.to_owned();
    let mut start_pos = 0;
    // Outermost variable being substituted: (start, distance from end).
    let mut outer: Option<(usize, usize)> = None;
    // Keys already substituted, to avoid endless recursion.
    let mut key_backtrace: HashSet<String> = HashSet::new();
    while let Some((pos, mut length, keys)) = search_variable(&s, start_pos) {
        start_pos = pos;
        if let Some(filter) = filter.as_deref_mut() {
            if let Some((start, end)) = outer
                && start_pos + length > s.len().saturating_sub(end)
            {
                start_pos = apply_filter(&mut s, start, end, filter);
                outer = None;
                continue; // Search again, positions might have changed.
            }
            if outer.is_none() {
                outer = Some((start_pos, s.len() - length - start_pos));
            }
        }
        let mut key_found = false;
        for key in &keys {
            if key.starts_with('\'') && key.ends_with('\'') {
                // Replace "{{'VALUE'}}" by "VALUE".
                let literal = key.get(1..key.len() - 1).unwrap_or_default();
                s.replace_range(start_pos..start_pos + length, literal);
                // Do not search for variables in the literal.
                start_pos = (start_pos + key.len()).saturating_sub(2);
                key_found = true;
                break;
            } else if !key_backtrace.contains(key)
                && let Some(value) = lookup(key).filter(|v| !v.is_empty())
            {
                // Replace "{{KEY}}" by the value of KEY.
                s.replace_range(start_pos..start_pos + length, &value);
                key_backtrace.insert(key.clone());
                key_found = true;
                break;
            }
        }
        if !key_found {
            // Attribute not found, remove "{{KEY}}".
            let end = start_pos + length;
            let prev = s[..start_pos].chars().next_back();
            let next = s[end..].chars().next();
            if let Some(prev) = prev.filter(|&c| end == s.len() && c.is_whitespace()) {
                // At the end, also remove the whitespace before "{{KEY}}".
                start_pos -= prev.len_utf8();
                length += prev.len_utf8();
            } else if let Some(next) = next.filter(|&c| c.is_whitespace()) {
                // Otherwise remove the whitespace after "{{KEY}}".
                length += next.len_utf8();
            }
            s.replace_range(start_pos..start_pos + length, "");
        }
    }
    if let (Some(filter), Some((start, end))) = (filter, outer) {
        apply_filter(&mut s, start, end, filter);
    }
    s
}

/// Searches the next variable at or after `start`. Returns its position, its
/// length (including the braces) and its keys.
fn search_variable(text: &str, start: usize) -> Option<(usize, usize, Vec<String>)> {
    let captures = VARIABLE.captures_at(text, start)?;
    let whole = captures.get(0)?;
    let pos = whole.start();
    if text[pos..].starts_with(ESCAPED_CLOSING_BRACES) {
        return Some((pos, ESCAPED_CLOSING_BRACES.len(), vec!["'}}'".to_owned()]));
    }
    let keys = captures
        .get(1)?
        .as_str()
        .split(" or ")
        .map(|key| key.trim().to_owned())
        .collect();
    Some((pos, whole.len(), keys))
}

/// Applies `filter` to the text between `start` and `end` (distance from the
/// end of the string). Returns the end position of the filtered text.
fn apply_filter(
    s: &mut String,
    start: usize,
    end: usize,
    filter: &mut dyn FnMut(&str) -> String,
) -> usize {
    let start = s.floor_char_boundary(start);
    let region_end = s
        .floor_char_boundary(s.len().saturating_sub(end))
        .max(start);
    let filtered = filter(&s[start..region_end]);
    s.replace_range(start..region_end, &filtered);
    start + filtered.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter() {
        let lookup = |key: &str| match key {
            "A" => Some("a/b".to_owned()),
            "B" => Some("x{{C}}y".to_owned()),
            "C" => Some("c/d".to_owned()),
            _ => None,
        };
        let mut upper = |s: &str| s.replace('/', "_").to_uppercase();
        assert_eq!(
            substitute("{{A}}/{{B}}/c", lookup, Some(&mut upper)),
            "A_B/XC_DY/c"
        );
        let mut shorten = |s: &str| s.replace('/', "");
        assert_eq!(substitute("{{A}} {{A}}", lookup, Some(&mut shorten)), "ab");
        assert_eq!(substitute("x/{{A}}", |_| None, Some(&mut shorten)), "x/");
    }

    #[test]
    fn unicode() {
        let lookup = |key: &str| (key == "Ä").then(|| "ö".to_owned());
        assert_eq!(substitute("ü {{Ä}}ü", lookup, None), "ü öü");
        assert_eq!(substitute("ü\u{3000}{{X}}", lookup, None), "ü");
        assert_eq!(substitute("{{X}}\u{3000}ü", lookup, None), "ü");
        assert_eq!(substitute("{{'}}", lookup, None), "");
    }
}
