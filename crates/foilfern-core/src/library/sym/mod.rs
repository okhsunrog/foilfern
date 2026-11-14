//! Port of libs/librepcb/core/library/sym (symbols).
//!
//! Not ported: `SymbolPainter` (rendering, to be implemented in the
//! rendering layer).

mod symbol;
mod symbol_check;
mod symbol_check_messages;
mod symbol_pin;

pub use symbol::Symbol;
pub(crate) use symbol_check::has_non_functional_inversion_sign;
pub use symbol_check::run_symbol_checks;
pub use symbol_check_messages::{ImageFileError, SymbolCheckMessage};
pub use symbol_pin::{SymbolPin, SymbolPinList, SymbolPinListTag};
