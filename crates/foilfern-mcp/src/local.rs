//! In-process calls through the MCP tool registry, without a client or protocol handshake.
//!
//! rmcp's router requires a request context containing a peer. The idle transport
//! supplies that SDK context only; no requests are serialized or sent through it.
//! Tools requiring client interaction fail explicitly instead of waiting forever.

use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, JsonObject};
use rmcp::service::{
    RequestContext, RoleServer, RunningService, RxJsonRpcMessage, TxJsonRpcMessage,
};
use rmcp::{ErrorData, ServerHandler};

use crate::server::FoilfernMcp;

/// A local session using exactly the same dispatch and argument validation as MCP.
pub struct LocalRunner {
    service: RunningService<RoleServer, FoilfernMcp>,
}

impl LocalRunner {
    /// Creates the SDK request context. No client, socket or subprocess is started.
    /// Call this within a Tokio runtime, like the ordinary MCP transports.
    pub fn new(server: FoilfernMcp) -> Self {
        Self {
            service: rmcp::service::serve_directly(server, IdleTransport, None),
        }
    }

    /// Calls a registered tool. Complete results retain images and structured content.
    pub async fn call(
        &self,
        tool: &str,
        arguments: JsonObject,
    ) -> Result<CallToolResult, ErrorData> {
        let request = CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments);
        let context = RequestContext::new(
            rmcp::model::RequestId::Number(0),
            self.service.peer().clone(),
        );
        match self.service.service().call_tool(request, context).await? {
            CallToolResponse::Complete(result) => Ok(result),
            _ => Err(ErrorData::invalid_params(
                "This tool requires an interactive MCP client.",
                None,
            )),
        }
    }

    /// The session state shared with the dispatcher.
    pub fn state(&self) -> &crate::McpState {
        self.service.service().state()
    }

    /// Releases the request context and its background SDK task.
    pub async fn close(self) -> Result<(), String> {
        self.service
            .cancel()
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

struct IdleTransport;

impl rmcp::transport::Transport<RoleServer> for IdleTransport {
    type Error = std::io::Error;

    fn send(
        &mut self,
        _item: TxJsonRpcMessage<RoleServer>,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'static {
        std::future::ready(Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Local tools have no MCP peer.",
        )))
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        std::future::pending().await
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
