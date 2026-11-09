//! Opens all current-format libraries and library elements in the upstream
//! test data and checks that saving them yields byte-identical element files
//! and version files.
//!
//! Elements are found by the root node of their `.lp` file; elements in an
//! older file format (version file != current file format) must be upgraded
//! by `open()`; the upgraded files are compared with upstream's in
//! `file_format_migration.rs`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use librepcb_core::application::file_format_version;
use librepcb_core::fileio::{
    FilePath, FileSystem, TransactionalDirectory, TransactionalFileSystem,
};
use librepcb_core::library::cat::{ComponentCategory, PackageCategory};
use librepcb_core::library::cmp::Component;
use librepcb_core::library::dev::Device;
use librepcb_core::library::org::Organization;
use librepcb_core::library::pkg::Package;
use librepcb_core::library::sym::Symbol;
use librepcb_core::library::{Library, LibraryBaseElement};

/// Upstream test data directory (see `LIBREPCB_UPSTREAM_DIR` in
/// `.cargo/config.toml`).
fn data_dir() -> PathBuf {
    Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data")
}

fn collect_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_files(&path, files);
        } else if path.extension().is_some_and(|e| e == "lp") {
            files.push(path);
        }
    }
}

fn open_dir(dir: &Path) -> TransactionalDirectory {
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(dir).unwrap()).unwrap();
    TransactionalDirectory::new(Arc::new(fs), "")
}

/// Opens the element in `dir`, saves it and compares the files. Returns
/// `Ok(false)` if the element has an older file format (and was upgraded).
fn roundtrip<E: LibraryBaseElement>(dir: &Path) -> Result<bool, String> {
    let version_file = format!(".librepcb-{}", E::SHORT_ELEMENT_NAME);
    let element_file = format!("{}.lp", E::LONG_ELEMENT_NAME);
    let version = std::fs::read(dir.join(&version_file)).map_err(|e| e.to_string())?;
    let is_current =
        version.split(|&b| b == b'\n').next() == Some(file_format_version().to_string().as_bytes());
    let mut element = E::open(open_dir(dir)).map_err(|e| format!("failed to open: {e}"))?;
    if !is_current {
        // Upgraded, compared with upstream in `file_format_migration.rs`.
        let version = element
            .directory()
            .read(&version_file)
            .map_err(|e| e.to_string())?;
        if !version.starts_with(format!("{}\n", file_format_version()).as_bytes()) {
            return Err("element was not upgraded".into());
        }
        return Ok(false);
    }
    element.save().map_err(|e| e.to_string())?;
    for file in [element_file, version_file] {
        let expected = std::fs::read(dir.join(&file)).map_err(|e| e.to_string())?;
        let actual = element.directory().read(&file).map_err(|e| e.to_string())?;
        if actual != expected {
            return Err(format!(
                "{file} differs:\n{}\n!=\n{}",
                String::from_utf8_lossy(&expected),
                String::from_utf8_lossy(&actual)
            ));
        }
    }
    // Saving must not modify any other file (e.g. icons).
    let modified = element
        .directory()
        .file_system()
        .check_for_modifications()
        .map_err(|e| e.to_string())?;
    if !modified.is_empty() {
        return Err(format!("modified files: {modified:?}"));
    }
    Ok(true)
}

#[test]
fn library_roundtrip_test_data() {
    let root = data_dir()
        .canonicalize()
        .expect("upstream test data not found (set LIBREPCB_UPSTREAM_DIR)");
    let mut files = Vec::new();
    collect_files(&root, &mut files);

    let mut counts: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let mut failures = Vec::new();
    for file in &files {
        let content = std::fs::read(file).unwrap();
        let Some(root_name) = content
            .strip_prefix(b"(librepcb_")
            .and_then(|rest| rest.split(|&b| b == b' ').next())
        else {
            continue;
        };
        let dir = file.parent().unwrap();
        let (name, result) = match root_name {
            b"library" => ("library", roundtrip::<Library>(dir)),
            b"component_category" => ("component_category", roundtrip::<ComponentCategory>(dir)),
            b"package_category" => ("package_category", roundtrip::<PackageCategory>(dir)),
            b"symbol" => ("symbol", roundtrip::<Symbol>(dir)),
            b"package" => ("package", roundtrip::<Package>(dir)),
            b"component" => ("component", roundtrip::<Component>(dir)),
            b"device" => ("device", roundtrip::<Device>(dir)),
            b"organization" => ("organization", roundtrip::<Organization>(dir)),
            _ => continue,
        };
        let entry = counts.entry(name).or_default();
        match result {
            Ok(true) => entry.0 += 1,
            Ok(false) => entry.1 += 1,
            Err(e) => failures.push(format!(
                "{}: {e}",
                file.strip_prefix(&root).unwrap().display()
            )),
        }
    }

    println!("checked elements (current format, upgraded old format): {counts:?}");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    for name in [
        "library",
        "component_category",
        "package_category",
        "symbol",
        "package",
        "component",
        "device",
        "organization",
    ] {
        assert!(
            counts.get(name).is_some_and(|c| c.0 > 0),
            "no current-format {name} found in test data"
        );
    }
}
