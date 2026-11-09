//! The `librepcb-mcp` binary (see the crate documentation).

use std::process::ExitCode;

use clap::Parser;
use std::sync::Arc;

use librepcb_mcp::{McpState, Session, StandaloneHost};

/// MCP server for LibrePCB projects and libraries.
#[derive(Debug, Parser)]
#[command(name = "librepcb-mcp", version, about)]
struct Args {
    /// Workspace directory to open (created if it does not exist).
    #[arg(long)]
    workspace: Option<String>,
    /// Project (*.lpp file or directory) to open at startup.
    #[arg(long)]
    project: Option<String>,
    /// Serve streamable HTTP on this address (e.g. 127.0.0.1:8080, path
    /// /mcp) instead of stdio (needs the cargo feature `http`).
    #[arg(long)]
    http: Option<std::net::SocketAddr>,
}

fn setup(args: &Args) -> Result<Session, String> {
    let mut session = Session::new();
    if let Some(ws) = &args.workspace {
        session
            .open_workspace(ws, true)
            .map_err(|e| format!("cannot open workspace {ws}: {e}"))?;
    }
    if let Some(p) = &args.project {
        session
            .open_project(p)
            .map_err(|e| format!("cannot open project {p}: {e}"))?;
    }
    Ok(session)
}

#[tokio::main]
async fn main() -> ExitCode {
    // Logs go to stderr; stdout is the MCP transport.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .target(env_logger::Target::Stderr)
        .init();
    let args = Args::parse();
    let session = match tokio::task::spawn_blocking({
        let args = Args {
            workspace: args.workspace.clone(),
            project: args.project.clone(),
            http: args.http,
        };
        move || setup(&args).map(Session::into_slots)
    })
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            eprintln!("librepcb-mcp: {e}");
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("librepcb-mcp: {e}");
            return ExitCode::FAILURE;
        }
    };
    // The standalone binary is a host which allows everything.
    let state = McpState::from_slots(session, Arc::new(StandaloneHost));
    let result = match args.http {
        Some(addr) => serve_http(state, addr).await,
        None => librepcb_mcp::transport::serve_stdio(state).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("librepcb-mcp: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(feature = "http")]
async fn serve_http(state: McpState, addr: std::net::SocketAddr) -> Result<(), String> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| e.to_string())?;
    let local = listener.local_addr().map_err(|e| e.to_string())?;
    eprintln!("librepcb-mcp: serving streamable HTTP on http://{local}/mcp");
    let shutdown = tokio_util::sync::CancellationToken::new();
    tokio::spawn({
        let shutdown = shutdown.clone();
        async move {
            let _ = tokio::signal::ctrl_c().await;
            shutdown.cancel();
        }
    });
    librepcb_mcp::transport::serve_http(state, listener, shutdown)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(not(feature = "http"))]
async fn serve_http(_state: McpState, _addr: std::net::SocketAddr) -> Result<(), String> {
    Err("this build has no HTTP transport (build with --features http)".to_owned())
}
