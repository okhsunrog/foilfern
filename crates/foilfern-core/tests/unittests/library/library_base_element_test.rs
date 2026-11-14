//! Port of tests/unittests/core/library/librarybaseelementtest.cpp.
//!
//! Upstream instantiates `LibraryBaseElement` directly as `"sym"`/`"symbol"`
//! element; here a [`Symbol`] is used.

use std::sync::Arc;

use foilfern_core::fileio::{TransactionalDirectory, TransactionalFileSystem, file_utils};
use foilfern_core::library::sym::Symbol;
use foilfern_core::library::{BaseMetadata, LibraryBaseElement};
use foilfern_core::types::{ElementName, Uuid};

use crate::helpers::TempDir;

fn new_element() -> Symbol {
    Symbol::new(BaseMetadata::new(
        Uuid::new_random(),
        "1.0".parse().unwrap(),
        "test",
        chrono::Utc::now(),
        ElementName::new("Test").unwrap(),
        "",
        "",
    ))
    .unwrap()
}

#[test]
fn test_save() {
    new_element().save().unwrap();
}

#[test]
fn test_move_to_non_existing_directory() {
    let tmp = TempDir::new();
    let mut element = new_element();
    let dest = tmp.path().path_to(&element.metadata().uuid().to_string());
    let dest_fs = Arc::new(TransactionalFileSystem::open_rw(&dest).unwrap());
    let mut dest_dir = TransactionalDirectory::new(Arc::clone(&dest_fs), "");
    element.move_to(&mut dest_dir).unwrap();
    assert!(dest_fs.file_exists("symbol.lp"));
    dest_fs.save().unwrap();
    assert!(dest.path_to("symbol.lp").is_existing_file());
}

#[test]
fn test_move_to_empty_directory() {
    // Saving into empty destination directory must work because empty
    // directories are sometimes created "accidentally" (for example by Git
    // operations which remove files, but not their parent directories). So
    // we handle empty directories like they are not existent...
    let tmp = TempDir::new();
    let mut element = new_element();
    let dest = tmp.path().path_to(&element.metadata().uuid().to_string());
    file_utils::make_path(&dest).unwrap();
    assert!(dest.is_existing_dir());
    let dest_fs = Arc::new(TransactionalFileSystem::open_rw(&dest).unwrap());
    let mut dest_dir = TransactionalDirectory::new(Arc::clone(&dest_fs), "");
    element.move_to(&mut dest_dir).unwrap();
    assert!(dest_fs.file_exists("symbol.lp"));
    dest_fs.save().unwrap();
    assert!(dest.path_to("symbol.lp").is_existing_file());
}

#[test]
fn test_save_into_parent_directory_and_reopen() {
    let tmp = TempDir::new();
    let mut element = new_element();
    let uuid = element.metadata().uuid();
    let lib_fs = Arc::new(TransactionalFileSystem::open_rw(&tmp.path().path_to("lib")).unwrap());
    let mut lib_dir = TransactionalDirectory::new(Arc::clone(&lib_fs), "sym");
    element.save_into_parent_directory(&mut lib_dir).unwrap();
    assert!(Symbol::is_valid_element_directory(
        &*lib_fs,
        &format!("sym/{uuid}")
    ));
    lib_fs.save().unwrap();
    let dir = tmp.path().path_to(&format!("lib/sym/{uuid}"));
    assert!(Symbol::is_valid_element_directory_path(&dir));
    let reopened = Symbol::open(super::open_dir(&dir, false)).unwrap();
    assert_eq!(reopened.metadata().uuid(), uuid);
    assert_eq!(reopened.metadata().names(), element.metadata().names());
    // The creation date is stored with seconds resolution.
    assert_eq!(
        reopened.metadata().created().timestamp(),
        element.metadata().created().timestamp()
    );
}

#[test]
fn test_directory_name_must_be_uuid() {
    let tmp = TempDir::new();
    let mut element = new_element();
    let fs = Arc::new(TransactionalFileSystem::open_rw(&tmp.path().path_to("foo")).unwrap());
    let mut dir = TransactionalDirectory::new(Arc::clone(&fs), "");
    element.move_to(&mut dir).unwrap();
    fs.save().unwrap();
    let err = Symbol::open(super::open_dir(&tmp.path().path_to("foo"), false)).unwrap_err();
    assert!(
        err.to_string().contains("Directory name UUID mismatch"),
        "{err}"
    );
}
