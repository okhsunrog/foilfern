//! Port of libs/librepcb/core/rulecheck/rulecheckmessage.{h,cpp}.
//!
//! Upstream has one subclass of `RuleCheckMessage` per message kind, which
//! callers downcast with `as<T>()` to access message specific data. Here,
//! checks return typed enums (e.g.
//! [`LibraryCheckMessage`](crate::library::LibraryCheckMessage)) carrying
//! that data, which are converted into the generic [`RuleCheckMessage`]
//! (severity, texts, approval) where only the common part is needed.
//!
//! Not ported: `getSeverityIcon()` (UI).

use std::collections::BTreeSet;
use std::fmt;

use foilfern_i18n::tr;

use crate::geometry::Path;
use crate::serialization::{List, SExpression};

/// Message severity (ordered: `Hint < Warning < Error`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Hint.
    Hint,
    /// Warning.
    Warning,
    /// Error.
    Error,
}

impl Severity {
    /// Returns the translated name (upstream `getSeverityTr()`).
    pub fn name_tr(self) -> String {
        match self {
            Self::Hint => tr!("RuleCheckMessage", "Hint"),
            Self::Warning => tr!("RuleCheckMessage", "Warning"),
            Self::Error => tr!("RuleCheckMessage", "Error"),
        }
    }
}

impl fmt::Display for Severity {
    /// Formats the translated name.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name_tr())
    }
}

/// A message of a rule check (library element check, ERC, DRC).
///
/// Equality compares severity, message, description and locations, but not
/// the approval (like upstream `operator==`).
#[derive(Debug, Clone)]
pub struct RuleCheckMessage {
    severity: Severity,
    message: String,
    description: String,
    approval: SExpression,
    locations: Vec<Path>,
}

impl RuleCheckMessage {
    /// Creates a message whose approval is `(approved <approval_name>)`.
    ///
    /// Use [`approval_mut()`](Self::approval_mut) to add more details to the
    /// approval node.
    pub fn new(
        severity: Severity,
        message: impl Into<String>,
        description: impl Into<String>,
        approval_name: &str,
        locations: Vec<Path>,
    ) -> Self {
        let mut approval = List::new("approved");
        approval.push(SExpression::token(approval_name));
        Self {
            severity,
            message: message.into(),
            description: description.into(),
            approval: SExpression::List(approval),
            locations,
        }
    }

    /// Returns the severity.
    pub fn severity(&self) -> Severity {
        self.severity
    }

    /// Returns the (translated) message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the (translated) description.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the approval node, i.e. the node stored in the file if the
    /// user approves this message (e.g. `(approved empty_author)`).
    pub fn approval(&self) -> &SExpression {
        &self.approval
    }

    /// Returns the approval node for adding details to it.
    pub fn approval_mut(&mut self) -> &mut List {
        match &mut self.approval {
            SExpression::List(list) => list,
            // Invariant: `new()` always creates a list.
            _ => unreachable!("approval is always a list"),
        }
    }

    /// Returns the locations the message refers to (for highlighting).
    pub fn locations(&self) -> &[Path] {
        &self.locations
    }
}

impl PartialEq for RuleCheckMessage {
    fn eq(&self, other: &Self) -> bool {
        self.severity == other.severity
            && self.message == other.message
            && self.description == other.description
            && self.locations == other.locations
    }
}

/// Returns the approvals of all `messages` (upstream `getAllApprovals()`).
pub fn all_approvals<'a>(
    messages: impl IntoIterator<Item = &'a RuleCheckMessage>,
) -> BTreeSet<SExpression> {
    messages.into_iter().map(|m| m.approval.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialization::Mode;

    #[test]
    fn approval() {
        let mut msg = RuleCheckMessage::new(Severity::Hint, "m", "d", "foo", Vec::new());
        msg.approval_mut().append_child("name", "x");
        assert_eq!(
            msg.approval().to_string_with_mode(Mode::LibrePcb).unwrap(),
            "(approved foo (name \"x\"))\n"
        );
    }

    #[test]
    fn severity_order() {
        assert!(Severity::Hint < Severity::Warning);
        assert!(Severity::Warning < Severity::Error);
    }
}
