//! The project lifecycle (M3c): the new project wizard, the directory
//! lock and autosave restore prompts, autosave and the file format upgrade
//! notification. Screenshots `lifecycle_*.png` are written to
//! `$CARGO_TARGET_TMPDIR`.

use std::path::{Path, PathBuf};

use librepcb_app::dialogs::new_project::{NewProjectMode, NewProjectWizard, create_project};
use librepcb_app::dialogs::{AppRequest, Applied, DialogContext, FormDialog};
use librepcb_app::screenshot::write_png;
use librepcb_app::{App, startup, ui};
use librepcb_core::fileio::{FilePath, file_utils};
use librepcb_core::types::ElementName;
use librepcb_editor::commands::AddSchematic;
use slint::{ComponentHandle, Model};

const W: u32 = 1400;
const H: u32 = 900;

fn fp(p: &Path) -> FilePath {
    FilePath::new(p).unwrap()
}

#[test]
fn new_project_wizard() {
    let tmp = tempfile::tempdir().unwrap();
    let location = fp(tmp.path());
    let mut wizard = NewProjectWizard::new(
        NewProjectMode::NewProject,
        location.clone(),
        "Tester",
        vec!["de_CH".into()],
        vec!["IEC 60617".into()],
    );
    // Without a name, the page is incomplete.
    assert!(wizard.next_page().is_err());
    assert_eq!(wizard.page_index(), 0);
    wizard
        .form_mut()
        .edit("name", |f| f.text = "My Project".into());
    wizard.field_event(
        &DialogContext::without_project(),
        "name",
        librepcb_app::dialogs::FieldEvent::Edited,
    );
    let expected = location.path_to("My_Project/My_Project.lpp");
    assert_eq!(wizard.form().get_text("path"), expected.to_native());
    assert_eq!(wizard.form().get_text("author"), "Tester");
    wizard.form_mut().edit("license", |f| f.index = 1);
    wizard.next_page().unwrap();
    assert_eq!(wizard.page_index(), 1);
    assert_eq!(wizard.form().get_text("schematic_dir"), "schematics/main/");
    assert_eq!(wizard.form().get_text("board_dir"), "boards/default/");
    let applied = wizard.apply(&DialogContext::without_project()).unwrap();
    assert_eq!(
        applied,
        Applied::App(AppRequest::OpenProject {
            path: expected.clone(),
            import_messages: Vec::new(),
        })
    );
    let dir = expected.parent_dir().unwrap();
    for file in [
        "My_Project.lpp",
        ".librepcb-project",
        "LICENSE.txt",
        "README.md",
        ".gitignore",
        ".gitattributes",
        "project/metadata.lp",
        "project/settings.lp",
        "schematics/main/schematic.lp",
        "boards/default/board.lp",
        "resources/fontobene/newstroke.bene",
    ] {
        assert!(dir.path_to(file).is_existing_file(), "{file}");
    }
    assert!(!dir.path_to(".lock").is_existing_file(), "lock released");
    let read = |f: &str| std::fs::read_to_string(dir.path_to(f).as_path()).unwrap();
    assert!(read("project/metadata.lp").contains("(name \"My Project\")"));
    assert!(read("project/metadata.lp").contains("(author \"Tester\")"));
    assert!(read("project/settings.lp").contains("(locale \"de_CH\")"));
    assert!(read("README.md").starts_with("# My Project"));
    assert!(read("README.md").contains("See [LICENSE.txt](LICENSE.txt)."));
    assert!(read("LICENSE.txt").contains("CC0"));
    assert!(read("boards/default/board.lp").contains("(layer brd_outlines)"));

    // The directory is not empty now.
    let mut again = NewProjectWizard::new(
        NewProjectMode::NewProject,
        location,
        "",
        Vec::new(),
        Vec::new(),
    );
    again
        .form_mut()
        .edit("name", |f| f.text = "My Project".into());
    again.field_event(
        &DialogContext::without_project(),
        "name",
        librepcb_app::dialogs::FieldEvent::Edited,
    );
    assert!(again.form().get_text("status").contains("not empty"));
    assert!(again.next_page().is_err());
}

#[test]
fn eagle_project_import_wizard() {
    let sch = Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/unittests/eagleimport/testproject/testproject.sch");
    let tmp = tempfile::tempdir().unwrap();
    let mut wizard = NewProjectWizard::new(
        NewProjectMode::EagleImport,
        fp(tmp.path()),
        "Tester",
        Vec::new(),
        Vec::new(),
    );
    assert!(wizard.next_page().is_err(), "no files chosen");
    let ctx = DialogContext::without_project();
    wizard.form_mut().edit("eagle_schematic", |f| {
        f.text = sch.to_string_lossy().as_ref().into()
    });
    wizard.field_event(
        &ctx,
        "eagle_schematic",
        librepcb_app::dialogs::FieldEvent::Edited,
    );
    let messages = wizard.form().get_text("messages");
    assert!(messages.contains("Ready to import"), "{messages}");
    wizard.next_page().unwrap();
    // The project name is taken from the schematic file.
    let stem = sch.file_stem().unwrap().to_string_lossy().into_owned();
    assert_eq!(wizard.form().get_text("name"), stem);
    let options = wizard.project_options().unwrap();
    assert_eq!(options.schematic, None);
    // Import without the EAGLE project (the import itself is tested in
    // librepcb-import).
    let messages = create_project(&options, None).unwrap();
    assert!(messages.is_empty());
    assert!(options.file.is_existing_file());
}

/// Edits a field of the open form dialog like the UI does.
pub fn ui_edit(dialogs: &ui::Dialogs<'_>, id: &str, f: impl FnOnce(&mut ui::FormField)) {
    let fields = dialogs.get_form_fields();
    let row = (0..fields.row_count())
        .find(|i| fields.row_data(*i).is_some_and(|d| d.id == id))
        .unwrap_or_else(|| panic!("field {id}"));
    let mut data = fields.row_data(row).unwrap();
    f(&mut data);
    fields.set_row_data(row, data);
}

/// Copies `.autosave` of a project directory (to simulate a crash: the
/// file system removes it when the project is closed).
fn copy_autosave(dir: &FilePath, to: &Path) {
    let src = dir.path_to(".autosave");
    assert!(src.is_existing_dir(), "autosave written");
    file_utils::copy_dir_recursively(&src, &fp(to)).unwrap();
}

#[test]
fn project_lifecycle_in_app() {
    crate::common::with_headless(W, H, |headless| {
        let save = |name: &str| {
            let pixels = headless.settle(20);
            let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
            write_png(&path, W, H, &pixels).unwrap();
            println!("screenshot: {}", path.display());
        };
        let tmp = tempfile::tempdir().unwrap();
        let workspace = startup::open_workspace(&tmp.path().join("workspace")).unwrap();
        let projects = workspace.projects_path().clone();
        librepcb_i18n::set_language("en").unwrap();
        let window = ui::AppWindow::new().unwrap();
        slint::select_bundled_translation("en").unwrap();
        let app = App::new(window, workspace);
        app.window().show().unwrap();
        let dialogs = app.window().global::<ui::Dialogs>();
        let backend = app.window().global::<ui::Backend>();
        headless.settle(5);

        // New project wizard from the menu.
        backend.invoke_trigger(ui::Action::ProjectNew);
        headless.settle(5);
        assert!(dialogs.get_form_shown());
        ui_edit(&dialogs, "name", |f| f.text = "Lifecycle".into());
        headless.settle(5);
        save("lifecycle_new_project_metadata.png");
        dialogs.invoke_form_button(1); // Next
        headless.settle(5);
        assert_eq!(dialogs.get_form_title(), "Create New Project");
        save("lifecycle_new_project_initialization.png");
        dialogs.invoke_form_button(-1); // Finish
        headless.settle(10);
        assert!(
            !dialogs.get_form_shown(),
            "{} {}",
            dialogs.get_form_title(),
            dialogs.get_form_error()
        );
        let lpp = projects.path_to("Lifecycle/Lifecycle.lpp");
        let dir = lpp.parent_dir().unwrap();
        {
            let s = app.state().borrow();
            assert_eq!(s.projects().len(), 1);
            assert_eq!(*s.projects()[0].path(), lpp);
            assert!(s.projects()[0].is_writable());
        }

        // Autosave after a modification.
        {
            let s = app.state().borrow();
            let project = s.projects()[0].clone();
            project
                .shared()
                .lock()
                .editor
                .execute(AddSchematic {
                    name: ElementName::new("Second").unwrap(),
                    index: None,
                })
                .unwrap();
            assert!(project.autosave().unwrap());
            assert!(!project.autosave().unwrap(), "nothing changed since");
        }
        let backup = tmp.path().join("autosave-copy");
        copy_autosave(&dir, &backup);
        {
            let mut s = app.state().borrow_mut();
            let project = s.projects()[0].clone();
            s.close_project(&project);
        }
        headless.settle(5);
        assert!(!dir.path_to(".autosave").is_existing_dir());
        // "Crash": the autosave backup is still there.
        file_utils::copy_dir_recursively(&fp(&backup), &dir.path_to(".autosave")).unwrap();

        // Reopening asks whether to restore the backup.
        assert_eq!(app.open_project(lpp.as_path()), None);
        headless.settle(5);
        assert!(dialogs.get_form_shown());
        assert_eq!(dialogs.get_form_title(), "Restore autosave backup?");
        save("lifecycle_restore_autosave.png");
        dialogs.invoke_form_button(-1); // Yes
        headless.settle(10);
        {
            let s = app.state().borrow();
            assert_eq!(s.projects().len(), 1);
            let p = s.projects()[0].shared().lock();
            assert_eq!(p.project().schematics().len(), 2, "backup restored");
            assert!(p.file_system.is_restored_from_autosave());
        }
        {
            let mut s = app.state().borrow_mut();
            let project = s.projects()[0].clone();
            s.close_project(&project);
        }
        headless.settle(5);
        // A restored, unsaved backup is kept.
        assert!(dir.path_to(".autosave").is_existing_dir());
        // "No" opens the last saved state (and removes the backup).
        assert_eq!(app.open_project(lpp.as_path()), None);
        headless.settle(5);
        dialogs.invoke_form_button(1); // No
        headless.settle(10);
        {
            let mut s = app.state().borrow_mut();
            let project = s.projects()[0].clone();
            assert_eq!(
                project.shared().lock().project().schematics().len(),
                1,
                "saved state"
            );
            s.close_project(&project);
        }
        headless.settle(5);

        // Locked by another user: the directory lock prompt.
        let lock = dir.path_to(".lock");
        std::fs::write(
            lock.as_path(),
            "Other User\nother\nother-host\n1\nlibrepcb\n2026-01-01T00:00:00Z",
        )
        .unwrap();
        assert_eq!(app.open_project(lpp.as_path()), None);
        headless.settle(5);
        assert_eq!(dialogs.get_form_title(), "Directory is locked");
        save("lifecycle_directory_locked.png");
        // "Open anyway" needs "I accept the risk.".
        dialogs.invoke_form_button(1);
        headless.settle(5);
        assert!(dialogs.get_form_shown());
        assert!(!dialogs.get_form_error().is_empty());
        ui_edit(&dialogs, "accept_risk", |f| f.checked = true);
        headless.settle(2);
        dialogs.invoke_form_button(1);
        headless.settle(10);
        {
            let mut s = app.state().borrow_mut();
            assert_eq!(s.projects().len(), 1);
            let project = s.projects()[0].clone();
            assert!(project.is_writable(), "lock overridden");
            s.close_project(&project);
        }
        headless.settle(5);
        std::fs::write(
            lock.as_path(),
            "Other User\nother\nother-host\n1\nlibrepcb\n2026-01-01T00:00:00Z",
        )
        .unwrap();
        assert_eq!(app.open_project(lpp.as_path()), None);
        headless.settle(5);
        dialogs.invoke_form_button(-1); // Open read-only
        headless.settle(10);
        {
            let mut s = app.state().borrow_mut();
            let project = s.projects()[0].clone();
            assert!(!project.is_writable());
            s.close_project(&project);
        }
        std::fs::remove_file(lock.as_path()).unwrap();

        // File format upgrade: notification with the migration log.
        let v1 = projects.path_to("v1");
        file_utils::copy_dir_recursively(
            &fp(&Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data/projects/v1")),
            &v1,
        )
        .unwrap();
        let index = app.open_project(v1.path_to("test project.lpp").as_path());
        assert_eq!(index, Some(0));
        headless.settle(5);
        let data = app.window().global::<ui::Data>();
        let notifications = data.get_notifications();
        let upgraded = (0..notifications.row_count())
            .filter_map(|i| notifications.row_data(i))
            .find(|n| n.title == "ATTENTION: Project File Format Upgraded")
            .expect("upgrade notification");
        assert!(
            upgraded
                .description
                .contains("migrated to a new file format")
        );
        assert!(data.get_projects().row_data(0).unwrap().unsaved_changes);
        save("lifecycle_upgraded.png");
        // Saving dismisses the notification.
        backend.invoke_trigger_project(0, ui::ProjectAction::Save);
        headless.settle(10);
        let notifications = data.get_notifications();
        assert!(
            (0..notifications.row_count())
                .filter_map(|i| notifications.row_data(i))
                .all(|n| n.title != "ATTENTION: Project File Format Upgraded")
        );
    });
}

#[test]
fn open_project_archive() {
    crate::common::with_headless(W, H, |headless| {
        let tmp = tempfile::tempdir().unwrap();
        // A *.lppz of an upstream test project.
        let src =
            fp(&Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data/projects/Gerber Test"));
        let lppz = fp(&tmp.path().join("Gerber Test.lppz"));
        librepcb_core::fileio::TransactionalFileSystem::open_ro(&src)
            .unwrap()
            .export_to_zip_file(&lppz, None)
            .unwrap();
        let workspace = startup::open_workspace(&tmp.path().join("workspace")).unwrap();
        librepcb_i18n::set_language("en").unwrap();
        let window = ui::AppWindow::new().unwrap();
        slint::select_bundled_translation("en").unwrap();
        let app = App::new(window, workspace);
        app.window().show().unwrap();
        assert_eq!(app.open_project(lppz.as_path()), Some(0));
        headless.settle(10);
        let project = app.state().borrow().projects()[0].clone();
        assert!(!project.is_writable());
        assert_eq!(*project.path(), lppz);
        assert!(!project.shared().lock().project().boards().is_empty());
        // The archive is extracted elsewhere: nothing is written next to it.
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 2);
    });
}

#[test]
fn project_library_updater() {
    crate::common::with_headless(W, H, |headless| {
        let save = |name: &str| {
            let pixels = headless.settle(20);
            let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
            write_png(&path, W, H, &pixels).unwrap();
            println!("screenshot: {}", path.display());
        };
        let tmp = tempfile::tempdir().unwrap();
        let workspace = startup::open_workspace(&tmp.path().join("workspace")).unwrap();
        let lib = workspace
            .local_libraries_path()
            .path_to("Populated Library.lplib");
        file_utils::copy_dir_recursively(
            &fp(&Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
                .join("tests/data/libraries/Populated Library.lplib")),
            &lib,
        )
        .unwrap();
        workspace
            .library_db()
            .rescan(&std::sync::atomic::AtomicBool::new(false), &mut |_| {})
            .unwrap();
        // A project whose component is newer than the workspace library's.
        let dir = workspace.projects_path().path_to("prj");
        std::fs::create_dir_all(dir.as_path()).unwrap();
        let lpp = {
            let (project, _, _) = crate::common::create_project(dir.as_path());
            project.path().clone()
        };
        let cmp = dir.path_to(&format!(
            "library/cmp/{}/component.lp",
            crate::common::RESISTOR
        ));
        let content = std::fs::read_to_string(cmp.as_path()).unwrap();
        std::fs::write(
            cmp.as_path(),
            content.replace("(version \"0.1\")", "(version \"99\")"),
        )
        .unwrap();

        librepcb_i18n::set_language("en").unwrap();
        let window = ui::AppWindow::new().unwrap();
        slint::select_bundled_translation("en").unwrap();
        let app = App::new(window, workspace);
        app.window().show().unwrap();
        let data = app.window().global::<ui::Data>();
        let dialogs = app.window().global::<ui::Dialogs>();
        let backend = app.window().global::<ui::Backend>();
        headless.run_until(std::time::Duration::from_secs(20), || {
            !data.get_libraries_rescan_in_progress()
        });
        assert_eq!(app.open_project(lpp.as_path()), Some(0));
        backend.invoke_trigger_project(0, ui::ProjectAction::OpenLibraryManager);
        headless.settle(10);
        let tab_data = || {
            let section = data.get_sections().row_data(0).unwrap();
            let index = section.current_tab_index as usize;
            section.project_library_tabs.row_data(index).unwrap()
        };
        let t = tab_data();
        assert_eq!(t.project_index, 0);
        assert_eq!(t.downgraded_items, 1);
        let items: Vec<_> = (0..t.items.row_count())
            .filter_map(|i| t.items.row_data(i))
            .collect();
        assert!(items.len() >= 4, "{}", items.len());
        let cmp_item = items
            .iter()
            .find(|i| i.path.ends_with(crate::common::RESISTOR))
            .unwrap();
        assert_eq!(cmp_item.version, "99.0.0");
        assert_eq!(cmp_item.latest_version, "0.1.0");
        assert!(cmp_item.downgrade);
        assert_eq!(cmp_item.library_name, "Populated Library");
        save("lifecycle_project_library.png");

        // "Update All Elements...": the updater dialog.
        let tab = data.get_sections().row_data(0).unwrap().current_tab_index;
        backend.invoke_trigger_tab(0, tab, ui::TabAction::Apply);
        headless.settle(5);
        assert_eq!(dialogs.get_form_title(), "Project Library Updater");
        dialogs.invoke_form_button(1); // Update
        headless.settle(20);
        assert_eq!(dialogs.get_form_title(), "Project Library Updater");
        save("lifecycle_project_library_updated.png");
        let fields = dialogs.get_form_fields();
        let log: Vec<String> = (0..fields.row_count())
            .filter_map(|i| fields.row_data(i))
            .find(|f| f.id == "log")
            .map(|f| {
                (0..f.items.row_count())
                    .filter_map(|i| f.items.row_data(i))
                    .map(|i| i.text.to_string())
                    .collect()
            })
            .unwrap();
        assert!(
            log.iter()
                .any(|l| l == "[SUCCESS] All library elements updated."),
            "{log:?}"
        );
        assert!(log.iter().any(|l| l.starts_with("Update library/cmp/")));
        // The project was reopened with its library tab, now up to date.
        assert_eq!(app.state().borrow().projects().len(), 1);
        let t = tab_data();
        assert_eq!(t.downgraded_items, 0);
        let content = std::fs::read_to_string(cmp.as_path()).unwrap();
        assert!(content.contains("(version \"0.1\")"));
        dialogs.invoke_form_button(-1); // Close
        headless.settle(2);
        assert!(!dialogs.get_form_shown());
    });
}
