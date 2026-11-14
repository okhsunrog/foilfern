//! Port of libs/librepcb/core/utils/overlinemarkupparser.{h,cpp}.
//!
//! Only the markup extraction is ported; `calculate()`/`process()` (overline
//! positions from Qt font metrics) belong to the text rendering, which is
//! not ported yet.

use std::ops::Range;

/// Extracts overlines from text with markup like `RST/!SHDN`: removes the
/// functional `!` and returns the output text together with the ranges to
/// draw overlines over.
///
/// Markup rules:
/// 1. A single `!` toggles the overline on/off, depending on its previous
///    state (initial state is off).
/// 2. The character `/` implicitly switches off the overline before
///    rendering. This can be prevented by prefixing it with `!`.
/// 3. A double `!!` has no effect on overlines, it is rendered as a single
///    `!`. In case of an odd number of `!` (e.g. `!!!`), the **last** one
///    toggles overline on/off.
/// 4. Any trailing `!` have no effect, they are rendered as-is (bypassing
///    rules 1 and 3).
///
/// The returned ranges are byte ranges of the output text (upstream returns
/// `(start, length)` pairs of UTF-16 indices).
pub fn extract_overlines(input: &str) -> (String, Vec<Range<usize>>) {
    let mut output = String::with_capacity(input.len());
    let mut spans = Vec::new();

    // Split off trailing '!'.
    let body = input.trim_end_matches('!');
    let trailing = &input[body.len()..];

    // Convert remaining '!' to overlines and '!!' to '!'.
    let mut span_start: Option<usize> = None;
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('!', Some('!')) => {
                // Substitute '!!' by '!'.
                output.push('!');
                chars.next();
            }
            ('!', Some('/')) => {
                // Do not end overline if '/' is prefixed with '!'.
                span_start.get_or_insert(output.len());
                output.push('/');
                chars.next();
            }
            ('!', _) => match span_start.take() {
                // Start overline on single '!'.
                None => span_start = Some(output.len()),
                // End overline on single '!'.
                Some(start) => spans.push(start..output.len()),
            },
            ('/', _) => {
                // End overline implicitly on '/'.
                if let Some(start) = span_start.take() {
                    spans.push(start..output.len());
                }
                output.push('/');
            }
            (c, _) => output.push(c),
        }
    }

    // Append trailing '!' as-is.
    output.push_str(trailing);

    // Finish current span.
    if let Some(start) = span_start {
        spans.push(start..output.len());
    }
    (output, spans)
}

/// Returns the width of overlines for a text height in pixels (upstream
/// `getLineWidth()`).
pub fn overline_width(height_px: f64) -> f64 {
    height_px / 15.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_ascii() {
        let (output, spans) = extract_overlines("ä!ÖÜ!ß");
        assert_eq!(output, "äÖÜß");
        assert_eq!(&output[spans[0].clone()], "ÖÜ");
    }
}
