//! Port of libs/librepcb/core/library/librarybaseelementcheckmessages.{h,cpp}.

use foilfern_i18n::tr;
use unicode_general_category::{GeneralCategory, get_general_category};

use crate::rule_check::{RuleCheckMessage, Severity};
use crate::types::ElementName;

/// Messages of the check common to all library elements (upstream
/// `Msg*` classes in librarybaseelementcheckmessages.h).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LibraryBaseElementCheckMessage {
    /// The author is not set (upstream `MsgMissingAuthor`).
    MissingAuthor,
    /// The default name is not title case (upstream `MsgNameNotTitleCase`).
    NameNotTitleCase {
        /// The name.
        name: ElementName,
    },
}

impl LibraryBaseElementCheckMessage {
    /// Returns the generic rule check message.
    pub fn to_message(&self) -> RuleCheckMessage {
        match self {
            Self::MissingAuthor => RuleCheckMessage::new(
                Severity::Warning,
                tr!("MsgMissingAuthor", "Author not set"),
                tr!(
                    "MsgMissingAuthor",
                    "It is recommended to set an author (e.g. full name or nickname), \
                     although it's not required."
                ),
                "empty_author",
                Vec::new(),
            ),
            Self::NameNotTitleCase { name } => RuleCheckMessage::new(
                Severity::Hint,
                tr!("MsgNameNotTitleCase", "Name not title case: '{0}'", name),
                tr!(
                    "MsgNameNotTitleCase",
                    "Generally the library element name should be written in title case \
                     (for consistency). As the current name has words starting with a \
                     lowercase character, it seems that it is not title cases. If this \
                     assumption is wrong, just ignore this message."
                ),
                "name_not_title_case",
                Vec::new(),
            ),
        }
    }
}

/// Returns whether `c` is a lowercase letter (upstream `QChar::isLetter() &&
/// QChar::isLower()`, i.e. general category Ll).
fn is_lowercase_letter(c: char) -> bool {
    get_general_category(c) == GeneralCategory::LowercaseLetter
}

/// Returns whether no word of `name` starts with a lowercase letter
/// (upstream `MsgNameNotTitleCase::isTitleCase()`).
pub fn is_title_case(name: &str) -> bool {
    let mut last_char_was_space = true;
    for c in name.chars() {
        if last_char_was_space && is_lowercase_letter(c) {
            return false;
        }
        last_char_was_space = c.is_whitespace();
    }
    true
}

/// Returns `name` with the first letter of every word converted to
/// uppercase (upstream `MsgNameNotTitleCase::getFixedName()`).
///
/// Letters without a single-character uppercase mapping (e.g. `ß`) are kept,
/// like `QChar::toUpper()` does.
pub fn title_case_fixed_name(name: &ElementName) -> ElementName {
    let mut fixed = String::with_capacity(name.len());
    let mut last_char_was_space = true;
    for mut c in name.chars() {
        if last_char_was_space && is_lowercase_letter(c) {
            let mut upper = c.to_uppercase();
            if let (Some(u), None) = (upper.next(), upper.next()) {
                c = u;
            }
        }
        fixed.push(c);
        last_char_was_space = c.is_whitespace();
    }
    ElementName::new(fixed).unwrap_or_else(|e| {
        log::error!("Could not fixup invalid name {name}: {e}");
        name.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_case() {
        assert!(is_title_case("Foo Bar"));
        assert!(is_title_case("Foo 3bar"));
        assert!(is_title_case("Foo-bar"));
        assert!(!is_title_case("foo Bar"));
        assert!(!is_title_case("Foo bar"));
        assert!(!is_title_case("Foo  ärger"));
        assert_eq!(
            title_case_fixed_name(&ElementName::new("foo bar-baz ärger ßx").unwrap()).as_str(),
            "Foo Bar-baz Ärger ßx"
        );
    }
}
