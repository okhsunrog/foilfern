//! Port of libs/librepcb/core/library/cmp/componentprefix.h, plus the
//! `NormDependentPrefixMap` of component.h.

use crate::serialization::{KeyValueMapPolicy, SerializableKeyValueMap};
use crate::types::string_newtype::string_newtype;
use crate::utils::toolbox::clean_user_input_string;

/// Maximum length.
const MAX_LENGTH: usize = 16;

fn is_allowed_char(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

string_newtype!(
    /// A component prefix, e.g. `"R"` for resistors (used to generate
    /// component names like `R1`).
    ///
    /// Valid prefixes contain 0..=16 characters of `[a-zA-Z_]` (the empty
    /// prefix is valid).
    #[derive(Default)]
    ComponentPrefix,
    |value| value.chars().count() <= MAX_LENGTH && value.chars().all(is_allowed_char),
    crate::types::Error::InvalidComponentPrefix
);

impl ComponentPrefix {
    /// Cleans a user input string to make it a valid prefix (upstream
    /// `cleanComponentPrefix()`).
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

/// Policy of [`NormDependentPrefixMap`].
#[derive(Debug, Clone, Copy)]
pub struct NormDependentPrefixMapPolicy;

impl KeyValueMapPolicy for NormDependentPrefixMapPolicy {
    type Value = ComponentPrefix;
    const TAG_NAME: &'static str = "prefix";
    const KEY_NAME: &'static str = "norm";
}

/// Component prefixes by norm (e.g. `(prefix "R")`,
/// `(prefix (norm "IEEE 315") "R")`).
pub type NormDependentPrefixMap = SerializableKeyValueMap<NormDependentPrefixMapPolicy>;
