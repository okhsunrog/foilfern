//! Port of the upstream `OutputJobRunnerTest`
//! (tests/unittests/core/project/outputjobrunnertest.cpp).

use std::sync::Arc;

use librepcb_core::fileio::{
    FilePath, TransactionalDirectory, TransactionalFileSystem, file_utils,
};
use librepcb_core::job::{BomOutputJob, CopyOutputJob, GerberExcellonOutputJob, OutputJob};
use librepcb_core::project::board::{Board, BoardItem, BoardPlane, BoardPolygonData, ExportInfo};
use librepcb_core::project::{
    GraphicsExportResult, GraphicsExporter, GraphicsPage, OutputJobRunner, Project,
};
use librepcb_core::types::{ElementName, Layer, Length, PositiveLength, UnsignedLength, Uuid};

use crate::helpers::TempDir;

fn rect() -> librepcb_core::geometry::Path {
    let size = PositiveLength::new(Length::new(5_000_000)).unwrap();
    librepcb_core::geometry::Path::centered_rect(size, size, UnsignedLength::ZERO)
}

fn create_project(dir: &FilePath) -> Project {
    let fs = TransactionalFileSystem::open_rw(dir).unwrap();
    Project::create(
        TransactionalDirectory::new(Arc::new(fs), ""),
        "project.lpp",
        Uuid::new_random,
    )
    .unwrap()
}

fn info() -> ExportInfo {
    ExportInfo::now("0.0.0")
}

fn bom_job(output_path: &str) -> OutputJob {
    let job = BomOutputJob {
        output_path: output_path.into(),
        ..BomOutputJob::default()
    };
    OutputJob::new(Uuid::new_random(), ElementName::new("BOM").unwrap(), job)
}

// Very important: Make sure the Gerber/Excellon output job rebuilds any
// outdated planes before exporting.
#[test]
fn test_gerber_excellon_rebuilds_planes() {
    let tmp = TempDir::new();
    let out_dir = tmp.path().path_to("out");
    let mut project = create_project(&tmp.path().path_to("project"));
    let mut board = Board::new(
        Uuid::new_random(),
        ElementName::new("New Board").unwrap(),
        "board",
    );
    let mut settings = board.settings().clone();
    settings.inner_layer_count = 2;
    board.set_settings(settings);
    let board = project.add_board(board, None).unwrap();
    project
        .add_board_item(
            board,
            BoardItem::Polygon(BoardPolygonData::new(
                Uuid::new_random(),
                Layer::BOARD_OUTLINES,
                UnsignedLength::ZERO,
                rect(),
                false,
                false,
                false,
            )),
        )
        .unwrap();
    let plane = BoardPlane::new(
        Uuid::new_random(),
        Layer::inner_copper(1).unwrap(),
        None,
        rect(),
    );
    let plane_id = plane.id();
    project.add_board_plane(board, plane).unwrap();

    let job = GerberExcellonOutputJob::protel_style();
    let fragments = |p: &Project| {
        p.board(board)
            .unwrap()
            .derived()
            .fragments_of(plane_id)
            .len()
    };
    assert_eq!(fragments(&project), 0);

    let mut runner = OutputJobRunner::new(&mut project, info()).unwrap();
    runner.set_output_directory(&out_dir);
    runner.run(&[job]).unwrap();
    drop(runner);

    assert!(fragments(&project) > 0);

    let content = file_utils::read_file(&out_dir.path_to("gerber/Unnamed_v1.g1")).unwrap();
    let content = String::from_utf8(content).unwrap();
    assert!(content.contains("\nG36*\n"));
    assert!(content.contains("\nG37*\n"));
}

// Very important: For portability reasons, no absolute file paths are
// allowed!
#[test]
fn test_absolute_output_file_path() {
    let tmp = TempDir::new();
    let out_dir = tmp.path().path_to("out");
    let mut project = create_project(&tmp.path().path_to("project"));

    let mut job = bom_job(out_dir.path_to("bom.csv").as_str());
    let mut runner = OutputJobRunner::new(&mut project, info()).unwrap();
    runner.set_output_directory(&out_dir);
    assert!(runner.run(std::slice::from_ref(&job)).is_err());
    assert!(!out_dir.path_to("bom.csv").is_existing_file());

    // Verify that a valid job would succeed.
    job = bom_job("bom.csv");
    runner.run(&[job]).unwrap();
}

// Very important: For security reasons, no write access outside the output
// directory is allowed!
#[test]
fn test_output_folder_breakout() {
    let tmp = TempDir::new();
    let out_dir = tmp.path().path_to("out");
    let mut project = create_project(&tmp.path().path_to("project"));

    let job = bom_job("../bom.csv");
    let mut runner = OutputJobRunner::new(&mut project, info()).unwrap();
    runner.set_output_directory(&out_dir);
    assert!(runner.run(&[job]).is_err());
    assert!(!out_dir.path_to("../bom.csv").is_existing_file());

    // Verify that a valid job would succeed.
    runner.run(&[bom_job("bom.csv")]).unwrap();
}

// Very important: For security reasons, no read access outside the project
// directory is allowed!
#[test]
fn test_project_folder_breakout() {
    let tmp = TempDir::new();
    let project_dir = tmp.path().path_to("project");
    let mut project = create_project(&project_dir);
    project.save().unwrap();
    project.directory().file_system().save().unwrap();

    let out_dir = project_dir.path_to("out");
    let input_fp = tmp.path().path_to(".librepcb-project");
    file_utils::write_file(&input_fp, b"foo").unwrap();

    let copy_job = |input: &str, output: &str| {
        let job = CopyOutputJob {
            input_path: input.into(),
            output_path: output.into(),
            ..CopyOutputJob::default()
        };
        OutputJob::new(Uuid::new_random(), ElementName::new("Copy").unwrap(), job)
    };

    let mut runner = OutputJobRunner::new(&mut project, info()).unwrap();
    runner.set_output_directory(&out_dir);
    assert!(
        runner
            .run(&[copy_job("../.librepcb-project", "out.txt")])
            .is_err()
    );
    assert!(!out_dir.path_to("out.txt").is_existing_file());

    // Verify that a valid job would succeed.
    runner
        .run(&[copy_job(".librepcb-project", "out2.txt")]) // Avoid "file overwritten" error.
        .unwrap();
    assert!(out_dir.path_to("out2.txt").is_existing_file());
}

/// Not upstream: unsupported and unknown job types fail with an error.
#[test]
fn test_unsupported_job_types() {
    let tmp = TempDir::new();
    let mut project = create_project(&tmp.path().path_to("project"));
    let mut runner = OutputJobRunner::new(&mut project, info()).unwrap();
    runner.set_output_directory(&tmp.path().path_to("out"));
    let graphics = OutputJob::new_default::<librepcb_core::job::GraphicsOutputJob>();
    let err = runner.run(&[graphics]).unwrap_err();
    assert!(err.to_string().contains("'graphics' are not supported yet"));
}

/// Records the pages and reports an additional file per call.
struct RecordingExporter(std::sync::Mutex<Vec<(usize, String, String)>>);

impl GraphicsExporter for RecordingExporter {
    fn export(
        &self,
        _project: &Project,
        pages: &[GraphicsPage],
        file_path: &FilePath,
        document_name: &str,
    ) -> GraphicsExportResult {
        self.0.lock().unwrap().push((
            pages.len(),
            file_path.file_name().to_owned(),
            document_name.to_owned(),
        ));
        let extra = file_path.parent_dir().unwrap().path_to("extra.png");
        GraphicsExportResult {
            written_files: vec![file_path.clone(), extra],
            errors: Vec::new(),
        }
    }
}

/// Not upstream: graphics jobs build the pages and pass them to the
/// exporter; additional files are tracked.
#[test]
fn test_graphics_job_with_exporter() {
    use librepcb_core::job::{GraphicsOutputJob, OutputJobKind};
    let tmp = TempDir::new();
    let mut project = create_project(&tmp.path().path_to("project"));
    let exporter = Arc::new(RecordingExporter(Default::default()));
    let mut runner = OutputJobRunner::new(&mut project, info()).unwrap();
    runner.set_output_directory(&tmp.path().path_to("out"));
    runner.set_graphics_exporter(Some(exporter.clone()));
    let job = GraphicsOutputJob::schematic_pdf();
    let uuid = job.uuid();
    runner.run(&[job]).unwrap();
    assert_eq!(runner.written_files().get(&uuid).unwrap().len(), 2);
    // A new project has no schematics, thus no pages.
    assert_eq!(
        exporter.0.lock().unwrap().as_slice(),
        &[(
            0,
            "Unnamed_v1_Schematic.pdf".to_owned(),
            "Unnamed - v1".to_owned()
        )]
    );

    // Unknown page sizes fail like upstream.
    let mut job = GraphicsOutputJob::schematic_pdf();
    if let OutputJobKind::Graphics(g) = job.kind_mut() {
        g.content[0].page_size = Some("Foo".into());
    }
    let err = runner.run(&[job]).unwrap_err();
    assert_eq!(err.to_string(), "Unsupported page size: 'Foo'");
}

/// Without a CAD exporter, failure must not reserve or create an output file.
#[test]
fn test_board_3d_without_step_export() {
    use librepcb_core::job::Board3DOutputJob;
    use librepcb_core::project::OutputJobError;
    let tmp = TempDir::new();
    let mut project = create_project(&tmp.path().path_to("project"));
    let board = Board::new(
        Uuid::new_random(),
        ElementName::new("default").unwrap(),
        "default",
    );
    project.add_board(board, None).unwrap();
    let job = |path: &str| {
        let job = Board3DOutputJob {
            output_path: path.into(),
            ..Board3DOutputJob::default()
        };
        OutputJob::new(Uuid::new_random(), ElementName::new("STEP").unwrap(), job)
    };
    let mut runner = OutputJobRunner::new(&mut project, info()).unwrap();
    runner.set_output_directory(&tmp.path().path_to("out"));
    let step = job("model.step");
    let uuid = step.uuid();
    let err = runner.run(&[step]).unwrap_err();
    assert!(matches!(&err, OutputJobError::Unsupported(kind) if kind == "3d_model"));
    assert!(runner.written_files().get(&uuid).is_none_or(Vec::is_empty));
    assert!(!tmp.path().path_to("out/model.step").is_existing_file());

    let err = runner.run(&[job("model.wrl")]).unwrap_err();
    assert_eq!(err.to_string(), "Unsupported STEP format: 'wrl'");
}
