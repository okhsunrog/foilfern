//! Errors of the [`geometry`](super) module.
//!
//! Replaces the `RuntimeError`/`LogicError` exceptions thrown by the upstream
//! geometry classes. Messages are the upstream strings; only those
//! translated upstream go through [`tr!`].

use foilfern_i18n::tr;

/// Error returned when constructing, modifying or deserializing geometry
/// objects fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A `NonEmptyPath` was constructed from an empty path.
    #[error("{}", tr!("Path", "Path doesn't contain vertices!"))]
    EmptyPath,
    /// A `StraightAreaPath` was constructed from an invalid path.
    #[error("{}", tr!("Path", "Path is not fillable or contains arcs!"))]
    NotStraightArea,
    /// Via layers are not copper layers or not in top-to-bottom order.
    #[error("Invalid via layer specification.")]
    InvalidViaLayers,
    /// Via drill is automatic, but size is manual.
    #[error("Via drill is 'auto', but size is not 'auto'.")]
    ViaDrillAutoSizeManual,
    /// Via drill is larger than via size.
    #[error("Via drill is larger than via size.")]
    ViaDrillLargerThanSize,
    /// Unknown pad shape token.
    #[error("Unknown footprint pad shape: '{0}'")]
    UnknownPadShape(String),
    /// Unknown pad component side token.
    #[error("Unknown footprint pad component side: '{0}'")]
    UnknownPadComponentSide(String),
    /// Unknown pad function token.
    #[error("Unknown footprint pad function: '{0}'")]
    UnknownPadFunction(String),
}
