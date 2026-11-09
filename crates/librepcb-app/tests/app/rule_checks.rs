//! Rule check runs of the application remove the approvals of messages
//! which disappeared (upstream `BoardEditor::setDrcResult()`,
//! `ProjectEditor::runErc()`): not as an undo step, but marking the project
//! as modified.

use std::collections::BTreeSet;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use librepcb_app::project::AppProject;
use librepcb_app::screenshot::Headless;
use librepcb_app::{App, InitialTab, startup, ui};
use librepcb_core::project::BoardId;
use librepcb_core::serialization::SExpression;
use librepcb_core::types::{Length, Point, PositiveLength};
use librepcb_editor::commands::{AddVia, BoardSelection, NetRef, RemoveBoardItems};
use slint::{ComponentHandle, Model};

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

fn run_drc(headless: &Headless, app: &App, project: &Rc<AppProject>, board: BoardId) {
    let backend = app.window().global::<ui::Backend>();
    backend.invoke_trigger_board(0, 0, ui::BoardAction::RunDrc);
    headless.settle(5);
    assert!(
        headless.run_until(TIMEOUT, || {
            project
                .checks()
                .borrow()
                .drc(board)
                .is_some_and(|c| !c.is_running() && c.messages().is_some())
        }),
        "DRC did not finish"
    );
    headless.settle(10);
}

/// The approvals of the messages of the last DRC run.
fn message_approvals(project: &Rc<AppProject>, board: BoardId) -> BTreeSet<SExpression> {
    project
        .checks()
        .borrow()
        .drc(board)
        .and_then(|c| c.messages())
        .map(|m| {
            m.entries()
                .iter()
                .map(|e| e.message.approval().clone())
                .collect()
        })
        .unwrap_or_default()
}

fn stored_approvals(project: &Rc<AppProject>, board: BoardId) -> BTreeSet<SExpression> {
    project
        .shared()
        .lock()
        .project()
        .board(board)
        .unwrap()
        .drc_approvals()
        .clone()
}

#[test]
fn drc_removes_disappeared_approvals() {
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
        app.show_tab(InitialTab::Board);
        headless.settle(30);
        let project = app.state().borrow().projects()[0].clone();
        let board = project.board_at(0).unwrap().0;

        // A huge via causes DRC messages.
        let via = {
            let mut p = project.shared().lock();
            let net = *p.project().circuit().net_signals().keys().next().unwrap();
            let size = PositiveLength::new(Length::from_mm(6.0).unwrap()).unwrap();
            let drill = PositiveLength::new(Length::from_mm(3.0).unwrap()).unwrap();
            p.editor
                .execute(AddVia {
                    board: Some(board),
                    position: Point::new(
                        Length::from_mm(30.0).unwrap(),
                        Length::from_mm(20.0).unwrap(),
                    ),
                    net: Some(NetRef::Id(net)),
                    start_layer: None,
                    end_layer: None,
                    drill_diameter: Some(drill),
                    size: Some(size),
                    exposure: None,
                })
                .unwrap()
        };
        run_drc(headless, &app, &project, board);
        let first = message_approvals(&project, board);
        assert!(!first.is_empty(), "the via should cause DRC messages");

        // Approve all messages in the rule check panel.
        let data = app.window().global::<ui::Data>();
        data.set_current_rule_check_board_index(0);
        app.show_panel(ui::PanelPage::RuleCheck);
        headless.settle(5);
        for _ in 0..first.len() {
            let rows = data.get_current_rule_check().messages;
            let Some(i) = (0..rows.row_count()).find(|&i| !rows.row_data(i).unwrap().approved)
            else {
                break;
            };
            let mut row = rows.row_data(i).unwrap();
            row.approved = true;
            rows.set_row_data(i, row);
            headless.settle(3);
        }
        assert_eq!(stored_approvals(&project, board), first);

        // Remove the via and save: its messages disappear with the next
        // run, and so do their approvals.
        {
            let mut p = project.shared().lock();
            let removed = p
                .editor
                .execute(RemoveBoardItems {
                    board: Some(board),
                    selection: BoardSelection {
                        vias: vec![(via.segment, via.via)],
                        ..Default::default()
                    },
                })
                .unwrap();
            assert!(removed);
            p.save().unwrap();
            assert!(!p.has_unsaved_changes());
        }
        let undo_index = project.shared().lock().editor.undo_stack().index();
        run_drc(headless, &app, &project, board);
        let second = message_approvals(&project, board);
        assert!(second.len() < first.len());
        assert!(second.is_subset(&first));
        assert_eq!(stored_approvals(&project, board), second);
        {
            let p = project.shared().lock();
            // No undo step, but the project is modified.
            assert_eq!(p.editor.undo_stack().index(), undo_index);
            assert!(p.editor.has_manual_modifications());
            assert!(p.has_unsaved_changes());
        }
        assert!(data.get_current_tab().unsaved_changes);

        // A second run changes nothing.
        project.shared().lock().save().unwrap();
        run_drc(headless, &app, &project, board);
        assert!(!project.shared().lock().has_unsaved_changes());
    });
}
