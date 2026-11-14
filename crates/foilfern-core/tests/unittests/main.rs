//! Port of the upstream unit tests in tests/unittests/core (same test
//! vectors; parametrized gtest suites are loops over the data tables), plus
//! tests of the font module and the file-level compatibility tests
//! ([`compat`]).
//!
//! The only integration test binary of the crate (one link instead of one
//! per file; see AGENTS.md).

mod algorithm;
mod attribute;
mod compat;
mod export;
mod fileio;
mod font;
mod geometry;
mod helpers;
mod job;
mod library;
mod project;
mod serialization;
mod sqlite_database_test;
mod system_info_test;
mod types;
mod utils;
mod workspace;

/// Not a test: the "dummy-binary" process used by the systeminfo and
/// directory lock tests (see [`helpers::spawn_dummy_process()`]). Sleeps only
/// if started by the tests.
#[test]
#[ignore = "helper process for other tests"]
fn dummy_binary() {
    if std::env::var_os(helpers::DUMMY_BINARY_ENV).is_some() {
        std::thread::sleep(std::time::Duration::from_secs(120));
    }
}

mod integration;
