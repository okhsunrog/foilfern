//! Errors of the [`pkg`](super) module.
//!
//! Replaces the `RuntimeError`/`LogicError` exceptions thrown by the
//! upstream package classes. Messages are the upstream strings (not
//! translated upstream).

/// Error returned when parsing package attributes or running the package
/// check fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Unknown package assembly type token.
    #[error("Unknown package assembly type: '{0}'")]
    UnknownAssemblyType(String),
    /// A polygon operation of the package check failed.
    #[error(transparent)]
    Clipper(#[from] crate::utils::clipper_helpers::Error),
}
