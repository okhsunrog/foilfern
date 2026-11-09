//! Indexed graphics output, selections, plane freshness and oracle contracts.
use librepcb_core::{
    fileio::{
        FilePath, FileSystem, TransactionalDirectory, TransactionalFileSystem, ZipArchive,
        file_utils,
    },
    job::{ArchiveOutputJob, GraphicsOutputJob, ObjectSet, OutputJob, OutputJobKind},
    project::{OutputJobRunner, Project, ProjectLoader},
    types::{Color, Uuid},
};
use librepcb_graphics::{GraphicsSettings, board_drawing, graphics_job_files};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

fn make_runner<'a>(project: &'a mut Project, dir: &FilePath) -> OutputJobRunner<'a> {
    let mut runner = OutputJobRunner::new(
        project,
        librepcb_core::project::board::ExportInfo::now("test"),
    )
    .unwrap();
    runner.set_output_directory(dir);
    runner
}
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
fn settings(job: &mut OutputJob) -> &mut GraphicsOutputJob {
    let OutputJobKind::Graphics(settings) = job.kind_mut() else {
        panic!("graphics fixture")
    };
    settings
}
fn renderer(runner: &mut OutputJobRunner<'_>) {
    runner.set_graphics_exporter(Some(Arc::new(
        librepcb_graphics::ProjectGraphicsExporter::new("test".into()),
    )));
}

#[test]
fn multipage_collision_preserves_owned_files_before_any_page_is_written() {
    use librepcb_core::job::CopyOutputJob;
    let mut project = load("Project With Two Boards");
    let mut graphics = configured("Schematic PDF");
    settings(&mut graphics).output_path = "pages.svg".into();
    settings(&mut graphics).content[0].boards = ObjectSet::All;
    let copy = OutputJob::new(
        Uuid::new_random(),
        "Owned file".parse().unwrap(),
        CopyOutputJob {
            substitute_variables: false,
            boards: ObjectSet::custom([None]),
            assembly_variants: ObjectSet::custom([None]),
            input_path: "project/metadata.lp".into(),
            output_path: "pages2.svg".into(),
        },
    );
    let temp = tempfile::tempdir().unwrap();
    let output = FilePath::new(temp.path()).unwrap();
    let mut runner = make_runner(&mut project, &output);
    renderer(&mut runner);
    runner.run(&[copy]).unwrap();
    let owned = std::fs::read(temp.path().join("pages2.svg")).unwrap();
    assert!(runner.run(&[graphics]).is_err());
    assert_eq!(
        std::fs::read(temp.path().join("pages2.svg")).unwrap(),
        owned
    );
    assert!(!temp.path().join("pages1.svg").exists());
}

#[test]
fn multipage_outputs_are_indexed_archived_and_cleaned_after_configuration_changes() {
    let mut project = load("Project With Two Boards");
    let mut job = configured("Schematic PDF");
    let graphics = settings(&mut job);
    graphics.output_path = "schematic.svg".into();
    graphics.content[0].boards = ObjectSet::All;
    let archive = OutputJob::new(
        Uuid::new_random(),
        "Archive".parse().unwrap(),
        ArchiveOutputJob {
            input_jobs: BTreeMap::from([(job.uuid(), "pages".into())]),
            output_path: "pages.zip".into(),
        },
    );
    let temp = tempfile::tempdir().unwrap();
    let output = FilePath::new(temp.path()).unwrap();
    let mut runner = make_runner(&mut project, &output);
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
    let mut runner = make_runner(&mut project, &output);
    renderer(&mut runner);
    runner.run(std::slice::from_ref(&job)).unwrap();
    assert!(output.path_to("schematic.svg").is_existing_file());
    assert!(!output.path_to("schematic1.svg").is_existing_file());
    assert!(!output.path_to("schematic2.svg").is_existing_file());
    drop(runner);
    let mut duplicate = job.clone();
    duplicate.set_uuid(Uuid::new_random());
    let mut runner = make_runner(&mut project, &output);
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
        graphics.output_path = "planes.svg".into();
        let temp = tempfile::tempdir().unwrap();
        let mut runner = make_runner(&mut project, &FilePath::new(temp.path()).unwrap());
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
        make_runner(&mut project, &output).run(std::slice::from_ref(&job)),
        Err(librepcb_core::project::OutputJobError::Unsupported(_))
    ));
    settings(&mut job).content[0].boards = ObjectSet::Custom(BTreeSet::from([Some(
        librepcb_core::project::BoardId(Uuid::new_random()),
    )]));
    let mut runner = make_runner(&mut project, &output);
    renderer(&mut runner);
    assert!(matches!(
        runner.run(&[job]),
        Err(librepcb_core::project::OutputJobError::BoardNotFound(_))
    ));
    drop(runner);
    let mut rendering = configured("Board Rendering PDF");
    let graphics = settings(&mut rendering);
    graphics.content[0].content_type = librepcb_core::job::GraphicsContentType::AssemblyGuide;
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
    let mut runner = make_runner(&mut project, &actual);
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
