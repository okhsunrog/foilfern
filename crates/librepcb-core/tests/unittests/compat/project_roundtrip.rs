//! Opens all current-format projects in the upstream test data and checks
//! that saving them writes byte-identical files.
//!
//! Every file the project writes is compared with the file on disk
//! (`TransactionalFileSystem::check_for_modifications()`); the only files
//! which may differ are the `settings.user.lp` files, which the test data
//! does not contain (upstream creates them on save too). Projects in an
//! older file format must be upgraded by `open()`; the upgraded files are
//! compared with upstream's in `file_format_migration.rs`. Schematics and
//! boards are re-serialized from the ported item types.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use librepcb_core::application::file_format_version;
use librepcb_core::fileio::{
    FilePath, FileSystem, TransactionalDirectory, TransactionalFileSystem,
};
use librepcb_core::project::ProjectLoader;

fn projects_dir() -> PathBuf {
    Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data/projects")
}

/// Returns `Ok(true)` if the project was checked, `Ok(false)` if it has an
/// older file format (and was upgraded).
fn roundtrip(dir: &Path) -> Result<bool, String> {
    let version = std::fs::read(dir.join(".librepcb-project")).map_err(|e| e.to_string())?;
    let is_current =
        version.split(|&b| b == b'\n').next() == Some(file_format_version().to_string().as_bytes());
    let lpp = std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.ends_with(".lpp"))
        .ok_or("no *.lpp file")?;
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(dir).unwrap())
        .map_err(|e| e.to_string())?;
    let directory = TransactionalDirectory::new(Arc::new(fs), "");
    let mut loader = ProjectLoader::new();
    let mut project = loader
        .open(directory, &lpp)
        .map_err(|e| format!("failed to open: {e}"))?;
    if loader.migration_log().is_some() == is_current {
        return Err(format!(
            "file format migration expected: {}, performed: {}",
            !is_current,
            loader.migration_log().is_some()
        ));
    }
    if !is_current {
        // Upgraded, compared with upstream in `file_format_migration.rs`.
        assert!(project.is_ref_index_consistent());
        return Ok(false);
    }
    assert!(project.is_ref_index_consistent());
    project.save().map_err(|e| e.to_string())?;
    let modified = project
        .directory()
        .file_system()
        .check_for_modifications()
        .map_err(|e| e.to_string())?;
    let mut diffs = Vec::new();
    for file in modified {
        if file.ends_with("settings.user.lp") && !dir.join(&file).exists() {
            continue; // Created by save(), not part of the test data.
        }
        let expected = std::fs::read(dir.join(&file)).unwrap_or_default();
        let actual = project.directory().read(&file).unwrap_or_default();
        diffs.push(format!(
            "{file} differs:\n{}\n!=\n{}",
            String::from_utf8_lossy(&expected),
            String::from_utf8_lossy(&actual)
        ));
    }
    if diffs.is_empty() {
        Ok(true)
    } else {
        Err(diffs.join("\n\n"))
    }
}

#[test]
fn project_roundtrip_test_data() {
    let root = projects_dir()
        .canonicalize()
        .expect("upstream test data not found (set LIBREPCB_UPSTREAM_DIR)");
    let mut dirs: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join(".librepcb-project").is_file())
        .collect();
    dirs.sort();

    let (mut checked, mut upgraded) = (Vec::new(), Vec::new());
    let mut failures = Vec::new();
    for dir in &dirs {
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        match roundtrip(dir) {
            Ok(true) => checked.push(name),
            Ok(false) => upgraded.push(name),
            Err(e) => failures.push(format!("{name}: {e}")),
        }
    }
    println!("checked projects: {checked:?}, upgraded (old format): {upgraded:?}");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    assert!(
        checked.len() >= 3,
        "too few current-format projects in the test data"
    );
}
