//! Runs ERC on all upstream project fixtures. If LIBREPCB_CLI (or the
//! upstream CLI in PATH) is available, compares non-approved message text,
//! severity and approval counts with its --erc output. Source fixtures are
//! never passed to the CLI: it receives a temporary copy.

use librepcb_core::fileio::{
    FilePath, TransactionalDirectory, TransactionalFileSystem, file_utils,
};
use librepcb_core::project::erc::{ElectricalRuleCheck, ElectricalRuleCheckMessage};
use librepcb_core::project::{Project, ProjectLoader};
use librepcb_core::rule_check::Severity;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

fn upstream_cli() -> Option<PathBuf> {
    let configured = std::env::var_os("LIBREPCB_CLI");
    let cli = configured
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| "librepcb-cli".into());
    let available = Command::new(&cli)
        .arg("--version")
        .env("QT_QPA_PLATFORM", "offscreen")
        .output()
        .is_ok_and(|out| out.status.success());
    assert!(
        available || configured.is_none(),
        "Configured LIBREPCB_CLI could not be run: {}",
        cli.display()
    );
    available.then_some(cli)
}

fn compare_cli(
    cli: &Path,
    directory: &Path,
    filename: &str,
    project: &Project,
    messages: &[ElectricalRuleCheckMessage],
) {
    let tmp = tempfile::tempdir().unwrap();
    let copy = tmp.path().join("project");
    file_utils::copy_dir_recursively(
        &FilePath::new(directory).unwrap(),
        &FilePath::new(&copy).unwrap(),
    )
    .unwrap();
    let output = Command::new(cli)
        .arg("open-project")
        .arg("--erc")
        .arg(copy.join(filename))
        .env("QT_QPA_PLATFORM", "offscreen")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("LANGUAGE", "C")
        .env_remove("LIBREPCB_UPGRADE_UNSTABLE")
        .output()
        .unwrap();
    let mut approved = 0;
    let mut expected = Vec::new();
    for message in messages {
        let rule = message.rule_message();
        if project.erc_approvals().contains(rule.approval()) {
            approved += 1;
            continue;
        }
        let severity = match rule.severity() {
            Severity::Hint => "HINT",
            Severity::Warning => "WARNING",
            Severity::Error => "ERROR",
        };
        expected.push(format!("[{severity}] {}", rule.message()));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut actual: Vec<_> = stderr
        .lines()
        .filter_map(|line| line.trim().strip_prefix("- [").map(|s| format!("[{s}")))
        .collect();
    expected.sort();
    actual.sort();
    assert_eq!(
        actual,
        expected,
        "{}: CLI stderr:\n{stderr}",
        directory.display()
    );
    assert_eq!(
        output.status.code(),
        Some(if expected.is_empty() { 0 } else { 1 }),
        "CLI failed: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!("  Approved messages: {approved}\n")),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("  Non-approved messages: {}\n", expected.len())),
        "{stdout}"
    );
}

#[test]
fn electrical_rule_check_test_data() {
    let root = Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/projects")
        .canonicalize()
        .expect("upstream test data not found (set LIBREPCB_UPSTREAM_DIR)");
    let mut directories: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.join(".librepcb-project").is_file())
        .collect();
    directories.sort();
    assert!(directories.len() >= 3, "too few upstream project fixtures");
    let cli = upstream_cli();
    if cli.is_none() {
        eprintln!("librepcb-cli not found; ERC runs on all fixtures, CLI comparisons are skipped");
    }
    for directory in directories {
        let filename = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .find(|name| name.ends_with(".lpp"))
            .unwrap();
        let fs = TransactionalFileSystem::open_ro(&FilePath::new(&directory).unwrap()).unwrap();
        let project = ProjectLoader::new()
            .open(TransactionalDirectory::new(Arc::new(fs), ""), &filename)
            .unwrap();
        let messages = ElectricalRuleCheck::new(&project)
            .run_checks()
            .unwrap_or_else(|err| panic!("{}: {err}", directory.display()));
        if directory.file_name().unwrap() == "Empty Project" {
            assert!(messages.is_empty());
        }
        println!("{}: {} ERC findings", directory.display(), messages.len());
        if let Some(cli) = &cli {
            compare_cli(cli, &directory, &filename, &project, &messages);
        }
    }
}
