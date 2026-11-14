//! Port of tests/unittests/core/project/outputjobrunnertest.cpp, plus
//! indexed ownership, dependencies and official CLI output comparisons.
use foilfern_core::{
    fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem, ZipArchive},
    job::{ArchiveOutputJob, Board3DOutputJob, ObjectSet, OutputJob},
    project::{OutputJobError as Error, board::ExportInfo},
    project::{OutputJobRunner, Project, ProjectLoader},
    types::Uuid,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};
fn load(name: &str) -> Project {
    let root = Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/projects")
        .join(name);
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(root).unwrap()).unwrap();
    let name = fs
        .files("")
        .into_iter()
        .find(|n| n.ends_with(".lpp"))
        .unwrap();
    ProjectLoader::new()
        .open(TransactionalDirectory::new(Arc::new(fs), ""), &name)
        .unwrap()
}
fn out(temp: &tempfile::TempDir) -> FilePath {
    FilePath::new(temp.path()).unwrap()
}
#[test]
fn step_jobs_dispatch_board_only_exports_and_archive_prepared_bytes() {
    use foilfern_core::project::{BoardStepOutput, OutputJobEvent};
    let mut project = load("Nested Planes");
    let board_id = project.boards()[0].id();
    let step = OutputJob::new(
        Uuid::new_random(),
        "STEP".parse().unwrap(),
        Board3DOutputJob {
            boards: ObjectSet::Default,
            assembly_variants: ObjectSet::Custom(BTreeSet::from([None])),
            output_path: "mechanical.STP".into(),
        },
    );
    let archive = OutputJob::new(
        Uuid::new_random(),
        "Archive".parse().unwrap(),
        ArchiveOutputJob {
            input_jobs: BTreeMap::from([(step.uuid(), String::new())]),
            output_path: "mechanical.zip".into(),
        },
    );
    let temp = tempfile::tempdir().unwrap();
    let mut runner = OutputJobRunner::new(&mut project, ExportInfo::now("test")).unwrap();
    runner.set_output_directory(&out(&temp));
    runner.set_step_exporter(Some(Box::new(move |_, board, variant| {
        assert_eq!(board.id(), board_id);
        assert_eq!(variant, None);
        assert!(!board.derived().plane_fragments().is_empty());
        Ok(BoardStepOutput {
            contents: b"prepared BRep".to_vec(),
            warnings: vec!["contour approximation".into()],
        })
    })));
    let warnings = Arc::new(std::sync::Mutex::new(Vec::new()));
    let messages = warnings.clone();
    runner.set_observer(Some(Box::new(move |event| {
        if let OutputJobEvent::Warning(message) = event {
            messages.lock().unwrap().push(message.to_owned());
        }
    })));
    runner.run(&[step.clone(), archive]).unwrap();
    assert_eq!(*warnings.lock().unwrap(), vec!["contour approximation"]);
    assert_eq!(
        std::fs::read(temp.path().join("mechanical.STP")).unwrap(),
        b"prepared BRep"
    );
    let mut zip = ZipArchive::open(&out(&temp).path_to("mechanical.zip")).unwrap();
    assert_eq!(
        zip.read_file_by_name("mechanical.STP").unwrap().unwrap(),
        b"prepared BRep"
    );
}

#[test]
fn step_preparation_failure_preserves_existing_output() {
    use foilfern_core::project::BoardStepOutput;
    let mut project = load("Nested Planes");
    let step = OutputJob::new(
        Uuid::new_random(),
        "STEP".parse().unwrap(),
        Board3DOutputJob {
            boards: ObjectSet::Default,
            assembly_variants: ObjectSet::Custom(BTreeSet::from([None])),
            output_path: "board.step".into(),
        },
    );
    let temp = tempfile::tempdir().unwrap();
    let mut runner = OutputJobRunner::new(&mut project, ExportInfo::now("test")).unwrap();
    runner.set_output_directory(&out(&temp));
    assert!(matches!(
        runner.run(std::slice::from_ref(&step)),
        Err(Error::Unsupported(_))
    ));
    runner.set_step_exporter(Some(Box::new(|_, _, _| {
        Ok(BoardStepOutput {
            contents: b"previous".to_vec(),
            warnings: Vec::new(),
        })
    })));
    runner.run(std::slice::from_ref(&step)).unwrap();
    runner.set_step_exporter(Some(Box::new(|_, _, _| {
        Err(Error::Step("invalid model".into()))
    })));
    assert!(matches!(
        runner.run(std::slice::from_ref(&step)),
        Err(Error::Step(_))
    ));
    assert_eq!(
        std::fs::read(temp.path().join("board.step")).unwrap(),
        b"previous"
    );
    let invalid = OutputJob::new(
        Uuid::new_random(),
        "STEP".parse().unwrap(),
        Board3DOutputJob {
            boards: ObjectSet::Default,
            assembly_variants: ObjectSet::Custom(BTreeSet::from([None])),
            output_path: "board.png".into(),
        },
    );
    assert!(matches!(
        runner.run(&[invalid]),
        Err(Error::UnsupportedStepFormat(_))
    ));
}
