//! Port of libs/librepcb/core/attribute/attributekey.h.

use crate::types::string_newtype::{is_restricted, string_newtype};
use crate::utils::toolbox::clean_user_input_string;

/// Maximum length.
const MAX_LENGTH: usize = 40;

fn is_allowed_char(c: char) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'
}

string_newtype!(
    /// The key of an [`Attribute`](super::Attribute) (e.g. `"MPN"`).
    ///
    /// Valid keys contain 1..40 characters of `[_0-9A-Z]`. Serialized as
    /// string.
    AttributeKey,
    |value| is_restricted(value, MAX_LENGTH, is_allowed_char),
    crate::types::Error::InvalidAttributeKey
);

impl AttributeKey {
    /// Cleans a user input string to make it a valid attribute key (if not
    /// empty afterwards): converts to uppercase, replaces spaces by `_` and
    /// removes invalid characters (upstream `cleanAttributeKey()`).
    pub fn clean(user_input: &str) -> String {
        clean_user_input_string(
            user_input,
            is_allowed_char,
            true,
            false,
            true,
            "_",
            Some(MAX_LENGTH),
        )
    }
}
