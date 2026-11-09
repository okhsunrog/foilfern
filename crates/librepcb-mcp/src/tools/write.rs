//! The common part of all write tools (see `docs/mcp-design.md`, "Tool
//! conventions"), and the history tools `undo`, `redo`, `history`.
//!
//! A write tool checks `expected_revision`, runs its editor commands as
//! one undo group labeled `"AI: <tool>"` (rolled back completely on
//! error) and responds with the affected entities re-read from the model
//! ("committed read-back"), the change events of the project's change
//! journal and the new revision (in the envelope).

use librepcb_core::project::{ChangesSince, Project};
use librepcb_editor::ProjectEditor;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::session::{OpenProject, Session};

/// At most this many change events are returned (the count is always
/// returned).
const MAX_CHANGES: usize = 200;

/// Checks `expected_revision` against the project revision.
pub fn check_revision(project: &Project, expected: Option<u64>) -> ToolResult<()> {
    match expected {
        Some(rev) if rev != project.revision() => Err(ToolError::new(
            ErrorKind::StaleRevision,
            format!(
                "The project revision is {}, not {rev}; re-read the project and retry.",
                project.revision()
            ),
        )),
        _ => Ok(()),
    }
}

/// The changes after `revision` as JSON (`"Resync"` if the journal does
/// not reach back that far) and their count.
pub fn changes_since(project: &Project, revision: u64) -> (Value, usize) {
    match project.changes_since(revision) {
        ChangesSince::Changes(changes) => {
            let count = changes.len();
            let shown = &changes[..count.min(MAX_CHANGES)];
            (serde_json::to_value(shown).unwrap_or(Value::Null), count)
        }
        ChangesSince::Resync => (json!("Resync"), 0),
    }
}

/// A committed write.
#[derive(Debug)]
pub struct Written<T> {
    /// What the tool's commands returned.
    pub value: T,
    /// The project revision before the write.
    pub revision_before: u64,
}

impl<T> Written<T> {
    /// Builds the tool output: `result` (the re-read entities) plus
    /// `revision_before`, `changes` and `change_count`.
    pub fn output(
        &self,
        open: &OpenProject,
        summary: impl Into<String>,
        mut result: Value,
    ) -> ToolResult<ToolOutput> {
        let (changes, count) = changes_since(open.project(), self.revision_before);
        if !result.is_object() {
            result = json!({ "value": result });
        }
        result["revision_before"] = json!(self.revision_before);
        result["change_count"] = json!(count);
        if count > MAX_CHANGES {
            result["changes_truncated"] = json!(true);
        }
        result["changes"] = changes;
        ToolOutput::new(summary, result)
    }
}

/// Runs `f` as one undo group `"AI: <tool>"` on the open project: fails
/// with `stale_revision` if `expected` differs from the revision, rolls
/// back everything if `f` fails.
pub fn write<T>(
    session: &mut Session,
    tool: &str,
    expected: Option<u64>,
    f: impl FnOnce(&mut ProjectEditor) -> ToolResult<T>,
) -> ToolResult<Written<T>> {
    let open = session.project_mut()?;
    check_revision(open.project(), expected)?;
    let revision_before = open.project().revision();
    open.editor.begin_group(format!("AI: {tool}"))?;
    match f(&mut open.editor) {
        Ok(value) => {
            open.editor.commit_group()?;
            Ok(Written {
                value,
                revision_before,
            })
        }
        Err(e) => {
            open.editor.abort_group()?;
            Err(e)
        }
    }
}

/// Arguments of `undo` / `redo`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UndoArgs {
    /// Number of steps (default 1).
    #[serde(default)]
    pub steps: Option<usize>,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// `undo` (`redo == false`) or `redo`.
pub fn undo_redo(session: &mut Session, args: UndoArgs, redo: bool) -> ToolResult<ToolOutput> {
    let open = session.project_mut()?;
    check_revision(open.project(), args.expected_revision)?;
    let revision_before = open.project().revision();
    let mut done = Vec::new();
    for _ in 0..args.steps.unwrap_or(1).max(1) {
        let text = if redo {
            open.editor.undo_stack().redo_text()
        } else {
            open.editor.undo_stack().undo_text()
        }
        .map(str::to_owned);
        let ok = if redo {
            open.editor.redo()?
        } else {
            open.editor.undo()?
        };
        match (ok, text) {
            (true, Some(text)) => done.push(text),
            _ => break,
        }
    }
    let verb = if redo { "Redone" } else { "Undone" };
    let summary = if done.is_empty() {
        format!("Nothing to {}.", if redo { "redo" } else { "undo" })
    } else {
        format!("{verb}: {}.", done.join(", "))
    };
    let written = Written {
        value: (),
        revision_before,
    };
    let open = session.project()?;
    let stack = open.editor.undo_stack();
    written.output(
        open,
        summary,
        json!({
            "done": done,
            "can_undo": stack.can_undo(),
            "can_redo": stack.can_redo(),
            "undo_text": stack.undo_text(),
            "redo_text": stack.redo_text(),
            "unsaved_changes": open.has_unsaved_changes(),
        }),
    )
}

/// `history`.
pub fn history(session: &Session) -> ToolResult<ToolOutput> {
    let open = session.project()?;
    let stack = open.editor.undo_stack();
    let entries: Vec<Value> = stack
        .history()
        .into_iter()
        .enumerate()
        .map(|(i, e)| json!({ "index": i, "text": e.text, "done": e.done }))
        .collect();
    let lines: Vec<String> = stack
        .history()
        .iter()
        .map(|e| format!("{} {}", if e.done { "[x]" } else { "[ ]" }, e.text))
        .collect();
    ToolOutput::new(
        format!(
            "{} undo step(s), {} done{}.\n{}",
            entries.len(),
            stack.index(),
            if open.has_unsaved_changes() {
                ", unsaved changes"
            } else {
                ""
            },
            lines.join("\n")
        ),
        json!({
            "entries": entries,
            "done": stack.index(),
            "can_undo": stack.can_undo(),
            "can_redo": stack.can_redo(),
            "undo_text": stack.undo_text(),
            "redo_text": stack.redo_text(),
            "unsaved_changes": open.has_unsaved_changes(),
        }),
    )
}
