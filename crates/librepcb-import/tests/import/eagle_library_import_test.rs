//! Port of tests/unittests/eagleimport/eaglelibraryimporttest.cpp, plus
//! tests of the element selection and of the written library (opened with
//! `librepcb-core` and checked with the upstream `librepcb-cli`).

use librepcb_core::utils::message_logger::MessageLogger;
use librepcb_import::eagle::EagleLibraryImport;
use librepcb_import::{CheckState, ElementKind, Progress, UuidGenerator};

use super::helpers::{
    TempDir, check_library_with_upstream_cli, create_library, open_all_elements, unittest_data,
};

#[test]
fn test_import() {
    let src = unittest_data("eagleimport", "resistor.lbr");
    let tmp = TempDir::new();
    let dst = tmp.path().path_to("lib");

    let mut import = EagleLibraryImport::new(dst.clone());
    let parse_errors = import.open(&src).unwrap();
    assert_eq!(1, import.symbols().len());
    assert_eq!(1, import.packages().len());
    assert_eq!(1, import.components().len());
    assert_eq!(1, import.devices().len());
    assert_eq!(0, parse_errors.len());

    // Upstream imports nothing because nothing is checked.
    let log = MessageLogger::new();
    let summary = import.run(&log, &Progress::new());
    assert_eq!(0, summary.total);
    assert_eq!(0, log.messages().len());
}

#[test]
fn test_select_device_selects_dependencies() {
    let src = unittest_data("eagleimport", "resistor.lbr");
    let tmp = TempDir::new();
    let mut import = EagleLibraryImport::new(tmp.path().path_to("lib"));
    import.open(&src).unwrap();
    let dev_name = import.devices()[0].display_name.clone();
    let changes = import.set_device_checked(&dev_name, true);
    let kinds: Vec<ElementKind> = changes.iter().map(|c| c.kind).collect();
    assert_eq!(
        vec![
            ElementKind::Component,
            ElementKind::Package,
            ElementKind::Symbol
        ],
        kinds
    );
    assert!(
        changes
            .iter()
            .all(|c| c.state == CheckState::PartiallyChecked)
    );
    assert_eq!(4, import.checked_elements_count());

    // Unchecking the device unchecks the dependencies again.
    let changes = import.set_device_checked(&dev_name, false);
    assert_eq!(3, changes.len());
    assert!(changes.iter().all(|c| c.state == CheckState::Unchecked));
    assert_eq!(0, import.checked_elements_count());
}

fn import_all(lbr: &str) -> (TempDir, librepcb_core::fileio::FilePath, usize) {
    let tmp = TempDir::new();
    let lib = create_library(&tmp);
    let mut import = EagleLibraryImport::with_uuid_generator(lib.clone(), UuidGenerator::random());
    import.open(&unittest_data("eagleimport", lbr)).unwrap();
    import.set_all_checked(true);
    import.set_add_name_prefix(true);
    let total = import.total_elements_count();
    let log = MessageLogger::new();
    let progress = Progress::new();
    let summary = import.run(&log, &progress);
    assert_eq!(total, summary.total);
    assert_eq!(total, summary.processed);
    assert_eq!(100, progress.percent());
    for msg in log.messages() {
        eprintln!("{:?}: {}", msg.level, msg.message);
    }
    (tmp, lib, summary.imported)
}

#[test]
fn test_import_resistor_into_library() {
    let (_tmp, lib, imported) = import_all("resistor.lbr");
    assert_eq!(4, imported);
    let counts = open_all_elements(&lib);
    assert_eq!(Some(&1), counts.get("cmpcat"));
    assert_eq!(Some(&1), counts.get("pkgcat"));
    assert_eq!(Some(&1), counts.get("sym"));
    assert_eq!(Some(&1), counts.get("pkg"));
    assert_eq!(Some(&1), counts.get("cmp"));
    assert_eq!(Some(&1), counts.get("dev"));
    check_library_with_upstream_cli(&lib);
}

#[test]
fn test_import_test_library() {
    let (_tmp, lib, imported) = import_all("testlibrary.lbr");
    assert!(imported > 0);
    let counts = open_all_elements(&lib);
    assert_eq!(imported, counts.values().sum::<usize>() - 2); // minus categories
    check_library_with_upstream_cli(&lib);
}

#[test]
fn test_cancel() {
    let tmp = TempDir::new();
    let mut import = EagleLibraryImport::new(tmp.path().path_to("lib"));
    import
        .open(&unittest_data("eagleimport", "testlibrary.lbr"))
        .unwrap();
    import.set_all_checked(true);
    let progress = Progress::new();
    progress.cancel();
    let summary = import.run(&MessageLogger::new(), &progress);
    assert!(summary.canceled);
    assert_eq!(0, summary.processed);
}
