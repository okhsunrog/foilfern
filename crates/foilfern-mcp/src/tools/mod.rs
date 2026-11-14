//! Tool implementations: synchronous functions on a [`Session`](crate::session::Session)
//! returning a [`ToolOutput`](crate::outcome::ToolOutput), wired to MCP in
//! [`server`](crate::server). Each module documents its tools.

pub mod board_edit;
pub mod board_requirements;
pub mod circuit;
pub mod design;
pub mod design_rules;
pub mod ground_reference;
pub mod import;
pub mod layout;
pub mod library;
pub mod mutation;
pub mod output;
pub mod part;
pub mod project;
pub mod requirements;
pub mod schematic_edit;
pub mod write;

pub mod routing;

pub mod trace_path;
