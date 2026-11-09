//! Renders the main window headless with an upstream test project and
//! checks that the home tab, the board and the schematic show up.
//!
//! Runs on the UI thread of the test binary
//! ([`with_headless()`](crate::common::with_headless)). The screenshots are
//! written to `CARGO_TARGET_TMPDIR` for inspection.

use std::path::{Path, PathBuf};

use librepcb_app::screenshot::write_png;
use librepcb_app::{App, InitialTab, startup, ui};
use librepcb_core::project::Mutation;
use librepcb_core::types::ElementName;
use slint::{ComponentHandle, Model as _, Rgb8Pixel};

const WIDTH: u32 = 1200;
const HEIGHT: u32 = 800;

fn upstream_project(name: &str) -> PathBuf {
    Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/projects")
        .join(name)
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let target = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn pixel(pixels: &[Rgb8Pixel], x: u32, y: u32) -> (u8, u8, u8) {
    let p = pixels[(y * WIDTH + x) as usize];
    (p.r, p.g, p.b)
}

/// Pixels of the canvas area (right of the side panel, below the tab bar)
/// that are neither background nor gray (grid): items in layer colors.
fn colored_canvas_pixels(pixels: &[Rgb8Pixel]) -> usize {
    let mut count = 0;
    for y in 100..HEIGHT - 40 {
        for x in 500..WIDTH - 60 {
            let (r, g, b) = pixel(pixels, x, y);
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            if max - min > 40 {
                count += 1;
            }
        }
    }
    count
}

fn save(dir: &Path, name: &str, pixels: &[Rgb8Pixel]) {
    let path = dir.join(name);
    write_png(&path, WIDTH, HEIGHT, pixels).unwrap();
    println!("screenshot: {}", path.display());
}

#[test]
fn main_window_screenshots() {
    crate::common::with_headless(WIDTH, HEIGHT, |headless| {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("project");
        copy_dir(&upstream_project("Gerber Test"), &project_dir);
        let workspace = startup::open_workspace(&tmp.path().join("workspace")).unwrap();
        librepcb_i18n::set_language("en").unwrap();
        let window = ui::AppWindow::new().unwrap();
        slint::select_bundled_translation("en").unwrap();
        let app = App::new(window, workspace);
        app.window().show().unwrap();
        let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));

        // Home tab.
        let home = headless.settle(30);
        save(&out, "app_home.png", &home);
        // The side bar has the dark theme's window color.
        assert_eq!(pixel(&home, 10, 400), (0x35, 0x35, 0x35));
        assert!(
            home.iter()
                .filter(|p| (p.r, p.g, p.b) == (0x2a, 0x2a, 0x2a))
                .count()
                > 10_000,
            "tab content background expected"
        );

        // Opening the project opens its schematic and board tabs.
        let index = app.open_project(&project_dir.join("project.lpp"));
        assert_eq!(index, Some(0));
        {
            let state = app.state().borrow();
            assert_eq!(state.projects().len(), 1);
            assert_eq!(state.sections().len(), 1);
            assert_eq!(state.sections()[0].tabs().len(), 3);
        }

        app.show_tab(InitialTab::Board);
        let board = headless.settle(30);
        save(&out, "app_board.png", &board);
        let colored = colored_canvas_pixels(&board);
        assert!(
            colored > 500,
            "board content expected, got {colored} colored pixels"
        );
        {
            let data = app.window().global::<ui::Data>();
            assert_eq!(data.get_current_tab().r#type, ui::TabType::Board2d);
            assert!(data.get_current_tab().layers.row_count() > 10);
        }

        // Zooming in changes the image.
        {
            let backend = app.window().global::<ui::Backend>();
            backend.invoke_trigger_tab(0, 2, ui::TabAction::ZoomIn);
        }
        let zoomed = headless.settle(30);
        assert_ne!(colored_canvas_pixels(&zoomed), colored);

        app.show_tab(InitialTab::Schematic);
        let schematic = headless.settle(30);
        save(&out, "app_schematic.png", &schematic);
        // White schematic background.
        let white = schematic
            .iter()
            .filter(|p| (p.r, p.g, p.b) == (255, 255, 255))
            .count();
        assert!(
            white > (WIDTH * HEIGHT / 4) as usize,
            "{white} white pixels"
        );
        let data = app.window().global::<ui::Data>();
        assert_eq!(data.get_current_tab().r#type, ui::TabType::Schematic);
        assert_eq!(data.get_current_tab().title, "Main");

        // A modification from elsewhere (e.g. the MCP server) reaches the tab
        // through the change journal.
        {
            let state = app.state().borrow();
            let mut project = state.projects()[0].shared().lock();
            let mut props = project.project().schematics()[0].properties();
            props.name = ElementName::new("Renamed").unwrap();
            project
                .editor
                .apply_mutations("Rename", vec![Mutation::UpdateSchematic(props)])
                .unwrap();
        }
        // The tabs poll their project every 250 ms.
        std::thread::sleep(std::time::Duration::from_millis(300));
        headless.settle(5);
        assert_eq!(data.get_current_tab().title, "Renamed");
        assert!(data.get_current_tab().features.undo == ui::FeatureState::Enabled);
    });
}
