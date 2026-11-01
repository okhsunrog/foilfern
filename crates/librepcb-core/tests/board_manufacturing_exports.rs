//! Board manufacturing exports over upstream project fixtures. Optional
//! comparisons run official output jobs on temporary copies of projects.

use librepcb_core::export::{PickPlaceCsvWriter, PickPlaceSides};
use librepcb_core::fileio::{
    FilePath, TransactionalDirectory, TransactionalFileSystem, file_utils,
};
use librepcb_core::project::board::BoardPickPlaceGenerator;
use librepcb_core::project::{AssemblyVariantId, Project, ProjectLoader};
use std::collections::BTreeMap;
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

fn projects() -> Vec<(PathBuf, String, Project)> {
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
    directories
        .into_iter()
        .map(|directory| {
            let filename = std::fs::read_dir(&directory)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .find(|name| name.ends_with(".lpp"))
                .unwrap();
            let fs = TransactionalFileSystem::open_ro(&FilePath::new(&directory).unwrap()).unwrap();
            let project = ProjectLoader::new()
                .open(TransactionalDirectory::new(Arc::new(fs), ""), &filename)
                .unwrap();
            (directory, filename, project)
        })
        .collect()
}

fn oracle_outputs(
    cli: &Path,
    directory: &Path,
    filename: &str,
    job: &str,
) -> BTreeMap<String, String> {
    let tmp = tempfile::tempdir().unwrap();
    let copy = tmp.path().join("project");
    file_utils::copy_dir_recursively(
        &FilePath::new(directory).unwrap(),
        &FilePath::new(&copy).unwrap(),
    )
    .unwrap();
    let jobs = tmp.path().join("jobs.lp");
    std::fs::write(
        &jobs,
        format!(
            "(librepcb_jobs\n (job 9e20d82d-1f9b-4789-9e8c-e54bf21e779c (name \"Test\") {job})\n)\n"
        ),
    )
    .unwrap();
    let output_dir = tmp.path().join("output");
    let output = Command::new(cli)
        .arg("open-project")
        .arg("--run-jobs")
        .arg("--jobs")
        .arg(&jobs)
        .arg("--outdir")
        .arg(&output_dir)
        .arg(copy.join(filename))
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
    std::fs::read_dir(output_dir)
        .into_iter()
        .flatten()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            matches!(
                path.extension().and_then(|s| s.to_str()),
                Some("csv" | "d356")
            )
        })
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read_to_string(path).unwrap(),
            )
        })
        .collect()
}

#[test]
fn pick_place_matches_upstream_test_projects() {
    let cli = upstream_cli();
    if cli.is_none() {
        eprintln!("librepcb-cli not found; fixture exports run, CSV comparisons are skipped");
    }
    for (directory, filename, project) in projects() {
        let mut expected = BTreeMap::new();
        for (board_index, board) in project.boards().iter().enumerate() {
            for (variant_index, variant) in project.circuit().assembly_variants().iter().enumerate()
            {
                let data = BoardPickPlaceGenerator::new(
                    &project,
                    board,
                    AssemblyVariantId(variant.uuid()),
                )
                .generate()
                .unwrap();
                for (side, key) in [
                    (PickPlaceSides::Top, "top"),
                    (PickPlaceSides::Bottom, "bottom"),
                    (PickPlaceSides::Both, "both"),
                ] {
                    let mut writer = PickPlaceCsvWriter::new(&data, "2.1.1");
                    writer.set_board_sides(side);
                    writer.set_include_metadata_comment(false);
                    let csv = writer.generate_csv().unwrap().to_csv_string().unwrap();
                    expected.insert(format!("{key}_{board_index}_{variant_index}.csv"), csv);
                }
            }
        }
        if let Some(cli) = &cli {
            let mut job = "(type pnp) (board all) (variant all) (comment false) (tht true) (smt true) (mixed true) (fiducial true) (other true)".to_owned();
            for key in ["top", "bottom", "both"] {
                job.push_str(&format!(" ({key} (create true) (output \"{key}_{{{{BOARD_INDEX}}}}_{{{{VARIANT_INDEX}}}}.csv\"))"));
            }
            let actual = oracle_outputs(cli, &directory, &filename, &job);
            assert_eq!(actual, expected, "{}", directory.display());
        }
    }
}
