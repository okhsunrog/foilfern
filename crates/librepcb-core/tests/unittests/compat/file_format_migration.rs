//! Compares the file format migrations with upstream LibrePCB.
//!
//! Every project and library in an older file format in the upstream test
//! data (`tests/data/projects`, `tests/data/libraries`) is copied twice into
//! a temporary directory; one copy is upgraded and saved by the upstream
//! command line interface (`librepcb-cli open-project --save` /
//! `open-library --all --save`), the other one by this crate (the same
//! sequence of `open()`, `save()` and file system saves). Afterwards, both
//! directories must contain byte-identical files. The upstream results are
//! generated on the fly, so they are not stored in the repository.
//!
//! The upstream CLI is taken from the environment variable `LIBREPCB_CLI`,
//! or `librepcb-cli` in `PATH`. Without it, the comparison is skipped (with
//! a message on stderr); the upgrade itself is still covered by the other
//! tests.
//!
//! Only the footer of the migration log (application version and time of
//! the upgrade) is excluded from the comparison.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use librepcb_core::application::file_format_version;
use librepcb_core::fileio::{
    FilePath, TransactionalDirectory, TransactionalFileSystem, file_utils,
};
use librepcb_core::library::cat::{ComponentCategory, PackageCategory};
use librepcb_core::library::cmp::Component;
use librepcb_core::library::dev::Device;
use librepcb_core::library::org::Organization;
use librepcb_core::library::pkg::Package;
use librepcb_core::library::sym::Symbol;
use librepcb_core::library::{Library, LibraryBaseElement};
use librepcb_core::project::ProjectLoader;

fn data_dir() -> PathBuf {
    Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data")
}

/// Returns the upstream CLI command, if available.
fn upstream_cli() -> Option<PathBuf> {
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

/// Runs the upstream CLI with `args`, in English and without GUI.
fn run_cli(cli: &Path, args: &[&str]) {
    let out = Command::new(cli)
        .args(args)
        .env("QT_QPA_PLATFORM", "offscreen")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("LANGUAGE", "C")
        .env_remove("LIBREPCB_UPGRADE_UNSTABLE")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "librepcb-cli {args:?} failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Returns whether the version file `file` contains an older file format.
fn is_old_format(file: &Path) -> bool {
    let content = std::fs::read(file).unwrap();
    content.split(|&b| b == b'\n').next() != Some(file_format_version().to_string().as_bytes())
}

fn open_rw(dir: &Path) -> TransactionalDirectory {
    let fs = TransactionalFileSystem::open_rw(&FilePath::new(dir).unwrap()).unwrap();
    TransactionalDirectory::new(Arc::new(fs), "")
}

/// Upgrades and saves a project like `librepcb-cli open-project --save`.
fn upgrade_project(dir: &Path) {
    let lpp = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .find(|n| n.ends_with(".lpp"))
        .unwrap();
    let mut loader = ProjectLoader::new();
    let mut project = loader.open(open_rw(dir), &lpp).unwrap();
    assert!(loader.migration_log().is_some());
    project.save().unwrap();
    project.directory().file_system().save().unwrap();
}

/// Opens (upgrades) and saves one element like `librepcb-cli open-library
/// --save`.
fn upgrade_element<E: LibraryBaseElement>(dir: &Path) {
    let mut element = E::open(open_rw(dir)).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    element.save().unwrap();
    element.directory().file_system().save().unwrap();
}

/// Upgrades all elements of type `E` of the library in `lib_dir`.
fn upgrade_elements<E: LibraryBaseElement>(lib: &Library, lib_dir: &Path) {
    let mut elements = lib.search_for_elements::<E>();
    elements.sort();
    for element in elements {
        upgrade_element::<E>(&lib_dir.join(element));
    }
}

/// Upgrades and saves a library like `librepcb-cli open-library --all
/// --save`.
fn upgrade_library(dir: &Path) {
    let mut lib = Library::open(open_rw(dir)).unwrap();
    lib.save().unwrap();
    lib.directory().file_system().save().unwrap();
    upgrade_elements::<ComponentCategory>(&lib, dir);
    upgrade_elements::<PackageCategory>(&lib, dir);
    upgrade_elements::<Symbol>(&lib, dir);
    upgrade_elements::<Package>(&lib, dir);
    upgrade_elements::<Component>(&lib, dir);
    upgrade_elements::<Device>(&lib, dir);
    upgrade_elements::<Organization>(&lib, dir);
}

/// Returns all files in `dir` (relative paths, recursively).
fn files(dir: &Path) -> BTreeSet<PathBuf> {
    walk(dir)
        .into_iter()
        .map(|p| p.strip_prefix(dir).unwrap().to_path_buf())
        .collect()
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.extend(walk(&path));
        } else {
            result.push(path);
        }
    }
    result
}

/// Removes the footer (generation time and application version) of a
/// migration log.
fn strip_log_footer(content: &[u8]) -> Vec<u8> {
    String::from_utf8_lossy(content)
        .lines()
        .filter(|l| !l.trim_start().starts_with("<p class=\"footer\">"))
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes()
}

/// Describes the first difference between two files.
fn describe_diff(expected: &[u8], actual: &[u8]) -> String {
    let expected = String::from_utf8_lossy(expected);
    let actual = String::from_utf8_lossy(actual);
    let mut e = expected.lines();
    let mut a = actual.lines();
    for line in 1.. {
        match (e.next(), a.next()) {
            (None, None) => break,
            (el, al) if el != al => {
                return format!(
                    "first difference in line {line}:\n  upstream: {}\n  ours:     {}",
                    el.unwrap_or("<EOF>"),
                    al.unwrap_or("<EOF>")
                );
            }
            _ => {}
        }
    }
    "files differ (line endings)".into()
}

/// Result of comparing two directories.
#[derive(Default)]
struct Comparison {
    identical: usize,
    failures: Vec<String>,
}

fn compare(name: &str, upstream: &Path, ours: &Path, result: &mut Comparison) {
    let upstream_files = files(upstream);
    let our_files = files(ours);
    for file in upstream_files.union(&our_files) {
        let expected = std::fs::read(upstream.join(file)).ok();
        let actual = std::fs::read(ours.join(file)).ok();
        let (expected, actual) = match (expected, actual) {
            (Some(e), Some(a)) if file.starts_with("logs") => {
                (strip_log_footer(&e), strip_log_footer(&a))
            }
            (Some(e), Some(a)) => (e, a),
            (e, _) => {
                let which = if e.is_some() { "ours" } else { "upstream" };
                result
                    .failures
                    .push(format!("{name}/{}: missing in {which}", file.display()));
                continue;
            }
        };
        if expected == actual {
            result.identical += 1;
            continue;
        }
        result.failures.push(format!(
            "{name}/{}: {}",
            file.display(),
            describe_diff(&expected, &actual)
        ));
    }
}

/// Copies `src` into `<tmp>/upstream/<name>` and `<tmp>/ours/<name>`.
fn copy_twice(src: &Path, tmp: &Path, name: &str) -> (PathBuf, PathBuf) {
    let upstream = tmp.join("upstream").join(name);
    let ours = tmp.join("ours").join(name);
    for dest in [&upstream, &ours] {
        file_utils::copy_dir_recursively(
            &FilePath::new(src.canonicalize().unwrap()).unwrap(),
            &FilePath::new(dest).unwrap(),
        )
        .unwrap();
    }
    (upstream, ours)
}

fn sorted_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

#[test]
fn upgrade_like_upstream_cli() {
    let Some(cli) = upstream_cli() else {
        eprintln!(
            "librepcb-cli not found (set LIBREPCB_CLI), skipping the comparison with upstream"
        );
        return;
    };
    let mut tmp = tempfile::Builder::new()
        .prefix("librepcb-migration-")
        .tempdir()
        .unwrap();
    // Set LIBREPCB_KEEP_MIGRATION_OUTPUT=1 to inspect the upgraded files.
    if std::env::var_os("LIBREPCB_KEEP_MIGRATION_OUTPUT").is_some_and(|v| v == "1") {
        tmp.disable_cleanup(true);
        println!("upgraded files are kept in {}", tmp.path().display());
    }
    let mut result = Comparison::default();
    let mut checked = Vec::new();

    for src in sorted_dirs(&data_dir().join("projects")) {
        if !src.join(".librepcb-project").is_file()
            || !is_old_format(&src.join(".librepcb-project"))
        {
            continue;
        }
        let name = format!("projects/{}", src.file_name().unwrap().to_string_lossy());
        let (upstream, ours) = copy_twice(&src, tmp.path(), &name);
        let lpp = std::fs::read_dir(&upstream)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|e| e == "lpp"))
            .unwrap();
        run_cli(&cli, &["open-project", "--save", lpp.to_str().unwrap()]);
        upgrade_project(&ours);
        compare(&name, &upstream, &ours, &mut result);
        checked.push(name);
    }

    for src in sorted_dirs(&data_dir().join("libraries")) {
        if !src.join(".librepcb-lib").is_file() || !is_old_format(&src.join(".librepcb-lib")) {
            continue;
        }
        let name = format!("libraries/{}", src.file_name().unwrap().to_string_lossy());
        let (upstream, ours) = copy_twice(&src, tmp.path(), &name);
        run_cli(
            &cli,
            &[
                "open-library",
                "--all",
                "--save",
                upstream.to_str().unwrap(),
            ],
        );
        upgrade_library(&ours);
        compare(&name, &upstream, &ours, &mut result);
        checked.push(name);
    }

    println!("compared {checked:?}: {} identical files", result.identical);
    assert!(
        result.failures.is_empty(),
        "{} differences to upstream:\n{}",
        result.failures.len(),
        result.failures.join("\n")
    );
    assert!(checked.len() >= 4, "too few old-format projects/libraries");
}
