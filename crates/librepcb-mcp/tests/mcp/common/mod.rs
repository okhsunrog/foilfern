//! Helpers shared by the integration tests.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// The upstream test data directory.
pub fn test_data_dir() -> PathBuf {
    Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data")
}

/// Copies a directory recursively.
pub fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let target = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Copies the upstream test project `name` into `dir` and returns the
/// path of its `*.lpp` file.
pub fn copy_test_project(name: &str, dir: &Path) -> PathBuf {
    let dst = dir.join(name);
    copy_dir(&test_data_dir().join("projects").join(name), &dst);
    let lpp = std::fs::read_dir(&dst)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().ends_with(".lpp"))
        .unwrap();
    lpp.path()
}

/// Copies the upstream "Populated Library" into the local libraries of a
/// workspace (`libraries_path` as returned by `workspace_create`).
pub fn install_populated_library(libraries_path: &Path) {
    copy_dir(
        &test_data_dir().join("libraries/Populated Library.lplib"),
        &libraries_path.join("local/Populated Library.lplib"),
    );
}

/// Returns the upstream CLI, if available (`LIBREPCB_CLI` or `PATH`).
pub fn upstream_cli() -> Option<PathBuf> {
    let cli = std::env::var_os("LIBREPCB_CLI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("librepcb-cli"));
    let ok = Command::new(&cli)
        .arg("--version")
        .env("QT_QPA_PLATFORM", "offscreen")
        .output()
        .is_ok_and(|out| out.status.success());
    ok.then_some(cli)
}

/// Runs the upstream CLI (English, no GUI); returns (success, output).
pub fn run_cli(cli: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(cli)
        .args(args)
        .env("QT_QPA_PLATFORM", "offscreen")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("LANGUAGE", "C")
        .env("LIBREPCB_SUPPRESS_DEPRECATION_WARNINGS", "1")
        .output()
        .unwrap();
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}
