//! Port of libs/librepcb/core/types/circuitidentifier.h.

use super::string_newtype::{is_restricted, optional_string_newtype_serialization, string_newtype};
use crate::utils::toolbox::clean_user_input_string;

/// Maximum length.
const MAX_LENGTH: usize = 32;

fn is_allowed_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "-._+/!?&@#$()".contains(c)
}

string_newtype!(
    /// A valid identifier used in circuits (net names, component names, ...),
    /// compatible with the strict requirements of e.g. SPICE netlists.
    ///
    /// Valid identifiers contain 1..32 characters of `[-a-zA-Z0-9._+/!?&@#$()]`.
    CircuitIdentifier,
    |value| is_restricted(value, MAX_LENGTH, is_allowed_char),
    crate::types::Error::InvalidCircuitIdentifier
);

optional_string_newtype_serialization!(CircuitIdentifier);

impl CircuitIdentifier {
    /// Cleans a user input string to make it a valid identifier (if not empty
    /// afterwards), replacing spaces by `_` (upstream
    /// `cleanCircuitIdentifier()`).
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
