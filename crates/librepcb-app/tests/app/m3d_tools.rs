//! The M3d tools in the schematic and board tabs (through the tabs' Rust
//! API, like the Slint UI calls it): buses with the bus chooser and the
//! bus member menu, images, "find", the standalone pad tools with their
//! tool bar, the DXF import dialog, plane visibility and cross-probing of
//! component signals.

use std::rc::Rc;

use crate::common::*;
use librepcb_app::dialogs::{Applied, DialogContext, FormDialog, TabDialogResult};
use librepcb_app::tabs::{Board2dTab, CrossProbe, SchematicTab, TabRequest};
use librepcb_app::ui;
use librepcb_core::project::ComponentSignalRef;
use librepcb_core::types::{GridStyle, LengthUnit, Point};
use librepcb_editor::commands::AddPlane;
use librepcb_editor::fsm::board::BoardTool;
use librepcb_editor::fsm::schematic::SchematicTool;
use slint::Model;

fn context_menu(requests: &[TabRequest]) -> Option<Vec<String>> {
    requests.iter().find_map(|r| match r {
        TabRequest::ContextMenu { entries, .. } => {
            Some(entries.iter().map(|e| e.text.clone()).collect())
        }
        _ => None,
    })
}

fn bus_count(tab: &SchematicTab) -> usize {
    tab.project()
        .shared()
        .lock()
        .project()
        .circuit()
        .buses()
        .len()
}

#[test]
fn schematic_bus_tool_chooser_and_member_menu() {
    let dir = tempfile::tempdir().unwrap();
    let (project, sch, _) = create_project(dir.path());
    let mut tab = SchematicTab::new(Rc::clone(&project), sch, GridStyle::Lines);
    let _ = tab.render_scene(1200.0, 800.0, 1.0);
    let r1_2 = pins(&project, sch, "R1")[1];
    let r2_1 = pins(&project, sch, "R2")[0];
    let y = mm(0.0, 30.48).y;
    let (a, b) = (Point::new(r1_2.x, y), Point::new(r2_1.x, y));

    // The tool bar button starts the bus tool (bus tool bar: wire mode).
    tab.trigger(ui::TabAction::ToolBus);
    assert_eq!(tab.fsm().tool(), SchematicTool::Bus);
    assert_eq!(tab.derived_ui_data(&[]).tool, ui::EditorTool::Bus);
    tab.click(a);
    tab.click(b);
    tab.trigger(ui::TabAction::Abort);
    tab.trigger(ui::TabAction::Abort);
    assert_eq!(bus_count(&tab), 1);
    // The project data lists the bus for the chooser of the tool button.
    let data = project.ui_data();
    assert_eq!(data.buses.row_count(), 1);
    let bus = data.buses.row_data(0).unwrap();
    // Choosing it starts the bus tool with that bus.
    let mut d = tab.derived_ui_data(&[]);
    d.new_bus_uuid = bus.uuid.clone();
    tab.set_derived_ui_data(&d);
    assert_eq!(tab.fsm().tool(), SchematicTool::Bus);
    assert_eq!(
        tab.fsm().tool_data().bus.map(|b| b.to_string()),
        Some(bus.uuid.to_string())
    );
    tab.trigger(ui::TabAction::Abort);

    // A wire starting at the bus opens the bus member menu.
    tab.trigger(ui::TabAction::ToolWire);
    tab.move_to(a);
    let update = tab.press(a);
    let entries = context_menu(&update.requests).expect("bus member menu");
    assert!(entries[0].starts_with("Add New Bus Member"), "{entries:?}");
    assert_eq!(entries.last().map(String::as_str), Some("Cancel"));
    // "Add New Bus Member": the wire is drawn from the bus.
    tab.context_menu_action(0);
    tab.release(a);
    assert!(
        project
            .shared()
            .lock()
            .editor
            .undo_stack()
            .is_group_active()
    );
    tab.move_to(r1_2);
    tab.press(r1_2);
    tab.release(r1_2);
    tab.trigger(ui::TabAction::Abort);
    tab.trigger(ui::TabAction::Abort);
    let p = project.shared().lock();
    let s = p.project().schematic(sch).unwrap();
    assert!(
        s.net_segments()
            .values()
            .any(|seg| !seg.connected_bus_junctions().is_empty()),
        "net segment attached to the bus"
    );
}

#[test]
fn schematic_image_tool() {
    let dir = tempfile::tempdir().unwrap();
    let (project, sch, _) = create_project(dir.path());
    let mut tab = SchematicTab::new(Rc::clone(&project), sch, GridStyle::Lines);
    let _ = tab.render_scene(1200.0, 800.0, 1.0);

    // The tool asks the application for a file.
    let update = tab.trigger(ui::TabAction::ToolImage);
    assert!(update.requests.contains(&TabRequest::ChooseImageFile));
    // The application loads the chosen file and passes it back.
    let file =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/resources/img/app/librepcb.png");
    let data = librepcb_app::app::load_image_file(&file).unwrap();
    assert_eq!(data.format, "png");
    assert_eq!(data.basename, "librepcb");
    tab.add_image(data);
    assert_eq!(tab.fsm().tool(), SchematicTool::Image);
    // First click places, second click sets the size.
    tab.click(mm(50.8, 25.4));
    tab.click(mm(60.96, 35.56));
    tab.trigger(ui::TabAction::Abort);
    let p = project.shared().lock();
    assert_eq!(p.project().schematic(sch).unwrap().images().len(), 1);
    // Unsupported formats are rejected.
    assert!(librepcb_app::app::load_image_file(std::path::Path::new("x.bmp")).is_err());
}

#[test]
fn schematic_and_board_find() {
    let dir = tempfile::tempdir().unwrap();
    let (project, sch, brd) = create_project(dir.path());
    let mut tab = SchematicTab::new(Rc::clone(&project), sch, GridStyle::Lines);
    let _ = tab.render_scene(1200.0, 800.0, 1.0);
    assert_eq!(tab.ui_data().features.find, ui::FeatureState::Enabled);

    tab.trigger(ui::TabAction::FindRefreshSuggestions);
    assert!(tab.set_find_term("R2"));
    let suggestions: Vec<String> = tab
        .ui_data()
        .find_autocompletions
        .iter()
        .map(|i| i.text.to_string())
        .collect();
    assert_eq!(suggestions, vec!["R2"]);
    let update = tab.trigger(ui::TabAction::FindNext);
    assert!(update.repaint, "zoomed to the found symbol");
    let (r2, _) = symbol(&project, sch, "R2");
    assert!(
        tab.fsm()
            .selection()
            .contains(&librepcb_editor::fsm::schematic::SchematicItem::Symbol(r2))
    );
    // The found component is cross-probed.
    let r2_component = {
        let p = project.shared().lock();
        p.project()
            .circuit()
            .component_instance_by_name("R2")
            .unwrap()
            .0
    };
    assert!(tab.cross_probe().components.contains(&r2_component));
    // Nets are found too.
    tab.set_find_term("GND");
    tab.trigger(ui::TabAction::FindNext);
    assert!(!tab.cross_probe().nets.is_empty());

    // The board: devices and nets.
    let mut board = Board2dTab::new(Rc::clone(&project), brd, GridStyle::Lines);
    let _ = board.render_scene(1200.0, 800.0, 1.0);
    assert_eq!(board.ui_data().features.find, ui::FeatureState::Enabled);
    board.trigger(ui::TabAction::FindRefreshSuggestions);
    board.set_find_term("r*");
    assert_eq!(board.ui_data().find_autocompletions.row_count(), 2);
    board.trigger(ui::TabAction::FindNext);
    assert_eq!(board.fsm().selection().items().len(), 2, "wildcard: all");
    assert!(board.cross_probe().components.contains(&r2_component));
}

#[test]
fn board_pad_tools_and_toolbar() {
    let dir = tempfile::tempdir().unwrap();
    let (project, _, brd) = create_project(dir.path());
    let mut tab = Board2dTab::new(Rc::clone(&project), brd, GridStyle::Lines);
    let _ = tab.render_scene(1200.0, 800.0, 1.0);
    let pads = |project: &librepcb_app::project::AppProject| {
        let p = project.shared().lock();
        p.project()
            .board(brd)
            .unwrap()
            .net_segments()
            .values()
            .map(|s| s.pads().len())
            .sum::<usize>()
    };

    // THT pad: net, shape, size, drill, radius and press-fit in the tool bar.
    tab.trigger(ui::TabAction::ToolPadTht);
    assert_eq!(tab.fsm().tool(), BoardTool::AddThtPad);
    let d = tab.derived_ui_data(&[]);
    assert_eq!(d.tool, ui::EditorTool::PadTht);
    assert!(!d.tool_fiducial);
    assert!(d.tool_net.items.row_count() >= 2, "[None] and the nets");
    assert!(!d.tool_pressfit);
    // Rectangular, press-fit, larger radius step.
    let mut d = tab.derived_ui_data(&[]);
    d.tool_shape = ui::PadShape::Octagon;
    d.tool_pressfit = true;
    d.tool_ratio.increase = true;
    let radius_before = tab.fsm().tool_data().pad_radius.unwrap().get().to_ppm();
    tab.set_derived_ui_data(&d);
    let data = tab.fsm().tool_data().clone();
    assert_eq!(
        data.pad_shape,
        Some(librepcb_editor::fsm::board::ToolPadShape::Octagon)
    );
    assert_eq!(data.press_fit, Some(true));
    assert!(tab.derived_ui_data(&[]).tool_pressfit);
    assert_ne!(data.pad_radius.unwrap().get().to_ppm(), radius_before);
    let before = pads(&project);
    tab.click(mm(30.0, 15.0));
    assert_eq!(pads(&project), before + 1);
    tab.trigger(ui::TabAction::Abort);

    // SMT pad: board side selector.
    tab.trigger(ui::TabAction::ToolPadSmt);
    assert_eq!(tab.derived_ui_data(&[]).tool, ui::EditorTool::PadSmt);
    let mut d = tab.derived_ui_data(&[]);
    assert!(!d.tool_bottom);
    d.tool_bottom = true;
    tab.set_derived_ui_data(&d);
    assert!(tab.derived_ui_data(&[]).tool_bottom);
    tab.trigger(ui::TabAction::Abort);

    // Fiducials: the size field is the clearance, no net.
    tab.trigger(ui::TabAction::ToolPadLocalFiducial);
    let d = tab.derived_ui_data(&[]);
    assert!(d.tool_fiducial);
    assert_eq!(d.tool_net.items.row_count(), 0);
    tab.trigger(ui::TabAction::Abort);
}

fn write_dxf(dir: &std::path::Path) -> std::path::PathBuf {
    let file = dir.join("import.dxf");
    std::fs::write(
        &file,
        "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n\
         0\nSECTION\n2\nENTITIES\n\
         0\nLWPOLYLINE\n90\n4\n70\n1\n10\n0.0\n20\n0.0\n10\n10.0\n20\n0.0\n\
         10\n10.0\n20\n5.0\n10\n0.0\n20\n5.0\n\
         0\nCIRCLE\n10\n5.0\n20\n2.5\n40\n0.5\n\
         0\nENDSEC\n0\nEOF\n",
    )
    .unwrap();
    file
}

#[test]
fn board_dxf_import_dialog() {
    let dir = tempfile::tempdir().unwrap();
    let (project, _, brd) = create_project(dir.path());
    let mut tab = Board2dTab::new(Rc::clone(&project), brd, GridStyle::Lines);
    let _ = tab.render_scene(1200.0, 800.0, 1.0);
    assert_eq!(
        tab.ui_data().features.import_graphics,
        ui::FeatureState::Enabled
    );
    // The action asks the application for the file and the options.
    let update = tab.trigger(ui::TabAction::ImportDxf);
    let layers = update
        .requests
        .iter()
        .find_map(|r| match r {
            TabRequest::ImportDxf { layers, kind } => {
                assert_eq!(*kind, librepcb_app::tabs::DxfImportKind::Board);
                Some(layers.clone())
            }
            _ => None,
        })
        .expect("DXF import request");
    assert!(layers.contains(&librepcb_core::types::Layer::BOARD_OUTLINES));

    let file = write_dxf(dir.path());
    let fp = librepcb_core::fileio::FilePath::new(&file).unwrap();
    let mut dialog = librepcb_app::dialogs::board::DxfImportDialog::new(
        fp,
        &layers,
        LengthUnit::Millimeters,
        librepcb_app::tabs::DxfImportKind::Board,
    );
    let ctx = DialogContext::new(&project);
    // Fixed position instead of interactive placement.
    assert!(!dialog.form().field("pos_x").unwrap().enabled);
    let (id, event) = dialog
        .form_mut()
        .edit("interactive", |f| f.checked = false)
        .unwrap();
    dialog.field_event(&ctx, &id, event);
    assert!(dialog.form().field("pos_x").unwrap().enabled);
    let (id, event) = dialog
        .form_mut()
        .edit("circles_as_drills", |f| f.checked = true)
        .unwrap();
    dialog.field_event(&ctx, &id, event);
    // An invalid scale factor keeps the dialog open.
    dialog
        .form_mut()
        .edit("scale_factor", |f| f.text = "x".into());
    assert!(dialog.apply(&ctx).is_err());
    dialog
        .form_mut()
        .edit("scale_factor", |f| f.text = "2".into());
    let Ok(Applied::Tab(TabDialogResult::ImportDxf(settings))) = dialog.apply(&ctx) else {
        panic!("DXF import settings");
    };
    assert_eq!(settings.scale_factor, 2.0);
    assert!(settings.circles_as_drills);
    assert_eq!(settings.placement, Some(Point::ORIGIN));

    let (polygons, holes) = {
        let p = project.shared().lock();
        let b = p.project().board(brd).unwrap();
        (b.polygons().len(), b.holes().len())
    };
    tab.import_dxf(settings);
    let p = project.shared().lock();
    let b = p.project().board(brd).unwrap();
    assert_eq!(b.polygons().len(), polygons + 1);
    assert_eq!(b.holes().len(), holes + 1);
}

#[test]
fn board_plane_visibility_and_signal_cross_probe() {
    let dir = tempfile::tempdir().unwrap();
    let (project, sch, brd) = create_project(dir.path());
    let plane = project
        .shared()
        .lock()
        .editor
        .execute(AddPlane {
            board: Some(brd),
            net: Some("GND".into()),
            layer: None,
            outline: None,
            settings: Default::default(),
        })
        .unwrap();
    let mut tab = Board2dTab::new(Rc::clone(&project), brd, GridStyle::Lines);
    let _ = tab.render_scene(1200.0, 800.0, 1.0);
    let index = undo_index(&project);
    tab.set_plane_visible(plane, false);
    let visible = |project: &librepcb_app::project::AppProject| {
        let p = project.shared().lock();
        p.project()
            .board(brd)
            .unwrap()
            .plane(plane)
            .unwrap()
            .visible()
    };
    assert!(!visible(&project));
    // Not undoable and not a modification to save.
    assert_eq!(undo_index(&project), index);
    tab.set_plane_visible(plane, true);
    assert!(visible(&project));

    // Component signals cross-probed by the board highlight the pins.
    let mut schematic = SchematicTab::new(Rc::clone(&project), sch, GridStyle::Lines);
    let _ = schematic.render_scene(1200.0, 800.0, 1.0);
    let signal = {
        let p = project.shared().lock();
        let proj = p.project();
        let (c, _) = proj.circuit().component_instance_by_name("R1").unwrap();
        let s = proj.schematic(sch).unwrap();
        let sym = s.symbols().values().find(|s| s.component() == c).unwrap();
        let pin = sym.pins(proj.view()).unwrap().remove(0);
        pin.signal()
    };
    let probe = CrossProbe {
        component_signals: [ComponentSignalRef { ..signal }].into(),
        ..CrossProbe::default()
    };
    let update = schematic.set_cross_probe(&probe);
    assert!(update.repaint);
    assert!(schematic.scene().unwrap().scene().selection().count() >= 1);
    // And the pads on the board.
    let update = tab.set_cross_probe(&probe);
    assert!(update.repaint);
    assert!(tab.scene().unwrap().scene().selection().count() >= 1);
}

/// The tools in the headless application (screenshots `m3d_*.png` in
/// `$CARGO_TARGET_TMPDIR`): the bus tool bar, the "find" field, the pad
/// tool bar and the DXF import dialog.
#[test]
fn m3d_tools_in_the_application() {
    use librepcb_app::screenshot::write_png;
    use librepcb_app::{App, InitialTab, startup};
    use slint::ComponentHandle;
    const W: u32 = 1400;
    const H: u32 = 900;
    crate::common::with_headless(W, H, |headless| {
        let save = |name: &str| {
            let pixels = headless.settle(20);
            let path = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
            write_png(&path, W, H, &pixels).unwrap();
            println!("screenshot: {}", path.display());
        };
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("project");
        std::fs::create_dir_all(&project_dir).unwrap();
        drop(create_project(&project_dir));
        let workspace = startup::open_workspace(&tmp.path().join("workspace")).unwrap();
        librepcb_i18n::set_language("en").unwrap();
        let window = ui::AppWindow::new().unwrap();
        slint::select_bundled_translation("en").unwrap();
        let app = App::new(window, workspace);
        app.window().show().unwrap();
        assert_eq!(app.open_project(&project_dir.join("test.lpp")), Some(0));
        app.show_tab(InitialTab::Schematic);
        headless.settle(20);
        let tab_index = |board: bool| {
            let state = app.state().borrow();
            state.sections()[0]
                .tabs()
                .iter()
                .position(|t| match t {
                    librepcb_app::tabs::Tab::Board2d(_) => board,
                    librepcb_app::tabs::Tab::Schematic(_) => !board,
                    _ => false,
                })
                .unwrap() as i32
        };
        let backend = app.window().global::<ui::Backend>();
        let data = app.window().global::<ui::Data>();

        // Bus tool with its tool bar (wire mode).
        let sch = tab_index(false);
        backend.invoke_trigger_tab(0, sch, ui::TabAction::ToolBus);
        headless.settle(10);
        let sections = data.get_sections();
        let section = sections.row_data(0).unwrap();
        let tabs = section.schematic_tabs.clone();
        assert_eq!(
            tabs.row_data(sch as usize).unwrap().tool,
            ui::EditorTool::Bus
        );
        save("m3d_schematic_bus_tool.png");
        backend.invoke_trigger_tab(0, sch, ui::TabAction::ToolSelect);

        // "Find": the term written by the UI, then "find next".
        backend.invoke_trigger_tab(0, sch, ui::TabAction::FindRefreshSuggestions);
        headless.settle(5);
        let tabs = data.get_sections().row_data(0).unwrap().tabs;
        let mut t = tabs.row_data(sch as usize).unwrap();
        t.find_term = "R2".into();
        tabs.set_row_data(sch as usize, t);
        headless.settle(5);
        backend.invoke_trigger_tab(0, sch, ui::TabAction::FindNext);
        headless.settle(10);
        {
            let state = app.state().borrow();
            let librepcb_app::tabs::Tab::Schematic(s) = &state.sections()[0].tabs()[sch as usize]
            else {
                panic!("schematic tab");
            };
            assert_eq!(s.fsm().selection().len(), 1);
        }
        save("m3d_schematic_find.png");

        // Board: THT pad tool bar.
        app.show_tab(InitialTab::Board);
        headless.settle(10);
        let brd = tab_index(true);
        backend.invoke_trigger_tab(0, brd, ui::TabAction::ToolPadTht);
        headless.settle(10);
        let board_tabs = data.get_sections().row_data(0).unwrap().board_2d_tabs;
        assert_eq!(
            board_tabs.row_data(brd as usize).unwrap().tool,
            ui::EditorTool::PadTht
        );
        save("m3d_board_pad_tool.png");
        backend.invoke_trigger_tab(0, brd, ui::TabAction::ToolSelect);
        headless.settle(5);

        // DXF import dialog.
        let project = Rc::clone(&app.state().borrow().projects()[0]);
        let file = write_dxf(tmp.path());
        let dialog = librepcb_app::dialogs::board::DxfImportDialog::new(
            librepcb_core::fileio::FilePath::new(&file).unwrap(),
            &[
                librepcb_core::types::Layer::BOARD_OUTLINES,
                librepcb_core::types::Layer::BOARD_DOCUMENTATION,
            ],
            LengthUnit::Millimeters,
            librepcb_app::tabs::DxfImportKind::Board,
        );
        app.state()
            .borrow_mut()
            .show_form_dialog(project, Box::new(dialog));
        headless.settle(10);
        let forms = app.window().global::<ui::Dialogs>();
        assert!(forms.get_form_shown());
        save("m3d_dxf_import_dialog.png");
        forms.invoke_form_button(-2);
        headless.settle(5);
    });
}
