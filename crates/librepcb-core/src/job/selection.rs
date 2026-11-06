//! Port of OutputJob::ObjectSet in libs/librepcb/core/job/outputjob.h.

use crate::serialization::{FromSExpression, List, Result, SExpression, ToSExpression};
use std::collections::BTreeSet;

/// Objects selected in an output job. `Option<Uuid>` selections can include
/// `None`, meaning a generic output without a board or assembly variant.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ObjectSet<T: Ord> {
    /// All available objects.
    All,
    /// The first object in project order.
    Default,
    /// Explicit objects (empty is valid).
    Custom(BTreeSet<T>),
}
impl<T: Ord + FromSExpression> ObjectSet<T> {
    /// Reads repeated children, recognizing `all`/`default` in the first one.
    pub fn deserialize(node: &SExpression, key: &str) -> Result<Self> {
        Ok(
            match node
                .child(&format!("{key}/@0"))
                .map(SExpression::value)
                .transpose()?
            {
                Some("all") => Self::All,
                Some("default") => Self::Default,
                _ => Self::Custom(
                    node.children_named(key)
                        .map(|n| n.child_value("@0"))
                        .collect::<Result<_>>()?,
                ),
            },
        )
    }
}
impl<T: Ord + ToSExpression> ObjectSet<T> {
    /// Writes objects sorted by their values, matching upstream.
    pub fn serialize(&self, root: &mut List, key: &str) {
        match self {
            Self::All | Self::Default => {
                root.ensure_line_break();
                root.append_child(
                    key,
                    &SExpression::token(if matches!(self, Self::All) {
                        "all"
                    } else {
                        "default"
                    }),
                );
            }
            Self::Custom(values) => {
                for value in values {
                    root.ensure_line_break();
                    root.append_child(key, value);
                }
            }
        }
        root.ensure_line_break();
    }
}
