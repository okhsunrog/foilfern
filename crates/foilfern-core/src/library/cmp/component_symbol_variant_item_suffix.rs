//! Port of libs/librepcb/core/library/cmp/componentsymbolvariantitemsuffix.h.

use crate::types::string_newtype::string_newtype;
use crate::utils::toolbox::clean_user_input_string;

/// Maximum length.
const MAX_LENGTH: usize = 16;

fn is_allowed_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

string_newtype!(
    /// The suffix of a symbol variant item (gate), appended to the
    /// component name in schematics (e.g. `"A"` for `U1A`).
    ///
    /// Valid suffixes contain 0..=16 characters of `[0-9a-zA-Z_]` (the empty
    /// suffix is valid).
    #[derive(Default)]
    ComponentSymbolVariantItemSuffix,
    |value| value.chars().count() <= MAX_LENGTH && value.chars().all(is_allowed_char),
    crate::types::Error::InvalidComponentSymbolVariantItemSuffix
);

impl ComponentSymbolVariantItemSuffix {
    /// Cleans a user input string to make it a valid suffix (upstream
    /// `cleanComponentSymbolVariantItemSuffix()`).
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
