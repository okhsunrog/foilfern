//! Images, DXF import and "Paste Geometry" in the symbol and package
//! editor tabs: the tabs ask the application for the files (image file
//! dialog, DXF import dialog) and pass them to the editor FSMs.

use librepcb_app::dialogs::board::DxfImportDialog;
use librepcb_app::helpers::length_to_ui;
use librepcb_app::tabs::{DxfImportKind, Tab, TabRequest, TabUpdate};
use librepcb_app::{App, ui};
use librepcb_canvas::{Modifiers, PointerButton, PointerKind};
use librepcb_core::fileio::FilePath;
use librepcb_core::types::{Layer, Length, LengthUnit, Point};
use librepcb_editor::fsm::library::LibraryTool;
use slint::ComponentHandle;

use crate::library_elements::{
    DIODE_SYMBOL, H, TO220_PACKAGE, W, app_with_library, find_tab, open_from_library_tab, save,
    with_tab,
};

fn mm(x: f64, y: f64) -> Point {
    Point::from_mm(x, y).unwrap()
}

/// Sends a pointer event at a position (world coordinates) to a symbol or
/// package tab.
fn pointer(t: &mut Tab, kind: PointerKind, button: PointerButton, p: Point) -> TabUpdate {
    let world = librepcb_app::tabs::editing::point_to_world(p);
    let m = Modifiers::default();
    match t {
        Tab::Symbol(t) => {
            let _ = t.render_scene(1000.0, 800.0, 1.0);
            let s = t.canvas().view().world_to_screen(world);
            t.pointer_event(kind, button, s, m)
        }
        Tab::Package(t) => {
            let _ = t.render_scene(1000.0, 800.0, 1.0);
            let s = t.canvas().view().world_to_screen(world);
            t.pointer_event(kind, button, s, m)
        }
        _ => unreachable!(),
    }
}

/// A left click (without double click detection).
fn click(t: &mut Tab, p: Point) {
    pointer(t, PointerKind::Move, PointerButton::Other, p);
    pointer(t, PointerKind::Down, PointerButton::Left, p);
    pointer(t, PointerKind::Up, PointerButton::Left, p);
    std::thread::sleep(std::time::Duration::from_millis(510));
}

fn write_dxf(dir: &std::path::Path) -> FilePath {
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
    FilePath::new(&file).unwrap()
}

/// Imports the DXF file through the application's DXF import dialog with
/// a fixed position.
fn import_dxf(
    app: &App,
    headless: &librepcb_app::screenshot::Headless,
    pos: (usize, usize),
    file: &FilePath,
    at: (f64, f64),
    line_width: f64,
    drills: bool,
) {
    let (layers, kind) = with_tab(app, pos, |t| {
        let update = t.trigger(ui::TabAction::ImportDxf);
        update
            .requests
            .iter()
            .find_map(|r| match r {
                TabRequest::ImportDxf { layers, kind } => Some((layers.clone(), *kind)),
                _ => None,
            })
            .expect("DXF import request")
    });
    let id = app.state().borrow().sections()[pos.0].tabs()[pos.1].id();
    let dialog = DxfImportDialog::new(file.clone(), &layers, LengthUnit::Millimeters, kind);
    app.state()
        .borrow_mut()
        .open_library_dialog(Some(id), Box::new(dialog));
    headless.settle(5);
    {
        let mut state = app.state().borrow_mut();
        let form = state.form_dialog().unwrap().dialog.form_mut();
        assert_eq!(
            form.field("circles_as_drills").is_some(),
            kind != DxfImportKind::Symbol
        );
        form.edit("interactive", |f| f.checked = false);
        let length = |mm: f64| length_to_ui(Length::from_mm(mm).unwrap());
        form.edit("pos_x", |f| f.length.value = length(at.0));
        form.edit("pos_y", |f| f.length.value = length(at.1));
        form.edit("line_width", |f| f.length.value = length(line_width));
        if drills {
            form.edit("circles_as_drills", |f| f.checked = true);
        }
    }
    let forms = app.window().global::<ui::Dialogs>();
    forms.invoke_form_button(-1);
    headless.settle(5);
    assert!(!forms.get_form_shown(), "{}", forms.get_form_error());
}

#[test]
fn symbol_editor_image_and_dxf_import() {
    crate::common::with_headless(W, H, |headless| {
        let tmp = tempfile::tempdir().unwrap();
        let (app, lib) = app_with_library(headless, tmp.path());
        let sym_dir = lib.path_to(&format!("sym/{DIODE_SYMBOL}"));
        open_from_library_tab(&app, headless, &lib, &sym_dir);
        let sym = find_tab(&app, |t| matches!(t, Tab::Symbol(_))).expect("symbol tab");
        let images = |app: &App| {
            with_tab(app, sym, |t| {
                let Tab::Symbol(t) = t else { unreachable!() };
                t.core()
                    .editor
                    .element()
                    .images()
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
            })
        };
        assert!(images(&app).is_empty());

        // The image tool asks the application for the file; the chosen
        // image follows the cursor, a click places it, the next sets its
        // size.
        with_tab(&app, sym, |t| {
            let update = t.trigger(ui::TabAction::ToolImage);
            assert!(update.requests.contains(&TabRequest::ChooseImageFile));
            let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("ui/resources/img/app/librepcb.png");
            let data = librepcb_app::app::load_image_file(&file).unwrap();
            let Tab::Symbol(s) = t else { unreachable!() };
            s.add_image(data);
            pointer(t, PointerKind::Move, PointerButton::Other, mm(20.32, 20.32));
            click(t, mm(20.32, 20.32));
            click(t, mm(30.48, 25.4));
        });
        let image = images(&app).first().cloned().expect("image added");
        assert_eq!(image.position(), mm(20.32, 20.32));
        let ratio = image.width().to_mm() / image.height().to_mm();
        assert!((image.width().to_mm() - 10.16).abs() < 0.01);
        save(headless, "element_symbol_image.png");

        // Resize it with the handle of the selected image (upper right
        // corner), keeping the aspect ratio.
        with_tab(&app, sym, |t| {
            let Tab::Symbol(s) = t else { unreachable!() };
            assert_eq!(s.fsm().tool(), LibraryTool::Select);
            let center = mm(
                20.32 + image.width().to_mm() / 2.0,
                20.32 + image.height().to_mm() / 2.0,
            );
            click(t, center);
            let corner = mm(
                20.32 + image.width().to_mm(),
                20.32 + image.height().to_mm(),
            );
            pointer(t, PointerKind::Move, PointerButton::Other, corner);
            pointer(t, PointerKind::Down, PointerButton::Left, corner);
            pointer(t, PointerKind::Move, PointerButton::Other, mm(40.64, 30.0));
            pointer(t, PointerKind::Up, PointerButton::Left, mm(40.64, 30.0));
        });
        let resized = images(&app).first().cloned().unwrap();
        assert!((resized.width().to_mm() - 20.32).abs() < 0.01);
        assert!((resized.width().to_mm() / resized.height().to_mm() - ratio).abs() < 0.01);
        assert_eq!(resized.position(), image.position());

        // DXF import through the application's dialog (no "circles as
        // drills" in the symbol editor): the rectangle and the circle
        // become polygons on the chosen layer.
        let polygons = |app: &App| {
            with_tab(app, sym, |t| {
                let Tab::Symbol(t) = t else { unreachable!() };
                t.core().editor.element().polygons().len()
            })
        };
        let before = polygons(&app);
        with_tab(&app, sym, |t| {
            assert_eq!(
                t.ui_data().features.import_graphics,
                ui::FeatureState::Enabled
            );
        });
        let file = write_dxf(tmp.path());
        import_dxf(&app, headless, sym, &file, (-50.0, 0.0), 0.2, false);
        assert_eq!(polygons(&app), before + 2);
        with_tab(&app, sym, |t| {
            let Tab::Symbol(t) = t else { unreachable!() };
            let e = t.core().editor.element();
            assert!(
                e.polygons()
                    .iter()
                    .filter(|p| p.layer() == Layer::SYMBOL_OUTLINES)
                    .count()
                    >= 2
            );
            // One undo step.
            t.trigger(ui::TabAction::Undo);
        });
        assert_eq!(polygons(&app), before);
    });
}

#[test]
fn package_editor_dxf_import_and_paste_geometry() {
    crate::common::with_headless(W, H, |headless| {
        let tmp = tempfile::tempdir().unwrap();
        let (app, lib) = app_with_library(headless, tmp.path());
        let pkg_dir = lib.path_to(&format!("pkg/{TO220_PACKAGE}"));
        open_from_library_tab(&app, headless, &lib, &pkg_dir);
        let pkg = find_tab(&app, |t| matches!(t, Tab::Package(_))).expect("package tab");
        let footprint = |app: &App| {
            with_tab(app, pkg, |t| {
                let Tab::Package(t) = t else { unreachable!() };
                let uuid = t.fsm().footprint().unwrap();
                t.core()
                    .editor
                    .element()
                    .footprints()
                    .by_uuid(&uuid)
                    .unwrap()
                    .clone()
            })
        };
        let before = footprint(&app);

        // DXF import with circles as drills, twice with different line
        // widths.
        let file = write_dxf(tmp.path());
        import_dxf(&app, headless, pkg, &file, (100.0, 100.0), 0.5, true);
        import_dxf(&app, headless, pkg, &file, (150.0, 100.0), 0.1, true);
        let after = footprint(&app);
        assert_eq!(after.polygons().len(), before.polygons().len() + 2);
        assert_eq!(after.holes().len(), before.holes().len() + 2);
        let new: Vec<_> = after
            .polygons()
            .iter()
            .filter(|p| before.polygons().by_uuid(&p.uuid()).is_none())
            .cloned()
            .collect();
        assert!(new.iter().all(|p| p.layer() == Layer::TOP_DOCUMENTATION));
        let a = new.iter().find(|p| p.line_width().to_mm() == 0.5).unwrap();
        let b = new.iter().find(|p| p.line_width().to_mm() == 0.1).unwrap();
        save(headless, "element_package_dxf.png");

        // Copy the first polygon, select the second: its context menu
        // offers "Paste Geometry", which applies the line width.
        let entries = with_tab(&app, pkg, |t| {
            t.trigger(ui::TabAction::ZoomFit);
            click(t, mm(105.0, 100.0));
            t.trigger(ui::TabAction::Copy);
            click(t, mm(155.0, 100.0));
            let p = mm(155.0, 100.0);
            pointer(t, PointerKind::Down, PointerButton::Right, p);
            let update = pointer(t, PointerKind::Up, PointerButton::Right, p);
            update
                .requests
                .iter()
                .find_map(|r| match r {
                    TabRequest::ContextMenu { entries, .. } => Some(entries.clone()),
                    _ => None,
                })
                .expect("context menu")
        });
        let index = entries
            .iter()
            .position(|e| e.text == "Paste Geometry")
            .expect("Paste Geometry entry");
        with_tab(&app, pkg, |t| t.context_menu_action(index));
        let pasted = footprint(&app);
        assert_eq!(
            pasted
                .polygons()
                .by_uuid(&b.uuid())
                .unwrap()
                .line_width()
                .to_mm(),
            0.5
        );
        assert_eq!(pasted.polygons().by_uuid(&a.uuid()).unwrap(), a);
        assert_eq!(pasted.polygons().len(), after.polygons().len());
    });
}
