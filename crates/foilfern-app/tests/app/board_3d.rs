//! Actual Bevy pixels in the Slint board tab, project edits and shared undo.
use foilfern_app::{
    App, InitialTab,
    screenshot::{Headless, write_png},
    startup, ui,
};
use foilfern_core::{
    project::{BoardMutation, Mutation},
    types::PcbColor,
};
use slint::{ComponentHandle, Model, Rgb8Pixel};
use std::{path::Path, time::Duration};

fn frame(headless: &Headless, app: &App) -> Vec<Rgb8Pixel> {
    headless.settle(3);
    assert!(
        headless.run_until(Duration::from_secs(60), || {
            headless.settle(1);
            !app.has_pending_3d_frames()
        }),
        "GPU render timed out"
    );
    assert!(
        app.window()
            .global::<ui::Data>()
            .get_current_board_3d_tab()
            .error
            .is_empty()
    );
    headless.settle(3)
}

#[test]
#[ignore = "requires an offscreen GPU; run explicitly on a GPU host"]
fn board_3d_navigation_materials_and_undo() {
    crate::common::with_headless(1200, 800, |headless| {
        let tmp = tempfile::tempdir().unwrap();
        let (project, _, board) = crate::common::create_project(&tmp.path().join("project"));
        let path = project.path().as_path().to_path_buf();
        drop(project); // Release the fixture's directory lock before opening it in the app.
        let workspace = startup::open_workspace(&tmp.path().join("workspace")).unwrap();
        let app = App::new(ui::AppWindow::new().unwrap(), workspace);
        app.window().show().unwrap();
        app.open_project(&path).unwrap();
        let backend = app.window().global::<ui::Backend>();
        backend.invoke_trigger_board(0, 0, ui::BoardAction::Open3d);
        let top = frame(headless, &app);
        let data = app.window().global::<ui::Data>();
        assert_eq!(data.get_current_tab().r#type, ui::TabType::Board3d);
        let index = data.get_current_section().current_tab_index;
        write_png(
            &Path::new(env!("CARGO_TARGET_TMPDIR")).join("app_board3d.png"),
            1200,
            800,
            &top,
        )
        .unwrap();

        // Navigation must alter the presented image without changing the project.
        let opened = app.state().borrow().projects()[0].clone();
        let revision = opened.revision();
        backend.invoke_trigger_tab(0, index, ui::TabAction::ZoomIn);
        assert_ne!(frame(headless, &app), top);
        backend.invoke_trigger_tab(0, index, ui::TabAction::LayersBottom);
        let bottom = frame(headless, &app);
        assert_ne!(bottom, top);
        assert_eq!(opened.revision(), revision);
        backend.invoke_trigger_tab(0, index, ui::TabAction::ZoomFit);
        let original = frame(headless, &app);

        // A material slider is written through the actual per-tab Slint model.
        let model = data.get_current_section().board_3d_tabs;
        let mut row = model.row_data(index as usize).unwrap();
        row.solderresist_alpha = 0.;
        model.set_row_data(index as usize, row);
        assert_ne!(frame(headless, &app), original);
        let mut row = model.row_data(index as usize).unwrap();
        row.solderresist_alpha = 1.;
        model.set_row_data(index as usize, row);
        frame(headless, &app);

        // Changes through the same command bus as MCP replace the owned scene.
        {
            let mut shared = opened.shared().lock();
            let mut settings = shared.project().board(board).unwrap().settings().clone();
            settings.solder_resist = Some(PcbColor::Red);
            shared
                .editor
                .apply_mutations(
                    "Change solder resist",
                    vec![Mutation::Board(BoardMutation::SetSettings {
                        board,
                        settings: Box::new(settings),
                    })],
                )
                .unwrap();
        }
        assert!(headless.run_until(Duration::from_secs(5), || {
            headless.settle(1);
            data.get_current_tab().unsaved_changes && app.has_pending_3d_frames()
        }));
        let red = frame(headless, &app);
        assert_ne!(red, original);
        let red_pixels = |pixels: &[Rgb8Pixel]| {
            pixels
                .iter()
                .filter(|p| u16::from(p.r) > u16::from(p.g) * 2 && p.r > 80 && p.r > p.b)
                .count()
        };
        assert!(
            red_pixels(&red) > red_pixels(&original) + 500,
            "new solder resist must reach the rendered board"
        );
        backend.invoke_trigger_tab(0, index, ui::TabAction::Undo);
        let undone = frame(headless, &app);
        assert!(red_pixels(&undone) + 500 < red_pixels(&red));

        backend.invoke_trigger_board(0, 0, ui::BoardAction::Open2d);
        headless.settle(5);
        assert_eq!(data.get_current_tab().r#type, ui::TabType::Board2d);
        app.show_tab(InitialTab::Board3d);
        backend.invoke_trigger_tab(0, index, ui::TabAction::Close);
        headless.settle(5);
        assert!(
            !app.state().borrow().sections()[0]
                .tabs()
                .iter()
                .any(|t| matches!(t, foilfern_app::tabs::Tab::Board3d(_)))
        );
    });
}
