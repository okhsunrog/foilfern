//! Compilation of `.po` catalogs into the compact embedded format.
//!
//! Used by `build.rs` (via `#[path]`) and by unit tests. Only finished
//! translations are kept (no fuzzy, obsolete or incomplete entries, no
//! comments or locations). The blob is a sequence of length-prefixed
//! (u32 LE) UTF-8 strings:
//!
//! ```text
//! plural_forms_header
//! { context msgid nforms form0 .. form(nforms-1) }*
//! ```
//! where `nforms` is itself encoded as a decimal string.

use crate::plural::PluralForms;

#[derive(Debug, Default)]
struct Entry {
    context: Option<String>,
    msgid: Option<String>,
    msgid_plural: Option<String>,
    msgstr: Vec<(usize, String)>,
    fuzzy: bool,
    obsolete: bool,
}

/// Which field continuation lines (`"..."`) append to.
#[derive(Clone, Copy)]
enum Field {
    Context,
    Msgid,
    MsgidPlural,
    Msgstr(usize),
}

fn unescape(s: &str) -> String {
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
            Some('a') => out.push('\x07'),
            Some('b') => out.push('\x08'),
            Some('f') => out.push('\x0c'),
            Some('v') => out.push('\x0b'),
            Some(c) => out.push(c),
            None => {}
        }
    }
    out
}

fn quoted(s: &str, line_no: usize) -> Result<String, String> {
    let s = s.trim();
    s.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .map(unescape)
        .ok_or_else(|| format!("line {line_no}: expected quoted string, got {s:?}"))
}

/// Minimal `.po` parser.
fn parse_po(text: &str) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    let mut cur = Entry::default();
    let mut field: Option<Field> = None;
    let flush = |cur: &mut Entry, entries: &mut Vec<Entry>| {
        let entry = std::mem::take(cur);
        if entry.msgid.is_some() {
            entries.push(entry);
        }
    };
    for (i, raw) in text.lines().enumerate() {
        let line_no = i + 1;
        let line = raw.trim();
        let (line, obsolete) = match line.strip_prefix("#~") {
            Some(rest) => (rest.trim(), true),
            None => (line, false),
        };
        if line.is_empty() {
            continue;
        }
        // A comment or keyword after a msgstr starts a new entry.
        let starts_entry =
            line.starts_with('#') || line.starts_with("msgctxt") || line.starts_with("msgid ");
        if starts_entry && !cur.msgstr.is_empty() {
            flush(&mut cur, &mut entries);
            field = None;
        }
        if let Some(flags) = line.strip_prefix("#,") {
            cur.fuzzy |= flags.split(',').any(|f| f.trim() == "fuzzy");
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        cur.obsolete |= obsolete;
        if let Some(rest) = line.strip_prefix("msgctxt") {
            cur.context = Some(quoted(rest, line_no)?);
            field = Some(Field::Context);
        } else if let Some(rest) = line.strip_prefix("msgid_plural") {
            cur.msgid_plural = Some(quoted(rest, line_no)?);
            field = Some(Field::MsgidPlural);
        } else if let Some(rest) = line.strip_prefix("msgid") {
            cur.msgid = Some(quoted(rest, line_no)?);
            field = Some(Field::Msgid);
        } else if let Some(rest) = line.strip_prefix("msgstr") {
            let (index, rest) = match rest.strip_prefix('[') {
                Some(r) => {
                    let bad = || format!("line {line_no}: bad msgstr index");
                    let (idx, r) = r.split_once(']').ok_or_else(bad)?;
                    (idx.parse().map_err(|_| bad())?, r)
                }
                None => (0, rest),
            };
            cur.msgstr.push((index, quoted(rest, line_no)?));
            field = Some(Field::Msgstr(cur.msgstr.len() - 1));
        } else if line.starts_with('"') {
            let s = quoted(line, line_no)?;
            match field {
                Some(Field::Context) => cur.context.get_or_insert_default().push_str(&s),
                Some(Field::Msgid) => cur.msgid.get_or_insert_default().push_str(&s),
                Some(Field::MsgidPlural) => cur.msgid_plural.get_or_insert_default().push_str(&s),
                Some(Field::Msgstr(i)) => cur.msgstr[i].1.push_str(&s),
                None => return Err(format!("line {line_no}: unexpected string")),
            }
        } else {
            return Err(format!("line {line_no}: cannot parse {line:?}"));
        }
    }
    flush(&mut cur, &mut entries);
    Ok(entries)
}

/// Returns the `(context, msgid)` keys of all entries of a catalog or
/// template, including untranslated ones (entries without context have an
/// empty context).
#[cfg(test)]
pub(crate) fn catalog_keys(
    po: &str,
) -> Result<std::collections::BTreeSet<(String, String)>, String> {
    Ok(parse_po(po)?
        .into_iter()
        .filter(|e| !e.obsolete)
        .filter_map(|e| Some((e.context.unwrap_or_default(), e.msgid?)))
        .collect())
}

fn push_str(out: &mut Vec<u8>, s: &str) -> Result<(), String> {
    let len = u32::try_from(s.len()).map_err(|_| "string too long".to_owned())?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

fn header_field<'a>(header: &'a str, key: &str) -> Option<&'a str> {
    header.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case(key).then(|| v.trim())
    })
}

/// Compiled catalog plus non-fatal diagnostics.
pub struct Compiled {
    pub blob: Vec<u8>,
    pub warnings: Vec<String>,
}

/// Compiles the text of a `.po` file.
pub fn compile(po: &str) -> Result<Compiled, String> {
    let entries = parse_po(po)?;
    let header = entries
        .iter()
        .find(|e| e.context.is_none() && e.msgid.as_deref() == Some(""))
        .and_then(|e| e.msgstr.first())
        .map(|(_, s)| s.as_str())
        .unwrap_or_default();
    let plural_forms =
        header_field(header, "Plural-Forms").unwrap_or("nplurals=2; plural=(n != 1);");
    let parsed = PluralForms::parse(plural_forms).map_err(|e| e.to_string())?;

    let mut blob = Vec::new();
    let mut warnings = Vec::new();
    push_str(&mut blob, plural_forms)?;
    for e in &entries {
        let Some(msgid) = e.msgid.as_deref() else {
            continue;
        };
        if e.fuzzy || e.obsolete || (msgid.is_empty() && e.context.is_none()) {
            continue;
        }
        let mut forms = e.msgstr.clone();
        forms.sort_by_key(|(i, _)| *i);
        if forms.is_empty() || forms.iter().any(|(_, s)| s.is_empty()) {
            continue;
        }
        if e.msgid_plural.is_some() && forms.len() != parsed.nplurals() {
            warnings.push(format!(
                "{msgid:?} has {} plural forms, expected {}",
                forms.len(),
                parsed.nplurals()
            ));
        }
        push_str(&mut blob, e.context.as_deref().unwrap_or(""))?;
        push_str(&mut blob, msgid)?;
        push_str(&mut blob, &forms.len().to_string())?;
        for (_, s) in &forms {
            push_str(&mut blob, s)?;
        }
    }
    Ok(Compiled { blob, warnings })
}
