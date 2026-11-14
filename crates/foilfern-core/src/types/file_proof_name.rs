//! Port of libs/librepcb/core/types/fileproofname.h.

use super::string_newtype::{is_restricted, string_newtype};
use crate::utils::toolbox::clean_user_input_string;

/// Maximum length.
const MAX_LENGTH: usize = 20;

fn is_allowed_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "-_+().".contains(c)
}

string_newtype!(
    /// A string usable within file names.
    ///
    /// Valid names contain 1..20 characters of `[-a-zA-Z0-9_+().]` and do not
    /// consist of dots only (like `"."` or `".."`).
    FileProofName,
    |value| is_restricted(value, MAX_LENGTH, is_allowed_char) && !value.chars().all(|c| c == '.'),
    crate::types::Error::InvalidFileProofName
);

impl FileProofName {
    /// Cleans a user input string to make it a valid file-proof name (if not
    /// empty afterwards), replacing spaces by `-` (upstream
    /// `cleanFileProofName()`).
    pub fn clean(user_input: &str) -> String {
        let s = clean_user_input_string(
            user_input,
            is_allowed_char,
            true,
            false,
            false,
            "-",
            Some(MAX_LENGTH),
        );
        if !s.is_empty() && !Self::is_valid(&s) {
            String::new() // Consists of only dots -> invalid.
        } else {
            s
        }
    }
}
