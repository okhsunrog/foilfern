//! Test helpers (replacing `Application::getRandomTempPath()` and the
//! upstream `dummy-binary` test executable).

use std::path::PathBuf;
use std::process::{Child, Command};

use foilfern_core::fileio::FilePath;

/// Returns the upstream test data directory (see `LIBREPCB_UPSTREAM_DIR` in
/// `.cargo/config.toml`).
pub fn test_data_dir() -> PathBuf {
    PathBuf::from(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data")
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
            .prefix("librepcb-test-")
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

/// Environment variable which makes [`dummy_binary`](crate::dummy_binary)
/// sleep.
pub const DUMMY_BINARY_ENV: &str = "LIBREPCB_UNITTESTS_DUMMY_BINARY";

/// Starts another long running process (upstream: `dummy-binary`), which
/// is this test executable running only the ignored `dummy_binary` test.
///
/// Its process name is the one of this executable (see
/// [`own_process_name()`]).
pub fn spawn_dummy_process() -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "dummy_binary", "--test-threads=1"])
        .env(DUMMY_BINARY_ENV, "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap()
}

/// Kills and reaps a process started with [`spawn_dummy_process()`].
pub fn kill(mut child: Child) {
    child.kill().unwrap();
    child.wait().unwrap();
}

/// Returns the expected process name of this test executable.
pub fn own_process_name() -> String {
    let exe = std::env::current_exe().unwrap();
    let name = if cfg!(windows) {
        exe.file_stem()
    } else {
        exe.file_name()
    };
    name.unwrap().to_string_lossy().into_owned()
}
