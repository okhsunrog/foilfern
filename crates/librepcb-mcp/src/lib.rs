//! MCP server for LibrePCB (milestone M1.5, design: `docs/mcp-design.md`):
//! lets AI agents inspect and edit LibrePCB projects and libraries through
//! the [Model Context Protocol](https://modelcontextprotocol.io), built on
//! the official Rust SDK [`rmcp`]. No upstream counterpart.
//!
//! # Usage
//!
//! The binary `librepcb-mcp` speaks MCP over stdio (default) or streamable
//! HTTP (`--http <addr>`, cargo feature `http`):
//!
//! ```text
//! librepcb-mcp [--workspace <dir>] [--project <file.lpp>] [--http 127.0.0.1:8080]
//! ```
//!
//! - `--workspace <dir>` opens (or creates) a LibrePCB workspace, which
//!   provides the libraries (`library_*` tools) and the default location of
//!   new projects.
//! - `--project <file>` opens a project at startup.
//!
//! ## Claude Code
//!
//! ```text
//! cargo install --path crates/librepcb-mcp
//! claude mcp add librepcb -- librepcb-mcp --workspace ~/LibrePCB-Workspace
//! ```
//!
//! Then ask e.g. "open ~/LibrePCB-Workspace/projects/foo/foo.lpp and run
//! the ERC". The stroke fonts new projects need are embedded into the
//! binary (a `share/librepcb` directory in `LIBREPCB_SHARE` or next to the
//! executable is used instead if present). Install the official libraries once with the
//! `library_install` tool (`libraries: ["LibrePCB Base"]`), or copy
//! libraries to `<workspace>/data/libraries/local/` and call
//! `library_rescan`. For other clients, configure the command
//! `librepcb-mcp --workspace <dir>` as stdio server; logs go to stderr
//! (`RUST_LOG=debug` for more).
//!
//! The server takes the upstream-compatible directory lock of the open
//! project, so LibrePCB refuses to open the project at the same time (and
//! vice versa); close the project (`project_close`) before editing it in
//! LibrePCB.
//!
//! # Embedding in an application
//!
//! The desktop app runs the same server in-process ("watch the agent",
//! `docs/ui-design.md`, decision 4), sharing its open project so agent
//! edits appear live and land in the same undo history:
//!
//! ```ignore
//! let state = McpState::new(Arc::new(my_host));         // impl McpHost
//! state.set_workspace(Some(workspace));                  // SharedWorkspace
//! state.set_project(Some(project));                      // SharedProject (active tab)
//! let server = transport::start_http(                    // feature `http`
//!     state.clone(),
//!     ([127, 0, 0, 1], transport::EMBEDDED_PORT).into(), // 8766
//!     runtime.handle(),                                  // the app's tokio runtime
//! )?;
//! // ... server.url() == "http://127.0.0.1:8766/mcp"; dropping `server` stops it.
//! ```
//!
//! - The host owns the workspace and projects as
//!   `Arc<parking_lot::Mutex<_>>` ([`SharedWorkspace`], [`SharedProject`]
//!   = `Arc<Mutex<`[`OpenProject`]`>>` of the editor crate) and edits them
//!   itself; the server locks them only during one synchronous tool step
//!   (workspace before project; never across `.await`).
//! - [`McpHost`] decides about `workspace_open`/`_create`,
//!   `project_open`/`_create`/`_close` ([`HostDecision`]: allow, refuse
//!   with a message (kind `refused`), or delegate, e.g. open the project
//!   in a tab and hand it over) and receives a [`ChangeNotice`] (tool,
//!   revisions, touched schematics/boards as [`Focus`]) after every call
//!   which changed something; [`McpState::subscribe()`] offers the same as
//!   a `watch` channel. The UI then pulls `Project::changes_since()`.
//! - The binary is a host too ([`StandaloneHost`]: allows everything).
//!
//! # Conventions
//!
//! - Units: millimeters and degrees (JSON numbers), converted exactly to
//!   the integer nanometers of the model ([`units`]).
//! - Addressing ([`resolve`]): designators (`"R1"`), pins (`"R1.1"` by pad
//!   name or `"U1.VCC"` by signal name), net names, schematic/board names
//!   or indices; UUIDs are accepted everywhere.
//! - Arguments are checked against the tool's input schema: unknown
//!   arguments (also in nested objects) fail with `invalid_argument`
//!   naming the key and the valid ones.
//! - Every result is the envelope `{outcome, revision, result, warnings}`
//!   as `structuredContent` plus a short text ([`outcome`]); errors have a
//!   stable kind ([`error::ErrorKind`]).
//! - Write tools take `expected_revision` and fail with `stale_revision`
//!   on a mismatch. The open project is edited through the
//!   [`ProjectEditor`](librepcb_editor::ProjectEditor) of the editor crate:
//!   every write tool is one undo group `"AI: <tool>"` (rolled back
//!   completely on error) and returns the affected entities re-read from
//!   the model, the change events and the new revision
//!   ([`tools::write`]). Unsaved changes are the undo stack's clean state.
//!   Library elements are copied from the workspace library database.
//!
//! # Tools
//!
//! Session/project: `server_info`, `workspace_open`, `workspace_create`,
//! `project_create`, `project_open`, `project_save`, `project_close`,
//! `project_summary`, `undo`, `redo`, `history`. Library: `library_list`,
//! `library_rescan`, `library_install`, `library_search`,
//! `library_element`. Circuit and schematic: `component_list`,
//! `component_get`, `component_add`, `component_remove`,
//! `component_update`, `symbol_move`, `connect`, `disconnect`,
//! `net_rename`, `net_class_set`, `net_list`, `netlist`, `schematic_list`,
//! `schematic_get`, `schematic_add`, `schematic_tidy`. Board: `board_get`, `board_add`,
//! `board_set_outline`, `device_place`, `device_auto_place`, `trace_add`,
//! `via_add`, `trace_remove`, `plane_add`, `design_rules_set`,
//! `planes_rebuild`, `unrouted`, `autoroute` (Freerouting if installed,
//! else the built-in router), `specctra_export`, `specctra_import`. Checks
//! and outputs: `erc_run`, `drc_run`, `export_fabrication`, `export_bom`,
//! `export_pick_place`, `export_netlist`, `jobs_list`, `jobs_run`,
//! `render`. Escape hatch: `mutation_apply`, `mutation_schema`. The
//! server's `instructions` ([`server::INSTRUCTIONS`]) describe the
//! workflow for agents.
//!
//! # Modules
//!
//! - [`server`]: rmcp tool registration, `spawn_blocking` wrapper.
//! - [`host`]: the host interface and the shared server state.
//! - [`transport`]: stdio and streamable HTTP serving.
//! - [`session`]: workspace and open project (shared handles, locked per
//!   tool call).
//! - [`tools`]: the tool implementations (synchronous, testable without
//!   MCP).
//! - [`resolve`], [`units`], [`views`], [`outcome`], [`error`]: shared
//!   conventions.

pub mod error;
pub mod host;
pub mod outcome;
pub mod resolve;
pub mod server;
pub mod session;
pub mod tools;
pub mod transport;
pub mod units;
pub mod views;

pub use error::{ErrorKind, ToolError, ToolResult};
pub use host::{
    ChangeNotice, Focus, HostDecision, McpHost, McpState, ProjectCreateRequest, StandaloneHost,
};
pub use outcome::ToolOutput;
pub use server::LibrePcbMcp;
pub use session::{OpenProject, Session, SharedProject, SharedWorkspace};
