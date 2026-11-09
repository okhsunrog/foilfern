//! Tests of the file format migrations: ports of `ProjectTest::testUpgradeV01`
//! and `ProjectTest::testUpgradeV1` of
//! tests/unittests/core/project/projecttest.cpp (upstream has no unit tests
//! of the migration classes themselves; the library element variants are in
//! `library/`), plus tests of the migration selection and the unstable
//! migration. The results are compared with upstream in
//! `tests/file_format_migration.rs`.

use librepcb_core::application::file_format_version;
use librepcb_core::fileio::{FilePath, FileSystem, file_utils};
use librepcb_core::job::{GraphicsContentType, OutputJob, OutputJobKind, OutputJobList};
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::library::dev::Device;
use librepcb_core::project::ProjectLoader;
use librepcb_core::serialization::{
    FileFormatMigration, MigrationError, MigrationSeverity, Mode, SExpression, UnstableMigration,
    V1Migration, file_format_migrations,
};

use crate::library::{copy_to_temp, open_dir};

// The whitespaces in the path are there to make the test even stronger ;)
const PROJECT_DIR: &str = "test project dir";
const PROJECT_FILE: &str = "test project.lpp";

fn read_version_file(dir: &FilePath, name: &str) -> Vec<u8> {
    file_utils::read_file(&dir.path_to(name)).unwrap()
}

fn html_files(dir: &FilePath) -> Vec<String> {
    let logs = dir.path_to("logs");
    if !logs.is_existing_dir() {
        return Vec::new();
    }
    std::fs::read_dir(logs.as_path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".html"))
        .collect()
}

/// Upstream `testUpgradeV01` / `testUpgradeV1`: open (upgrade), save and
/// re-open the project `src`, which has the file format `version`.
/// Returns the re-opened project's output jobs.
fn assert_project_upgrade(src: &str, version: &str) -> OutputJobList {
    // Copy project into temporary directory.
    let (_tmp, dir) = copy_to_temp(src, PROJECT_DIR);

    // Open/upgrade/close project.
    assert!(
        read_version_file(&dir, ".librepcb-project").starts_with(format!("{version}\n").as_bytes())
    );
    assert!(html_files(&dir).is_empty());
    let (schematic_count, board_count) = {
        let mut loader = ProjectLoader::new();
        let mut project = loader.open(open_dir(&dir, true), PROJECT_FILE).unwrap();
        let log = loader.migration_log().unwrap();
        assert_eq!(log.from_version.to_string(), version);
        assert_eq!(log.to_version, file_format_version());
        assert!(!log.messages.is_empty());
        project.save().unwrap();
        project.directory().file_system().save().unwrap();
        (project.schematics().len(), project.boards().len())
    };

    // Check written files.
    assert!(
        read_version_file(&dir, ".librepcb-project")
            .starts_with(format!("{}\n", file_format_version()).as_bytes())
    );
    assert_eq!(html_files(&dir).len(), 1);

    // Re-open project.
    let mut loader = ProjectLoader::new();
    let project = loader.open(open_dir(&dir, true), PROJECT_FILE).unwrap();
    assert!(loader.migration_log().is_none());
    assert_eq!(project.schematics().len(), schematic_count);
    assert_eq!(project.boards().len(), board_count);
    project.output_jobs().clone()
}

#[test]
fn test_upgrade_v01() {
    assert_project_upgrade("projects/v0.1", "0.1");
}

#[test]
fn test_upgrade_v1() {
    let jobs = assert_project_upgrade("projects/v1", "1");

    // Check if the "realistic" flag has been migrated.
    let Some(OutputJobKind::Graphics(job)) = jobs.first().map(OutputJob::kind) else {
        panic!("first job is not a graphics job");
    };
    assert_eq!(job.content.len(), 2);
    // Output job without "(option realistic)" is not modified.
    assert_eq!(job.content[0].content_type, GraphicsContentType::Board);
    assert!(!job.content[0].options.contains_key("realistic"));
    assert_eq!(job.content[0].layers.len(), 22);
    // Output job with "(option realistic)" is migrated.
    assert_eq!(
        job.content[1].content_type,
        GraphicsContentType::BoardRendering
    );
    assert!(!job.content[1].options.contains_key("realistic"));
    assert_eq!(job.content[1].layers.len(), 4);
}

#[test]
fn test_migrations_by_file_format() {
    let versions = |format: &str| -> Vec<(String, String)> {
        file_format_migrations(&format.parse().unwrap())
            .iter()
            .map(|m| (m.from_version().to_string(), m.to_version().to_string()))
            .collect()
    };
    let pair = |a: &str, b: &str| (a.to_owned(), b.to_owned());
    assert_eq!(versions("0.1"), [pair("0.1", "1"), pair("1", "2")]);
    assert_eq!(versions("1"), [pair("1", "2")]);
    // Without LIBREPCB_UPGRADE_UNSTABLE=1, current files are not touched.
    assert!(versions("2").is_empty());
}

#[test]
fn test_unexpected_version_file() {
    const UUID: &str = "35aad2af-5cd4-42ae-8576-fe7febad0d8a";
    let (_tmp, dir) = copy_to_temp(&format!("libraries/v0.1.lplib/sym/{UUID}"), UUID);
    let mut directory = open_dir(&dir, false);
    let err = V1Migration::new()
        .upgrade_symbol(&mut directory)
        .unwrap_err();
    assert!(matches!(err, MigrationError::UnexpectedFileFormat { .. }));
    assert!(
        err.to_string()
            .starts_with("Unexpected file format version:\nExpected v1, found v0.1.\n")
    );
}

#[test]
fn test_unstable_upgrade_device() {
    const UUID: &str = "4f5ee784-4b1b-407c-802b-44625163d90f";
    let (_tmp, dir) = copy_to_temp(
        &format!("libraries/Populated Library.lplib/dev/{UUID}"),
        UUID,
    );
    let migration = UnstableMigration::new();
    assert_eq!(*migration.from_version(), file_format_version());
    assert_eq!(*migration.to_version(), file_format_version());
    let mut directory = open_dir(&dir, false);
    let original = directory.read("device.lp").unwrap();
    migration.upgrade_device(&mut directory).unwrap();
    let root =
        SExpression::parse(&directory.read("device.lp").unwrap(), None, Mode::LibrePcb).unwrap();
    let pads: Vec<_> = root.children_named("pad").collect();
    assert!(!pads.is_empty());
    for pad in pads {
        let last = pad
            .children()
            .iter()
            .rev()
            .find(|c| !c.is_line_break())
            .unwrap();
        assert_eq!(last.name().unwrap(), "optional");
    }
    // The version file is not modified, and the element still loads (the
    // first `optional` of each pad is used) and is formatted by saving.
    assert_eq!(
        directory.read(".librepcb-dev").unwrap(),
        format!("{}\n", file_format_version()).into_bytes()
    );
    let mut device = Device::open(directory).unwrap();
    device.save().unwrap();
    assert_eq!(device.directory().read("device.lp").unwrap(), original);
}

#[test]
fn test_unstable_upgrade_project() {
    let (_tmp, dir) = copy_to_temp("projects/Empty Project", PROJECT_DIR);
    let mut directory = open_dir(&dir, false);
    let lpp = directory
        .files("")
        .into_iter()
        .find(|f| f.ends_with(".lpp"))
        .unwrap();
    let original_circuit = directory.read("circuit/circuit.lp").unwrap();
    let mut messages = Vec::new();
    UnstableMigration::new()
        .upgrade_project(&mut directory, &mut messages)
        .unwrap();
    // The circuit is not upgraded, and the project still loads.
    assert_eq!(
        directory.read("circuit/circuit.lp").unwrap(),
        original_circuit
    );
    let board_count = directory
        .read("boards/boards.lp")
        .map(|c| String::from_utf8_lossy(&c).matches("(board ").count())
        .unwrap();
    // Like upstream, the Gerber/Excellon output job reminder is emitted if
    // there are boards (the unstable migration does not look at the jobs).
    assert_eq!(messages.len(), usize::from(board_count > 0));
    for message in &messages {
        assert_eq!(message.severity, MigrationSeverity::Warning);
        assert_eq!(message.from_version, file_format_version());
    }
    let mut loader = ProjectLoader::new();
    loader.open(directory, &lpp).unwrap();
    assert!(loader.migration_log().is_none());
}
