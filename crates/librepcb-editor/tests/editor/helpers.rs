//! Shared helpers of the editor tests.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::project::{Project, ProjectLoader};
use librepcb_core::types::Point;
use librepcb_editor::{DirectoryLibrarySource, ProjectEditor};

/// Upstream test data directory.
pub fn test_data_dir() -> PathBuf {
    Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data")
}

/// UUIDs of elements of the upstream "Populated Library".
pub mod lib {
    use librepcb_core::types::Uuid;

    fn uuid(s: &str) -> Uuid {
        s.parse().expect("valid UUID")
    }

    /// Component "Resistor".
    pub fn resistor() -> Uuid {
        uuid("ef80cd5e-2689-47ee-8888-31d04fc99174")
    }
    /// Device "R-0805".
    pub fn r0805() -> Uuid {
        uuid("078650d3-483c-4b9e-a848-b14f1aad2edc")
    }
    /// Device "R-0603".
    pub fn r0603() -> Uuid {
        uuid("483a71eb-318e-448e-82ff-f02efc4821aa")
    }
    /// Component "Capacitor Bipolar".
    pub fn capacitor() -> Uuid {
        uuid("d167e0e3-6a92-4b76-b013-77b9c230e5f1")
    }
    /// Device "C-0805".
    pub fn c0805() -> Uuid {
        uuid("c139e505-592b-46ba-bdf2-acb7383ea0cd")
    }
    /// Component "Supply VCC" (forced net name `{{VALUE}}`).
    pub fn supply_vcc() -> Uuid {
        uuid("58c3c6cd-11eb-4557-aa3f-d3e05874afde")
    }
    /// Component "Supply GND" (forced net name `{{VALUE}}`).
    pub fn supply_gnd() -> Uuid {
        uuid("8076f6be-bfab-4fc1-9772-5d54465dd7e1")
    }
}

/// Returns a library element source over the upstream "Populated Library".
pub fn library_source() -> DirectoryLibrarySource {
    let dir = FilePath::new(test_data_dir().join("libraries/Populated Library.lplib"))
        .expect("absolute path");
    DirectoryLibrarySource::from_libraries([&dir]).expect("library scan")
}

/// Creates a new project in `dir` (opened read-write).
pub fn create_project(dir: &Path) -> Project {
    let fs = TransactionalFileSystem::open_rw(&FilePath::new(dir).expect("absolute path"))
        .expect("open file system");
    let directory = TransactionalDirectory::new(Arc::new(fs), "");
    let fonts =
        FilePath::new(Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("share/librepcb/fontobene"))
            .expect("absolute path");
    librepcb_editor::create_project(directory, "test.lpp", Some(&fonts)).expect("create project")
}

/// Creates an editor for a new project in `dir`.
pub fn create_editor(dir: &Path) -> ProjectEditor {
    ProjectEditor::with_source(create_project(dir), Arc::new(library_source()))
}

/// Opens the project in `dir` read-only with our loader.
pub fn open_project(dir: &Path) -> Project {
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(dir).expect("absolute path"))
        .expect("open file system");
    let directory = TransactionalDirectory::new(Arc::new(fs), "");
    ProjectLoader::new()
        .open(directory, "test.lpp")
        .expect("open project")
}

/// Returns all files below `dir` (except the lock file) with their content.
pub fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn collect(root: &Path, dir: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .expect("read dir")
            .map(|e| e.expect("dir entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                collect(root, &path, files);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .expect("below root")
                    .to_string_lossy()
                    .replace('\\', "/");
                if rel != ".lock" {
                    files.insert(rel, std::fs::read(&path).expect("read file"));
                }
            }
        }
    }
    let mut files = BTreeMap::new();
    collect(dir, dir, &mut files);
    files
}

/// Describes the differences between two snapshots.
pub fn diff(a: &BTreeMap<String, Vec<u8>>, b: &BTreeMap<String, Vec<u8>>) -> String {
    let mut out = String::new();
    for (file, content) in a {
        match b.get(file) {
            None => out += &format!("only in first: {file}\n"),
            Some(other) if other != content => {
                out += &format!(
                    "{file} differs:\n{}\n!=\n{}\n",
                    String::from_utf8_lossy(content),
                    String::from_utf8_lossy(other)
                );
            }
            _ => {}
        }
    }
    for file in b.keys() {
        if !a.contains_key(file) {
            out += &format!("only in second: {file}\n");
        }
    }
    out
}

/// Returns a point in millimeters.
pub fn mm(x: f64, y: f64) -> Point {
    Point::from_mm(x, y).expect("valid point")
}

/// Runs the official `librepcb-cli open-project --erc` on a copy of the
/// project directory. Returns `None` if the CLI is not available,
/// otherwise the exit status and the output.
pub fn run_cli_erc(dir: &Path) -> Option<(bool, String)> {
    run_cli(dir, &["--erc"])
}

/// Runs the official `librepcb-cli open-project <args>` on a copy of the
/// project directory (the copy is kept in a temporary directory which is
/// removed afterwards). Returns `None` if the CLI is not available,
/// otherwise the exit status and the output.
pub fn run_cli(dir: &Path, args: &[&str]) -> Option<(bool, String)> {
    let cli = std::env::var_os("LIBREPCB_CLI").unwrap_or_else(|| "librepcb-cli".into());
    let copy = tempfile::tempdir().expect("temp dir");
    copy_dir(dir, copy.path());
    let output = std::process::Command::new(cli)
        .env("LIBREPCB_SUPPRESS_DEPRECATION_WARNINGS", "1")
        .arg("open-project")
        .args(args)
        .arg(copy.path().join("test.lpp"))
        .output()
        .ok()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Some((output.status.success(), text))
}

/// Copies a directory recursively (without the lock file).
pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create dir");
    for entry in std::fs::read_dir(from).expect("read dir") {
        let path = entry.expect("dir entry").path();
        let name = path.file_name().expect("file name");
        if name == ".lock" {
            continue;
        }
        if path.is_dir() {
            copy_dir(&path, &to.join(name));
        } else {
            std::fs::copy(&path, to.join(name)).expect("copy file");
        }
    }
}
