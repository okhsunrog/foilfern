//! Port of tests/unittests/core/project/outputjobrunnertest.cpp, plus
//! indexed ownership, dependencies and official CLI output comparisons.
use librepcb_core::{
    export::Timestamp,
    fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem, ZipArchive, file_utils},
    job::{Error, JobKind, ObjectSet, OutputJob},
    project::{OutputJobRunner, Project, ProjectLoader},
    types::Uuid,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
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
fn job(kind: JobKind) -> OutputJob {
    OutputJob::new(Uuid::new_random(), "Test".parse().unwrap(), kind)
}
fn copy(input: &str, output: &str) -> OutputJob {
    job(JobKind::Copy {
        substitute_variables: false,
        boards: ObjectSet::Custom(BTreeSet::from([None])),
        variants: ObjectSet::Custom(BTreeSet::from([None])),
        input: input.into(),
        output: output.into(),
    })
}
fn out(temp: &tempfile::TempDir) -> FilePath {
    FilePath::new(temp.path()).unwrap()
}
#[test]
fn test_gerber_excellon_rebuilds_planes() {
    let mut project = load("Nested Planes");
    let board = project.boards()[0].id();
    assert!(
        project
            .board(board)
            .unwrap()
            .derived()
            .plane_fragments()
            .is_empty()
    );
    let temp = tempfile::tempdir().unwrap();
    let settings = librepcb_core::project::board::BoardFabricationOutputSettings {
        output_base_path: "gerber/board".into(),
        ..Default::default()
    };
    let job = job(JobKind::GerberExcellon {
        boards: ObjectSet::Default,
        settings: Box::new(settings),
    });
    OutputJobRunner::new(&mut project, &out(&temp), "test", Timestamp::now())
        .run(&[job])
        .unwrap();
    assert!(
        project
            .board(board)
            .unwrap()
            .derived()
            .plane_fragments()
            .values()
            .any(|p| !p.is_empty())
    );
    assert!(
        std::fs::read_to_string(temp.path().join("gerber/board_COPPER-TOP.gbr"))
            .unwrap()
            .contains("\nG36*\n")
    );
}
#[test]
fn test_absolute_output_file_path_and_output_folder_breakout() {
    let temp = tempfile::tempdir().unwrap();
    let mut project = load("Empty Project");
    for path in [
        temp.path().join("absolute.txt").to_str().unwrap(),
        "../escape.txt",
    ] {
        let mut runner = OutputJobRunner::new(&mut project, &out(&temp), "test", Timestamp::now());
        assert!(runner.run(&[copy(".librepcb-project", path)]).is_err());
    }
    assert!(!temp.path().join("absolute.txt").exists());
    assert!(!temp.path().parent().unwrap().join("escape.txt").exists());
    OutputJobRunner::new(&mut project, &out(&temp), "test", Timestamp::now())
        .run(&[copy(".librepcb-project", "valid.txt")])
        .unwrap();
    assert_eq!(
        std::fs::read(temp.path().join("valid.txt")).unwrap(),
        b"2\n"
    );
}
#[test]
fn test_project_folder_breakout() {
    let temp = tempfile::tempdir().unwrap();
    let mut project = load("Empty Project");
    for path in [
        "../.librepcb-project",
        "a/../../.librepcb-project",
        "a\\..\\..\\outside.txt",
        "/etc/passwd",
    ] {
        assert!(matches!(
            OutputJobRunner::new(&mut project, &out(&temp), "test", Timestamp::now())
                .run(&[copy(path, "result.txt")]),
            Err(Error::InvalidCopyInput)
        ));
        assert!(!temp.path().join("result.txt").exists());
    }
}
#[test]
fn ownership_cleanup_and_ordered_archive_dependencies() {
    let temp = tempfile::tempdir().unwrap();
    let mut project = load("Empty Project");
    let first = copy(".librepcb-project", "old.txt");
    OutputJobRunner::new(&mut project, &out(&temp), "test", Timestamp::now())
        .run(std::slice::from_ref(&first))
        .unwrap();
    let mut renamed = first.clone();
    if let JobKind::Copy { output, .. } = &mut renamed.kind {
        *output = "new.txt".into();
    }
    let archive = job(JobKind::Archive {
        inputs: BTreeMap::from([(first.uuid(), "nested".into())]),
        output: "archive.zip".into(),
    });
    let mut runner = OutputJobRunner::new(&mut project, &out(&temp), "test", Timestamp::now());
    runner.run(&[renamed.clone(), archive.clone()]).unwrap();
    assert!(!temp.path().join("old.txt").exists());
    assert!(temp.path().join("new.txt").exists());
    let mut zip = ZipArchive::open(&out(&temp).path_to("archive.zip")).unwrap();
    assert_eq!(
        zip.read_file_by_name("nested/new.txt").unwrap().unwrap(),
        b"2\n"
    );
    drop(runner);
    assert!(matches!(
        OutputJobRunner::new(&mut project, &out(&temp), "test", Timestamp::now()).run(&[archive]),
        Err(Error::ArchiveDependencyNotRun)
    ));
    let mut runner = OutputJobRunner::new(&mut project, &out(&temp), "test", Timestamp::now());
    assert!(runner.run(&[renamed.clone(), renamed]).is_err());
}
#[test]
fn duplicate_gerber_names_fail_and_failed_jobs_preserve_previous_outputs() {
    let temp = tempfile::tempdir().unwrap();
    let mut project = load("Empty Project");
    let mut settings = librepcb_core::project::board::BoardFabricationOutputSettings::default();
    settings.output_base_path = "board".into();
    settings.suffix_drills_pth = settings.suffix_drills_npth.clone();
    let job = job(JobKind::GerberExcellon {
        boards: ObjectSet::Default,
        settings: Box::new(settings),
    });
    assert!(
        OutputJobRunner::new(&mut project, &out(&temp), "test", Timestamp::now())
            .run(&[job])
            .is_err()
    );
    let first = copy(".librepcb-project", "old.txt");
    OutputJobRunner::new(&mut project, &out(&temp), "test", Timestamp::now())
        .run(std::slice::from_ref(&first))
        .unwrap();
    let mut bad = copy("missing.txt", "new.txt");
    bad.set_uuid(first.uuid());
    assert!(
        OutputJobRunner::new(&mut project, &out(&temp), "test", Timestamp::now())
            .run(&[bad])
            .is_err()
    );
    assert!(temp.path().join("old.txt").exists());
}
fn normalized(data: &[u8]) -> String {
    String::from_utf8_lossy(data)
        .lines()
        .filter(|line| {
            !line.starts_with("G04 #@! TF.CreationDate,")
                && !line.starts_with("G04 #@! TF.GenerationSoftware,")
                && !line.starts_with("G04 #@! TF.MD5,")
                && !line.starts_with("; #@! TF.CreationDate,")
                && !line.starts_with("; #@! TF.GenerationSoftware,")
                && !line.starts_with("C  Created:")
                && !line.starts_with("C  File created:")
                && !line.starts_with("C  Generation Date:")
                && !line.starts_with('#')
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .map(|e| e.unwrap())
        .filter(|e| e.file_type().is_file() && e.file_name() != ".librepcb-output")
        .map(|e| {
            (
                e.path()
                    .strip_prefix(dir)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .replace('\\', "/"),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect()
}
#[test]
fn supported_fixture_jobs_match_official_cli() {
    use librepcb_core::serialization::{List, Mode, SExpression, SerializeObject};
    let configured = std::env::var_os("LIBREPCB_CLI");
    let cli = PathBuf::from(configured.clone().unwrap_or_else(|| "librepcb-cli".into()));
    if !std::process::Command::new(&cli)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        assert!(configured.is_none(), "configured CLI unavailable");
        return;
    }
    let mut project = load("Project With Two Boards");
    let mut jobs: Vec<_> = project
        .output_jobs()
        .iter()
        .filter(|j| {
            !matches!(
                j.kind,
                JobKind::Graphics(_)
                    | JobKind::Board3d { .. }
                    | JobKind::InteractiveBom(_)
                    | JobKind::Archive { .. }
                    | JobKind::Lppz { .. }
            )
        })
        .cloned()
        .collect();
    let archive = job(JobKind::Archive {
        inputs: jobs
            .iter()
            .map(|j| (j.uuid(), j.uuid().to_string()))
            .collect(),
        output: "combined.zip".into(),
    });
    jobs.push(archive);
    let temp = tempfile::tempdir().unwrap();
    let oracle = out(&temp).path_to("oracle");
    let actual = out(&temp).path_to("actual");
    let mut root = List::new("librepcb_jobs");
    for job in &jobs {
        root.ensure_line_break();
        job.serialize(root.append_list("job"));
    }
    root.ensure_line_break();
    let config = out(&temp).path_to("jobs.lp");
    file_utils::write_file(
        &config,
        &SExpression::from(root)
            .to_byte_array(Mode::LibrePcb)
            .unwrap(),
    )
    .unwrap();
    let source = Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/projects/Project With Two Boards");
    let copy = out(&temp).path_to("project");
    file_utils::copy_dir_recursively(&FilePath::new(source).unwrap(), &copy).unwrap();
    let output = std::process::Command::new(cli)
        .args(["open-project", "--run-jobs", "--jobs"])
        .arg(config.as_ref())
        .arg("--outdir")
        .arg(oracle.as_ref())
        .arg(copy.path_to(project.file_name()).as_ref())
        .env("LC_ALL", "C")
        .env("QT_QPA_PLATFORM", "offscreen")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    OutputJobRunner::new(&mut project, &actual, "2.1.1", Timestamp::now())
        .run(&jobs)
        .unwrap();
    let expected = files(oracle.as_ref());
    let actual = files(actual.as_ref());
    assert_eq!(
        actual.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>()
    );
    for (path, data) in actual {
        if path.ends_with(".zip") {
            let mut a = ZipArchive::from_bytes(data).unwrap();
            let mut e = ZipArchive::from_bytes(expected[&path].clone()).unwrap();
            let collect = |zip: &mut ZipArchive| -> BTreeMap<String, String> {
                (0..zip.len())
                    .map(|i| {
                        (
                            zip.file_name(i).unwrap(),
                            normalized(&zip.read_file(i).unwrap()),
                        )
                    })
                    .collect()
            };
            assert_eq!(collect(&mut a), collect(&mut e), "{path}");
        } else {
            assert_eq!(normalized(&data), normalized(&expected[&path]), "{path}");
        }
    }
}

#[test]
fn null_selections_empty_boards_and_missing_references() {
    use librepcb_core::project::{Mutation, OutputJobEvent};
    let mut project = load("Empty Project");
    let board = project.boards()[0].id();
    project.apply(Mutation::RemoveBoard(board)).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let netlist = job(JobKind::Netlist {
        boards: ObjectSet::Default,
        output: "none.d356".into(),
    });
    let bom = job(JobKind::Bom {
        custom_attributes: vec![],
        boards: ObjectSet::Default,
        variants: ObjectSet::Default,
        output: "generic.csv".into(),
    });
    let mut events = Vec::new();
    OutputJobRunner::new(&mut project, &out(&temp), "test", Timestamp::now())
        .run_with_observer(&[netlist, bom], |e| events.push(e))
        .unwrap();
    assert!(!temp.path().join("none.d356").exists());
    assert!(temp.path().join("generic.csv").exists());
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OutputJobEvent::Warning(_)))
    );
    let invalid = job(JobKind::Netlist {
        boards: ObjectSet::Custom(BTreeSet::from([board.0])),
        output: "bad.d356".into(),
    });
    assert!(matches!(
        OutputJobRunner::new(&mut project, &out(&temp), "test", Timestamp::now()).run(&[invalid]),
        Err(Error::BoardNotFound(_))
    ));
}
#[test]
fn lppz_contains_current_state_and_excludes_outputs_logs_and_user_settings() {
    use librepcb_core::project::Mutation;
    let mut project = load("Empty Project");
    let mut metadata = project.metadata().clone();
    metadata.name = "Current state".parse().unwrap();
    project
        .apply(Mutation::SetProjectMetadata(metadata))
        .unwrap();
    let fs = project.directory().file_system();
    for name in [
        "output/huge.bin",
        "logs/log.txt",
        "boards/default/settings.user.lp",
        "resources/new.txt",
    ] {
        fs.write(name, b"data").unwrap();
    }
    let temp = tempfile::tempdir().unwrap();
    let job = job(JobKind::Lppz {
        output: "project.lppz".into(),
    });
    OutputJobRunner::new(&mut project, &out(&temp), "test", Timestamp::now())
        .run(&[job])
        .unwrap();
    let mut zip = ZipArchive::open(&out(&temp).path_to("project.lppz")).unwrap();
    assert!(
        String::from_utf8(
            zip.read_file_by_name("project/metadata.lp")
                .unwrap()
                .unwrap()
        )
        .unwrap()
        .contains("Current state")
    );
    assert_eq!(
        zip.read_file_by_name("resources/new.txt").unwrap().unwrap(),
        b"data"
    );
    for name in [
        "output/huge.bin",
        "logs/log.txt",
        "boards/default/settings.user.lp",
    ] {
        assert!(zip.read_file_by_name(name).unwrap().is_none());
    }
    assert!(
        !project
            .directory()
            .file_system()
            .check_for_modifications()
            .unwrap()
            .is_empty()
    );
}
