//! Serving the MCP server: stdio (the standalone binary) and streamable
//! HTTP on a local address (cargo feature `http`; the binary's `--http`
//! and the embedded server of the desktop app). No upstream counterpart.
//!
//! All MCP client sessions of one server share one [`McpState`].

use crate::host::McpState;
use crate::server::LibrePcbMcp;

/// Port of the LibrePCB MCP server embedded in the desktop app (Slint's UI
/// debugging MCP server uses 8765).
pub const EMBEDDED_PORT: u16 = 8766;

/// Serves MCP over stdin/stdout until the client disconnects.
pub async fn serve_stdio(state: McpState) -> Result<(), String> {
    use rmcp::ServiceExt;
    let running = LibrePcbMcp::with_state(state)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| e.to_string())?;
    running.waiting().await.map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(feature = "http")]
pub use http::{HttpServerHandle, serve_http, start_http};

#[cfg(feature = "http")]
mod http {
    use std::net::SocketAddr;

    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };
    use tokio_util::sync::CancellationToken;

    use super::*;

    /// Serves streamable HTTP at `http://<addr>/mcp` on `listener` until
    /// `shutdown` is cancelled (active MCP sessions are terminated then).
    pub async fn serve_http(
        state: McpState,
        listener: tokio::net::TcpListener,
        shutdown: CancellationToken,
    ) -> std::io::Result<()> {
        let config =
            StreamableHttpServerConfig::default().with_cancellation_token(shutdown.child_token());
        let service: StreamableHttpService<LibrePcbMcp, LocalSessionManager> =
            StreamableHttpService::new(
                move || Ok(LibrePcbMcp::with_state(state.clone())),
                Default::default(),
                config,
            );
        let router = axum::Router::new().nest_service("/mcp", service);
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { shutdown.cancelled().await })
            .await
    }

    /// A running HTTP server (see [`start_http()`]); dropping the handle
    /// stops the server.
    #[derive(Debug)]
    pub struct HttpServerHandle {
        local_addr: SocketAddr,
        shutdown: CancellationToken,
        task: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
    }

    impl HttpServerHandle {
        /// The address the server listens on (with the actual port if port
        /// 0 was requested).
        pub fn local_addr(&self) -> SocketAddr {
            self.local_addr
        }

        /// The MCP endpoint, e.g. `http://127.0.0.1:8766/mcp`.
        pub fn url(&self) -> String {
            format!("http://{}/mcp", self.local_addr)
        }

        /// Whether the server task has ended (stopped or failed).
        pub fn is_finished(&self) -> bool {
            self.task.as_ref().is_none_or(|t| t.is_finished())
        }

        /// Requests the server to stop (returns immediately).
        pub fn stop(&self) {
            self.shutdown.cancel();
        }

        /// Stops the server and waits until it has ended.
        pub async fn shutdown(mut self) -> std::io::Result<()> {
            self.shutdown.cancel();
            match self.task.take() {
                Some(task) => task.await.map_err(std::io::Error::other)?,
                None => Ok(()),
            }
        }
    }

    impl Drop for HttpServerHandle {
        fn drop(&mut self) {
            self.shutdown.cancel();
        }
    }

    /// Starts serving streamable HTTP on `addr` (e.g.
    /// `127.0.0.1:`[`EMBEDDED_PORT`]) as a task on `runtime`; callable from
    /// any thread (e.g. the UI thread). Binding errors (port in use) are
    /// returned immediately. Only loopback `Host` headers are accepted
    /// (rmcp's DNS rebinding protection).
    pub fn start_http(
        state: McpState,
        addr: SocketAddr,
        runtime: &tokio::runtime::Handle,
    ) -> std::io::Result<HttpServerHandle> {
        let listener = std::net::TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        let local_addr = listener.local_addr()?;
        let listener = {
            let _guard = runtime.enter();
            tokio::net::TcpListener::from_std(listener)?
        };
        let shutdown = CancellationToken::new();
        let task = runtime.spawn(serve_http(state, listener, shutdown.clone()));
        log::info!("MCP server listening on http://{local_addr}/mcp");
        Ok(HttpServerHandle {
            local_addr,
            shutdown,
            task: Some(task),
        })
    }
}
