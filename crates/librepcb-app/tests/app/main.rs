//! Integration tests of the application, in one test binary (one link
//! instead of one per file; see AGENTS.md). The tests on Slint's headless
//! platform share one UI thread, see [`common::with_headless()`].

mod common;
mod dialog_screenshots;
mod dialogs;
mod editing;
mod element_graphics;
mod library_elements;
mod library_import;
mod library_management;
mod lifecycle;
mod live_mcp;
mod m3d_tools;
mod output_dialogs;
mod planes;
mod rule_checks;
mod screenshot;
mod setup_dialogs;
mod workspace_settings;
mod workspace_wizard;
