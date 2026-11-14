//! Ports of tests/unittests/core/library/**, plus tests of the library
//! element checks.
//!
//! The upstream `testUpgradeV01` tests share [`assert_upgrade_v01()`].

mod check_test;
mod cmp;
mod cmpcat;
mod dev;
mod library_base_element_test;
mod library_test;
mod pkg;
mod pkgcat;
mod sym;

use std::path::PathBuf;
use std::sync::Arc;

use foilfern_core::application::file_format_version;
use foilfern_core::fileio::{
    FilePath, TransactionalDirectory, TransactionalFileSystem, file_utils,
};
use foilfern_core::library::LibraryBaseElement;

use crate::helpers::TempDir;

/// Returns the upstream test data directory (see `LIBREPCB_UPSTREAM_DIR` in
/// `.cargo/config.toml`).
pub fn data_dir() -> PathBuf {
    PathBuf::from(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data")
}

/// Returns `path` (relative to the test data directory) as file path.
pub fn data_path(path: &str) -> FilePath {
    FilePath::new(data_dir().join(path).canonicalize().unwrap()).unwrap()
}

/// Opens `path` as transactional directory.
pub fn open_dir(path: &FilePath, writable: bool) -> TransactionalDirectory {
    let fs = if writable {
        TransactionalFileSystem::open_rw(path)
    } else {
        TransactionalFileSystem::open_ro(path)
    };
    TransactionalDirectory::new(Arc::new(fs.unwrap()), "")
}

/// Copies the test data directory `src` to `dest_name` in a new temporary
/// directory.
pub fn copy_to_temp(src: &str, dest_name: &str) -> (TempDir, FilePath) {
    let tmp = TempDir::new();
    let dest = tmp.path().path_to(dest_name);
    file_utils::copy_dir_recursively(&data_path(src), &dest).unwrap();
    (tmp, dest)
}

/// Upstream `testUpgradeV01`: copy the v0.1 element `src` into a temporary
/// directory named `dest_name`, open (upgrade), save and re-open it.
pub fn assert_upgrade_v01<E: LibraryBaseElement>(src: &str, dest_name: &str) {
    // Copy into temporary directory.
    let (_tmp, dir) = copy_to_temp(src, dest_name);
    let version_file = dir.path_to(&format!(".librepcb-{}", E::SHORT_ELEMENT_NAME));

    // Open/upgrade/close.
    assert!(
        file_utils::read_file(&version_file)
            .unwrap()
            .starts_with(b"0.1\n")
    );
    {
        let mut obj = E::open(open_dir(&dir, true)).unwrap();
        obj.save().unwrap();
        obj.directory().file_system().save().unwrap();
    }

    // Re-open.
    assert!(
        file_utils::read_file(&version_file)
            .unwrap()
            .starts_with(format!("{}\n", file_format_version()).as_bytes())
    );
    E::open(open_dir(&dir, true)).unwrap();
}
