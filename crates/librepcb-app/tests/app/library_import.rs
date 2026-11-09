//! The EAGLE and KiCad library import wizards (M4c) in the headless
//! application: choose the source, select elements (with dependencies),
//! options, the import in a worker thread and the library rescan.
//! Screenshots `library_import_*.png` are written to `$CARGO_TARGET_TMPDIR`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use librepcb_app::dialogs::library_import::{LibraryImportWizard, Page};
use librepcb_app::screenshot::write_png;
use librepcb_app::{App, startup, ui};
use librepcb_core::fileio::FilePath;
use slint::{ComponentHandle, Model};

use crate::lifecycle::ui_edit;

const W: u32 = 1400;
const H: u32 = 900;

fn upstream(path: &str) -> PathBuf {
    Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join(path)
}

fn list_items(dialogs: &ui::Dialogs<'_>, id: &str) -> Vec<(String, bool)> {
    let fields = dialogs.get_form_fields();
    (0..fields.row_count())
        .filter_map(|i| fields.row_data(i))
        .find(|f| f.id == id)
        .map(|f| {
            (0..f.items.row_count())
                .filter_map(|i| f.items.row_data(i))
                .map(|i| (i.text.to_string(), i.checked))
                .collect()
        })
        .unwrap_or_default()
}

fn text(dialogs: &ui::Dialogs<'_>, id: &str) -> String {
    let fields = dialogs.get_form_fields();
    (0..fields.row_count())
        .filter_map(|i| fields.row_data(i))
        .find(|f| f.id == id)
        .map(|f| f.text.to_string())
        .unwrap_or_default()
}

#[test]
fn import_wizards() {
    crate::common::with_headless(W, H, |headless| {
        let save = |name: &str| {
            let pixels = headless.settle(20);
            let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
            write_png(&path, W, H, &pixels).unwrap();
            println!("screenshot: {}", path.display());
        };
        let tmp = tempfile::tempdir().unwrap();
        let workspace = startup::open_workspace(&tmp.path().join("workspace")).unwrap();
        // An empty local library as destination.
        let lib = workspace.local_libraries_path().path_to("Import.lplib");
        librepcb_core::fileio::file_utils::copy_dir_recursively(
            &FilePath::new(upstream("tests/data/libraries/Empty Library.lplib")).unwrap(),
            &lib,
        )
        .unwrap();
        workspace
            .library_db()
            .rescan(&std::sync::atomic::AtomicBool::new(false), &mut |_| {})
            .unwrap();
        librepcb_i18n::set_language("en").unwrap();
        let window = ui::AppWindow::new().unwrap();
        slint::select_bundled_translation("en").unwrap();
        let app = App::new(window, workspace);
        app.window().show().unwrap();
        let data = app.window().global::<ui::Data>();
        let dialogs = app.window().global::<ui::Dialogs>();
        let backend = app.window().global::<ui::Backend>();
        headless.run_until(Duration::from_secs(20), || {
            !data.get_libraries_rescan_in_progress()
        });
        backend.invoke_trigger_library(lib.to_native().into(), ui::LibraryAction::Open);
        headless.settle(10);

        // EAGLE.
        backend.invoke_trigger_library_element(
            lib.to_native().into(),
            ui::LibraryElementAction::ImportEagleLibrary,
        );
        headless.settle(5);
        assert!(dialogs.get_form_shown());
        assert_eq!(dialogs.get_form_title(), "EAGLE Library Import");
        save("library_import_eagle_start.png");
        dialogs.invoke_form_button(-1); // Next
        headless.settle(5);
        let lbr = upstream("tests/data/unittests/eagleimport/testlibrary.lbr");
        ui_edit(&dialogs, "source", |f| {
            f.text = lbr.to_string_lossy().as_ref().into();
        });
        headless.settle(2);
        dialogs.invoke_form_button(-1); // Next: parse
        headless.settle(5);
        let page = |app: &App| {
            app.state()
                .borrow_mut()
                .form_dialog()
                .and_then(|o| o.dialog.as_any_mut())
                .and_then(|a| a.downcast_mut::<LibraryImportWizard>())
                .map(|w| w.page())
        };
        assert_eq!(
            page(&app),
            Some(Page::SelectElements),
            "{}",
            dialogs.get_form_error()
        );
        let devices = list_items(&dialogs, "devices");
        assert!(!devices.is_empty());
        assert!(devices.iter().all(|(_, c)| *c), "all checked initially");
        // Uncheck all, then check one device: its dependencies follow.
        for id in [
            "devices_all",
            "components_all",
            "symbols_all",
            "packages_all",
        ] {
            ui_edit(&dialogs, id, |f| f.checked = false);
            headless.settle(2);
        }
        assert!(list_items(&dialogs, "packages").iter().all(|(_, c)| !*c));
        ui_edit(&dialogs, "devices", |f| {
            f.action = ui::FormFieldAction::Toggle;
            f.action_row = 0;
            f.items.set_row_data(0, {
                let mut item = f.items.row_data(0).unwrap();
                item.checked = true;
                item
            });
        });
        headless.settle(5);
        assert!(list_items(&dialogs, "devices")[0].1);
        assert!(
            list_items(&dialogs, "packages").iter().any(|(_, c)| *c),
            "the package of the device is selected as dependency"
        );
        save("library_import_eagle_select.png");
        dialogs.invoke_form_button(-1); // Next: options
        headless.settle(5);
        save("library_import_eagle_options.png");
        dialogs.invoke_form_button(-1); // Import!
        let finished = headless.run_until(Duration::from_secs(60), || {
            app.state()
                .borrow_mut()
                .form_dialog()
                .and_then(|o| o.dialog.as_any_mut())
                .and_then(|a| a.downcast_mut::<LibraryImportWizard>())
                .is_some_and(|w| w.is_finished())
        });
        assert!(finished);
        headless.settle(10);
        assert_eq!(text(&dialogs, "status"), "Finished!");
        save("library_import_eagle_result.png");
        for kind in ["dev", "cmp", "sym", "pkg"] {
            let dir = lib.path_to(kind);
            let count = std::fs::read_dir(dir.as_path()).map_or(0, |d| d.count());
            assert!(count > 0, "{kind}");
        }
        dialogs.invoke_form_button(-1); // Close
        headless.settle(2);
        assert!(!dialogs.get_form_shown());

        // KiCad.
        backend.invoke_trigger_library_element(
            lib.to_native().into(),
            ui::LibraryElementAction::ImportKicadLibrary,
        );
        headless.settle(5);
        assert_eq!(dialogs.get_form_title(), "KiCad Library Import");
        dialogs.invoke_form_button(-1); // Next
        headless.settle(5);
        let src = upstream("tests/data/unittests/kicadimport");
        ui_edit(&dialogs, "source", |f| {
            f.text = src.to_string_lossy().as_ref().into();
        });
        headless.settle(2);
        dialogs.invoke_form_button(-1); // Next: scan and parse
        headless.settle(5);
        assert_eq!(
            page(&app),
            Some(Page::SelectElements),
            "{}",
            dialogs.get_form_error()
        );
        assert!(!list_items(&dialogs, "packages").is_empty());
        save("library_import_kicad_select.png");
        dialogs.invoke_form_button(-1); // Next: options
        headless.settle(5);
        dialogs.invoke_form_button(-1); // Import!
        let finished = headless.run_until(Duration::from_secs(120), || {
            app.state()
                .borrow_mut()
                .form_dialog()
                .and_then(|o| o.dialog.as_any_mut())
                .and_then(|a| a.downcast_mut::<LibraryImportWizard>())
                .is_some_and(|w| w.is_finished())
        });
        assert!(finished);
        headless.settle(10);
        assert!(
            text(&dialogs, "messages").contains("It is highly recommended"),
            "{}",
            text(&dialogs, "messages")
        );
        save("library_import_kicad_result.png");
    });
}
