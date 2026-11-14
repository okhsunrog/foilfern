//! File-level interoperability with upstream: round-trips of all files in
//! the upstream test data, and comparisons with the upstream `librepcb-cli`
//! (file format migrations, rule checks, exports).

mod board_export;
mod drc_cli;
mod file_format_migration;
mod geometry_roundtrip;
mod interactive_html_bom;
mod library_roundtrip;
mod package_roundtrip;
mod project_exports;
mod project_roundtrip;
mod sexpression_roundtrip;
