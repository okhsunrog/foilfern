//! Library element editors (M4b) in the headless application: opening
//! elements from the library tab, editing with the FSMs and tool bars,
//! properties dialogs, metadata, checks, undo and saving. Screenshots
//! `element_*.png` are written to `$CARGO_TARGET_TMPDIR`.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use librepcb_app::screenshot::{Headless, write_png};
use librepcb_app::tabs::{Tab, TabRequest};
use librepcb_app::{App, startup, ui};
use librepcb_core::fileio::{FilePath, file_utils};
use librepcb_core::types::Point;
use librepcb_editor::fsm::library::LibraryTool;
use slint::{ComponentHandle, Model};

pub const W: u32 = 1400;
pub const H: u32 = 900;

/// The diode symbol of the upstream test library.
pub const DIODE_SYMBOL: &str = "9b75d0ce-ac4e-4a52-a88a-8777f66d3241";

pub fn save(headless: &Headless, name: &str) {
    let pixels = headless.settle(20);
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    write_png(&path, W, H, &pixels).unwrap();
    println!("screenshot: {}", path.display());
}

/// A headless application with a workspace containing a copy of the
/// upstream test library; returns the app and the library directory.
pub fn app_with_library(headless: &Headless, dir: &Path) -> (App, FilePath) {
    let workspace = startup::open_workspace(&dir.join("workspace")).unwrap();
    let lib = workspace
        .local_libraries_path()
        .path_to("Populated Library.lplib");
    file_utils::copy_dir_recursively(
        &FilePath::new(
            Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
                .join("tests/data/libraries/Populated Library.lplib"),
        )
        .unwrap(),
        &lib,
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
    let data = app.window().global::<ui::Data>();
    headless.run_until(std::time::Duration::from_secs(20), || {
        !data.get_libraries_rescan_in_progress()
    });
    headless.settle(5);
    (app, lib)
}

pub fn find_tab(app: &App, f: impl Fn(&Tab) -> bool) -> Option<(usize, usize)> {
    let state = app.state().borrow();
    state
        .sections()
        .iter()
        .enumerate()
        .find_map(|(si, s)| s.tabs().iter().position(&f).map(|ti| (si, ti)))
}

pub fn with_tab<R>(app: &App, pos: (usize, usize), f: impl FnOnce(&mut Tab) -> R) -> R {
    let mut state = app.state().borrow_mut();
    f(state.sections_mut()[pos.0].tab_mut(pos.1).unwrap())
}

fn mm(x: f64, y: f64) -> Point {
    Point::from_mm(x, y).unwrap()
}

/// Opens the element in `dir` from the library tab (like double-clicking
/// it, opens the library tab first).
pub fn open_from_library_tab(app: &App, headless: &Headless, lib: &FilePath, dir: &FilePath) {
    let backend = app.window().global::<ui::Backend>();
    backend.invoke_trigger_library(lib.to_native().into(), ui::LibraryAction::Open);
    headless.settle(10);
    let lib_tab = find_tab(app, |t| matches!(t, Tab::Library(_))).unwrap();
    let update = with_tab(app, lib_tab, |t| {
        let Tab::Library(t) = t else { unreachable!() };
        let find =
            |rows: &std::rc::Rc<librepcb_app::models::UiModel<ui::LibraryTreeViewItemData>>| {
                (0..rows.len()).find(|i| {
                    let u = rows.get(*i).unwrap().user_data;
                    u == dir.as_str() || u == dir.file_name()
                })
            };
        if let Some(row) = find(t.categories_model()) {
            let mut d = t.derived_ui_data();
            d.categories_index = row as i32;
            d.filtered_elements_index = -1;
            t.set_derived_ui_data(&d);
        } else {
            // Show the categories one after another until the element is
            // listed.
            let categories = t.categories_model().len();
            let found = (0..categories).any(|c| {
                let mut d = t.derived_ui_data();
                d.categories_index = c as i32;
                t.set_derived_ui_data(&d);
                match find(t.elements_model()) {
                    Some(row) => {
                        let mut d = t.derived_ui_data();
                        d.filtered_elements_index = row as i32;
                        t.set_derived_ui_data(&d);
                        true
                    }
                    None => false,
                }
            });
            assert!(found, "element row");
        }
        t.trigger(ui::TabAction::EditProperties)
    });
    assert!(update.requests.iter().any(|r| matches!(
        r,
        TabRequest::OpenLibraryElement { path, .. } if path == dir
    )));
    backend.invoke_trigger_tab(
        lib_tab.0 as i32,
        lib_tab.1 as i32,
        ui::TabAction::EditProperties,
    );
    headless.settle(10);
}

/// Clicks at a position (world coordinates) of a scene tab.
fn click_at(t: &mut Tab, p: Point) {
    use librepcb_canvas::{Modifiers, PointerButton, PointerKind};
    let _ = t.render_scene(1000.0, 800.0, 1.0, 0);
    let world = librepcb_app::tabs::editing::point_to_world(p);
    let (sec_pos, update) = match t {
        Tab::Package(t) => {
            let s = t.canvas().view().world_to_screen(world);
            let m = Modifiers::default();
            t.pointer_event(PointerKind::Move, PointerButton::Other, s, m);
            t.pointer_event(PointerKind::Down, PointerButton::Left, s, m);
            (
                s,
                t.pointer_event(PointerKind::Up, PointerButton::Left, s, m),
            )
        }
        _ => unreachable!(),
    };
    let _ = (sec_pos, update);
    std::thread::sleep(std::time::Duration::from_millis(510));
}

/// The TO220AB package of the upstream test library (2 footprints, a 3D
/// model).
pub const TO220_PACKAGE: &str = "0eaf289c-166d-4bd9-a4ba-dbf6bbc76ef1";

#[test]
fn package_editor() {
    use librepcb_app::tabs::LibraryItemRef;
    use librepcb_editor::library_editor::commands::FootprintItem;
    crate::common::with_headless(W, H, |headless| {
        let tmp = tempfile::tempdir().unwrap();
        let (app, lib) = app_with_library(headless, tmp.path());
        let backend = app.window().global::<ui::Backend>();
        let forms = app.window().global::<ui::Dialogs>();
        let data = app.window().global::<ui::Data>();
        let pkg_dir = lib.path_to(&format!("pkg/{TO220_PACKAGE}"));
        open_from_library_tab(&app, headless, &lib, &pkg_dir);
        let pkg = find_tab(&app, |t| matches!(t, Tab::Package(_))).expect("package tab");
        // The data of the tab (the list models are shared with the UI).
        let derived = || {
            with_tab(&app, pkg, |t| {
                let Tab::Package(p) = t else { unreachable!() };
                p.derived_ui_data()
            })
        };
        // What the UI shows.
        let d = {
            let sections = data.get_sections();
            sections
                .row_data(pkg.0)
                .unwrap()
                .package_tabs
                .row_data(pkg.1)
                .unwrap()
        };
        assert_eq!(d.name, "TO220AB");
        assert_eq!(d.page_index, 2);
        assert_eq!(d.footprints.row_count(), 2);
        assert_eq!(d.footprint_index, 0);
        assert_eq!(d.pads.row_count(), 4);
        assert_eq!(d.models.row_count(), 1);
        assert_eq!(d.model_index, 0, "model of the first footprint");
        assert_eq!(
            d.footprints.row_data(0).unwrap().models.row_count(),
            1,
            "one model flag per model"
        );
        with_tab(&app, pkg, |t| {
            let Tab::Package(t) = t else { unreachable!() };
            assert!(t.scene().is_some_and(|s| s.content_bounds().is_some()));
            assert!(t.ui_data().features.select == ui::FeatureState::Enabled);
        });
        backend.invoke_trigger_tab(pkg.0 as i32, pkg.1 as i32, ui::TabAction::ZoomFit);
        save(headless, "element_package.png");

        let fpt_pads = |app: &App| {
            with_tab(app, pkg, |t| {
                let Tab::Package(t) = t else { unreachable!() };
                let p = t.core().editor.element();
                let fpt = t.current_footprint().unwrap();
                p.footprints().by_uuid(&fpt).unwrap().pads().len()
            })
        };
        let pads_before = fpt_pads(&app);

        // Add an SMT pad with the tool: the tool bar shows the package pads.
        with_tab(&app, pkg, |t| {
            t.trigger(ui::TabAction::ToolPadSmt);
            let Tab::Package(p) = t else { unreachable!() };
            let d = p.derived_ui_data();
            assert_eq!(d.tool, ui::EditorTool::PadSmt);
            assert_eq!(d.tool_package_pad.items.row_count(), 5, "unconnected + 4");
            assert!(!d.tool_fiducial);
            // Bottom side and round shape via the tool bar.
            let mut d = p.derived_ui_data();
            d.tool_bottom = true;
            d.tool_shape = ui::PadShape::Round;
            p.set_derived_ui_data(&d);
            let t2 = p.fsm().tool_data();
            assert_eq!(
                t2.component_side,
                librepcb_core::geometry::ComponentSide::Bottom
            );
            assert_eq!(p.derived_ui_data().tool_shape, ui::PadShape::Round);
            click_at(t, mm(20.0, 20.0));
            t.trigger(ui::TabAction::Abort);
            t.trigger(ui::TabAction::Abort);
        });
        assert_eq!(fpt_pads(&app), pads_before + 1);
        save(headless, "element_package_pad.png");

        // Pad properties dialog.
        let (fpt, pad) = with_tab(&app, pkg, |t| {
            let Tab::Package(t) = t else { unreachable!() };
            let fpt = t.current_footprint().unwrap();
            let pad = *t
                .core()
                .editor
                .element()
                .footprints()
                .by_uuid(&fpt)
                .unwrap()
                .pads()
                .uuids()
                .first()
                .unwrap();
            (fpt, pad)
        });
        let id = {
            let state = app.state().borrow();
            state.sections()[pkg.0].tabs()[pkg.1].id()
        };
        app.state().borrow_mut().open_library_item_properties(
            id,
            LibraryItemRef::Footprint(Some(fpt), FootprintItem::Pad(pad)),
        );
        headless.settle(10);
        assert!(forms.get_form_shown());
        save(headless, "element_package_pad_dialog.png");
        {
            let mut state = app.state().borrow_mut();
            let dialog = state.form_dialog().unwrap();
            dialog.dialog.form_mut().edit("width", |f| {
                f.length.value = librepcb_app::helpers::length_to_ui(
                    librepcb_core::types::Length::new(3_000_000),
                )
            });
        }
        forms.invoke_form_button(-1);
        headless.settle(10);
        assert!(!forms.get_form_shown());
        let width = with_tab(&app, pkg, |t| {
            let Tab::Package(t) = t else { unreachable!() };
            let p = t.core().editor.element();
            let fp = p.footprints().by_uuid(&fpt).unwrap();
            *fp.pads().by_uuid(&pad).unwrap().pad().width()
        });
        assert_eq!(width.to_nm(), 3_000_000);

        // Package pads: add a range, rename one through the list.
        with_tab(&app, pkg, |t| {
            let Tab::Package(p) = t else { unreachable!() };
            let mut d = p.derived_ui_data();
            d.new_pad_name = "1".into();
            p.set_derived_ui_data(&d);
            assert_eq!(p.derived_ui_data().new_pad_name_error, "Duplicate");
            let mut d = p.derived_ui_data();
            d.new_pad_name = "10..11".into();
            p.set_derived_ui_data(&d);
            assert_eq!(p.derived_ui_data().new_pad_name_error, "");
            t.trigger(ui::TabAction::PackageAddPads);
            // Adding pads breaks the interface: read-only until unlocked.
            let Tab::Package(p) = t else { unreachable!() };
            assert!(p.derived_ui_data().interface_broken_msg);
            assert!(!p.core().is_writable());
            t.trigger(ui::TabAction::Unlock);
        });
        headless.settle(5);
        let d = derived();
        assert_eq!(d.pads.row_count(), 6);
        assert_eq!(d.new_pad_name, "");
        let row = (0..d.pads.row_count())
            .find(|i| d.pads.row_data(*i).unwrap().name == "10")
            .unwrap();
        let mut r = d.pads.row_data(row).unwrap();
        r.name = "GND".into();
        d.pads.set_row_data(row, r);
        headless.settle(5);
        backend.invoke_trigger_tab(pkg.0 as i32, pkg.1 as i32, ui::TabAction::Apply);
        headless.settle(5);
        let names: Vec<String> = with_tab(&app, pkg, |t| {
            let Tab::Package(t) = t else { unreachable!() };
            t.core()
                .editor
                .element()
                .pads()
                .iter()
                .map(|p| p.name().to_string())
                .collect()
        });
        assert!(names.contains(&"GND".to_owned()), "{names:?}");
        assert!(names.contains(&"11".to_owned()));

        // Footprints: add one (becomes current), tag it, remove it again.
        with_tab(&app, pkg, |t| {
            let Tab::Package(p) = t else { unreachable!() };
            let mut d = p.derived_ui_data();
            d.new_footprint = "My Footprint".into();
            p.set_derived_ui_data(&d);
        });
        headless.settle(5);
        let d = derived();
        assert_eq!(d.footprints.row_count(), 3);
        assert_eq!(d.footprint_index, 2);
        let mut r = d.footprints.row_data(2).unwrap();
        r.new_tag = "reflow-soldering (recommended)".into();
        d.footprints.set_row_data(2, r);
        headless.settle(5);
        let d = derived();
        let tags = d.footprints.row_data(2).unwrap().tags;
        assert_eq!(tags.row_count(), 1);
        assert_eq!(tags.row_data(0).unwrap(), "reflow-soldering");
        let mut r = d.footprints.row_data(2).unwrap();
        r.action = ui::FootprintAction::Delete;
        d.footprints.set_row_data(2, r);
        headless.settle(5);
        let d = derived();
        assert_eq!(d.footprints.row_count(), 2);
        assert_eq!(d.footprint_index, 0, "switched back to the first footprint");

        // 3D model of the second footprint checked through the flags.
        let flags = d.footprints.row_data(1).unwrap().models;
        assert!(!flags.row_data(0).unwrap());
        flags.set_row_data(0, true);
        headless.settle(5);
        let used = with_tab(&app, pkg, |t| {
            let Tab::Package(t) = t else { unreachable!() };
            let p = t.core().editor.element();
            p.footprints().iter().nth(1).unwrap().models().len()
        });
        assert_eq!(used, 1);

        // Generate the courtyard (asks for the excess).
        backend.invoke_trigger_tab(
            pkg.0 as i32,
            pkg.1 as i32,
            ui::TabAction::PackageGenerateCourtyard,
        );
        headless.settle(10);
        assert!(forms.get_form_shown());
        save(headless, "element_package_courtyard_dialog.png");
        forms.invoke_form_button(-1);
        headless.settle(10);

        // Assembly type, then save.
        with_tab(&app, pkg, |t| {
            let Tab::Package(p) = t else { unreachable!() };
            let mut d = p.derived_ui_data();
            d.assembly_type = 0;
            p.set_derived_ui_data(&d);
            t.trigger(ui::TabAction::Save);
            assert!(!t.ui_data().unsaved_changes);
        });
        headless.settle(10);
        let content = std::fs::read_to_string(pkg_dir.path_to("package.lp").as_path()).unwrap();
        assert!(content.contains("(assembly_type tht)"), "{content}");
        assert!(content.contains("(name \"GND\")"));
        assert!(content.contains("(shape roundrect)"));
        save(headless, "element_package_saved.png");

        // A new package: wizard pages metadata, pads, then the editor.
        backend.invoke_trigger_library(lib.to_native().into(), ui::LibraryAction::NewPackage);
        headless.settle(10);
        let new = find_tab(
            &app,
            |t| matches!(t, Tab::Package(s) if s.core().wizard_mode),
        )
        .expect("new package tab");
        with_tab(&app, new, |t| {
            let Tab::Package(p) = t else { unreachable!() };
            assert_eq!(
                p.derived_ui_data().checks.messages.row_count(),
                0,
                "no checks in the wizard"
            );
            let mut d = p.derived_ui_data();
            d.name = "My Package".into();
            p.set_derived_ui_data(&d);
        });
        let trigger = |action| {
            backend.invoke_trigger_tab(new.0 as i32, new.1 as i32, action);
            headless.settle(5);
        };
        trigger(ui::TabAction::Next);
        with_tab(&app, new, |t| {
            let Tab::Package(p) = t else { unreachable!() };
            assert_eq!(p.core().page_index, 1);
            assert!(p.core().wizard_mode);
            assert_eq!(p.ui_data().title.to_string(), "My Package");
            let mut d = p.derived_ui_data();
            d.new_pad_name = "1..3".into();
            p.set_derived_ui_data(&d);
        });
        trigger(ui::TabAction::PackageAddPads);
        save(headless, "element_package_new_pads.png");
        trigger(ui::TabAction::Next);
        let new_dir = with_tab(&app, new, |t| {
            let Tab::Package(p) = t else { unreachable!() };
            assert_eq!(p.core().page_index, 2);
            assert!(!p.core().wizard_mode);
            assert_eq!(p.core().editor.element().pads().len(), 3);
            assert_eq!(p.core().editor.element().footprints().len(), 1);
            assert!(p.current_footprint().is_some());
            p.core().directory_path()
        });
        assert!(new_dir.path_to("package.lp").is_existing_file());
        // Add a pad connected to the first package pad.
        with_tab(&app, new, |t| {
            t.trigger(ui::TabAction::ToolPadTht);
            click_at(t, mm(0.0, 0.0));
            t.trigger(ui::TabAction::Abort);
            t.trigger(ui::TabAction::Abort);
            let Tab::Package(p) = t else { unreachable!() };
            let connected: Vec<_> = p.connected_pads().into_values().collect();
            assert_eq!(connected.len(), 1);
            assert!(connected[0].is_some(), "next free package pad");
        });
        trigger(ui::TabAction::ZoomFit);
        save(headless, "element_package_new.png");
    });
}

/// The diode component of the upstream test library.
pub const DIODE_COMPONENT: &str = "0ca0993c-bf55-4529-865b-849ef898b28a";

/// Chooses the element named `name` in the open chooser dialog and
/// accepts it.
pub fn choose_in_dialog(app: &App, headless: &Headless, name: &str) {
    let forms = app.window().global::<ui::Dialogs>();
    assert!(forms.get_form_shown(), "chooser dialog shown");
    {
        let mut state = app.state().borrow_mut();
        let dialog = state.form_dialog().unwrap();
        let form = dialog.dialog.form_mut();
        let items = form.field("list").unwrap().items;
        let row = (0..items.row_count())
            .find(|i| items.row_data(*i).unwrap().text == name)
            .unwrap_or_else(|| panic!("{name} listed"));
        form.edit("list", |f| f.index = row as i32);
    }
    forms.invoke_form_button(-1);
    headless.settle(10);
    assert!(!forms.get_form_shown());
}

#[test]
fn component_editor() {
    crate::common::with_headless(W, H, |headless| {
        let tmp = tempfile::tempdir().unwrap();
        let (app, lib) = app_with_library(headless, tmp.path());
        let backend = app.window().global::<ui::Backend>();
        let data = app.window().global::<ui::Data>();
        let cmp_dir = lib.path_to(&format!("cmp/{DIODE_COMPONENT}"));
        open_from_library_tab(&app, headless, &lib, &cmp_dir);
        let tab = find_tab(&app, |t| matches!(t, Tab::Component(_))).expect("component tab");
        let derived = || {
            with_tab(&app, tab, |t| {
                let Tab::Component(c) = t else { unreachable!() };
                c.derived_ui_data()
            })
        };
        // What the UI shows.
        {
            let sections = data.get_sections();
            let d = sections
                .row_data(tab.0)
                .unwrap()
                .component_tabs
                .row_data(tab.1)
                .unwrap();
            assert_eq!(d.name, "Diode");
            assert_eq!(d.page_index, 2);
            assert_eq!(d.variants.row_count(), 1);
            let gates = d.variants.row_data(0).unwrap().gates;
            assert_eq!(gates.row_count(), 1);
            let gate = gates.row_data(0).unwrap();
            assert_eq!(gate.pinout.row_count(), 2);
            assert!(!gate.symbol_name.is_empty());
            assert!(d.signal_names.row_count() >= 3, "unconnected + signals");
        }
        // The symbol preview renders.
        let image = with_tab(&app, tab, |t| t.render_scene(300.0, 200.0, 1.0, 0));
        assert_eq!(image.size().width, 300);
        save(headless, "element_component.png");
        for (page, name) in [(0, "metadata"), (1, "signals")] {
            with_tab(&app, tab, |t| {
                let Tab::Component(c) = t else { unreachable!() };
                let mut d = c.derived_ui_data();
                d.page_index = page;
                c.set_derived_ui_data(&d);
            });
            backend.invoke_trigger_tab(tab.0 as i32, tab.1 as i32, ui::TabAction::Apply);
            headless.settle(10);
            save(headless, &format!("element_component_{name}.png"));
        }

        // Signals: add a range, rename one through the list.
        let signals_before = derived().cmp_signals.row_count();
        with_tab(&app, tab, |t| {
            let Tab::Component(c) = t else { unreachable!() };
            let mut d = c.derived_ui_data();
            d.new_signal_name = "X1..2".into();
            c.set_derived_ui_data(&d);
            assert_eq!(c.derived_ui_data().new_signal_name_error, "");
            t.trigger(ui::TabAction::ComponentAddSignals);
            // Adding signals breaks the interface.
            t.trigger(ui::TabAction::Unlock);
        });
        let d = derived();
        assert_eq!(d.cmp_signals.row_count(), signals_before + 2);
        let row = (0..d.cmp_signals.row_count())
            .find(|i| d.cmp_signals.row_data(*i).unwrap().name == "X1")
            .unwrap();
        let mut r = d.cmp_signals.row_data(row).unwrap();
        r.name = "Y1".into();
        r.forced_net_name = "GND".into();
        d.cmp_signals.set_row_data(row, r);
        headless.settle(5);
        backend.invoke_trigger_tab(tab.0 as i32, tab.1 as i32, ui::TabAction::Apply);
        headless.settle(5);
        let (names, forced) = with_tab(&app, tab, |t| {
            let Tab::Component(c) = t else { unreachable!() };
            let cmp = c.core().editor.element();
            let names: Vec<String> = cmp.signals().iter().map(|s| s.name().to_string()).collect();
            let forced = cmp
                .signals()
                .iter()
                .find(|s| s.name().as_str() == "Y1")
                .map(|s| s.forced_net_name().clone());
            (names, forced)
        });
        assert!(names.contains(&"Y1".to_owned()), "{names:?}");
        assert_eq!(forced.as_deref(), Some("GND"));

        // Pinout: disconnect the first pin.
        let d = derived();
        let pinout = d
            .variants
            .row_data(0)
            .unwrap()
            .gates
            .row_data(0)
            .unwrap()
            .pinout;
        let mut p = pinout.row_data(0).unwrap();
        assert!(p.signal_index > 0);
        p.signal_index = 0;
        pinout.set_row_data(0, p);
        headless.settle(5);
        let unconnected = with_tab(&app, tab, |t| {
            let Tab::Component(c) = t else { unreachable!() };
            let cmp = c.core().editor.element();
            let gate = cmp.symbol_variants().iter().next().unwrap();
            let gate = gate.symbol_items().iter().next().unwrap();
            gate.pin_signal_map()
                .iter()
                .next()
                .unwrap()
                .signal_uuid()
                .is_none()
        });
        assert!(unconnected);
        assert!(
            derived()
                .variants
                .row_data(0)
                .unwrap()
                .has_unassigned_signals
        );

        // Add a gate: the symbol chooser dialog.
        let d = derived();
        let mut v = d.variants.row_data(0).unwrap();
        v.action = ui::ComponentVariantAction::AddGate;
        d.variants.set_row_data(0, v);
        headless.settle(10);
        save(headless, "element_component_chooser.png");
        choose_in_dialog(&app, headless, "Diode");
        let d = derived();
        let gates = d.variants.row_data(0).unwrap().gates;
        assert_eq!(gates.row_count(), 2);
        assert_eq!(gates.row_data(0).unwrap().suffix, "A", "suffixes updated");
        // The preview of the whole variant.
        let image = with_tab(&app, tab, |t| t.render_scene(300.0, 200.0, 1.0, 2));
        assert_eq!(image.size().width, 300);

        // Prefix, then save.
        with_tab(&app, tab, |t| {
            let Tab::Component(c) = t else { unreachable!() };
            let mut d = c.derived_ui_data();
            d.prefix = "DX".into();
            d.default_value = "{{MPN}}".into();
            d.datasheet_url = "example.com/diode.pdf".into();
            c.set_derived_ui_data(&d);
            assert_eq!(c.derived_ui_data().prefix_error, "");
            t.trigger(ui::TabAction::Save);
            assert!(!t.ui_data().unsaved_changes);
        });
        headless.settle(10);
        let content = std::fs::read_to_string(cmp_dir.path_to("component.lp").as_path()).unwrap();
        assert!(content.contains("(prefix \"DX\")"), "{content}");
        assert!(content.contains("(name \"Y1\")"));
        assert!(content.contains("example.com/diode.pdf"));
        backend.invoke_trigger_tab(tab.0 as i32, tab.1 as i32, ui::TabAction::Apply);
        headless.settle(10);
        save(headless, "element_component_saved.png");

        // A new component: metadata, signals, then the symbol variants
        // (still in the wizard: adding a gate creates the signals).
        backend.invoke_trigger_library(lib.to_native().into(), ui::LibraryAction::NewComponent);
        headless.settle(10);
        let new = find_tab(
            &app,
            |t| matches!(t, Tab::Component(s) if s.core().wizard_mode),
        )
        .expect("new component tab");
        with_tab(&app, new, |t| {
            let Tab::Component(c) = t else { unreachable!() };
            let mut d = c.derived_ui_data();
            d.name = "My Component".into();
            c.set_derived_ui_data(&d);
        });
        let trigger = |action| {
            backend.invoke_trigger_tab(new.0 as i32, new.1 as i32, action);
            headless.settle(5);
        };
        trigger(ui::TabAction::Next);
        trigger(ui::TabAction::Next);
        let d = with_tab(&app, new, |t| {
            let Tab::Component(c) = t else { unreachable!() };
            assert_eq!(c.core().page_index, 2);
            assert!(c.core().wizard_mode);
            c.derived_ui_data()
        });
        assert_eq!(d.variants.row_count(), 1, "the default variant");
        let mut v = d.variants.row_data(0).unwrap();
        v.action = ui::ComponentVariantAction::AddGate;
        d.variants.set_row_data(0, v);
        headless.settle(10);
        choose_in_dialog(&app, headless, "Diode");
        let signals = with_tab(&app, new, |t| {
            let Tab::Component(c) = t else { unreachable!() };
            c.core().editor.element().signals().len()
        });
        assert_eq!(signals, 2, "a signal per pin");
        save(headless, "element_component_new.png");
        trigger(ui::TabAction::Next);
        let dir = with_tab(&app, new, |t| {
            let Tab::Component(c) = t else { unreachable!() };
            assert!(!c.core().wizard_mode);
            c.core().directory_path()
        });
        assert!(dir.path_to("component.lp").is_existing_file());
    });
}

/// The R-0805 device of the upstream test library.
pub const R0805_DEVICE: &str = "078650d3-483c-4b9e-a848-b14f1aad2edc";

#[test]
fn device_editor() {
    crate::common::with_headless(W, H, |headless| {
        let tmp = tempfile::tempdir().unwrap();
        let (app, lib) = app_with_library(headless, tmp.path());
        let backend = app.window().global::<ui::Backend>();
        let data = app.window().global::<ui::Data>();
        let dev_dir = lib.path_to(&format!("dev/{R0805_DEVICE}"));
        open_from_library_tab(&app, headless, &lib, &dev_dir);
        let tab = find_tab(&app, |t| matches!(t, Tab::Device(_))).expect("device tab");
        let derived = || {
            with_tab(&app, tab, |t| {
                let Tab::Device(d) = t else { unreachable!() };
                d.derived_ui_data()
            })
        };
        {
            let sections = data.get_sections();
            let d = sections
                .row_data(tab.0)
                .unwrap()
                .device_tabs
                .row_data(tab.1)
                .unwrap();
            assert_eq!(d.name, "R-0805");
            assert_eq!(d.page_index, 1);
            assert_eq!(d.component_name, "Resistor");
            assert_eq!(d.package_name, "RESC2012 (0805)");
            assert!(!d.component_error && !d.package_error);
            assert_eq!(d.pinout.row_count(), 2);
            assert!(!d.has_unconnected_pads);
        }
        for scene in [0, 1] {
            let image = with_tab(&app, tab, |t| t.render_scene(300.0, 200.0, 1.0, scene));
            assert_eq!(image.size().width, 300, "preview {scene}");
        }
        save(headless, "element_device.png");

        // Pinout: disconnect a pad, then reconnect it interactively.
        let d = derived();
        let mut row = d.pinout.row_data(0).unwrap();
        let signal_index = row.signal_index;
        assert!(signal_index > 0);
        row.signal_index = 0;
        d.pinout.set_row_data(0, row);
        headless.settle(5);
        backend.invoke_trigger_tab(tab.0 as i32, tab.1 as i32, ui::TabAction::Unlock);
        headless.settle(5);
        let d = derived();
        assert_eq!(d.pinout.row_data(0).unwrap().signal_index, 0);
        assert!(d.has_unconnected_pads);
        backend.invoke_trigger_tab(
            tab.0 as i32,
            tab.1 as i32,
            ui::TabAction::DevicePinoutConnectInteractively,
        );
        headless.settle(5);
        let d = derived();
        assert!(d.interactive_pinout_number > 0);
        assert!(d.interactive_pinout_signals.row_count() >= 2);
        save(headless, "element_device_interactive.png");
        // Choose the first unused signal.
        let choice = (0..d.interactive_pinout_signals.row_count())
            .find(|i| {
                let s = d.interactive_pinout_signals.row_data(*i).unwrap();
                !s.name.is_empty() && !s.used
            })
            .unwrap();
        with_tab(&app, tab, |t| {
            let Tab::Device(dt) = t else { unreachable!() };
            let mut d = dt.derived_ui_data();
            d.interactive_pinout_signal_index = choice as i32;
            dt.set_derived_ui_data(&d);
            t.trigger(ui::TabAction::Accept);
            let Tab::Device(dt) = t else { unreachable!() };
            assert!(!dt.is_interactive(), "all pads connected");
        });
        assert!(!derived().has_unconnected_pads);

        // Parts: the "new part" row is added on "apply".
        let d = derived();
        assert_eq!(d.parts.row_count(), 1, "the new part row");
        let mut part = d.parts.row_data(0).unwrap();
        part.mpn = "RC0805".into();
        part.manufacturer = "ACME".into();
        d.parts.set_row_data(0, part);
        headless.settle(5);
        backend.invoke_trigger_tab(tab.0 as i32, tab.1 as i32, ui::TabAction::Apply);
        headless.settle(5);
        let d = derived();
        assert_eq!(d.parts.row_count(), 2);
        assert_eq!(d.parts.row_data(0).unwrap().mpn, "RC0805");
        assert_eq!(
            d.parts.row_data(1).unwrap().mpn,
            "",
            "MPN of the new row reset"
        );
        assert_eq!(d.parts.row_data(1).unwrap().manufacturer, "ACME");
        with_tab(&app, tab, |t| {
            t.trigger(ui::TabAction::Save);
            assert!(!t.ui_data().unsaved_changes);
        });
        headless.settle(10);
        let content = std::fs::read_to_string(dev_dir.path_to("device.lp").as_path()).unwrap();
        assert!(
            content.contains("(part \"RC0805\" (manufacturer \"ACME\")"),
            "{content}"
        );
        backend.invoke_trigger_tab(tab.0 as i32, tab.1 as i32, ui::TabAction::Apply);
        headless.settle(10);
        save(headless, "element_device_saved.png");

        // A new device: component & package, metadata, pinout, parts.
        backend.invoke_trigger_library(lib.to_native().into(), ui::LibraryAction::NewDevice);
        headless.settle(10);
        let new = find_tab(
            &app,
            |t| matches!(t, Tab::Device(s) if s.core().wizard_mode),
        )
        .expect("new device tab");
        let trigger = |action| {
            backend.invoke_trigger_tab(new.0 as i32, new.1 as i32, action);
            headless.settle(10);
        };
        assert_eq!(
            with_tab(&app, new, |t| {
                let Tab::Device(d) = t else { unreachable!() };
                d.derived_ui_data().component_name
            }),
            ""
        );
        trigger(ui::TabAction::DeviceSelectComponent);
        save(headless, "element_device_choose_component.png");
        choose_in_dialog(&app, headless, "Resistor");
        trigger(ui::TabAction::DeviceSelectPackage);
        choose_in_dialog(&app, headless, "RESC2012 (0805)");
        save(headless, "element_device_new.png");
        trigger(ui::TabAction::Next);
        let name = with_tab(&app, new, |t| {
            let Tab::Device(d) = t else { unreachable!() };
            assert_eq!(d.core().page_index, 1);
            d.derived_ui_data().name
        });
        assert_eq!(name, "Resistor (RESC2012 (0805))");
        trigger(ui::TabAction::Next);
        let page = with_tab(&app, new, |t| {
            let Tab::Device(d) = t else { unreachable!() };
            d.core().page_index
        });
        assert_eq!(page, 2, "the pinout page");
        trigger(ui::TabAction::DevicePinoutConnectAuto);
        trigger(ui::TabAction::Next);
        trigger(ui::TabAction::Next);
        let dir = with_tab(&app, new, |t| {
            let Tab::Device(d) = t else { unreachable!() };
            assert!(!d.core().wizard_mode);
            assert_eq!(d.core().page_index, 1);
            d.core().directory_path()
        });
        assert!(dir.path_to("device.lp").is_existing_file());
    });
}

/// The "LEDs" component category (parent "Optoelectronics").
pub const LEDS_CATEGORY: &str = "70421345-ae1d-4fed-aa60-e7619524b97f";
/// The "PCBWay" organization.
pub const PCBWAY_ORGANIZATION: &str = "504dcf76-4e7d-4270-9239-2c296484e21f";

#[test]
fn category_and_organization_editors() {
    crate::common::with_headless(W, H, |headless| {
        let tmp = tempfile::tempdir().unwrap();
        let (app, lib) = app_with_library(headless, tmp.path());
        let backend = app.window().global::<ui::Backend>();
        let forms = app.window().global::<ui::Dialogs>();

        // Component category: parent chooser, undo, save.
        let cat_dir = lib.path_to(&format!("cmpcat/{LEDS_CATEGORY}"));
        open_from_library_tab(&app, headless, &lib, &cat_dir);
        let tab = find_tab(&app, |t| matches!(t, Tab::ComponentCategory(_))).expect("category tab");
        let data = || {
            with_tab(&app, tab, |t| {
                let Tab::ComponentCategory(c) = t else {
                    unreachable!()
                };
                c.derived_ui_data()
            })
        };
        let d = data();
        assert_eq!(d.name, "LEDs");
        let parents: Vec<String> = (0..d.parents.row_count())
            .map(|i| d.parents.row_data(i).unwrap().to_string())
            .collect();
        assert_eq!(parents, ["Root Category", "Optoelectronics"]);
        let tree: Vec<String> = (0..d.parents_tree.row_count())
            .map(|i| d.parents_tree.row_data(i).unwrap().text.to_string())
            .collect();
        assert_eq!(tree[0], "Root Category");
        assert!(tree.contains(&"Optoelectronics".to_owned()));
        assert!(!tree.contains(&"LEDs".to_owned()), "itself is hidden");
        save(headless, "element_category.png");
        with_tab(&app, tab, |t| {
            let Tab::ComponentCategory(c) = t else {
                unreachable!()
            };
            let mut d = c.derived_ui_data();
            d.new_parent = "null".into();
            c.set_derived_ui_data(&d);
            assert_eq!(c.derived_ui_data().parents.row_count(), 1);
            assert!(t.ui_data().unsaved_changes);
            t.trigger(ui::TabAction::Undo);
            let Tab::ComponentCategory(c) = t else {
                unreachable!()
            };
            assert_eq!(c.derived_ui_data().parents.row_count(), 2, "undone");
            t.trigger(ui::TabAction::Redo);
            let Tab::ComponentCategory(c) = t else {
                unreachable!()
            };
            let mut d = c.derived_ui_data();
            d.description = "Light emitting diodes".into();
            c.set_derived_ui_data(&d);
            t.trigger(ui::TabAction::Save);
            assert!(!t.ui_data().unsaved_changes);
        });
        headless.settle(10);
        let content =
            std::fs::read_to_string(cat_dir.path_to("component_category.lp").as_path()).unwrap();
        assert!(content.contains("(parent none)"), "{content}");
        assert!(content.contains("(description \"Light emitting diodes\")"));

        // Organization: priority, PCB design rules.
        let org_dir = lib.path_to(&format!("org/{PCBWAY_ORGANIZATION}"));
        open_from_library_tab(&app, headless, &lib, &org_dir);
        let tab = find_tab(&app, |t| matches!(t, Tab::Organization(_))).expect("organization tab");
        let data = || {
            with_tab(&app, tab, |t| {
                let Tab::Organization(o) = t else {
                    unreachable!()
                };
                o.derived_ui_data()
            })
        };
        let d = data();
        assert_eq!(d.name, "PCBWay");
        assert_eq!(d.priority, 34);
        let rules = d.pcb_design_rules.row_count();
        assert!(rules >= 1);
        save(headless, "element_organization.png");
        backend.invoke_trigger_tab(
            tab.0 as i32,
            tab.1 as i32,
            ui::TabAction::OrganizationAddPcbDesignRules,
        );
        headless.settle(10);
        assert!(forms.get_form_shown());
        {
            let mut state = app.state().borrow_mut();
            let dialog = state.form_dialog().unwrap();
            dialog
                .dialog
                .form_mut()
                .edit("name", |f| f.text = "Custom Rules".into());
        }
        forms.invoke_form_button(-1);
        headless.settle(10);
        let d = data();
        assert_eq!(d.pcb_design_rules.row_count(), rules + 1);
        assert_eq!(
            d.pcb_design_rules.row_data(rules).unwrap().name,
            "Custom Rules"
        );
        // Remove it again through the row action.
        let mut row = d.pcb_design_rules.row_data(rules).unwrap();
        row.action = ui::OrganizationPcbDesignRulesAction::Delete;
        d.pcb_design_rules.set_row_data(rules, row);
        headless.settle(10);
        assert_eq!(data().pcb_design_rules.row_count(), rules);
        with_tab(&app, tab, |t| {
            let Tab::Organization(o) = t else {
                unreachable!()
            };
            let mut d = o.derived_ui_data();
            d.priority = 40;
            o.set_derived_ui_data(&d);
            t.trigger(ui::TabAction::Save);
            assert!(!t.ui_data().unsaved_changes);
        });
        headless.settle(10);
        let content =
            std::fs::read_to_string(org_dir.path_to("organization.lp").as_path()).unwrap();
        assert!(content.contains("(priority 40)"), "{content}");

        // Closing with unsaved changes asks; "No" discards them.
        with_tab(&app, tab, |t| {
            let Tab::Organization(o) = t else {
                unreachable!()
            };
            let mut d = o.derived_ui_data();
            d.priority = 50;
            o.set_derived_ui_data(&d);
        });
        backend.invoke_trigger_tab(tab.0 as i32, tab.1 as i32, ui::TabAction::Close);
        headless.settle(10);
        assert!(forms.get_form_shown(), "save changes question");
        save(headless, "element_close_question.png");
        forms.invoke_form_button(1);
        headless.settle(10);
        assert!(!forms.get_form_shown());
        assert!(find_tab(&app, |t| matches!(t, Tab::Organization(_))).is_none());
        let content =
            std::fs::read_to_string(org_dir.path_to("organization.lp").as_path()).unwrap();
        assert!(content.contains("(priority 40)"));

        // A new package category.
        backend.invoke_trigger_library(
            lib.to_native().into(),
            ui::LibraryAction::NewPackageCategory,
        );
        headless.settle(10);
        let new = find_tab(&app, |t| matches!(t, Tab::PackageCategory(_))).expect("new tab");
        let dir = with_tab(&app, new, |t| {
            let Tab::PackageCategory(c) = t else {
                unreachable!()
            };
            let mut d = c.derived_ui_data();
            assert_eq!(d.name, "");
            d.name = "My Packages".into();
            c.set_derived_ui_data(&d);
            t.trigger(ui::TabAction::Save);
            let Tab::PackageCategory(c) = t else {
                unreachable!()
            };
            c.core().directory_path()
        });
        assert!(dir.path_to("package_category.lp").is_existing_file());
        headless.settle(10);
        save(headless, "element_package_category_new.png");
    });
}

#[test]
fn symbol_editor() {
    crate::common::with_headless(W, H, |headless| {
        let tmp = tempfile::tempdir().unwrap();
        let (app, lib) = app_with_library(headless, tmp.path());
        let backend = app.window().global::<ui::Backend>();
        let forms = app.window().global::<ui::Dialogs>();

        // Open the symbol from the library tab (like double-clicking it).
        backend.invoke_trigger_library(lib.to_native().into(), ui::LibraryAction::Open);
        headless.settle(10);
        let lib_tab = find_tab(&app, |t| matches!(t, Tab::Library(_))).unwrap();
        let sym_dir = lib.path_to(&format!("sym/{DIODE_SYMBOL}"));
        let update = with_tab(&app, lib_tab, |t| {
            let Tab::Library(t) = t else { unreachable!() };
            let rows = t.elements_model();
            let row = (0..rows.len())
                .find(|i| rows.get(*i).unwrap().user_data == sym_dir.as_str())
                .expect("diode symbol row");
            let mut d = t.derived_ui_data();
            d.filtered_elements_index = row as i32;
            t.set_derived_ui_data(&d);
            t.trigger(ui::TabAction::EditProperties)
        });
        assert!(update.requests.iter().any(|r| matches!(
            r,
            TabRequest::OpenLibraryElement { path, .. } if *path == sym_dir
        )));
        backend.invoke_trigger_tab(
            lib_tab.0 as i32,
            lib_tab.1 as i32,
            ui::TabAction::EditProperties,
        );
        headless.settle(10);
        let sym = find_tab(&app, |t| matches!(t, Tab::Symbol(_))).expect("symbol tab");
        let symbol = |app: &App| {
            with_tab(app, sym, |t| {
                let Tab::Symbol(t) = t else { unreachable!() };
                (
                    t.core().editor.element().polygons().len(),
                    t.core().editor.element().pins().len(),
                )
            })
        };
        let (polygons, pins) = symbol(&app);
        assert!(pins >= 2);
        {
            let data = app.window().global::<ui::Data>();
            let sections = data.get_sections();
            let tabs = sections.row_data(sym.0).unwrap().symbol_tabs;
            let d = tabs.row_data(sym.1).unwrap();
            assert_eq!(d.name, "Diode");
            assert_eq!(d.page_index, 1);
            assert!(d.categories.row_count() >= 1, "category with path");
        }
        save(headless, "element_symbol.png");

        // Draw a line with the line tool.
        with_tab(&app, sym, |t| {
            let Tab::Symbol(t) = t else { unreachable!() };
            t.trigger(ui::TabAction::ToolLine);
            assert_eq!(t.fsm().tool(), LibraryTool::DrawLine);
            assert_eq!(t.derived_ui_data().tool, ui::EditorTool::Line);
            let _ = t.render_scene(1000.0, 800.0, 1.0);
            for p in [mm(5.08, 5.08), mm(10.16, 5.08)] {
                let s = t
                    .canvas()
                    .view()
                    .world_to_screen(librepcb_app::tabs::editing::point_to_world(p));
                use librepcb_canvas::{Modifiers, PointerButton, PointerKind};
                t.pointer_event(
                    PointerKind::Move,
                    PointerButton::Other,
                    s,
                    Modifiers::default(),
                );
                t.pointer_event(
                    PointerKind::Down,
                    PointerButton::Left,
                    s,
                    Modifiers::default(),
                );
                t.pointer_event(
                    PointerKind::Up,
                    PointerButton::Left,
                    s,
                    Modifiers::default(),
                );
                std::thread::sleep(std::time::Duration::from_millis(510));
            }
            t.trigger(ui::TabAction::Abort);
            t.trigger(ui::TabAction::Abort);
        });
        assert_eq!(symbol(&app).0, polygons + 1);
        save(headless, "element_symbol_line.png");
        // Undo removes it again.
        with_tab(&app, sym, |t| {
            let Tab::Symbol(t) = t else { unreachable!() };
            assert!(t.ui_data().unsaved_changes);
            t.trigger(ui::TabAction::Undo);
        });
        assert_eq!(symbol(&app).0, polygons);

        // Pin properties dialog: rename a pin.
        let pin = with_tab(&app, sym, |t| {
            let Tab::Symbol(t) = t else { unreachable!() };
            *t.core().editor.element().pins().uuids().first().unwrap()
        });
        let id = {
            let state = app.state().borrow();
            state.sections()[sym.0].tabs()[sym.1].id()
        };
        app.state().borrow_mut().open_library_item_properties(
            id,
            librepcb_app::tabs::LibraryItemRef::Symbol(
                librepcb_editor::library_editor::commands::SymbolItem::Pin(pin),
            ),
        );
        headless.settle(10);
        assert!(forms.get_form_shown());
        save(headless, "element_symbol_pin_dialog.png");
        {
            let mut state = app.state().borrow_mut();
            let dialog = state.form_dialog().unwrap();
            dialog
                .dialog
                .form_mut()
                .edit("name", |f| f.text = "XYZ".into());
        }
        forms.invoke_form_button(-1);
        headless.settle(10);
        assert!(!forms.get_form_shown());
        let name = with_tab(&app, sym, |t| {
            let Tab::Symbol(t) = t else { unreachable!() };
            t.core()
                .editor
                .element()
                .pins()
                .by_uuid(&pin)
                .unwrap()
                .name()
                .to_string()
        });
        assert_eq!(name, "XYZ");

        // Metadata: description, then save; the file is written.
        with_tab(&app, sym, |t| {
            let Tab::Symbol(t) = t else { unreachable!() };
            let mut d = t.derived_ui_data();
            d.description = "Edited description".into();
            t.set_derived_ui_data(&d);
            t.trigger(ui::TabAction::Save);
            assert!(!t.ui_data().unsaved_changes);
        });
        headless.settle(10);
        with_tab(&app, sym, |t| {
            let Tab::Symbol(t) = t else { unreachable!() };
            assert!(!t.ui_data().unsaved_changes, "still saved after UI updates");
        });
        let content = std::fs::read_to_string(sym_dir.path_to("symbol.lp").as_path()).unwrap();
        assert!(content.contains("Edited description"));
        assert!(content.contains("(name \"XYZ\")"));

        // Checks with approvals.
        with_tab(&app, sym, |t| {
            let Tab::Symbol(t) = t else { unreachable!() };
            let checks = t.derived_ui_data().checks;
            assert!(checks.execution_error.is_empty());
        });

        // A new symbol (wizard mode), saved into the library.
        backend.invoke_trigger_library(lib.to_native().into(), ui::LibraryAction::NewSymbol);
        headless.settle(10);
        let new = find_tab(
            &app,
            |t| matches!(t, Tab::Symbol(s) if s.core().wizard_mode),
        )
        .expect("new symbol tab");
        save(headless, "element_symbol_new.png");
        with_tab(&app, sym, |t| {
            let Tab::Symbol(t) = t else { unreachable!() };
            let d = t.core().editor.is_dirty();
            let ui = t.ui_data().unsaved_changes;
            assert!(!ui && !d, "diode still saved: {ui} {d}");
        });
        let new_dir = with_tab(&app, new, |t| {
            let Tab::Symbol(t) = t else { unreachable!() };
            let mut d = t.derived_ui_data();
            assert_eq!(d.name, "");
            d.name = "My Symbol".into();
            t.set_derived_ui_data(&d);
            t.trigger(ui::TabAction::Save);
            assert!(!t.core().wizard_mode);
            t.core().directory_path()
        });
        assert!(new_dir.path_to("symbol.lp").is_existing_file());
    });
}
