//! Renders every schematic page and board (both sides) of the upstream test
//! projects and checks that the images show something. The PNGs are
//! written to `<CARGO_TARGET_TMPDIR>/renders/` for visual inspection.

mod export;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use librepcb_core::application::file_format_version;
use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::project::{Project, ProjectLoader};
use librepcb_scene::librepcb_canvas::peniko::Color;
use librepcb_scene::{
    BoardScene, BoardSceneLayer, BoardSide, ColorScheme, RenderOptions, RenderSize, RgbaImage,
    SchematicScene, render_board_png, render_board_scene, render_schematic_png,
    render_schematic_scene,
};

fn projects_dir() -> PathBuf {
    Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data/projects")
}

fn output_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("renders");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Opens a project (upgrading older formats in memory); returns whether it
/// has the current file format.
fn open(dir: &Path) -> (Project, bool) {
    let version = std::fs::read(dir.join(".librepcb-project")).unwrap();
    let is_current =
        version.split(|&b| b == b'\n').next() == Some(file_format_version().to_string().as_bytes());
    let lpp = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.ends_with(".lpp"))
        .expect("*.lpp file");
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(dir).unwrap()).unwrap();
    let directory = TransactionalDirectory::new(Arc::new(fs), "");
    let project = ProjectLoader::new()
        .open(directory, &lpp)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    (project, is_current)
}

fn projects() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(projects_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join(".librepcb-project").exists())
        .collect();
    dirs.sort();
    dirs
}

/// Number of pixels differing from the background.
fn painted_pixels(img: &RgbaImage, background: Color) -> usize {
    let bg = background.to_rgba8();
    img.data
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| {
            px.iter()
                .zip([bg.r, bg.g, bg.b, bg.a])
                .any(|(a, b)| a.abs_diff(b) > 8)
        })
        .count()
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

const OPTIONS: RenderOptions = RenderOptions {
    size: RenderSize::Fit {
        width: 1200,
        height: 900,
    },
    margin: 20.0,
    background: None,
};

#[test]
fn render_all_test_projects() {
    let out = output_dir();
    let mut current = 0;
    let mut rendered = 0;
    for dir in projects() {
        let (project, is_current) = open(&dir);
        current += usize::from(is_current);
        let name = slug(&dir.file_name().unwrap().to_string_lossy());
        for (index, sch) in project.schematics().iter().enumerate() {
            let scene =
                SchematicScene::build(&project, sch.id(), &ColorScheme::SCHEMATIC_LIGHT).unwrap();
            assert!(
                scene.warnings().is_empty(),
                "{name} schematic {index}: {:?}",
                scene.warnings()
            );
            let img = render_schematic_scene(&scene, &OPTIONS).unwrap();
            assert_eq!(img.data.len(), 1200 * 900 * 4);
            if !scene.scene().is_empty() {
                let painted = painted_pixels(&img, scene.background());
                assert!(painted > 500, "{name} schematic {index}: {painted} pixels");
            }
            std::fs::write(
                out.join(format!("{name}-sch{index}.png")),
                img.to_png().unwrap(),
            )
            .unwrap();
            rendered += 1;
        }
        for (index, board) in project.boards().iter().enumerate() {
            // Both sides of the first board, the top of the others (the DRC
            // project has 30 boards).
            let sides: &[BoardSide] = if index == 0 {
                &[BoardSide::Top, BoardSide::Bottom]
            } else {
                &[BoardSide::Top]
            };
            for &side in sides {
                let mut scene =
                    BoardScene::build(&project, board.id(), side, &ColorScheme::BOARD_DARK)
                        .unwrap();
                assert!(
                    scene.warnings().is_empty(),
                    "{name} board {index}: {:?}",
                    scene.warnings()
                );
                let img = render_board_scene(&mut scene, &OPTIONS).unwrap();
                if !scene.scene().is_empty() {
                    let painted = painted_pixels(&img, scene.background());
                    assert!(painted > 500, "{name} board {index}: {painted} pixels");
                }
                std::fs::write(
                    out.join(format!("{name}-brd{index}-{side:?}.png")),
                    img.to_png().unwrap(),
                )
                .unwrap();
                rendered += 1;
            }
        }
    }
    assert!(current >= 5, "only {current} current-format projects");
    assert!(rendered >= 10, "only {rendered} images");
}

#[test]
fn png_api() {
    let (project, _) = open(&projects_dir().join("Nested Planes"));
    let sch = project.schematics()[0].id();
    let png = render_schematic_png(&project, sch, &RenderOptions::default()).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    let decoder = png::Decoder::new(std::io::Cursor::new(&png));
    let reader = decoder.read_info().unwrap();
    assert_eq!(reader.info().width, 1600);
    assert_eq!(reader.info().height, 1200);

    let board = project.boards()[0].id();
    let png = render_board_png(
        &project,
        board,
        BoardSide::Bottom,
        &RenderOptions {
            size: RenderSize::Dpi(300.0),
            margin: 10.0,
            background: Some(Color::WHITE),
        },
    )
    .unwrap();
    let decoder = png::Decoder::new(std::io::Cursor::new(&png));
    let reader = decoder.read_info().unwrap();
    // A few centimetres at 300 DPI.
    assert!(reader.info().width > 100 && reader.info().width < 16384);
}

#[test]
fn layer_visibility() {
    let (project, _) = open(&projects_dir().join("Gerber Test"));
    let board = project.boards()[0].id();
    let mut scene =
        BoardScene::build(&project, board, BoardSide::Top, &ColorScheme::BOARD_DARK).unwrap();
    let full = render_board_scene(&mut scene, &OPTIONS).unwrap();
    for layer in scene.layers().to_vec() {
        if layer != BoardSceneLayer::Board(librepcb_core::types::Layer::BOARD_OUTLINES) {
            scene.set_scene_layer_visible(layer, false);
        }
    }
    let outline_only = render_board_scene(&mut scene, &OPTIONS).unwrap();
    let bg = scene.background();
    assert!(painted_pixels(&outline_only, bg) > 0);
    assert!(painted_pixels(&outline_only, bg) < painted_pixels(&full, bg));
}
