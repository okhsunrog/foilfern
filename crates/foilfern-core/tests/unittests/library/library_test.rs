//! Port of tests/unittests/core/library/librarytest.cpp.

use foilfern_core::library::cat::{ComponentCategory, PackageCategory};
use foilfern_core::library::cmp::Component;
use foilfern_core::library::dev::Device;
use foilfern_core::library::org::Organization;
use foilfern_core::library::sym::Symbol;
use foilfern_core::library::{Library, LibraryBaseElement};

use super::{assert_upgrade_v01, data_path, open_dir};

// The whitespaces in the path are there to make the test even stronger ;)
const DEST_NAME: &str = "test dir.lplib";

#[test]
fn test_upgrade_v01() {
    assert_upgrade_v01::<Library>("libraries/v0.1.lplib", DEST_NAME);
}

#[test]
fn test_open_empty_library() {
    let lib = Library::open(open_dir(&data_path("libraries/Empty Library.lplib"), false)).unwrap();
    assert!(lib.search_for_elements::<Symbol>().is_empty());
}

#[test]
fn test_invalid_suffix() {
    let (_tmp, dir) = super::copy_to_temp("libraries/Empty Library.lplib", "lib");
    let err = Library::open(open_dir(&dir, false)).unwrap_err();
    assert!(err.to_string().contains("suffix '.lplib'"), "{err}");
}

#[test]
fn test_search_for_elements() {
    let lib = Library::open(open_dir(
        &data_path("libraries/Populated Library.lplib"),
        false,
    ))
    .unwrap();
    assert_eq!(lib.manufacturer().as_str(), "Demo Manufacturer");
    assert_eq!(
        lib.url(),
        "https://github.com/LibrePCB/PopulatedLibrary.lplib"
    );
    assert!(!lib.icon().is_empty());
    let counts = [
        lib.search_for_elements::<ComponentCategory>().len(),
        lib.search_for_elements::<PackageCategory>().len(),
        lib.search_for_elements::<Symbol>().len(),
        lib.search_for_elements::<Component>().len(),
        lib.search_for_elements::<Device>().len(),
        lib.search_for_elements::<Organization>().len(),
    ];
    assert_eq!(counts, [11, 4, 19, 15, 8, 2]);
    for path in lib.search_for_elements::<Symbol>() {
        assert!(path.starts_with("sym/"), "{path}");
    }
}
