//! Exports all upstream project fixtures. With LIBREPCB_CLI (or an
//! upstream CLI in PATH), compares complete JSON bytes against a custom
//! project_json output job. The CLI only receives temporary copies.

use librepcb_core::fileio::{
    FilePath, TransactionalDirectory, TransactionalFileSystem, file_utils,
};
use librepcb_core::project::{ProjectJsonExport, ProjectLoader};
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
        .is_ok_and(|output| output.status.success());
    assert!(
        available || configured.is_none(),
        "Configured LIBREPCB_CLI could not be run: {}",
        cli.display()
    );
    available.then_some(cli)
}

#[test]
fn project_json_export_test_data() {
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
        eprintln!("librepcb-cli not found; fixture exports run, JSON byte comparisons are skipped");
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
        let bytes = ProjectJsonExport::to_utf8(&project)
            .unwrap_or_else(|error| panic!("{}: {error}", directory.display()));
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["format"]["type"], "librepcb-project");
        assert_eq!(
            value["project"]["boards"].as_array().unwrap().len(),
            project.boards().len()
        );
        let Some(cli) = &cli else {
            continue;
        };
        let tmp = tempfile::tempdir().unwrap();
        let copy = tmp.path().join("project");
        file_utils::copy_dir_recursively(
            &FilePath::new(&directory).unwrap(),
            &FilePath::new(&copy).unwrap(),
        )
        .unwrap();
        let jobs = tmp.path().join("jobs.lp");
        std::fs::write(&jobs, b"(librepcb_jobs\n (job 9e20d82d-1f9b-4789-9e8c-e54bf21e779c (type project_json) (name \"JSON\") (output \"project.json\"))\n)\n").unwrap();
        let output_dir = tmp.path().join("output");
        let output = Command::new(cli)
            .arg("open-project")
            .arg("--run-jobs")
            .arg("--jobs")
            .arg(&jobs)
            .arg("--outdir")
            .arg(&output_dir)
            .arg(copy.join(&filename))
            .env("QT_QPA_PLATFORM", "offscreen")
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .env("LANGUAGE", "C")
            .env_remove("LIBREPCB_UPGRADE_UNSTABLE")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}: CLI failed:\n{}{}",
            directory.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let expected = std::fs::read(output_dir.join("project.json")).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            String::from_utf8(expected).unwrap(),
            "{}",
            directory.display()
        );
    }
}
