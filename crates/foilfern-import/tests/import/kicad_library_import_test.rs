//! Port of tests/unittests/kicadimport/kicadlibraryimporttest.cpp, plus
//! tests of the written library (opened with `foilfern-core` and checked
//! with the upstream `librepcb-cli`).

use foilfern_core::fileio::FilePath;
use foilfern_core::utils::message_logger::MessageLogger;
use foilfern_import::kicad::{ImportState, KiCadLibraryImport, NoImportedElements, ScanResult};
use foilfern_import::{CheckState, Progress};

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
fn unnamed_pins_keep_signal_ids_without_a_name_legend() {
    use std::sync::Arc;

    use foilfern_core::fileio::{TransactionalDirectory, TransactionalFileSystem};
    use foilfern_core::library::LibraryBaseElement;
    use foilfern_core::library::cmp::{CmpSigPinDisplayType, Component};

    let tmp = TempDir::new();
    let src = tmp.path().path_to("source");
    let dst = tmp.path().path_to("library");
    std::fs::create_dir(src.as_path()).unwrap();
    std::fs::write(
        src.path_to("passive.kicad_sym").as_path(),
        r#"(kicad_symbol_lib (version 20231120) (generator test)
            (symbol "Test"
                (symbol "Test_1_1"
                    (pin passive line (at -5 0 0) (length 2) (name "") (number "1"))
                    (pin passive line (at 5 0 180) (length 2) (name "~") (number "2"))
                    (pin passive line (at 0 5 270) (length 2) (name "3") (number "3")))))"#,
    )
    .unwrap();
    let mut import = KiCadLibraryImport::new(dst.clone());
    let log = MessageLogger::new();
    let progress = Progress::new();
    assert!(import.scan(&src, None, &log, &progress));
    assert!(import.parse(&NoImportedElements, &log, &progress));
    assert_eq!(
        import
            .import(&NoImportedElements, &log, &progress)
            .unwrap()
            .imported,
        2
    );
    let entry = std::fs::read_dir(dst.path_to("cmp").as_path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(entry.path()).unwrap()).unwrap();
    let component = Component::open(TransactionalDirectory::new(Arc::new(fs), "")).unwrap();
    let item = component
        .symbol_variants()
        .iter()
        .next()
        .unwrap()
        .symbol_items()
        .iter()
        .next()
        .unwrap();
    let displays = item
        .pin_signal_map()
        .iter()
        .map(|pin| {
            let signal = component
                .signals()
                .iter()
                .find(|signal| Some(signal.uuid()) == pin.signal_uuid())
                .unwrap();
            (signal.name().as_str(), pin.display_type())
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        displays,
        [
            ("1", CmpSigPinDisplayType::None),
            ("2", CmpSigPinDisplayType::None),
            ("3", CmpSigPinDisplayType::ComponentSignal),
        ]
        .into()
    );
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
