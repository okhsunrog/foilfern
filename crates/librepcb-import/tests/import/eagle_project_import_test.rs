//! Port of tests/unittests/eagleimport/eagleprojectimporttest.cpp.
//!
//! `test_import_with_board` compares the imported project byte for byte
//! with upstream's golden sample (`testproject/expected`), which also covers
//! the conversion of the library elements.

use std::path::Path;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::project::{Mutation, Project, ProjectLoader};
use librepcb_core::utils::message_logger::MessageLogger;
use librepcb_import::UuidGenerator;
use librepcb_import::eagle::EagleProjectImport;

use super::helpers::{TempDir, test_data_dir, unittest_data};

fn files_recursive(base: &Path, dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_recursive(base, &path, out);
        } else {
            out.push(
                path.strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
}

/// Compares two directories recursively (upstream
/// `TestHelpers::compareDirectories()`).
fn compare_directories(actual: &Path, expected: &Path) {
    let mut actual_files = Vec::new();
    files_recursive(actual, actual, &mut actual_files);
    actual_files.sort();
    let mut expected_files = Vec::new();
    files_recursive(expected, expected, &mut expected_files);
    expected_files.sort();
    assert_eq!(expected_files, actual_files, "different file lists");
    let mut failures = Vec::new();
    for file in &expected_files {
        let a = std::fs::read(actual.join(file)).unwrap();
        let e = std::fs::read(expected.join(file)).unwrap();
        if a != e {
            let a = String::from_utf8_lossy(&a);
            let e = String::from_utf8_lossy(&e);
            let line = a
                .lines()
                .zip(e.lines())
                .position(|(x, y)| x != y)
                .unwrap_or(a.lines().count().min(e.lines().count()));
            failures.push(format!(
                "{file}: first difference in line {}:\n  actual:   {}\n  expected: {}",
                line + 1,
                a.lines().nth(line).unwrap_or("<eof>"),
                e.lines().nth(line).unwrap_or("<eof>")
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} files differ:\n{}",
        failures.len(),
        expected_files.len(),
        failures.join("\n")
    );
}

/// Creates a new project like upstream `Project::create()`, including the
/// stroke font copied from the resources.
fn create_project(dir: &FilePath, uuids: &UuidGenerator) -> Project {
    let font = FilePath::new(
        Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("share/librepcb/fontobene/newstroke.bene"),
    )
    .unwrap();
    let fs = Arc::new(TransactionalFileSystem::open_rw(dir).unwrap());
    fs.write(
        "resources/fontobene/newstroke.bene",
        &std::fs::read(font.as_path()).unwrap(),
    )
    .unwrap();
    let uuids = uuids.clone();
    Project::create(TransactionalDirectory::new(fs, ""), "test.lpp", move || {
        uuids.generate()
    })
    .unwrap()
}

fn save(project: &mut Project) {
    project.save().unwrap();
    project.directory().file_system().save().unwrap();
}

fn reopen(dir: &FilePath) -> Project {
    let fs = Arc::new(TransactionalFileSystem::open_ro(dir).unwrap());
    ProjectLoader::new()
        .open(TransactionalDirectory::new(fs, ""), "test.lpp")
        .unwrap()
}

fn import_project(sch: &str, brd: Option<&str>) -> (TempDir, FilePath, EagleProjectImport) {
    let mut import = EagleProjectImport::new(UuidGenerator::random(), Utc::now());
    assert!(!import.is_ready());
    let msgs = import
        .open(
            &unittest_data("eagleimport", sch),
            brd.map(|b| unittest_data("eagleimport", b)).as_ref(),
        )
        .unwrap();
    assert!(import.is_ready());
    assert_eq!("", msgs.join(";"));
    let tmp = TempDir::new();
    let dir = tmp.path().path_to("project");
    let uuids = UuidGenerator::random();
    let mut project = create_project(&dir, &uuids);
    let log = MessageLogger::new();
    import.import(&mut project, &log).unwrap();
    save(&mut project);
    (tmp, dir, import)
}

#[test]
fn test_import_only_schematic() {
    let (_tmp, dir, import) = import_project("testproject/testproject.sch", None);
    assert_eq!("testproject", import.project_name());
    let project = reopen(&dir);
    assert_eq!(1, project.schematics().len());
    assert_eq!(0, project.boards().len());
    assert_eq!(4, project.circuit().buses().len());
}

#[test]
fn test_import_with_board() {
    let uuids = UuidGenerator::sequential();
    let created = Utc.with_ymd_and_hms(2000, 1, 2, 3, 4, 5).unwrap();
    let mut import = EagleProjectImport::new(uuids.clone(), created);
    assert!(!import.is_ready());
    let msgs = import
        .open(
            &unittest_data("eagleimport", "testproject/testproject.sch"),
            Some(&unittest_data("eagleimport", "testproject/testproject.brd")),
        )
        .unwrap();
    assert!(import.is_ready());
    assert_eq!("testproject", import.project_name());
    assert_eq!("", msgs.join(";"));

    let tmp = TempDir::new();
    let dir = tmp.path().path_to("actual");
    {
        // Populate and save project.
        let mut project = create_project(&dir, &uuids);
        let mut metadata = project.metadata().clone();
        metadata.created = created;
        project
            .apply(Mutation::SetProjectMetadata(metadata))
            .unwrap();
        let log = MessageLogger::new();
        import.import(&mut project, &log).unwrap();
        for msg in log.messages() {
            eprintln!("{:?}: {}", msg.level, msg.message);
        }
        save(&mut project);
    }

    {
        // Open project again to see if there's no problem with it.
        let project = reopen(&dir);
        assert_eq!(1, project.schematics().len());
        assert_eq!(1, project.boards().len());
        assert_eq!(4, project.circuit().buses().len());
    }

    // Compare exported project with golden sample.
    compare_directories(
        dir.as_path(),
        &test_data_dir().join("unittests/eagleimport/testproject/expected"),
    );
}

/// This project has strange embedded libraries which shall be tested.
#[test]
fn test_arduino_micro() {
    let (_tmp, dir, import) = import_project(
        "arduino-micro/Micro_Rev1j.sch",
        Some("arduino-micro/Micro_Rev1j.brd"),
    );
    assert_eq!("Micro_Rev1j", import.project_name());
    let project = reopen(&dir);
    assert_eq!(1, project.schematics().len());
    assert_eq!(1, project.boards().len());
}

#[test]
fn test_nodino() {
    let (_tmp, dir, import) = import_project(
        "nodino-rc7/Nodino-RC7.sch",
        Some("nodino-rc7/Nodino-RC7.brd"),
    );
    assert_eq!("Nodino-RC7", import.project_name());
    let project = reopen(&dir);
    assert_eq!(1, project.schematics().len());
    assert_eq!(1, project.boards().len());
}
