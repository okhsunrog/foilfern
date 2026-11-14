//! Port of tests/unittests/core/fileio/zipwriterziparchivetest.cpp.

use foilfern_core::fileio::{FilePath, ZipArchive, ZipWriter, file_utils};

use crate::helpers::TempDir;

fn setup() -> (TempDir, FilePath) {
    let tmp = TempDir::new();
    let zip = tmp.path().path_to("test file.zip");
    (tmp, zip)
}

#[test]
fn test_in_memory() {
    let mut w = ZipWriter::new_in_memory();
    w.write_file("test dir/file 1", b"a", 0o644).unwrap();
    w.write_file("test dir/file 2", b"b", 0o644).unwrap();
    let data = w.finish().unwrap();

    let mut a = ZipArchive::from_bytes(data).unwrap();
    assert_eq!(a.len(), 2);
    assert_eq!(a.file_name(0).unwrap(), "test dir/file 1");
    assert_eq!(a.file_name(1).unwrap(), "test dir/file 2");
    assert_eq!(a.read_file(0).unwrap(), b"a");
    assert_eq!(a.read_file(1).unwrap(), b"b");
    assert_eq!(
        a.read_file_by_name("test dir/file 2").unwrap().unwrap(),
        b"b"
    );
    assert_eq!(a.read_file_by_name("nonexistent").unwrap(), None);
}

#[test]
fn test_write_read_empty_archive() {
    let (_tmp, zip_fp) = setup();
    let w = ZipWriter::create(&zip_fp).unwrap();
    w.finish().unwrap();

    assert!(zip_fp.is_existing_file());

    let a = ZipArchive::open(&zip_fp).unwrap();
    assert_eq!(a.len(), 0);
}

#[test]
fn test_write_read_empty_file() {
    let (_tmp, zip_fp) = setup();
    let mut w = ZipWriter::create(&zip_fp).unwrap();
    w.write_file("empty.txt", b"", 0o644).unwrap();
    w.finish().unwrap();

    assert!(zip_fp.is_existing_file());

    let mut a = ZipArchive::open(&zip_fp).unwrap();
    assert_eq!(a.len(), 1);
    assert_eq!(a.file_name(0).unwrap(), "empty.txt");
    assert_eq!(a.read_file(0).unwrap(), b"");
}

#[test]
fn test_write_read_large_file() {
    let (_tmp, zip_fp) = setup();
    let arr: Vec<u8> = (0..100usize * 1024 * 1024) // 100MB
        .map(|i| (i.wrapping_mul(i) % 255) as u8)
        .collect();

    let mut w = ZipWriter::create(&zip_fp).unwrap();
    w.write_file("test dir/large file.bin", &arr, 0o644)
        .unwrap();
    w.finish().unwrap();

    assert!(zip_fp.is_existing_file());

    let mut a = ZipArchive::open(&zip_fp).unwrap();
    assert_eq!(a.len(), 1);
    assert_eq!(a.file_name(0).unwrap(), "test dir/large file.bin");
    let readback = a.read_file(0).unwrap();
    assert_eq!(readback.len(), arr.len());
    assert!(readback == arr);
}

#[test]
fn test_extract_to() {
    let (tmp, zip_fp) = setup();
    let mut w = ZipWriter::create(&zip_fp).unwrap();
    w.write_file("test dir/file 1", b"a", 0o644).unwrap();
    w.write_file("test dir/file 2", b"b", 0o644).unwrap();
    w.finish().unwrap();

    assert!(zip_fp.is_existing_file());

    let mut a = ZipArchive::open(&zip_fp).unwrap();
    assert_eq!(a.len(), 2);
    let dst = tmp.path().path_to("sub dir");
    a.extract_to(&dst).unwrap();
    assert_eq!(
        file_utils::read_file(&dst.path_to("test dir/file 1")).unwrap(),
        b"a"
    );
    assert_eq!(
        file_utils::read_file(&dst.path_to("test dir/file 2")).unwrap(),
        b"b"
    );
}

#[test]
fn test_unsafe_file_names_are_rejected() {
    let mut w = ZipWriter::new_in_memory();
    w.write_file("../evil", b"x", 0o644).unwrap();
    let data = w.finish().unwrap();
    let mut a = ZipArchive::from_bytes(data).unwrap();
    assert!(a.file_name(0).is_err());
}

#[test]
fn test_invalid_archive() {
    assert!(ZipArchive::from_bytes(b"not a zip".to_vec()).is_err());
    let (_tmp, zip_fp) = setup();
    assert!(ZipArchive::open(&zip_fp).is_err());
}
