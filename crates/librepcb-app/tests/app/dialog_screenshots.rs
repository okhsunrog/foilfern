//! The dialogs (M3b) in the headless application: the "add component"
//! dialog opened from the schematic tab (search in a workspace library,
//! previews, accept starts placing the component; "Add more" opens it
//! again after the placement was aborted) and form dialogs rendered in
//! the main window. Screenshots are written to `$CARGO_TARGET_TMPDIR`
//! (`dialog_*.png`).

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::AtomicBool;

use crate::common::*;
use librepcb_app::dialogs;
use librepcb_app::screenshot::{Headless, write_png};
use librepcb_app::tabs::PropertiesTarget;
use librepcb_app::{App, InitialTab, startup, ui};
use librepcb_core::fileio::{FilePath, file_utils};
use librepcb_core::types::LengthUnit;
use librepcb_editor::fsm::schematic::SchematicTool;
use slint::{ComponentHandle, Model};

const WIDTH: u32 = 1400;
const HEIGHT: u32 = 900;

fn save(headless: &Headless, name: &str) {
    let pixels = headless.settle(20);
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    write_png(&path, WIDTH, HEIGHT, &pixels).unwrap();
    println!("screenshot: {}", path.display());
}

fn schematic_tool(app: &App) -> Option<SchematicTool> {
    let state = app.state().borrow();
    state.sections()[0].tabs().iter().find_map(|t| match t {
        librepcb_app::tabs::Tab::Schematic(s) => Some(s.fsm().tool()),
        _ => None,
    })
}

fn schematic_tab_index(app: &App) -> i32 {
    let state = app.state().borrow();
    state.sections()[0]
        .tabs()
        .iter()
        .position(|t| matches!(t, librepcb_app::tabs::Tab::Schematic(_)))
        .unwrap() as i32
}

#[test]
fn dialogs_in_the_application() {
    crate::common::with_headless(WIDTH, HEIGHT, |headless| {
        let tmp = tempfile::tempdir().unwrap();

        // The test project (created and closed again) and a workspace with the
        // upstream test library.
        let project_dir = tmp.path().join("project");
        std::fs::create_dir_all(&project_dir).unwrap();
        drop(create_project(&project_dir));
        let workspace = startup::open_workspace(&tmp.path().join("workspace")).unwrap();
        file_utils::copy_dir_recursively(
            &FilePath::new(
                Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
                    .join("tests/data/libraries/Populated Library.lplib"),
            )
            .unwrap(),
            &workspace
                .local_libraries_path()
                .path_to("Populated Library.lplib"),
        )
        .unwrap();
        workspace
            .library_db()
            .rescan(&AtomicBool::new(false), &mut |_| {})
            .unwrap();

        librepcb_i18n::set_language("en").unwrap();
        let window = ui::AppWindow::new().unwrap();
        slint::select_bundled_translation("en").unwrap();
        let app = App::new(window, workspace);
        app.window().show().unwrap();
        assert_eq!(app.open_project(&project_dir.join("test.lpp")), Some(0));
        app.show_tab(InitialTab::Schematic);
        headless.settle(20);
        let tab = schematic_tab_index(&app);

        // "Add component" tool: the dialog opens (category tree, no filter).
        let backend = app.window().global::<ui::Backend>();
        backend.invoke_trigger_tab(0, tab, ui::TabAction::ToolComponent);
        headless.settle(10);
        let data = app.window().global::<ui::AddComponentDialogData>();
        assert!(data.get_shown());
        assert!(data.get_categories().row_count() > 1);
        assert!(!data.get_can_accept());

        // Search: the resistor component with its devices, the first device is
        // selected with symbol and footprint previews.
        data.set_filter("resistor".into());
        data.invoke_filter_edited("resistor".into());
        headless.settle(10);
        assert!(data.get_components().row_count() >= 2);
        assert_eq!(data.get_current_component_row(), 0);
        data.invoke_component_selected(1);
        headless.settle(10);
        assert_eq!(data.get_current_component_row(), 1);
        assert!(data.get_can_accept());
        assert!(data.get_symbol_preview().size().width > 0);
        assert!(data.get_footprint_preview().size().width > 0);
        assert!(!data.get_device_name().is_empty());
        {
            let mut state = app.state().borrow_mut();
            let choice = state.add_component_dialog().unwrap().choice().unwrap();
            assert_eq!(choice.component, uuid(RESISTOR));
            assert!(choice.device.is_some());
        }
        save(headless, "dialog_add_component.png");

        // Accept: the component follows the cursor.
        data.invoke_accepted();
        headless.settle(10);
        assert!(!data.get_shown());
        assert_eq!(schematic_tool(&app), Some(SchematicTool::Component));

        // Aborting the placement ("Add more" is checked by default) opens the
        // dialog again; canceling it ends the tool.
        backend.invoke_trigger_tab(0, tab, ui::TabAction::Abort);
        headless.settle(10);
        assert_eq!(schematic_tool(&app), Some(SchematicTool::Select));
        assert!(data.get_shown());
        data.invoke_rejected();
        headless.settle(10);
        assert!(!data.get_shown());
        assert_eq!(schematic_tool(&app), Some(SchematicTool::Select));

        // Form dialogs rendered in the main window.
        let (project, sch) = {
            let state = app.state().borrow();
            let project = Rc::clone(&state.projects()[0]);
            let sch = project.shared().lock().project().schematics()[0].id();
            (project, sch)
        };
        let (r1, _) = symbol(&project, sch, "R1");
        let dialog = dialogs::open_properties(
            &project,
            &PropertiesTarget::Symbol(r1),
            LengthUnit::Millimeters,
        )
        .unwrap();
        let title = dialog.title();
        app.state()
            .borrow_mut()
            .show_form_dialog(Rc::clone(&project), dialog);
        let forms = app.window().global::<ui::Dialogs>();
        headless.settle(10);
        assert!(forms.get_form_shown());
        assert_eq!(forms.get_form_title(), title.as_str());
        save(headless, "dialog_symbol_properties.png");
        forms.invoke_form_button(-2);
        headless.settle(5);
        assert!(!forms.get_form_shown());

        // Board setup (from the board action, DRC settings page), project
        // setup, output jobs and graphics export.
        backend.invoke_trigger_board(0, 0, ui::BoardAction::OpenDrcSetupDialog);
        headless.settle(10);
        assert!(forms.get_form_shown());
        assert_eq!(forms.get_form_page(), 2);
        save(headless, "dialog_board_setup_drc.png");
        forms.set_form_page(1);
        save(headless, "dialog_board_setup_rules.png");
        forms.invoke_form_button(-2);
        headless.settle(5);
        backend.invoke_trigger_project(0, ui::ProjectAction::OpenSetupDialog);
        headless.settle(10);
        assert!(forms.get_form_shown());
        save(headless, "dialog_project_setup.png");
        forms.invoke_form_button(-2);
        headless.settle(5);
        backend.invoke_trigger_project(0, ui::ProjectAction::OpenOutputJobs);
        headless.settle(10);
        assert!(forms.get_form_shown());
        save(headless, "dialog_output_jobs.png");
        forms.invoke_form_button(-2);
        headless.settle(5);
        backend.invoke_trigger_tab(0, tab, ui::TabAction::ExportPdf);
        headless.settle(10);
        assert!(forms.get_form_shown());
        save(headless, "dialog_graphics_export.png");
        forms.invoke_form_button(-2);
        headless.settle(5);
        backend.invoke_trigger_project(0, ui::ProjectAction::BillOfMaterials);
        headless.settle(10);
        assert!(forms.get_form_shown());
        save(headless, "dialog_bom_review.png");
        forms.invoke_form_button(-2);
        headless.settle(5);
        let board_tab = {
            let state = app.state().borrow();
            state.sections()[0]
                .tabs()
                .iter()
                .position(|t| matches!(t, librepcb_app::tabs::Tab::Board2d(_)))
                .unwrap() as i32
        };
        backend.invoke_trigger_tab(0, board_tab, ui::TabAction::ExportPickPlace);
        headless.settle(10);
        assert!(forms.get_form_shown());
        save(headless, "dialog_pick_place.png");
        forms.invoke_form_button(-2);
        headless.settle(5);
        assert!(!forms.get_form_shown());
    });
}
