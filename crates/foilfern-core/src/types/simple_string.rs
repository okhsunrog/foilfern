//! Port of libs/librepcb/core/types/simplestring.h.

use super::string_newtype::{is_printable, string_newtype};

string_newtype!(
    /// A string consisting of printable characters of the Basic Multilingual
    /// Plane only (same rule as for [`ElementName`](super::ElementName)),
    /// without leading or trailing whitespace. The empty string is valid (and
    /// the default).
    #[derive(Default)]
    SimpleString,
    |value| value.trim() == value && value.chars().all(is_printable),
    crate::types::Error::InvalidSimpleString
);

impl SimpleString {
    /// Cleans a user input string: collapses whitespace and removes
    /// non-printable characters (upstream `cleanSimpleString()`).
    pub fn clean(user_input: &str) -> Self {
        let cleaned: String = user_input
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .filter(|&c| is_printable(c))
            .collect();
        // Upstream would abort if removing a non-printable character exposed
        // leading/trailing whitespace; trim instead to keep the invariant.
        Self(cleaned.trim().to_owned())
    }
}
