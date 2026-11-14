//! Interactive assembly jobs: indexing, archives, settings and plane freshness.
use foilfern_core::{
    export::{InteractiveHtmlBomHighlightPin1Mode, InteractiveHtmlBomViewMode},
    fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem, ZipArchive},
    job::{ArchiveOutputJob, InteractiveHtmlBomOutputJob, ObjectSet, OutputJob, OutputJobKind},
    project::OutputJobError as Error,
    project::{OutputJobRunner, Project, ProjectLoader},
    types::{Angle, Uuid},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};
fn make_runner<'a>(project: &'a mut Project, dir: &FilePath) -> OutputJobRunner<'a> {
    let mut runner = OutputJobRunner::new(
        project,
        foilfern_core::project::board::ExportInfo::now("test"),
    )
    .unwrap();
    runner.set_output_directory(dir);
    runner
}
fn load(name: &str) -> Project {
    let path = Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/projects")
        .join(name);
    let fs = Arc::new(TransactionalFileSystem::open_ro(&FilePath::new(path).unwrap()).unwrap());
    let file = fs
        .files("")
        .into_iter()
        .find(|name| name.ends_with(".lpp"))
        .unwrap();
    ProjectLoader::new()
        .open(TransactionalDirectory::new(fs, ""), &file)
        .unwrap()
}
fn configured() -> OutputJob {
    load("Project With Two Boards")
        .output_jobs()
        .iter()
        .find(|job| matches!(job.kind(), OutputJobKind::InteractiveHtmlBom(_)))
        .unwrap()
        .clone()
}
fn config(job: &mut OutputJob) -> &mut InteractiveHtmlBomOutputJob {
    let OutputJobKind::InteractiveHtmlBom(config) = job.kind_mut() else {
        panic!("HTML job fixture");
    };
    config
}
fn decode(html: &str) -> Value {
    let encoded = html
        .split("var pcbdata = JSON.parse(LZString.decompressFromBase64(\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    serde_json::from_str(
        &String::from_utf16(&lz_str::decompress_from_base64(encoded).unwrap()).unwrap(),
    )
    .unwrap()
}
pub(super) fn settings(html: &str) -> Value {
    serde_json::from_str(
        html.split("var config = ")
            .nth(1)
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .trim_end_matches(';'),
    )
    .unwrap()
}
#[test]
fn selections_are_indexed_archived_and_obsolete_html_is_cleaned() {
    let mut project = load("Project With Two Boards");
    let count = project.boards().len() * project.circuit().assembly_variants().len();
    let mut job = configured();
    let cfg = config(&mut job);
    cfg.boards = ObjectSet::All;
    cfg.assembly_variants = ObjectSet::All;
    cfg.output_path = "html/{{BOARD}}_{{VARIANT}}.HTML".into();
    let archive = OutputJob::new(
        Uuid::new_random(),
        "HTML archive".parse().unwrap(),
        ArchiveOutputJob {
            input_jobs: BTreeMap::from([(job.uuid(), "assembly".into())]),
            output_path: "assembly.zip".into(),
        },
    );
    let temp = tempfile::tempdir().unwrap();
    let root = FilePath::new(temp.path()).unwrap();
    let mut runner = make_runner(&mut project, &root);
    runner.run(&[job.clone(), archive]).unwrap();
    let files = runner.written_files()[&job.uuid()].clone();
    assert_eq!(files.len(), count);
    let mut zip = ZipArchive::open(&root.path_to("assembly.zip")).unwrap();
    assert_eq!(zip.len(), count);
    for file in &files {
        let html = std::fs::read(file.as_ref()).unwrap();
        assert_eq!(
            zip.read_file_by_name(&format!("assembly/{}", file.file_name()))
                .unwrap()
                .unwrap(),
            html
        );
        assert!(
            !decode(std::str::from_utf8(&html).unwrap())["metadata"]["title"]
                .as_str()
                .unwrap()
                .is_empty()
        );
    }
    drop(runner);
    let cfg = config(&mut job);
    cfg.boards = ObjectSet::Default;
    cfg.assembly_variants = ObjectSet::Default;
    cfg.output_path = "assembly.xhtml".into();
    make_runner(&mut project, &root)
        .run(std::slice::from_ref(&job))
        .unwrap();
    assert!(root.path_to("assembly.xhtml").is_existing_file());
    assert!(files.iter().all(|path| !path.is_existing_file()));
    let mut duplicate = job.clone();
    duplicate.set_uuid(Uuid::new_random());
    assert!(
        make_runner(&mut project, &root)
            .run(&[job, duplicate])
            .is_err()
    );
}
#[test]
fn html_settings_are_applied_after_rebuilding_planes() {
    let mut project = load("Nested Planes");
    assert!(project.boards()[0].derived().plane_fragments().is_empty());
    let mut job = configured();
    let cfg = config(&mut job);
    cfg.output_path = "assembly.htm".into();
    cfg.view_mode = InteractiveHtmlBomViewMode::TopBottom;
    cfg.highlight_pin1 = InteractiveHtmlBomHighlightPin1Mode::All;
    cfg.dark_mode = true;
    cfg.board_rotation = Angle::DEG45;
    cfg.offset_back_rotation = true;
    cfg.show_silkscreen = false;
    cfg.show_fabrication = false;
    cfg.show_pads = false;
    cfg.check_boxes = vec!["Ready".into(), "Installed".into()];
    cfg.custom_attributes = vec!["MPN".into()];
    let temp = tempfile::tempdir().unwrap();
    make_runner(&mut project, &FilePath::new(temp.path()).unwrap())
        .run(&[job])
        .unwrap();
    assert!(
        project.boards()[0]
            .derived()
            .plane_fragments()
            .values()
            .any(|paths| !paths.is_empty())
    );
    let html = std::fs::read_to_string(temp.path().join("assembly.htm")).unwrap();
    let cfg = settings(&html);
    assert_eq!(cfg["bom_view"], "top-bottom");
    assert_eq!(cfg["highlight_pin1"], "all");
    assert_eq!(cfg["dark_mode"], true);
    assert_eq!(cfg["board_rotation"], 9);
    assert_eq!(cfg["offset_back_rotation"], true);
    for key in ["show_silkscreen", "show_fabrication", "show_pads"] {
        assert_eq!(cfg[key], false);
    }
    assert_eq!(cfg["checkboxes"], "Ready,Installed");
    assert_eq!(
        cfg["fields"],
        serde_json::json!(["Value / MPN", "Package", "MPN"])
    );
    assert!(!decode(&html)["zones"]["F"].as_array().unwrap().is_empty());
}
#[test]
fn invalid_selections_and_extensions_fail_without_creating_a_file() {
    let mut project = load("Empty Project");
    let temp = tempfile::tempdir().unwrap();
    let root = FilePath::new(temp.path()).unwrap();
    let mut job = configured();
    config(&mut job).output_path = "invalid.pdf".into();
    assert!(matches!(
        make_runner(&mut project, &root).run(std::slice::from_ref(&job)),
        Err(Error::UnsupportedInteractiveBomFormat(_))
    ));
    assert!(!root.path_to("invalid.pdf").is_existing_file());
    config(&mut job).output_path = "assembly.html".into();
    config(&mut job).boards = ObjectSet::Custom(BTreeSet::from([foilfern_core::project::BoardId(
        Uuid::new_random(),
    )]));
    assert!(matches!(
        make_runner(&mut project, &root).run(std::slice::from_ref(&job)),
        Err(Error::BoardNotFound(_))
    ));
    config(&mut job).boards = ObjectSet::Default;
    config(&mut job).assembly_variants =
        ObjectSet::Custom(BTreeSet::from([foilfern_core::project::AssemblyVariantId(
            Uuid::new_random(),
        )]));
    assert!(matches!(
        make_runner(&mut project, &root).run(&[job]),
        Err(Error::AssemblyVariantNotFound(_))
    ));
    assert!(!root.path_to("assembly.html").is_existing_file());
}
