//! Port of libs/librepcb/core/attribute (attributes and their substitution).
//!
//! Differences to upstream:
//! - The `AttributeType` class hierarchy (`AttrTypeString`,
//!   `AttrTypeResistance`, ... singletons) is the enum [`AttributeType`] with
//!   static unit tables; units are `&'static` [`AttributeUnit`]s and "no
//!   unit" is `None` instead of `nullptr`.
//! - [`Attribute`] keeps its invariant (unit available for the type, value
//!   valid) through fallible constructors/setters instead of `LogicError`.
//! - `AttributeType::valueFromTr()` is not ported: it parses with the
//!   system locale and has no callers upstream.

#[allow(clippy::module_inception)] // Upstream file name.
mod attribute;
mod attribute_key;
mod attribute_substitutor;
mod attribute_type;
mod error;

pub use attribute::{Attribute, AttributeList, AttributeListTag};
pub use attribute_key::AttributeKey;
pub use attribute_substitutor::substitute;
pub use attribute_type::{AttributeType, AttributeUnit};
pub use error::Error;
