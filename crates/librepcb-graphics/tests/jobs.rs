//! Indexed graphics output, selections, plane freshness and oracle contracts.
use librepcb_core::{
    export::Timestamp,
    fileio::{
        FilePath, FileSystem, TransactionalDirectory, TransactionalFileSystem, ZipArchive,
        file_utils,
    },
    job::{GraphicsJob, JobKind, ObjectSet, OutputJob},
    project::{GraphicsJobFile, OutputJobRunner, Project, ProjectLoader},
    types::{Color, Uuid},
};
use librepcb_graphics::{GraphicsSettings, board_drawing, graphics_job_files};
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
fn configured(name: &str) -> OutputJob {
    load("Project With Two Boards")
        .output_jobs()
        .iter()
        .find(|j| j.name().as_str() == name)
        .unwrap()
        .clone()
}
fn settings(job: &mut OutputJob) -> &mut GraphicsJob {
    let JobKind::Graphics(settings) = &mut job.kind else {
        panic!("graphics fixture")
    };
    settings
}
fn renderer(runner: &mut OutputJobRunner<'_>) {
    runner.set_graphics_exporter(Some(Box::new(|project, job| {
        graphics_job_files(project, job)
            .map(|files| {
                files
                    .into_iter()
                    .map(|f| GraphicsJobFile {
                        name: f.name,
                        contents: f.contents,
                    })
                    .collect()
            })
            .map_err(|e| librepcb_core::job::Error::Graphics(e.to_string()))
    })));
}

#[test]
fn multipage_outputs_are_indexed_archived_and_cleaned_after_configuration_changes() {
    let mut project = load("Project With Two Boards");
    let mut job = configured("Schematic PDF");
    let graphics = settings(&mut job);
    graphics.output = "schematic.svg".into();
    graphics.content[0].boards = ObjectSet::All;
    let archive = OutputJob::new(
        Uuid::new_random(),
        "Archive".parse().unwrap(),
        JobKind::Archive {
            inputs: BTreeMap::from([(job.uuid(), "pages".into())]),
            output: "pages.zip".into(),
        },
    );
    let temp = tempfile::tempdir().unwrap();
    let output = FilePath::new(temp.path()).unwrap();
    let mut runner = OutputJobRunner::new(&mut project, &output, "test", Timestamp::now());
    renderer(&mut runner);
    runner.run(&[job.clone(), archive]).unwrap();
    assert_eq!(runner.written_files()[&job.uuid()].len(), 2);
    let mut zip = ZipArchive::open(&output.path_to("pages.zip")).unwrap();
    for name in ["schematic1.svg", "schematic2.svg"] {
        let actual = std::fs::read(temp.path().join(name)).unwrap();
        assert_eq!(
            zip.read_file_by_name(&format!("pages/{name}"))
                .unwrap()
                .unwrap(),
            actual
        );
    }
    drop(runner);
    settings(&mut job).content[0].boards = ObjectSet::Default;
    let mut runner = OutputJobRunner::new(&mut project, &output, "test", Timestamp::now());
    renderer(&mut runner);
    runner.run(std::slice::from_ref(&job)).unwrap();
    assert!(output.path_to("schematic.svg").is_existing_file());
    assert!(!output.path_to("schematic1.svg").is_existing_file());
    assert!(!output.path_to("schematic2.svg").is_existing_file());
    drop(runner);
    let mut duplicate = job.clone();
    duplicate.set_uuid(Uuid::new_random());
    let mut runner = OutputJobRunner::new(&mut project, &output, "test", Timestamp::now());
    renderer(&mut runner);
    assert!(runner.run(&[job, duplicate]).is_err());
}

#[test]
fn board_jobs_rebuild_planes_and_drawing_does_not_mutate_the_model() {
    for job_name in ["Board Assembly PDF", "Board Rendering PDF"] {
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
        let mut job = configured(job_name);
        let graphics = settings(&mut job);
        graphics.content.truncate(1);
        graphics.content[0].layers = BTreeMap::from([
            ("board_copper_top".into(), Color::rgb(255, 0, 0)),
            ("board_pads".into(), Color::rgb(255, 0, 0)),
            ("board_vias".into(), Color::rgb(255, 0, 0)),
        ]);
        graphics.output = "planes.svg".into();
        let temp = tempfile::tempdir().unwrap();
        let mut runner = OutputJobRunner::new(
            &mut project,
            &FilePath::new(temp.path()).unwrap(),
            "test",
            Timestamp::now(),
        );
        renderer(&mut runner);
        runner.run(std::slice::from_ref(&job)).unwrap();
        drop(runner);
        assert!(
            project
                .board(board)
                .unwrap()
                .derived()
                .plane_fragments()
                .values()
                .any(|p| !p.is_empty())
        );
        let svg = std::fs::read_to_string(temp.path().join("planes.svg")).unwrap();
        assert!(svg.contains("fill-rule=\"evenodd\""));
        assert!(svg.contains("#ff0000"));
        let revision = project.revision();
        let board = project.board(board).unwrap();
        let drawing = board_drawing(&project, board, &GraphicsSettings::default()).unwrap();
        assert!(!drawing.is_empty());
        assert_eq!(project.revision(), revision);
    }
}

#[test]
fn invalid_selections_and_unavailable_content_fail_explicitly() {
    let mut project = load("Empty Project");
    let mut job = configured("Board Assembly PDF");
    let temp = tempfile::tempdir().unwrap();
    let output = FilePath::new(temp.path()).unwrap();
    assert!(matches!(
        OutputJobRunner::new(&mut project, &output, "test", Timestamp::now())
            .run(std::slice::from_ref(&job)),
        Err(librepcb_core::job::Error::DeferredJob(_))
    ));
    settings(&mut job).content[0].boards =
        ObjectSet::Custom(BTreeSet::from([Some(Uuid::new_random())]));
    let mut runner = OutputJobRunner::new(&mut project, &output, "test", Timestamp::now());
    renderer(&mut runner);
    assert!(matches!(
        runner.run(&[job]),
        Err(librepcb_core::job::Error::BoardNotFound(_))
    ));
    drop(runner);
    let mut rendering = configured("Board Rendering PDF");
    let graphics = settings(&mut rendering);
    graphics.content[0].kind = librepcb_core::job::GraphicsContentType::AssemblyGuide;
    assert!(
        matches!(graphics_job_files(&project, graphics), Err(librepcb_graphics::Error::UnsupportedContent(ref kind)) if kind == "assembly_guide")
    );
}

#[test]
fn schematic_board_and_realistic_pdf_jobs_match_official_cli_names_and_page_counts() {
    let configured_cli = std::env::var_os("LIBREPCB_CLI");
    let cli = std::path::PathBuf::from(
        configured_cli
            .clone()
            .unwrap_or_else(|| "librepcb-cli".into()),
    );
    if !std::process::Command::new(&cli)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        assert!(configured_cli.is_none(), "configured CLI unavailable");
        return;
    }
    use librepcb_core::serialization::{Mode, SExpression, SerializeObject};
    let mut project = load("Gerber Test");
    let jobs = [
        configured("Schematic PDF"),
        configured("Board Assembly PDF"),
        configured("Board Rendering PDF"),
    ];
    let temp = tempfile::tempdir().unwrap();
    let root = FilePath::new(temp.path()).unwrap();
    let mut config = SExpression::list("librepcb_jobs");
    for job in &jobs {
        job.serialize(config.as_list_mut().unwrap().append_list("job"));
    }
    file_utils::write_file(
        &root.path_to("jobs.lp"),
        &config.to_byte_array(Mode::LibrePcb).unwrap(),
    )
    .unwrap();
    file_utils::copy_dir_recursively(
        &project.directory().abs_path("").unwrap(),
        &root.path_to("project"),
    )
    .unwrap();
    let output = std::process::Command::new(cli)
        .args(["open-project", "--run-jobs", "--jobs"])
        .arg(root.path_to("jobs.lp").as_ref())
        .arg("--outdir")
        .arg(root.path_to("oracle").as_ref())
        .arg(
            root.path_to(&format!("project/{}", project.file_name()))
                .as_ref(),
        )
        .env("LC_ALL", "C")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let actual = root.path_to("actual");
    let mut runner = OutputJobRunner::new(&mut project, &actual, "test", Timestamp::now());
    renderer(&mut runner);
    runner.run(&jobs).unwrap();
    for (job, pages) in jobs.iter().zip([1, 2, 2]) {
        let paths = &runner.written_files()[&job.uuid()];
        assert_eq!(paths.len(), 1);
        let name = paths[0].file_name();
        let expected =
            lopdf::Document::load(root.path_to(&format!("oracle/{name}")).as_ref()).unwrap();
        let actual = lopdf::Document::load(paths[0].as_ref()).unwrap();
        assert_eq!(actual.get_pages().len(), pages);
        assert_eq!(actual.get_pages().len(), expected.get_pages().len());
    }
}
