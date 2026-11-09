//! Library management (M4a): the libraries panel with a local library, the
//! create library tab (creates and opens a library), the download library
//! tab (validation, no network) and the library tab (content tree, filter,
//! metadata editing with undo, checks with autofix and approvals, save).
//! Screenshots `library_*.png` are written to `$CARGO_TARGET_TMPDIR`.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use librepcb_app::screenshot::write_png;
use librepcb_app::tabs::{CreateLibraryTab, DownloadLibraryTab, Tab, TabRequest};
use librepcb_app::{App, startup, ui};
use librepcb_core::fileio::{FilePath, file_utils};
use slint::{ComponentHandle, Model};

const W: u32 = 1400;
const H: u32 = 900;

fn populated_library() -> FilePath {
    FilePath::new(
        Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
            .join("tests/data/libraries/Populated Library.lplib"),
    )
    .unwrap()
}

/// A workspace with a copy of the upstream test library.
fn workspace(dir: &Path) -> (librepcb_core::workspace::Workspace, FilePath) {
    let workspace = startup::open_workspace(&dir.join("workspace")).unwrap();
    let lib = workspace
        .local_libraries_path()
        .path_to("Populated Library.lplib");
    file_utils::copy_dir_recursively(&populated_library(), &lib).unwrap();
    workspace
        .library_db()
        .rescan(&AtomicBool::new(false), &mut |_| {})
        .unwrap();
    (workspace, lib)
}

#[test]
fn create_library_tab() {
    let tmp = tempfile::tempdir().unwrap();
    let local = FilePath::new(tmp.path().join("local")).unwrap();
    let mut tab = CreateLibraryTab::new(local.clone(), "Tester");
    let mut d = tab.derived_ui_data();
    assert!(d.valid, "defaults are valid");
    assert_eq!(d.directory_default, "My_Library.lplib");
    // Invalid inputs.
    d.name = "".into();
    d.version = "x".into();
    d.url = "http://".into();
    tab.set_derived_ui_data(&d);
    let d2 = tab.derived_ui_data();
    assert!(!d2.valid);
    assert_eq!(d2.name_error, "Required");
    assert_eq!(d2.version_error, "Invalid");
    // Valid again, with CC0 license.
    d.name = "Test Lib".into();
    d.version = "".into();
    d.url = "https://example.com/lib".into();
    d.cc0 = true;
    d.description = "A test".into();
    tab.set_derived_ui_data(&d);
    assert!(tab.derived_ui_data().valid);
    let update = tab.trigger(ui::TabAction::Accept);
    assert!(update.close);
    let dir = local.path_to("Test_Lib.lplib");
    assert!(update.requests.contains(&TabRequest::OpenLibrary {
        path: dir.clone(),
        wizard: true
    }));
    for file in [
        "library.lp",
        ".librepcb-lib",
        "library.png",
        "README.md",
        "LICENSE.txt",
        ".gitignore",
        ".gitattributes",
    ] {
        assert!(dir.path_to(file).is_existing_file(), "{file}");
    }
    let lp = std::fs::read_to_string(dir.path_to("library.lp").as_path()).unwrap();
    assert!(lp.contains("(name \"Test Lib\")"), "{lp}");
    assert!(lp.contains("(version \"0.1\")"));
    assert!(lp.contains("(author \"Tester\")"));
    assert!(lp.contains("(url \"https://example.com/lib\")"));
    let readme = std::fs::read_to_string(dir.path_to("README.md").as_path()).unwrap();
    assert!(readme.starts_with("# Test Lib"));
    // The directory exists now.
    let tab = CreateLibraryTab::new(local, "Tester");
    let mut d = tab.derived_ui_data();
    d.name = "Test Lib".into();
    let mut tab = tab;
    tab.set_derived_ui_data(&d);
    assert_eq!(tab.derived_ui_data().directory_error, "Exists already");
    // The library can be opened.
    let lib = librepcb_app::open_library::OpenLibrary::open(&dir, false).unwrap();
    assert!(lib.run_checks().is_ok());
}

#[test]
fn download_library_tab() {
    let tmp = tempfile::tempdir().unwrap();
    let local = FilePath::new(tmp.path().join("local")).unwrap();
    let mut tab = DownloadLibraryTab::new(local.clone());
    assert!(!tab.derived_ui_data().valid);
    assert_eq!(tab.derived_ui_data().url_error, "Required");
    let mut d = tab.derived_ui_data();
    d.url = "https://github.com/LibrePCB-Libraries/LibrePCB_Base.lplib".into();
    tab.set_derived_ui_data(&d);
    let d = tab.derived_ui_data();
    assert!(d.valid);
    assert_eq!(
        d.url_suggestion,
        "https://github.com/LibrePCB-Libraries/LibrePCB_Base.lplib/archive/refs/heads/master.zip"
    );
    assert_eq!(d.directory_default, "LibrePCB_Base.lplib");
    let update = tab.trigger(ui::TabAction::Accept);
    assert!(update.requests.iter().any(|r| matches!(
        r,
        TabRequest::DownloadLibrary { dir, .. } if *dir == local.path_to("LibrePCB_Base.lplib")
    )));
    assert!(tab.derived_ui_data().download_running);
    // Without a started download (no network in tests), cancel closes.
    assert!(tab.trigger(ui::TabAction::Cancel).close);
}

fn library_tab(app: &App) -> (usize, usize) {
    let state = app.state().borrow();
    state
        .sections()
        .iter()
        .enumerate()
        .find_map(|(si, s)| {
            s.tabs()
                .iter()
                .position(|t| matches!(t, Tab::Library(_)))
                .map(|ti| (si, ti))
        })
        .expect("library tab")
}

fn with_library_tab<R>(app: &App, f: impl FnOnce(&mut librepcb_app::tabs::LibraryTab) -> R) -> R {
    let (si, ti) = library_tab(app);
    let mut state = app.state().borrow_mut();
    let Some(Tab::Library(t)) = state.sections_mut()[si].tab_mut(ti) else {
        panic!("library tab");
    };
    f(t)
}

#[test]
fn libraries_panel_and_library_tab() {
    crate::common::with_headless(W, H, |headless| {
        let save = |name: &str| {
            let pixels = headless.settle(20);
            let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
            write_png(&path, W, H, &pixels).unwrap();
            println!("screenshot: {}", path.display());
        };
        let tmp = tempfile::tempdir().unwrap();
        let (workspace, lib_dir) = workspace(tmp.path());
        librepcb_i18n::set_language("en").unwrap();
        let window = ui::AppWindow::new().unwrap();
        slint::select_bundled_translation("en").unwrap();
        let app = App::new(window, workspace);
        app.window().show().unwrap();
        let data = app.window().global::<ui::Data>();
        let backend = app.window().global::<ui::Backend>();
        headless.run_until(std::time::Duration::from_secs(20), || {
            !data.get_libraries_rescan_in_progress()
        });
        headless.settle(5);

        // Libraries panel: the local library.
        assert_eq!(data.get_local_libraries_data().count, 1);
        let local = data.get_local_libraries();
        assert_eq!(local.row_count(), 1);
        assert_eq!(local.row_data(0).unwrap().name, "Populated Library");
        app.show_panel(ui::PanelPage::Libraries);
        save("library_panel.png");

        // Open the library tab.
        backend.invoke_trigger_library(lib_dir.to_native().into(), ui::LibraryAction::Open);
        headless.settle(10);
        assert_eq!(data.get_libraries().row_count(), 1);
        let (categories, elements) = with_library_tab(&app, |t| {
            (t.categories_model().len(), t.elements_model().len())
        });
        assert!(categories > 3, "all, categories");
        assert!(elements > 10, "all elements with headers");
        save("library_tab.png");

        // Filter by the find term.
        with_library_tab(&app, |t| {
            assert!(t.set_find_term("zzz-nothing"));
            // Only the type headers remain.
            assert!(t.elements_model().len() <= 5);
            t.set_find_term("");
        });

        // Select a component category: its elements.
        with_library_tab(&app, |t| {
            let mut d = t.derived_ui_data();
            d.categories_index = 2;
            t.set_derived_ui_data(&d);
            assert!(t.elements_model().len() < elements);
        });

        // Edit the metadata: one undo step, dirty, undo restores it.
        with_library_tab(&app, |t| {
            let mut d = t.derived_ui_data();
            d.name = "Renamed Library".into();
            t.set_derived_ui_data(&d);
            t.trigger(ui::TabAction::Apply);
            assert!(t.has_unsaved_changes());
            assert_eq!(t.ui_data().title, "Renamed Library");
            assert_eq!(t.ui_data().undo_text, "Edit library metadata");
            t.trigger(ui::TabAction::Undo);
            assert_eq!(t.ui_data().title, "Populated Library");
            t.trigger(ui::TabAction::Redo);
            assert_eq!(t.ui_data().title, "Renamed Library");
        });

        // Checks: approve a message; autofix the lowercase name.
        with_library_tab(&app, |t| {
            let mut d = t.derived_ui_data();
            d.name = "renamed library".into();
            t.set_derived_ui_data(&d);
            t.trigger(ui::TabAction::Apply);
            let checks = t.derived_ui_data().checks;
            let messages = checks.messages;
            let row = (0..messages.row_count())
                .find(|i| messages.row_data(*i).unwrap().supports_autofix)
                .expect("autofix message");
            let mut m = messages.row_data(row).unwrap();
            m.action = ui::RuleCheckMessageAction::Autofix;
            t.check_row_written(row, &m);
            assert_eq!(t.ui_data().title, "Renamed Library");
        });
        save("library_tab_edited.png");

        // Save: the name is written to the file.
        with_library_tab(&app, |t| {
            t.trigger(ui::TabAction::Save);
            assert!(!t.has_unsaved_changes());
        });
        let lp = std::fs::read_to_string(lib_dir.path_to("library.lp").as_path()).unwrap();
        assert!(lp.contains("(name \"Renamed Library\")"));

        // Create and download library tabs.
        backend.invoke_trigger(ui::Action::LibraryCreate);
        headless.settle(10);
        save("library_create_tab.png");
        backend.invoke_trigger(ui::Action::LibraryDownload);
        headless.settle(10);
        save("library_download_tab.png");
        {
            let state = app.state().borrow();
            let tabs = state.sections()[0].tabs();
            assert!(tabs.iter().any(|t| matches!(t, Tab::CreateLibrary(_))));
            assert!(tabs.iter().any(|t| matches!(t, Tab::DownloadLibrary(_))));
        }
    });
}
