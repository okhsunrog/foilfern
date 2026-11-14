//! Port of libs/librepcb/core/types/tag.h.

use super::string_newtype::{is_restricted, string_newtype};
use crate::utils::toolbox::clean_user_input_string;

/// Maximum length.
const MAX_LENGTH: usize = 32;

fn is_allowed_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.'
}

string_newtype!(
    /// A kebab-case tag (e.g. `"ipc-density-level-a"`).
    ///
    /// Valid tags contain 1..32 characters of `[-a-z0-9.]`.
    Tag,
    |value| is_restricted(value, MAX_LENGTH, is_allowed_char),
    crate::types::Error::InvalidTag
);

impl Tag {
    /// Cleans a user input string to make it a valid tag (if not empty
    /// afterwards): converts to lowercase and replaces spaces by `-` (upstream
    /// `cleanTag()`).
    pub fn clean(user_input: &str) -> String {
        clean_user_input_string(
            user_input,
            is_allowed_char,
            true,
            true,
            false,
            "-",
            Some(MAX_LENGTH),
        )
    }
}
