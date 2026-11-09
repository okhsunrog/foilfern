//! Conversion of the HTML descriptions of EAGLE elements to plain text
//! (upstream `QTextDocument::setHtml()` + `toPlainText()`).
//!
//! Hand-written: the HTML-to-text crates render for terminals (wrapping,
//! list markers, link references) while upstream only needs the text
//! content with line breaks at `<br>` and block elements, so this is a
//! small tag stripper with HTML whitespace collapsing and entity decoding
//! (`html-escape`). See COMPAT.md for the differences to Qt's HTML import.

/// Tags starting a new line (block elements).
const BLOCK_TAGS: &[&str] = &[
    "p",
    "div",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "li",
    "ul",
    "ol",
    "dl",
    "dt",
    "dd",
    "table",
    "tr",
    "td",
    "th",
    "thead",
    "tbody",
    "tfoot",
    "blockquote",
    "pre",
    "hr",
    "center",
    "address",
    "body",
    "html",
];

/// Tags whose content is not displayed.
const HIDDEN_TAGS: &[&str] = &["head", "title", "style", "script"];

/// Converts HTML to plain text: tags are removed, `<br>` and block elements
/// start new lines, whitespace is collapsed and entities are decoded. Lines
/// are returned without surrounding whitespace.
pub fn html_to_plain_text(html: &str) -> String {
    let mut lines: Vec<String> = vec![String::new()];
    let mut hidden_depth = 0usize;
    let mut rest = html;
    while !rest.is_empty() {
        if let Some(tag_start) = rest.strip_prefix('<') {
            if let Some(comment) = tag_start.strip_prefix("!--") {
                rest = comment.find("-->").map_or("", |i| &comment[i + 3..]);
                continue;
            }
            let Some(end) = tag_start.find('>') else {
                // Not a tag, treat as text.
                push_text(&mut lines, "<", hidden_depth);
                rest = tag_start;
                continue;
            };
            let tag = &tag_start[..end];
            rest = &tag_start[end + 1..];
            let closing = tag.starts_with('/');
            let name: String = tag
                .trim_start_matches('/')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase();
            if HIDDEN_TAGS.contains(&name.as_str()) {
                if closing {
                    hidden_depth = hidden_depth.saturating_sub(1);
                } else if !tag.ends_with('/') {
                    hidden_depth += 1;
                }
            } else if name == "br" || BLOCK_TAGS.contains(&name.as_str()) {
                lines.push(String::new());
            }
        } else {
            let end = rest.find('<').unwrap_or(rest.len());
            push_text(&mut lines, &rest[..end], hidden_depth);
            rest = &rest[end..];
        }
    }
    let lines: Vec<&str> = lines.iter().map(|l| l.trim()).collect();
    lines.join("\n")
}

fn push_text(lines: &mut [String], text: &str, hidden_depth: usize) {
    if hidden_depth > 0 {
        return;
    }
    let decoded = html_escape::decode_html_entities(text);
    let line = lines.last_mut().expect("there is always a line");
    for c in decoded.chars() {
        if c.is_whitespace() {
            // Collapse whitespace; non-breaking spaces become spaces in the
            // plain text (like `QTextDocument::toPlainText()`).
            if !line.ends_with(' ') && !(c == '\u{a0}' && line.is_empty()) {
                line.push(' ');
            }
        } else {
            line.push(c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_to_plain_text() {
        assert_eq!(html_to_plain_text(""), "");
        assert_eq!(html_to_plain_text("<b>X</b><br/>Y"), "X\nY");
        assert_eq!(html_to_plain_text("a &amp;  b\tc"), "a & b c");
        assert_eq!(
            html_to_plain_text("<html><head><title>T</title></head><p>A</p><p>B</p>"),
            "\n\nA\n\nB\n"
        );
        assert_eq!(html_to_plain_text("<!-- c -->x < y"), "x < y");
    }
}
