//! Integration tests of the MCP server, in one test binary (one link instead
//! of one per file; see AGENTS.md).

mod common;
mod design;
mod embedded;
mod import;
mod local_calls;
mod routing;
mod routing_candidates;
mod stdio_e2e;
mod tools;
