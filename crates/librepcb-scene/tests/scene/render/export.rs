//! Graphics export (PDF, SVG, PNG) of the upstream test projects and
//! library elements. The files are written to
//! `<CARGO_TARGET_TMPDIR>/exports/` for visual inspection.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use librepcb_core::export::GraphicsExportSettings;
use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::job::{GraphicsOutputJob, OutputJobKind};
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::library::pkg::Package;
use librepcb_core::library::sym::Symbol;
use librepcb_core::project::{GraphicsExporter, GraphicsPage, GraphicsPageContent};
use librepcb_scene::export::{
    ExportPage, GraphicsExport, ProjectGraphicsExporter, drawing_for_page, footprint_drawing,
    symbol_drawing,
};

use super::{open, projects_dir};

fn output_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("exports")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn fp(path: PathBuf) -> FilePath {
    FilePath::new(path).expect("absolute path")
}

#[test]
fn schematic_export_formats() {
    let (project, _) = open(&projects_dir().join("DRC"));
    let pages: Vec<GraphicsPage> = project
        .schematics()
        .iter()
        .map(|s| GraphicsPage {
            content: GraphicsPageContent::Schematic(s.id()),
            settings: GraphicsExportSettings {
                pixmap_dpi: 100,
                ..GraphicsExportSettings::default()
            },
        })
        .collect();
    let dir = output_dir("schematic");
    let exporter = ProjectGraphicsExporter::new("LibrePCB test");
    for ext in ["pdf", "svg", "png", "jpg", "bmp"] {
        let path = fp(dir.join("nested").join(format!("schematic.{ext}")));
        let result = exporter.export(&project, &pages, &path, "DRC");
        assert_eq!(result.errors, Vec::<String>::new(), "{ext}");
        assert_eq!(result.written_files, vec![path.clone()], "{ext}");
        let content = std::fs::read(path.as_str()).expect("written file");
        assert!(content.len() > 1000, "{ext}");
        if ext == "pdf" {
            assert!(content.starts_with(b"%PDF-"));
        }
    }

    // Unknown extension: upstream error message, no file.
    let path = fp(dir.join("schematic.foo"));
    let result = exporter.export(&project, &pages, &path, "DRC");
    assert_eq!(result.written_files, Vec::<FilePath>::new());
    assert_eq!(result.errors.len(), 1);
    assert!(
        result.errors[0]
            .contains("due to unknown file extension. Supported extensions: pdf, svg, bmp")
    );
    assert!(!Path::new(path.as_str()).exists());
}

#[test]
fn several_pages_get_numbered_files() {
    let (project, _) = open(&projects_dir().join("DRC"));
    let page = GraphicsPage {
        content: GraphicsPageContent::Schematic(project.schematics()[0].id()),
        settings: GraphicsExportSettings {
            pixmap_dpi: 50,
            ..GraphicsExportSettings::default()
        },
    };
    let pages = vec![page.clone(), page];
    let dir = output_dir("pages");
    let exporter = ProjectGraphicsExporter::new("LibrePCB test");
    let result = exporter.export(&project, &pages, &fp(dir.join("sch.svg")), "");
    assert_eq!(
        result.written_files,
        vec![fp(dir.join("sch1.svg")), fp(dir.join("sch2.svg"))]
    );
    // One PDF with all pages.
    let pdf = fp(dir.join("sch.pdf"));
    let result = exporter.export(&project, &pages, &pdf, "");
    assert_eq!(result.written_files, vec![pdf.clone()]);
    let content = std::fs::read(pdf.as_str()).expect("pdf");
    assert!(String::from_utf8_lossy(&content).contains("/Count 2"));

    // No pages.
    let result = exporter.export(&project, &[], &pdf, "");
    assert_eq!(result.errors, vec!["No pages to export/print.".to_owned()]);
}

#[test]
fn board_pages_of_default_jobs() {
    let (project, _) = open(&projects_dir().join("DRC"));
    let board = project.boards()[0].id();
    for job in [
        GraphicsOutputJob::board_assembly_pdf(),
        GraphicsOutputJob::board_rendering_pdf(),
    ] {
        let OutputJobKind::Graphics(job) = job.kind() else {
            panic!("graphics job");
        };
        for content in &job.content {
            let mut settings = GraphicsExportSettings::default();
            if content.content_type == librepcb_core::job::GraphicsContentType::BoardRendering {
                settings.load_board_rendering_colors(0);
            }
            settings
                .colors
                .retain(|(role, _)| content.layers.contains_key(role));
            settings.mirror = content.mirror;
            let page = GraphicsPage {
                content: match content.content_type {
                    librepcb_core::job::GraphicsContentType::BoardRendering => {
                        GraphicsPageContent::BoardRendering(board)
                    }
                    _ => GraphicsPageContent::Board(board),
                },
                settings,
            };
            let drawing = drawing_for_page(&project, &page).expect("drawing");
            assert!(drawing.primitives().len() >= 2, "{}", content.title);
        }
    }
}

#[test]
fn library_elements() {
    let lib = Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/libraries/Populated Library.lplib");
    let open_dir = |p: PathBuf| {
        let fs = TransactionalFileSystem::open_ro(&fp(p)).expect("fs");
        TransactionalDirectory::new(Arc::new(fs), "")
    };
    let font = librepcb_scene::default_stroke_font();
    assert!(font.is_some());
    let settings = GraphicsExportSettings {
        pixmap_dpi: 100,
        ..GraphicsExportSettings::default()
    };
    let dir = output_dir("library");
    let export = GraphicsExport::new("LibrePCB test");

    let symbol = Symbol::open(open_dir(
        lib.join("sym/f00ab942-6980-442b-86a8-51b92de5704d"),
    ))
    .expect("symbol");
    let drawing = symbol_drawing(&symbol, font.as_ref(), &settings);
    assert!(!drawing.is_empty());
    let path = fp(dir.join("symbol.png"));
    let result = export.export(
        &[ExportPage {
            drawing,
            settings: settings.clone(),
        }],
        &path,
    );
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(result.written_files, vec![path]);

    let package = Package::open(open_dir(
        lib.join("pkg/0eaf289c-166d-4bd9-a4ba-dbf6bbc76ef1"),
    ))
    .expect("package");
    for (i, footprint) in package.footprints().iter().enumerate() {
        let drawing = footprint_drawing(footprint, font.as_ref(), &settings);
        assert!(!drawing.is_empty());
        let path = fp(dir.join(format!("footprint{i}.svg")));
        let result = export.export(
            &[ExportPage {
                drawing,
                settings: settings.clone(),
            }],
            &path,
        );
        assert_eq!(result.errors, Vec::<String>::new());
    }
}
