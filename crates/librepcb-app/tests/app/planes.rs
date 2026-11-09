//! Plane rebuilds of the boards shown in board tabs: automatic after
//! opening a board tab and after modifications, and "Rebuild All Planes".

use std::path::Path;
use std::time::Duration;

use librepcb_app::{App, InitialTab, startup, ui};
use slint::ComponentHandle;

const TIMEOUT: Duration = Duration::from_secs(120);

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

#[test]
fn planes_are_rebuilt_in_board_tabs() {
    crate::common::with_headless(1200, 800, |headless| {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("project");
        copy_dir(
            &Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data/projects/Gerber Test"),
            &project_dir,
        );
        let workspace = startup::open_workspace(&tmp.path().join("workspace")).unwrap();
        librepcb_i18n::set_language("en").unwrap();
        let window = ui::AppWindow::new().unwrap();
        slint::select_bundled_translation("en").unwrap();
        let app = App::new(window, workspace);
        app.window().show().unwrap();
        assert_eq!(app.open_project(&project_dir.join("project.lpp")), Some(0));
        let project = app.state().borrow().projects()[0].clone();
        let board = project.board_at(0).unwrap().0;
        let fragments = |app: &App| {
            let _ = app;
            project
                .shared()
                .lock()
                .project()
                .board(board)
                .unwrap()
                .derived()
                .plane_fragments()
                .values()
                .map(Vec::len)
                .sum::<usize>()
        };
        assert_eq!(fragments(&app), 0);

        // Showing the board tab rebuilds the planes automatically.
        app.show_tab(InitialTab::Board);
        assert!(
            headless.run_until(TIMEOUT, || fragments(&app) > 0),
            "planes not rebuilt"
        );
        headless.settle(10);
        let count = fragments(&app);
        assert!(!project.shared().lock().has_unsaved_changes());

        // "Rebuild All Planes" rebuilds them again (same result).
        let tab = app.window().global::<ui::Data>().get_current_tab();
        assert_eq!(tab.r#type, ui::TabType::Board2d);
        let (si, ti) = {
            let state = app.state().borrow();
            state
                .sections()
                .iter()
                .enumerate()
                .find_map(|(si, s)| {
                    s.tabs()
                        .iter()
                        .position(|t| matches!(t, librepcb_app::tabs::Tab::Board2d(_)))
                        .map(|ti| (si, ti))
                })
                .unwrap()
        };
        let finished = app
            .state()
            .borrow()
            .plane_rebuilds()
            .last_finished(&project, board);
        assert!(finished.is_some());
        app.window().global::<ui::Backend>().invoke_trigger_tab(
            si as i32,
            ti as i32,
            ui::TabAction::PlanesRebuild,
        );
        assert!(
            headless.run_until(TIMEOUT, || {
                let state = app.state().borrow();
                let planes = state.plane_rebuilds();
                !planes.is_running(&project, board)
                    && planes.last_finished(&project, board) != finished
            }),
            "forced rebuild did not finish"
        );
        assert_eq!(fragments(&app), count);
        assert!(!project.shared().lock().has_unsaved_changes());
    });
}
