//! The `foilfern-mcp` binary (see the crate documentation).

use std::process::ExitCode;

use clap::Parser;
use std::sync::Arc;

use foilfern_mcp::{McpState, Session, StandaloneHost};

mod local_cli;

/// Foilfern tool server and local calls for LibrePCB-compatible projects and libraries.
#[derive(Debug, Clone, Parser)]
#[command(name = "foilfern-mcp", version, about)]
struct Args {
    /// Workspace directory to open (created if it does not exist).
    #[arg(long, global = true)]
    workspace: Option<String>,
    /// Project (*.lpp file or directory) to open at startup.
    #[arg(long, global = true)]
    project: Option<String>,
    /// Serve streamable HTTP on this address (e.g. 127.0.0.1:8080, path
    /// /mcp) instead of stdio (needs the cargo feature `http`).
    #[arg(long)]
    http: Option<std::net::SocketAddr>,
    /// Save the open project after successful local calls.
    #[arg(long, global = true)]
    save: bool,
    #[command(subcommand)]
    command: Option<local_cli::Command>,
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
    if (args.command.is_some() && args.http.is_some()) || (args.save && args.command.is_none()) {
        eprintln!("foilfern-mcp: call/batch cannot use --http; --save requires call or batch.");
        return ExitCode::FAILURE;
    }
    let session = match tokio::task::spawn_blocking({
        let args = args.clone();
        move || setup(&args).map(Session::into_slots)
    })
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            eprintln!("foilfern-mcp: {e}");
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("foilfern-mcp: {e}");
            return ExitCode::FAILURE;
        }
    };
    // The standalone binary is a host which allows everything.
    let state = McpState::from_slots(session, Arc::new(StandaloneHost));
    if let Some(command) = args.command {
        return match local_cli::execute(
            foilfern_mcp::FoilfernMcp::with_state(state),
            command,
            args.save,
        )
        .await
        {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(error) => {
                eprintln!("foilfern-mcp: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let result = match args.http {
        Some(addr) => serve_http(state, addr).await,
        None => foilfern_mcp::transport::serve_stdio(state).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("foilfern-mcp: {e}");
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
    eprintln!("foilfern-mcp: serving streamable HTTP on http://{local}/mcp");
    let shutdown = tokio_util::sync::CancellationToken::new();
    tokio::spawn({
        let shutdown = shutdown.clone();
        async move {
            let _ = tokio::signal::ctrl_c().await;
            shutdown.cancel();
        }
    });
    foilfern_mcp::transport::serve_http(state, listener, shutdown)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(not(feature = "http"))]
async fn serve_http(_state: McpState, _addr: std::net::SocketAddr) -> Result<(), String> {
    Err("this build has no HTTP transport (build with --features http)".to_owned())
}
