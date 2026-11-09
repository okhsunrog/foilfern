//! Native CLI contracts, complementing the unmodified upstream pytest suite.
use librepcb_core::fileio::{FilePath, TransactionalFileSystem, ZipArchive, file_utils};
use std::{
    path::Path,
    process::{Command, Output},
};
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_librepcb-cli"))
        .args(args)
        .env("LC_ALL", "C")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .unwrap()
}
#[test]
fn step_commands_validate_and_tessellate_without_a_display() {
    let path = Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/unittests/librepcbcommon/OccModelTest/model.step");
    let bytes = std::fs::read(&path).unwrap();
    let stats = librepcb_step::StepModel::from_step(&bytes)
        .unwrap()
        .tessellation_stats(0.01)
        .unwrap();
    let output = run(&["open-step", "--tesselate", path.to_str().unwrap()]);
    assert!(output.status.success(), "{:?}", output);
    assert!(output.stderr.is_empty());
    assert!(String::from_utf8_lossy(&output.stdout).contains(&format!(
        " - Built {} vertices with {} different colors\nSUCCESS\n",
        stats.vertices, stats.colors
    )));
    let temp = tempfile::tempdir().unwrap();
    let invalid = temp.path().join("invalid.step");
    let saved = temp.path().join("saved.step");
    std::fs::write(&invalid, b"not STEP").unwrap();
    let output = run(&[
        "open-step",
        "--save-to",
        saved.to_str().unwrap(),
        invalid.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(!output.stderr.is_empty());
    assert_eq!(std::fs::read(saved).unwrap(), b"not STEP");
    assert!(
        String::from_utf8_lossy(&output.stdout).ends_with("Load model...\nFinished with errors!\n")
    );
}
#[test]
fn failed_library_model_validation_preserves_the_step_file_on_save() {
    let temp = tempfile::tempdir().unwrap();
    let source = FilePath::new(
        Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
            .join("tests/data/libraries/Populated Library.lplib"),
    )
    .unwrap();
    let library = FilePath::new(temp.path().join("library.lplib")).unwrap();
    file_utils::copy_dir_recursively(&source, &library).unwrap();
    let model = library.path_to(
        "pkg/0eaf289c-166d-4bd9-a4ba-dbf6bbc76ef1/4e198b4d-b61a-47bd-a7ad-39b2f6dc77e9.step",
    );
    // Lexically minifiable, but not a valid geometric STEP model.
    let invalid = b"DATA;\n#1=FOO();\n#2=FOO();\nENDSEC;\n";
    std::fs::write(model.as_ref(), invalid).unwrap();
    let output = run(&[
        "open-library",
        "--all",
        "--minify-step",
        "--save",
        library.as_str(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Failed to minify STEP model"));
    assert_eq!(std::fs::read(model.as_ref()).unwrap(), invalid);
}
fn fixture(temp: &tempfile::TempDir) -> FilePath {
    let src = Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data/projects/Empty Project");
    let dst = FilePath::new(temp.path().join("project")).unwrap();
    file_utils::copy_dir_recursively(&FilePath::new(src).unwrap(), &dst).unwrap();
    dst
}
#[test]
fn project_checks_need_no_display_and_do_not_save() {
    let temp = tempfile::tempdir().unwrap();
    let project = fixture(&temp);
    let fs = TransactionalFileSystem::open_ro(&project).unwrap();
    let before = fs.export_to_zip(None).unwrap();
    let output = run(&[
        "open-project",
        "--strict",
        "--erc",
        "--drc",
        &format!("{}/./Empty Project.lpp", project.as_str()),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .ends_with("SUCCESS\n")
    );
    // Opening a path containing `.` is reported using its canonical lexical form.
    let normalized = run(&[
        "open-project",
        &format!("{}/./Empty Project.lpp", project.as_str()),
    ]);
    assert!(
        String::from_utf8_lossy(&normalized.stdout).starts_with(&format!(
            "Open project '{}'",
            project.path_to("Empty Project.lpp")
        ))
    );
    // Compare uncompressed contents so ZIP metadata cannot hide disk writes.
    let contents = |bytes: Vec<u8>| {
        let mut zip = ZipArchive::from_bytes(bytes).unwrap();
        (0..zip.len())
            .map(|i| (zip.file_name(i).unwrap(), zip.read_file(i).unwrap()))
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    assert_eq!(contents(before), contents(fs.export_to_zip(None).unwrap()));
}
#[test]
fn lppz_save_keeps_sibling_files_outside_the_archive() {
    let temp = tempfile::tempdir().unwrap();
    let project = fixture(&temp);
    let fs = TransactionalFileSystem::open_ro(&project).unwrap();
    let archive = FilePath::new(temp.path().join("project.lppz")).unwrap();
    file_utils::write_file(&archive, &fs.export_to_zip(None).unwrap()).unwrap();
    std::fs::write(temp.path().join("sibling.txt"), "sibling").unwrap();
    let output = run(&[
        "open-project",
        "--remove-other-boards",
        "--save",
        archive.as_str(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut zip = ZipArchive::open(&archive).unwrap();
    assert!(zip.read_file_by_name("sibling.txt").unwrap().is_none());
    assert_eq!(
        zip.read_file_by_name("boards/boards.lp").unwrap().unwrap(),
        b"(librepcb_boards\n)\n"
    );
    assert!(
        project
            .path_to("boards/default/board.lp")
            .is_existing_file()
    );
    assert!(run(&["open-project", archive.as_str()]).status.success());
}
#[test]
fn failed_copy_job_cannot_read_sibling_files() {
    let temp = tempfile::tempdir().unwrap();
    let project = fixture(&temp);
    let config = temp.path().join("jobs.lp");
    std::fs::write(&config,"(librepcb_jobs (job 11111111-1111-4111-8111-111111111111 (name \"Copy\") (type copy) (substitute_variables false) (board none) (variant none) (input \"../sibling.txt\") (output \"copy.txt\")))").unwrap();
    let output = run(&[
        "open-project",
        "--run-jobs",
        "--jobs",
        config.to_str().unwrap(),
        project.path_to("Empty Project.lpp").as_str(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("input file must be located within the project directory")
    );
    assert!(!project.path_to("output/v1/copy.txt").is_existing_file());
}

#[test]
fn schematic_board_and_realistic_graphics_jobs_run_without_a_display() {
    let temp = tempfile::tempdir().unwrap();
    let source = FilePath::new(
        Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
            .join("tests/data/projects/Project With Two Boards"),
    )
    .unwrap();
    let root = FilePath::new(temp.path().join("project")).unwrap();
    file_utils::copy_dir_recursively(&source, &root).unwrap();
    let project = root.path_to("Project With Two Boards.lpp");
    let output = FilePath::new(temp.path().join("graphics")).unwrap();
    let result = run(&[
        "open-project",
        "--run-job",
        "Schematic PDF",
        "--run-job",
        "Board Assembly PDF",
        "--outdir",
        output.as_str(),
        project.as_str(),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    for name in [
        "Empty_Project_v1_Schematic.pdf",
        "Empty_Project_v1_Assembly.pdf",
    ] {
        assert!(
            std::fs::read(output.path_to(name).as_ref())
                .unwrap()
                .starts_with(b"%PDF-")
        );
    }
    let result = run(&[
        "open-project",
        "--run-job",
        "Board Rendering PDF",
        "--outdir",
        output.as_str(),
        project.as_str(),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        output
            .path_to("Empty_Project_v1_Rendering.pdf")
            .is_existing_file()
    );
}

#[test]
fn legacy_export_removes_disabled_files_and_preserves_unrelated_files() {
    let temp = tempfile::tempdir().unwrap();
    let project = fixture(&temp);
    let gerber = project.path_to("output/v1/gerber");
    for name in [
        "Empty_Project_SOLDERPASTE-TOP.gbr",
        "Empty_Project_DRILLS.drl",
        "unrelated.txt",
    ] {
        file_utils::write_file(&gerber.path_to(name), b"old").unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_librepcb-cli"))
        .args([
            "open-project",
            "--export-pcb-fabrication-data",
            project.path_to("Empty Project.lpp").as_str(),
        ])
        .env("LIBREPCB_SUPPRESS_DEPRECATION_WARNINGS", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !gerber
            .path_to("Empty_Project_SOLDERPASTE-TOP.gbr")
            .is_existing_file()
    );
    assert!(
        !gerber
            .path_to("Empty_Project_DRILLS.drl")
            .is_existing_file()
    );
    assert!(gerber.path_to("unrelated.txt").is_existing_file());
    assert!(
        gerber
            .path_to("Empty_Project_DRILLS-PTH.drl")
            .is_existing_file()
    );
}
