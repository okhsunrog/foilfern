//! Port of libs/librepcb/core/job/unknownoutputjob.{h,cpp}.
//!
//! Differences to upstream: the type name is stored explicitly (upstream
//! keeps it in the `OutputJob` base class).

use librepcb_i18n::tr;

use crate::serialization::SExpression;

/// An output job of a type not known to this version, kept verbatim so it
/// is written back unchanged (upstream `UnknownOutputJob`). Note that like
/// upstream, a changed name or UUID of such a job is not written.
///
/// Serde: an object with the fields `type_name` and `node`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UnknownOutputJob {
    type_name: String,
    node: SExpression,
}

impl UnknownOutputJob {
    /// Creates an unknown job from its type name and complete node.
    pub fn new(type_name: impl Into<String>, node: SExpression) -> Self {
        Self {
            type_name: type_name.into(),
            node,
        }
    }

    /// Returns the type name in files.
    pub fn type_name(&self) -> &str {
        &self.type_name
    }

    /// Returns the translated type name, e.g. `"Unknown (foo)"` (upstream
    /// `getTypeTr()`).
    pub fn type_tr(&self) -> String {
        format!(
            "{} ({})",
            tr!("UnknownOutputJob", "Unknown"),
            self.type_name
        )
    }

    /// Returns the complete job node, which is written back verbatim.
    pub fn node(&self) -> &SExpression {
        &self.node
    }
}
