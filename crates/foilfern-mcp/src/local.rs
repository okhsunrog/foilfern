//! In-process operations through the shared dispatcher, without an MCP peer.

use crate::server::FoilfernMcp;
use rmcp::ErrorData;
use rmcp::model::{CallToolResult, JsonObject};

/// A local session using exactly the same dispatch and validation as MCP.
pub struct LocalRunner {
    server: FoilfernMcp,
}

impl LocalRunner {
    /// Creates a local adapter without spawning an SDK service or transport.
    pub fn new(server: FoilfernMcp) -> Self {
        Self { server }
    }

    /// Calls an operation, retaining images and structured content.
    pub async fn call(
        &self,
        tool: &str,
        arguments: JsonObject,
    ) -> Result<CallToolResult, ErrorData> {
        self.server.dispatch(tool, arguments).await
    }

    /// The shared session state.
    pub fn state(&self) -> &crate::McpState {
        self.server.state()
    }

    /// Finishes this adapter. It owns no background tasks or transport.
    pub async fn close(self) -> Result<(), String> {
        Ok(())
    }
}
