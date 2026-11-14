//! Port of libs/librepcb/core/types/busname.h.

use super::string_newtype::{is_restricted, string_newtype};
use crate::utils::toolbox::clean_user_input_string;

/// Maximum length.
const MAX_LENGTH: usize = 32;

fn is_allowed_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "-._+/!?&@#$()[]".contains(c)
}

string_newtype!(
    /// A valid bus name: same rules as
    /// [`CircuitIdentifier`](super::CircuitIdentifier), but additionally
    /// allowing `[` and `]` (to denote vectors).
    BusName,
    |value| is_restricted(value, MAX_LENGTH, is_allowed_char),
    crate::types::Error::InvalidBusName
);

impl BusName {
    /// Cleans a user input string to make it a valid bus name (if not empty
    /// afterwards), replacing spaces by `_` (upstream `cleanBusName()`).
    pub fn clean(user_input: &str) -> String {
        clean_user_input_string(
            user_input,
            is_allowed_char,
            true,
            false,
            false,
            "_",
            Some(MAX_LENGTH),
        )
    }
}
