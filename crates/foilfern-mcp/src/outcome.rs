//! The response envelope of all tools (see `docs/mcp-design.md`, "Tool
//! conventions").
//!
//! A successful call returns `structuredContent`
//! `{ "outcome": "complete"|"partial", "revision": n|null, "result": {...},
//! "warnings": [...] }` plus a short text summary (and images for
//! `render`). A failed call returns `isError: true` with
//! `{ "outcome": "failed", "revision": n|null, "error": { "kind", "message" } }`.

use base64::Engine;
use rmcp::model::{CallToolResult, ContentBlock};
use serde::Serialize;
use serde_json::{Value, json};

use crate::error::{ToolError, ToolResult};

/// Overall outcome of a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Everything requested was done.
    Complete,
    /// Only part of the request was done (see the warnings).
    Partial,
    /// Nothing was done (error).
    Failed,
}

/// The output of a successful tool implementation.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    /// `Complete` or `Partial`.
    pub outcome: Outcome,
    /// One or a few lines for the text content.
    pub summary: String,
    /// The structured result.
    pub result: Value,
    /// Warnings (e.g. things which could not be done).
    pub warnings: Vec<String>,
    /// PNG images (MCP image content).
    pub images: Vec<Vec<u8>>,
    /// Project revision after the call (filled in by the server).
    pub revision: Option<u64>,
}

impl ToolOutput {
    /// Creates a complete output with a structured result.
    pub fn new(summary: impl Into<String>, result: impl Serialize) -> ToolResult<Self> {
        let result = serde_json::to_value(result)
            .map_err(|e| ToolError::internal(format!("cannot serialize the result: {e}")))?;
        Ok(Self {
            outcome: Outcome::Complete,
            summary: summary.into(),
            result,
            warnings: Vec::new(),
            images: Vec::new(),
            revision: None,
        })
    }

    /// Adds a warning.
    pub fn warn(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }

    /// Adds warnings.
    pub fn warnings(mut self, warnings: impl IntoIterator<Item = String>) -> Self {
        self.warnings.extend(warnings);
        self
    }

    /// Marks the output as partial.
    pub fn partial(mut self) -> Self {
        self.outcome = Outcome::Partial;
        self
    }

    /// Attaches a PNG image.
    pub fn image(mut self, png: Vec<u8>) -> Self {
        self.images.push(png);
        self
    }

    /// The structured content (envelope).
    pub fn envelope(&self) -> Value {
        json!({
            "outcome": self.outcome,
            "revision": self.revision,
            "result": self.result,
            "warnings": self.warnings,
        })
    }

    /// Converts to the MCP tool result.
    pub fn into_call_tool_result(self) -> CallToolResult {
        let mut text = self.summary.clone();
        for w in &self.warnings {
            text.push_str("\nwarning: ");
            text.push_str(w);
        }
        let mut content = vec![ContentBlock::text(text)];
        for png in &self.images {
            content.push(ContentBlock::image(
                base64::engine::general_purpose::STANDARD.encode(png),
                "image/png",
            ));
        }
        let mut result = CallToolResult::success(content);
        result.structured_content = Some(self.envelope());
        result
    }
}

/// Converts a tool error to the MCP tool result (`isError: true`).
pub fn error_result(error: &ToolError, revision: Option<u64>) -> CallToolResult {
    let mut result = CallToolResult::error(vec![ContentBlock::text(format!(
        "error ({}): {}",
        error.kind, error.message
    ))]);
    result.structured_content = Some(json!({
        "outcome": Outcome::Failed,
        "revision": revision,
        "error": { "kind": error.kind, "message": error.message },
    }));
    result
}
