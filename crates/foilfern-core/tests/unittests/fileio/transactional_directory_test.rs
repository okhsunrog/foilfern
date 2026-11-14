//! Port of tests/unittests/core/fileio/transactionaldirectorytest.cpp.

use std::sync::Arc;

use foilfern_core::fileio::{
    FileSystem, TransactionalDirectory, TransactionalFileSystem, file_utils,
};

use crate::helpers::TempDir;

struct Fixture {
    _tmp: TempDir,
    fs: Arc<TransactionalFileSystem>,
    empty_fs: Arc<TransactionalFileSystem>,
}

fn setup() -> Fixture {
    let tmp = TempDir::new();
    // Open in read-only mode to avoid creating a ".lock" file which would
    // influence the tests.
    let fs = TransactionalFileSystem::open_ro(&tmp.path().path_to("fs")).unwrap();
    fs.write("a.txt", b"a").unwrap();
    fs.write("a/b.txt", b"b").unwrap();
    fs.write("a/b/c.txt", b"c").unwrap();
    fs.write("a/b/c/d.txt", b"d").unwrap();
    let empty_fs = TransactionalFileSystem::open_ro(&tmp.path().path_to("empty")).unwrap();
    Fixture {
        _tmp: tmp,
        fs: Arc::new(fs),
        empty_fs: Arc::new(empty_fs),
    }
}

fn dir(fs: &Arc<TransactionalFileSystem>, path: &str) -> TransactionalDirectory {
    TransactionalDirectory::new(Arc::clone(fs), path)
}

fn list(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn test_default_constructor_creates_temp_fs() {
    let dir = TransactionalDirectory::new_temporary().unwrap();
    assert!(
        dir.file_system()
            .abs_path("")
            .unwrap()
            .is_located_in_dir(file_utils::temp_dir())
    );
}

#[test]
fn test_default_constructor_creates_empty_fs() {
    let dir = TransactionalDirectory::new_temporary().unwrap();
    assert!(dir.dirs("").is_empty());
    assert!(dir.files("").is_empty());
}

#[test]
fn test_copy_constructor_with_default_path() {
    let f = setup();
    let mut d = dir(&f.fs, "foo");
    let copy = d.subdir("");
    assert!(Arc::ptr_eq(&f.fs, copy.file_system()));
    assert_eq!(copy.path(), "foo");
}

#[test]
fn test_copy_constructor_with_path() {
    let f = setup();
    let mut d = dir(&f.fs, "foo");
    let copy = d.subdir("bar");
    assert!(Arc::ptr_eq(&f.fs, copy.file_system()));
    assert_eq!(copy.path(), "foo/bar");
}

#[test]
fn test_constructor_with_default_path() {
    let f = setup();
    let d = dir(&f.fs, "");
    assert!(Arc::ptr_eq(&f.fs, d.file_system()));
    assert_eq!(d.path(), "");
}

#[test]
fn test_constructor_with_path() {
    let f = setup();
    let d = dir(&f.fs, "foo");
    assert!(Arc::ptr_eq(&f.fs, d.file_system()));
    assert_eq!(d.path(), "foo");
}

#[test]
fn test_constructor_removes_trailing_slashes() {
    let f = setup();
    let d = dir(&f.fs, "foo///");
    assert!(Arc::ptr_eq(&f.fs, d.file_system()));
    assert_eq!(d.path(), "foo");
}

#[test]
fn test_get_abs_path_in_root() {
    let f = setup();
    assert_eq!(dir(&f.fs, "").abs_path(""), f.fs.abs_path(""));
}

#[test]
fn test_get_abs_path_in_root_path() {
    let f = setup();
    assert_eq!(dir(&f.fs, "").abs_path("hello"), f.fs.abs_path("hello"));
}

#[test]
fn test_get_abs_path_in_subdir() {
    let f = setup();
    assert_eq!(dir(&f.fs, "foo bar").abs_path(""), f.fs.abs_path("foo bar"));
}

#[test]
fn test_get_abs_path_in_subdir_path() {
    let f = setup();
    assert_eq!(
        dir(&f.fs, "foo bar").abs_path("hello"),
        f.fs.abs_path("foo bar/hello")
    );
}

#[test]
fn test_get_dirs() {
    let f = setup();
    assert_eq!(dir(&f.fs, "").dirs(""), list(&["a"]));
    assert_eq!(dir(&f.fs, "").dirs("a"), list(&["b"]));
    assert_eq!(dir(&f.fs, "a").dirs(""), list(&["b"]));
    assert_eq!(dir(&f.fs, "a").dirs("b"), list(&["c"]));
}

#[test]
fn test_get_files() {
    let f = setup();
    assert_eq!(dir(&f.fs, "").files(""), list(&["a.txt"]));
    assert_eq!(dir(&f.fs, "").files("a"), list(&["b.txt"]));
    assert_eq!(dir(&f.fs, "a").files(""), list(&["b.txt"]));
    assert_eq!(dir(&f.fs, "a").files("b"), list(&["c.txt"]));
}

#[test]
fn test_file_exists() {
    let f = setup();
    assert!(dir(&f.fs, "").file_exists("a.txt"));
    assert!(!dir(&f.fs, "").file_exists("b.txt"));
    assert!(dir(&f.fs, "").file_exists("a/b.txt"));
    assert!(!dir(&f.fs, "").file_exists("a/c.txt"));
    assert!(dir(&f.fs, "a").file_exists("b.txt"));
    assert!(!dir(&f.fs, "a").file_exists("c.txt"));
    assert!(dir(&f.fs, "a").file_exists("b/c.txt"));
    assert!(!dir(&f.fs, "a").file_exists("b/d.txt"));
}

#[test]
fn test_read() {
    let f = setup();
    assert_eq!(dir(&f.fs, "").read("a.txt").unwrap(), b"a");
    assert!(dir(&f.fs, "").read("b.txt").is_err());
    assert_eq!(dir(&f.fs, "").read("a/b.txt").unwrap(), b"b");
    assert!(dir(&f.fs, "").read("a/c.txt").is_err());
    assert_eq!(dir(&f.fs, "a").read("b.txt").unwrap(), b"b");
    assert!(dir(&f.fs, "a").read("c.txt").is_err());
    assert_eq!(dir(&f.fs, "a").read("b/c.txt").unwrap(), b"c");
    assert!(dir(&f.fs, "a").read("b/d.txt").is_err());
}

#[test]
fn test_read_if_exists_existing() {
    let f = setup();
    let d = dir(&f.fs, "");
    assert_eq!(
        d.read_if_exists("a.txt").unwrap(),
        Some(d.read("a.txt").unwrap())
    );
}

#[test]
fn test_read_if_exists_non_existent() {
    let f = setup();
    assert_eq!(dir(&f.fs, "").read_if_exists("b.txt").unwrap(), None);
}

#[test]
fn test_write() {
    let f = setup();
    dir(&f.fs, "").write("a.txt", b"foo1").unwrap();
    assert_eq!(f.fs.read("a.txt").unwrap(), b"foo1");
    dir(&f.fs, "").write("a/b.txt", b"foo2").unwrap();
    assert_eq!(f.fs.read("a/b.txt").unwrap(), b"foo2");
    dir(&f.fs, "a").write("b.txt", b"foo3").unwrap();
    assert_eq!(f.fs.read("a/b.txt").unwrap(), b"foo3");
    dir(&f.fs, "a").write("b/c.txt", b"foo4").unwrap();
    assert_eq!(f.fs.read("a/b/c.txt").unwrap(), b"foo4");
}

#[test]
fn test_rename_file() {
    let f = setup();
    dir(&f.fs, "a").rename_file("b.txt", "x.txt").unwrap();
    assert!(!f.fs.file_exists("a/b.txt"));
    assert_eq!(f.fs.read("a/x.txt").unwrap(), b"b");
}

#[test]
fn test_remove_file() {
    for (root, path, fs_path) in [
        ("", "a.txt", "a.txt"),
        ("", "a/b.txt", "a/b.txt"),
        ("a", "b.txt", "a/b.txt"),
        ("a", "b/c.txt", "a/b/c.txt"),
    ] {
        let f = setup();
        let mut d = dir(&f.fs, root);
        assert!(d.file_exists(path));
        assert!(f.fs.file_exists(fs_path));
        d.remove_file(path).unwrap();
        assert!(!d.file_exists(path));
        assert!(!f.fs.file_exists(fs_path));
    }
}

#[test]
fn test_remove_dir_recursively_in_root() {
    let f = setup();
    let mut d = dir(&f.fs, "");
    assert_eq!(d.dirs(""), list(&["a"]));
    assert_eq!(f.fs.dirs(""), list(&["a"]));
    d.remove_dir_recursively("").unwrap();
    assert!(d.dirs("").is_empty());
    assert!(d.files("").is_empty());
    assert!(f.fs.dirs("").is_empty());
    assert!(f.fs.files("").is_empty());
}

#[test]
fn test_remove_dir_recursively_in_root_path() {
    let f = setup();
    let mut d = dir(&f.fs, "");
    assert_eq!(d.dirs(""), list(&["a"]));
    assert_eq!(f.fs.dirs(""), list(&["a"]));
    d.remove_dir_recursively("a").unwrap();
    assert!(d.dirs("").is_empty());
    assert!(f.fs.dirs("").is_empty());
    assert_eq!(d.files(""), list(&["a.txt"]));
    assert_eq!(f.fs.files(""), list(&["a.txt"]));
}

#[test]
fn test_remove_dir_recursively_in_subdir() {
    let f = setup();
    let mut d = dir(&f.fs, "a");
    assert_eq!(d.dirs(""), list(&["b"]));
    assert_eq!(f.fs.dirs(""), list(&["a"]));
    d.remove_dir_recursively("").unwrap();
    assert!(d.dirs("").is_empty());
    assert!(f.fs.dirs("").is_empty());
    assert!(d.files("").is_empty());
    assert_eq!(f.fs.files(""), list(&["a.txt"]));
}

#[test]
fn test_remove_dir_recursively_in_subdir_path() {
    let f = setup();
    let mut d = dir(&f.fs, "a");
    assert_eq!(d.dirs(""), list(&["b"]));
    assert_eq!(f.fs.dirs(""), list(&["a"]));
    assert_eq!(f.fs.dirs("a"), list(&["b"]));
    d.remove_dir_recursively("b").unwrap();
    assert!(d.dirs("").is_empty());
    assert_eq!(f.fs.dirs(""), list(&["a"]));
    assert!(f.fs.dirs("a").is_empty());
    assert_eq!(d.files(""), list(&["b.txt"]));
    assert_eq!(f.fs.files("a"), list(&["b.txt"]));
    assert!(d.files("b").is_empty());
    assert!(f.fs.files("a/b").is_empty());
    assert_eq!(f.fs.files(""), list(&["a.txt"]));
}

/// Expected state of the source file system after copy/save operations.
fn assert_source_unchanged(fs: &TransactionalFileSystem) {
    assert_eq!(fs.dirs(""), list(&["a"]));
    assert_eq!(fs.files(""), list(&["a.txt"]));
    assert_eq!(fs.dirs("a"), list(&["b"]));
    assert_eq!(fs.files("a"), list(&["b.txt"]));
}

/// Expected state of the destination file system, for source/destination
/// directories "" or "a".
fn assert_destination(fs: &TransactionalFileSystem, src: &str, dst: &str) {
    match (src, dst) {
        ("", "") => {
            assert_eq!(fs.dirs(""), list(&["a"]));
            assert_eq!(fs.files(""), list(&["a.txt"]));
            assert_eq!(fs.files("a"), list(&["b.txt"]));
        }
        ("", "a") => {
            assert_eq!(fs.dirs(""), list(&["a"]));
            assert_eq!(fs.dirs("a"), list(&["a"]));
            assert!(fs.files("").is_empty());
            assert_eq!(fs.files("a"), list(&["a.txt"]));
            assert_eq!(fs.files("a/a"), list(&["b.txt"]));
        }
        ("a", "") => {
            assert_eq!(fs.dirs(""), list(&["b"]));
            assert_eq!(fs.files(""), list(&["b.txt"]));
            assert_eq!(fs.files("b"), list(&["c.txt"]));
        }
        ("a", "a") => {
            assert_eq!(fs.dirs(""), list(&["a"]));
            assert!(fs.files("").is_empty());
            assert_eq!(fs.dirs("a"), list(&["b"]));
            assert_eq!(fs.files("a"), list(&["b.txt"]));
            assert_eq!(fs.dirs("a/b"), list(&["c"]));
            assert_eq!(fs.files("a/b"), list(&["c.txt"]));
        }
        _ => unreachable!(),
    }
}

const SRC_DST: [(&str, &str); 4] = [("", ""), ("", "a"), ("a", ""), ("a", "a")];

#[test]
fn test_copy_to() {
    for (src_path, dst_path) in SRC_DST {
        let f = setup();
        let src = dir(&f.fs, src_path);
        let mut dst = dir(&f.empty_fs, dst_path);
        src.copy_to(&mut dst).unwrap();
        assert!(Arc::ptr_eq(&f.fs, src.file_system()));
        assert_eq!(src.path(), src_path);
        assert_source_unchanged(&f.fs);
        assert_destination(&f.empty_fs, src_path, dst_path);
    }
}

#[test]
fn test_save_to() {
    for (src_path, dst_path) in SRC_DST {
        let f = setup();
        let mut src = dir(&f.fs, src_path);
        let mut dst = dir(&f.empty_fs, dst_path);
        src.save_to(&mut dst).unwrap();
        assert!(Arc::ptr_eq(&f.empty_fs, src.file_system()));
        assert_eq!(src.path(), dst_path);
        assert_source_unchanged(&f.fs);
        assert_destination(&f.empty_fs, src_path, dst_path);
    }
}

#[test]
fn test_move_to() {
    for (src_path, dst_path) in SRC_DST {
        let f = setup();
        let mut src = dir(&f.fs, src_path);
        let mut dst = dir(&f.empty_fs, dst_path);
        src.move_to(&mut dst).unwrap();
        assert!(Arc::ptr_eq(&f.empty_fs, src.file_system()));
        assert_eq!(src.path(), dst_path);
        assert!(f.fs.dirs("").is_empty());
        if src_path.is_empty() {
            assert!(f.fs.files("").is_empty());
        } else {
            assert_eq!(f.fs.files(""), list(&["a.txt"]));
        }
        assert_destination(&f.empty_fs, src_path, dst_path);
    }
}
