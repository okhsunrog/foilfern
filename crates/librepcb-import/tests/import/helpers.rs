//! Test helpers.

use std::path::PathBuf;

use librepcb_core::fileio::FilePath;

/// Returns the upstream test data directory (see `LIBREPCB_UPSTREAM_DIR` in
/// `.cargo/config.toml`).
pub fn test_data_dir() -> PathBuf {
    PathBuf::from(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data")
}

/// Returns the path of a file in `tests/data/unittests/<dir>`.
pub fn unittest_data(dir: &str, file: &str) -> FilePath {
    FilePath::new(test_data_dir().join("unittests").join(dir).join(file)).unwrap()
}

/// Temporary directory which is removed on drop.
pub struct TempDir {
    _dir: tempfile::TempDir,
    path: FilePath,
}

impl TempDir {
    /// Creates a new, empty temporary directory.
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("librepcb-import-test-")
            .tempdir()
            .unwrap();
        let path = FilePath::new(dir.path()).unwrap();
        Self { _dir: dir, path }
    }

    /// Returns the path of the directory.
    pub fn path(&self) -> &FilePath {
        &self.path
    }
}

/// Returns the upstream `librepcb-cli` (from `LIBREPCB_CLI` or the `PATH`),
/// if available.
pub fn upstream_cli() -> Option<PathBuf> {
    if let Some(cli) = std::env::var_os("LIBREPCB_CLI") {
        return Some(PathBuf::from(cli));
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|p| p.join("librepcb-cli"))
            .find(|p| p.is_file())
    })
}

/// Creates an empty library `<tmp>/Test.lplib` and returns its path.
pub fn create_library(tmp: &TempDir) -> FilePath {
    use std::sync::Arc;

    use librepcb_core::fileio::{TransactionalDirectory, TransactionalFileSystem};
    use librepcb_core::library::{BaseMetadata, Library, LibraryBaseElement};
    use librepcb_core::types::{ElementName, Uuid};

    let path = tmp.path().path_to("Test.lplib");
    let mut lib = Library::new(BaseMetadata::new(
        Uuid::new_random(),
        "0.1".parse().unwrap(),
        "Test",
        chrono::Utc::now(),
        ElementName::new("Test").unwrap(),
        "",
        "",
    ))
    .unwrap();
    let fs = Arc::new(TransactionalFileSystem::open_rw(&path).unwrap());
    let mut dir = TransactionalDirectory::new(Arc::clone(&fs), "");
    lib.save_to(&mut dir).unwrap();
    fs.save().unwrap();
    path
}

/// Opens all elements of a library with `librepcb-core` and returns the
/// number of opened elements per type directory (e.g. `"sym"`).
pub fn open_all_elements(lib: &FilePath) -> std::collections::BTreeMap<String, usize> {
    use std::sync::Arc;

    use librepcb_core::fileio::{TransactionalDirectory, TransactionalFileSystem};
    use librepcb_core::library::LibraryBaseElement;
    use librepcb_core::library::cat::{ComponentCategory, PackageCategory};
    use librepcb_core::library::cmp::Component;
    use librepcb_core::library::dev::Device;
    use librepcb_core::library::pkg::Package;
    use librepcb_core::library::sym::Symbol;

    let mut counts = std::collections::BTreeMap::new();
    for kind in ["cmpcat", "pkgcat", "sym", "pkg", "cmp", "dev"] {
        let Ok(entries) = std::fs::read_dir(lib.path_to(kind).as_path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let fp = FilePath::new(entry.path()).unwrap();
            let dir = TransactionalDirectory::new(
                Arc::new(TransactionalFileSystem::open_ro(&fp).unwrap()),
                "",
            );
            let ok = match kind {
                "cmpcat" => ComponentCategory::open(dir).map(|_| ()),
                "pkgcat" => PackageCategory::open(dir).map(|_| ()),
                "sym" => Symbol::open(dir).map(|_| ()),
                "pkg" => Package::open(dir).map(|_| ()),
                "cmp" => Component::open(dir).map(|_| ()),
                _ => Device::open(dir).map(|_| ()),
            };
            ok.unwrap_or_else(|e| panic!("failed to open {fp}: {e}"));
            *counts.entry(kind.to_owned()).or_default() += 1;
        }
    }
    counts
}

/// Runs `librepcb-cli open-library --all --strict` on a library (checks that
/// upstream reads all elements and that they are canonical, i.e. upstream
/// would write the same bytes). Returns `false` if the CLI is not available.
pub fn check_library_with_upstream_cli(lib: &FilePath) -> bool {
    let Some(cli) = upstream_cli() else {
        eprintln!("librepcb-cli not found, skipping upstream comparison");
        return false;
    };
    let output = std::process::Command::new(cli)
        .args(["open-library", "--all", "--strict"])
        .arg(lib.as_path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "librepcb-cli failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}
