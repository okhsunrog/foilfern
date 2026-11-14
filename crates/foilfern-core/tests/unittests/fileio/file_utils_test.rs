//! Port of tests/unittests/core/fileio/fileutilstest.cpp.

use foilfern_core::fileio::{FilePath, file_utils};

use crate::helpers::TempDir;

struct Fixture {
    _tmp: TempDir,
    root: FilePath,
    root_file: FilePath,
    root_file_hidden: FilePath,
    subdir: FilePath,
    subdir_file: FilePath,
    subdir_subdir_file: FilePath,
    subdir_subdir_file_hidden: FilePath,
    root_file_missing: FilePath,
    root_file_copy: FilePath,
    subdir_copy: FilePath,
    subdir_copy_file: FilePath,
    subdir_copy_subdir_file: FilePath,
    subdir_copy_subdir_file_hidden: FilePath,
}

fn set_hidden(_path: &FilePath) {
    #[cfg(windows)]
    {
        // Hidden attribute via `attrib` (no unsafe Win32 calls in tests).
        let status = std::process::Command::new("attrib")
            .arg("+h")
            .arg(_path.to_native())
            .status()
            .unwrap();
        assert!(status.success());
    }
}

fn setup() -> Fixture {
    let tmp = TempDir::new();
    let root = tmp.path().clone();
    let subdir = root.path_to("subdir");
    let subdir_subdir = subdir.path_to("subdir");
    let subdir_copy = root.path_to("subdirCopy");
    let subdir_copy_subdir = subdir_copy.path_to("subdir");
    let f = Fixture {
        root_file: root.path_to("file.txt"),
        root_file_hidden: root.path_to(".hidden.txt"),
        subdir_file: subdir.path_to("file.txt"),
        subdir_subdir_file: subdir_subdir.path_to("file.txt"),
        subdir_subdir_file_hidden: subdir_subdir.path_to(".hidden.txt"),
        root_file_missing: root.path_to("missing.txt"),
        root_file_copy: root.path_to("fileCopy.txt"),
        subdir_copy_file: subdir_copy.path_to("file.txt"),
        subdir_copy_subdir_file: subdir_copy_subdir.path_to("file.txt"),
        subdir_copy_subdir_file_hidden: subdir_copy_subdir.path_to(".hidden.txt"),
        subdir_copy,
        subdir,
        root,
        _tmp: tmp,
    };
    std::fs::create_dir_all(&subdir_subdir).unwrap();
    std::fs::write(&f.root_file, "test\n").unwrap();
    std::fs::write(&f.root_file_hidden, "hiddenContent\n").unwrap();
    set_hidden(&f.root_file_hidden);
    std::fs::write(&f.subdir_file, "test\n").unwrap();
    std::fs::write(&f.subdir_subdir_file, "test\n").unwrap();
    std::fs::write(&f.subdir_subdir_file_hidden, "hiddenContent\n").unwrap();
    set_hidden(&f.subdir_subdir_file_hidden);
    f
}

fn comparable(paths: &[FilePath]) -> Vec<String> {
    let mut v: Vec<String> = paths.iter().map(|p| p.as_str().to_owned()).collect();
    v.sort();
    v
}

#[test]
fn test_read_existing_file() {
    let f = setup();
    assert_eq!(file_utils::read_file(&f.root_file).unwrap(), b"test\n");
}

#[test]
fn test_read_nonexistent_file_should_throw() {
    let f = setup();
    assert!(file_utils::read_file(&f.root_file_missing).is_err());
}

#[test]
fn test_written_data_should_be_read_back() {
    let f = setup();
    file_utils::write_file(&f.root_file, b"someData\n").unwrap();
    assert_eq!(file_utils::read_file(&f.root_file).unwrap(), b"someData\n");
}

#[test]
fn test_write_creates_parent_directories() {
    let f = setup();
    let fp = f.root.path_to("a/b/c.txt");
    file_utils::write_file(&fp, b"abc").unwrap();
    assert_eq!(file_utils::read_file(&fp).unwrap(), b"abc");
}

#[cfg(unix)]
#[test]
fn test_write_keeps_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let f = setup();
    let perms = std::fs::Permissions::from_mode(0o640);
    std::fs::set_permissions(&f.root_file, perms).unwrap();
    file_utils::write_file(&f.root_file, b"x").unwrap();
    let mode = std::fs::metadata(&f.root_file)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o640);
}

#[test]
fn test_copy_valid_file() {
    let f = setup();
    file_utils::copy_file(&f.root_file, &f.root_file_copy).unwrap();
    assert_eq!(file_utils::read_file(&f.root_file).unwrap(), b"test\n");
    assert_eq!(file_utils::read_file(&f.root_file_copy).unwrap(), b"test\n");
}

#[test]
fn test_copy_nonexisting_file_should_throw() {
    let f = setup();
    assert!(file_utils::copy_file(&f.root_file_missing, &f.root_file_copy).is_err());
}

#[test]
fn test_move_valid_file() {
    let f = setup();
    file_utils::move_path(&f.root_file, &f.root_file_copy).unwrap();
    let p = file_utils::read_file(&f.root_file_copy).unwrap();
    assert!(file_utils::read_file(&f.root_file).is_err());
    assert_eq!(p, b"test\n");
}

#[test]
fn test_move_nonexisting_file_should_throw() {
    let f = setup();
    assert!(file_utils::move_path(&f.root_file_missing, &f.root_file_copy).is_err());
}

#[test]
fn test_remove_valid_file() {
    let f = setup();
    file_utils::remove_file(&f.root_file).unwrap();
    assert!(file_utils::read_file(&f.root_file).is_err());
}

#[test]
fn test_remove_nonexisting_file_should_throw() {
    let f = setup();
    assert!(file_utils::remove_file(&f.root_file_missing).is_err());
}

#[test]
fn test_create_subdir() {
    let f = setup();
    file_utils::make_path(&f.subdir_copy).unwrap();
    assert!(f.subdir_copy.is_existing_dir());
}

#[test]
fn test_recursive_remove_subdir() {
    let f = setup();
    file_utils::remove_dir_recursively(&f.subdir).unwrap();
    assert!(!f.subdir.is_existing_dir());
    assert!(!f.subdir_file.is_existing_file());
}

#[test]
fn test_recursive_copy_subdir() {
    let f = setup();
    file_utils::copy_dir_recursively(&f.subdir, &f.subdir_copy).unwrap();

    // ensure source remain unchanged
    assert!(f.subdir.is_existing_dir());
    assert!(f.subdir_file.is_existing_file());
    assert!(f.subdir_subdir_file.is_existing_file());
    assert!(f.subdir_subdir_file_hidden.is_existing_file());

    // ensure destination is complete copy
    assert!(f.subdir_copy.is_existing_dir());
    assert!(f.subdir_copy_file.is_existing_file());
    assert!(f.subdir_copy_subdir_file.is_existing_file());
    assert!(f.subdir_copy_subdir_file_hidden.is_existing_file());
}

#[test]
fn test_find_directories() {
    let f = setup();
    let actual = file_utils::find_directories(&f.root);
    assert_eq!(
        comparable(&actual),
        comparable(std::slice::from_ref(&f.subdir))
    );
}

#[test]
fn test_get_files_in_directory() {
    let f = setup();
    let actual = file_utils::files_in_directory(&f.root, &[], false, false).unwrap();
    let expected = [f.root_file.clone(), f.root_file_hidden.clone()];
    assert_eq!(comparable(&actual), comparable(&expected));
}

#[test]
fn test_get_files_in_directory_recursive() {
    let f = setup();
    let actual = file_utils::files_in_directory(&f.root, &[], true, false).unwrap();
    let expected = [
        f.root_file.clone(),
        f.root_file_hidden.clone(),
        f.subdir_file.clone(),
        f.subdir_subdir_file.clone(),
        f.subdir_subdir_file_hidden.clone(),
    ];
    assert_eq!(comparable(&actual), comparable(&expected));
}

#[test]
fn test_get_files_in_directory_skip_hidden() {
    let f = setup();
    let actual = file_utils::files_in_directory(&f.root, &[], false, true).unwrap();
    assert_eq!(
        comparable(&actual),
        comparable(std::slice::from_ref(&f.root_file))
    );
}

#[test]
fn test_get_files_in_directory_recursive_skip_hidden() {
    let f = setup();
    let actual = file_utils::files_in_directory(&f.root, &[], true, true).unwrap();
    let expected = [
        f.root_file.clone(),
        f.subdir_file.clone(),
        f.subdir_subdir_file.clone(),
    ];
    assert_eq!(comparable(&actual), comparable(&expected));
}

#[test]
fn test_get_files_in_directory_filtered() {
    let f = setup();
    let actual = file_utils::files_in_directory(&f.root, &["*.txt"], false, false).unwrap();
    let expected = [f.root_file.clone(), f.root_file_hidden.clone()];
    assert_eq!(comparable(&actual), comparable(&expected));
}

#[test]
fn test_get_files_in_directory_recursive_filtered() {
    let f = setup();
    let actual = file_utils::files_in_directory(&f.root, &["*.txt"], true, false).unwrap();
    let expected = [
        f.root_file.clone(),
        f.root_file_hidden.clone(),
        f.subdir_file.clone(),
        f.subdir_subdir_file.clone(),
        f.subdir_subdir_file_hidden.clone(),
    ];
    assert_eq!(comparable(&actual), comparable(&expected));
}

#[test]
fn test_get_files_in_directory_recursive_filtered_skip_hidden() {
    let f = setup();
    let actual = file_utils::files_in_directory(&f.root, &["*.txt"], true, true).unwrap();
    let expected = [
        f.root_file.clone(),
        f.subdir_file.clone(),
        f.subdir_subdir_file.clone(),
    ];
    assert_eq!(comparable(&actual), comparable(&expected));
}

#[test]
fn test_get_files_in_directory_filter_is_case_insensitive() {
    let f = setup();
    let actual = file_utils::files_in_directory(&f.root, &["*.TXT"], false, true).unwrap();
    assert_eq!(
        comparable(&actual),
        comparable(std::slice::from_ref(&f.root_file))
    );
    let actual = file_utils::files_in_directory(&f.root, &["*.md"], false, true).unwrap();
    assert!(actual.is_empty());
}
