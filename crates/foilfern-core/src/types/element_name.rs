//! Port of libs/librepcb/core/types/elementname.h.

use super::string_newtype::{is_printable, optional_string_newtype_serialization, string_newtype};

/// Maximum length in characters.
const MAX_LENGTH: usize = 70;

string_newtype!(
    /// A valid element name (used as name of many objects).
    ///
    /// Valid names contain 1..=70 characters, only printable characters of
    /// the Basic Multilingual Plane (not of general category Control,
    /// Format, Surrogate, Private Use or Unassigned), and no leading or
    /// trailing whitespace.
    ///
    /// Upstream limits the length to 70 UTF-16 code units and rejects
    /// surrogates as non-printable. Since only BMP characters are accepted,
    /// "max 70 characters, BMP only" accepts exactly the same names.
    ElementName,
    |value| {
        !value.is_empty()
            && value.chars().count() <= MAX_LENGTH
            && value.trim() == value
            && value.chars().all(is_printable)
    },
    crate::types::Error::InvalidElementName
);

optional_string_newtype_serialization!(ElementName);

impl ElementName {
    /// Cleans a user input string to make it a valid element name (if not
    /// empty afterwards): trims it, removes non-printable characters and
    /// truncates it to 70 characters (upstream `cleanElementName()`).
    pub fn clean(user_input: &str) -> String {
        user_input
            .trim()
            .chars()
            .filter(|&c| is_printable(c))
            .take(MAX_LENGTH)
            .collect()
    }

    /// Creates a name from a translatable string: the translation is used if
    /// it (cleaned) is a valid name, otherwise the untranslated
    /// `text_no_tr`, which must be valid (upstream `elementNameFromTr()`).
    pub fn from_tr(context: &str, text_no_tr: &'static str) -> Self {
        let translated = Self::clean(&foilfern_i18n::translate(context, text_no_tr));
        Self::new(translated).unwrap_or_else(|_| {
            debug_assert!(Self::is_valid(text_no_tr), "invalid name: {text_no_tr}");
            Self(text_no_tr.to_owned())
        })
    }
}
