//! Validation of text inputs of the library tabs: port of the `validate*()`
//! functions of libs/librepcb/editor/utils/slinthelpers.{h,cpp} and of
//! `EditorToolbox::cleanKeywords()`.
//!
//! Each function returns the parsed value (if valid) and sets the error
//! text shown below the input ("Required", "Invalid", ...; translated with
//! upstream's `SlintHelpers` context).

use librepcb_core::fileio::{CleanFileNameOptions, FileNameCase, FilePath};
use librepcb_core::types::{CircuitIdentifier, ElementName, Version};
use librepcb_i18n::tr;
use librepcb_network::Url;

fn input_error(input: &str) -> String {
    if input.trim().is_empty() {
        tr!("SlintHelpers", "Required")
    } else {
        tr!("SlintHelpers", "Invalid")
    }
}

/// Upstream `validateElementName()`.
pub fn element_name(input: &str, error: &mut String) -> Option<ElementName> {
    match ElementName::new(ElementName::clean(input)) {
        Ok(name) => {
            error.clear();
            Some(name)
        }
        Err(_) => {
            *error = input_error(input);
            None
        }
    }
}

/// Upstream `validateCircuitIdentifier()`: "Duplicate" if `duplicate`.
pub fn circuit_identifier(
    input: &str,
    error: &mut String,
    duplicate: bool,
) -> Option<CircuitIdentifier> {
    let value = CircuitIdentifier::new(CircuitIdentifier::clean(input)).ok();
    if duplicate {
        *error = tr!("SlintHelpers", "Duplicate");
    } else if value.is_some() {
        error.clear();
    } else {
        *error = input_error(input);
    }
    value
}

/// Upstream `validateVersion()`.
pub fn version(input: &str, error: &mut String) -> Option<Version> {
    match input.trim().parse::<Version>() {
        Ok(v) => {
            error.clear();
            Some(v)
        }
        Err(_) => {
            *error = input_error(input);
            None
        }
    }
}

/// Parses a URL like `QUrl::fromUserInput()`: URLs without scheme get
/// `http://` (e.g. `github.com/foo`), local absolute paths `file://`.
pub fn parse_user_url(input: &str) -> Option<Url> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if let Ok(url) = Url::parse(input)
        && url.has_host()
    {
        return Some(url);
    }
    if input.starts_with('/') {
        return Url::from_file_path(input).ok();
    }
    Url::parse(&format!("http://{input}"))
        .ok()
        .filter(|u| u.host_str().is_some_and(|h| !h.is_empty()))
}

/// Upstream `validateUrl()`.
pub fn url(input: &str, error: &mut String, allow_empty: bool) -> Option<Url> {
    let url = parse_user_url(input);
    if url.is_some() || (allow_empty && input.trim().is_empty()) {
        error.clear();
    } else {
        *error = input_error(input);
    }
    url
}

/// The file name options of library directories (upstream
/// `ReplaceSpaces | KeepCase`).
pub const LIBRARY_DIR_OPTIONS: CleanFileNameOptions =
    CleanFileNameOptions::new(true, FileNameCase::Keep);

/// Upstream `validateFileName()`: the input must end with
/// `required_suffix`; the rest is cleaned and truncated.
pub fn file_name(
    input: &str,
    error: &mut String,
    options: CleanFileNameOptions,
    max_length: usize,
    required_suffix: &str,
) -> Option<String> {
    let s = input.trim();
    if !required_suffix.is_empty() && !s.ends_with(required_suffix) {
        *error = tr!("SlintHelpers", "Suffix '{0}' missing", required_suffix);
        return None;
    }
    let base = &s[..s.len() - required_suffix.len()];
    let cleaned = FilePath::clean_file_name(
        base,
        options,
        max_length.saturating_sub(required_suffix.len()),
    );
    if cleaned.is_empty() {
        *error = input_error(input);
        return None;
    }
    error.clear();
    Some(format!("{cleaned}{required_suffix}"))
}

/// Upstream `EditorToolbox::cleanKeywords()`: lower case, trimmed, without
/// empty entries and duplicates, comma separated.
pub fn clean_keywords(input: &str) -> String {
    let mut list: Vec<String> = Vec::new();
    for s in input.split(',') {
        let s = s.to_lowercase().trim().to_owned();
        if !s.is_empty() && !list.contains(&s) {
            list.push(s);
        }
    }
    list.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validators() {
        let mut e = String::new();
        assert!(element_name("  My Lib ", &mut e).is_some());
        assert!(e.is_empty());
        assert!(element_name("   ", &mut e).is_none());
        assert_eq!(e, "Required");
        assert!(version("0.1", &mut e).is_some());
        assert!(version("x", &mut e).is_none());
        assert_eq!(e, "Invalid");
        assert!(url("", &mut e, true).is_none() && e.is_empty());
        assert!(url("", &mut e, false).is_none() && e == "Required");
        assert_eq!(
            url("github.com/LibrePCB-Libraries/Base.lplib", &mut e, false)
                .unwrap()
                .as_str(),
            "http://github.com/LibrePCB-Libraries/Base.lplib"
        );
        assert_eq!(
            file_name("My Lib.lplib", &mut e, LIBRARY_DIR_OPTIONS, 50, ".lplib").as_deref(),
            Some("My_Lib.lplib")
        );
        assert!(file_name("My Lib", &mut e, LIBRARY_DIR_OPTIONS, 50, ".lplib").is_none());
        assert_eq!(e, "Suffix '.lplib' missing");
        assert_eq!(clean_keywords(" A, b ,,a"), "a,b");
    }
}
