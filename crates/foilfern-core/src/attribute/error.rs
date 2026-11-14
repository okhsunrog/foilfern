//! Errors of the [`attribute`](super) module.
//!
//! Replaces the `RuntimeError`/`LogicError` exceptions thrown by
//! libs/librepcb/core/attribute/*. Messages translated upstream go through
//! [`tr!`]; the `LogicError` replacement is not translated (like upstream).

use foilfern_i18n::tr;

/// Error returned for invalid attribute types, units or values.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Unknown attribute type name.
    #[error("{}", tr!("AttributeType", "Invalid attribute type: \"{0}\"", .0))]
    InvalidType(String),
    /// Unit name not available for the attribute type.
    #[error(
        "{}",
        tr!("AttributeType", "Unknown unit of attribute type \"{0}\": \"{1}\"", .type_name, .unit)
    )]
    UnknownUnit {
        /// Name of the attribute type (e.g. `"voltage"`).
        type_name: &'static str,
        /// The rejected unit name.
        unit: String,
    },
    /// The unit is not available for the type, or the value is not valid
    /// for the type (upstream `LogicError` with message `"type,value,unit"`).
    #[error("Invalid attribute: {type_name},{value},{unit}")]
    InvalidTypeValueUnit {
        /// Name of the attribute type.
        type_name: &'static str,
        /// The value.
        value: String,
        /// Name of the unit, or `-` for none.
        unit: &'static str,
    },
}
