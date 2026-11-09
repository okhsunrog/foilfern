//! Port of tests/unittests/kicadimport/kicadlibraryimporttest.cpp, plus
//! tests of the written library (opened with `librepcb-core` and checked
//! with the upstream `librepcb-cli`).

use librepcb_core::fileio::FilePath;
use librepcb_core::utils::message_logger::MessageLogger;
use librepcb_import::kicad::{ImportState, KiCadLibraryImport, NoImportedElements, ScanResult};
use librepcb_import::{CheckState, Progress};

use super::helpers::{
    TempDir, check_library_with_upstream_cli, create_library, open_all_elements, test_data_dir,
};

fn total_symbols(result: &ScanResult) -> usize {
    result.symbol_libs.iter().map(|l| l.symbols.len()).sum()
}

fn total_footprint_files(result: &ScanResult) -> usize {
    result.footprint_libs.iter().map(|l| l.files.len()).sum()
}

fn total_footprints(result: &ScanResult) -> usize {
    result
        .footprint_libs
        .iter()
        .map(|l| l.footprints.len())
        .sum()
}

fn src_dir() -> FilePath {
    FilePath::new(test_data_dir().join("unittests/kicadimport")).unwrap()
}

fn files_recursive(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_recursive(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
fn test_import() {
    let tmp = TempDir::new();
    let dst = tmp.path().path_to("lib");
    let mut import = KiCadLibraryImport::new(dst.clone());
    let log = MessageLogger::new();
    let progress = Progress::new();

    // Scan.
    assert!(import.scan(&src_dir(), None, &log, &progress));
    assert!(!log.messages().is_empty());
    assert_eq!(3, import.result().symbol_libs.len());
    assert_eq!(0, total_symbols(import.result()));
    assert_eq!(2, import.result().footprint_libs.len());
    assert_eq!(3, total_footprint_files(import.result()));
    assert_eq!(0, total_footprints(import.result()));
    log.clear();

    // Parse.
    assert!(import.parse(&NoImportedElements, &log, &progress));
    assert!(!log.messages().is_empty());
    assert_eq!(3, import.result().symbol_libs.len());
    assert_eq!(3, total_symbols(import.result()));
    assert_eq!(2, import.result().footprint_libs.len());
    assert_eq!(3, total_footprint_files(import.result()));
    assert_eq!(3, total_footprints(import.result()));
    log.clear();

    // Verify nothing is imported yet.
    assert!(!dst.is_existing_dir());

    // Import.
    let summary = import.import(&NoImportedElements, &log, &progress).unwrap();
    assert!(!log.messages().is_empty());
    for msg in log.messages() {
        eprintln!("{:?}: {}", msg.level, msg.message);
    }
    assert_eq!(ImportState::Imported, import.state());
    assert_eq!(summary.total, summary.imported);

    // Verify that files have been written (2 files per element).
    let mut files = Vec::new();
    files_recursive(dst.as_path(), &mut files);
    assert_eq!(22, files.len());
}

#[test]
fn test_wrong_state() {
    let tmp = TempDir::new();
    let mut import = KiCadLibraryImport::new(tmp.path().path_to("lib"));
    let log = MessageLogger::new();
    assert!(!import.parse(&NoImportedElements, &log, &Progress::new()));
    assert!(
        import
            .import(&NoImportedElements, &log, &Progress::new())
            .is_none()
    );
    assert_eq!(2, log.messages().len());
}

#[test]
fn test_import_into_library() {
    let tmp = TempDir::new();
    let lib = create_library(&tmp);
    let mut import = KiCadLibraryImport::new(lib.clone());
    import.set_add_to_category(true);
    import.set_add_name_prefix(true);
    let log = MessageLogger::new();
    let progress = Progress::new();
    assert!(import.scan(&src_dir(), None, &log, &progress));
    assert!(import.parse(&NoImportedElements, &log, &progress));
    assert!(import.can_start_import());
    let summary = import.import(&NoImportedElements, &log, &progress).unwrap();
    assert_eq!(11, summary.imported);
    let counts = open_all_elements(&lib);
    assert_eq!(Some(&1), counts.get("cmpcat"));
    assert_eq!(Some(&1), counts.get("pkgcat"));
    assert_eq!(Some(&3), counts.get("sym"));
    assert_eq!(Some(&3), counts.get("pkg"));
    assert_eq!(Some(&3), counts.get("cmp"));
    assert_eq!(Some(&2), counts.get("dev"));
    check_library_with_upstream_cli(&lib);
}

#[test]
fn test_select_device_selects_dependencies() {
    let tmp = TempDir::new();
    let mut import = KiCadLibraryImport::new(tmp.path().path_to("lib"));
    let log = MessageLogger::new();
    let progress = Progress::new();
    assert!(import.scan(&src_dir(), None, &log, &progress));
    assert!(import.parse(&NoImportedElements, &log, &progress));
    import.set_all_checked(false);
    let lib = import.result().symbol_libs[1]
        .file
        .complete_basename()
        .to_owned();
    let sym = import.result().symbol_libs[1].symbols[0].name.clone();
    let changes = import.set_device_checked(&lib, &sym, true);
    // The component and its symbols are selected as dependencies. The
    // package is not: Ultra Librarian references footprints without their
    // library name, which only the converter resolves (like upstream).
    assert_eq!(2, changes.len(), "{changes:?}");
    assert!(
        changes
            .iter()
            .all(|c| c.state == CheckState::PartiallyChecked)
    );
}
