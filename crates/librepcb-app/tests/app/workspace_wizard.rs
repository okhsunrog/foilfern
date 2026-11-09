//! The initialize workspace wizard (M3c): welcome, choose workspace,
//! upgrade and choose settings pages, shown before the application exists
//! (`modal_dialog`). Screenshots `workspace_wizard_*.png` are written to
//! `$CARGO_TARGET_TMPDIR`.

use std::path::PathBuf;

use librepcb_app::dialogs::initialize_workspace::{InitializeWorkspaceWizard, Page};
use librepcb_app::dialogs::{AppRequest, DialogContext, FormDialog};
use librepcb_app::modal_dialog::{ModalDialog, ModalResult};
use librepcb_app::screenshot::write_png;
use librepcb_app::ui;
use librepcb_core::fileio::FilePath;
use librepcb_core::workspace::Workspace;
use slint::ComponentHandle;

use crate::lifecycle::ui_edit;

const W: u32 = 1000;
const H: u32 = 700;

#[test]
fn new_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let path = FilePath::new(tmp.path().join("ws")).unwrap();
    // No workspace chosen yet: welcome page.
    let mut wizard = InitializeWorkspaceWizard::new(None, false);
    assert_eq!(wizard.page(), Page::Welcome);
    wizard.next_page().unwrap();
    assert_eq!(wizard.page(), Page::ChooseWorkspace);
    wizard
        .form_mut()
        .edit("path", |f| f.text = path.to_native().into());
    wizard.field_event(
        &DialogContext::without_project(),
        "path",
        librepcb_app::dialogs::FieldEvent::Edited,
    );
    assert!(
        wizard
            .form()
            .get_text("status")
            .contains("New workspace will be created.")
    );
    wizard.next_page().unwrap();
    assert_eq!(wizard.page(), Page::ChooseSettings);
    wizard
        .form_mut()
        .edit("user_name", |f| f.text = "Jane".into());
    wizard.form_mut().edit("norm", |f| f.index = 1);
    let chosen = wizard.finish().unwrap();
    assert_eq!(chosen, path);
    // The workspace is ready now: no wizard needed.
    let again = InitializeWorkspaceWizard::new(Some(path.clone()), false);
    assert!(!again.needs_to_be_shown());
    let ws = Workspace::open(&path, &again.status().data_dir, None).unwrap();
    assert_eq!(ws.settings().user_name.get(), "Jane");
    assert_eq!(
        ws.settings().library_norm_order.get(),
        &vec!["IEC 60617".to_owned()]
    );
    drop(ws);

    // A non-empty directory is not accepted.
    let other = tmp.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("file.txt"), "x").unwrap();
    let wizard = InitializeWorkspaceWizard::new(FilePath::new(&other), false);
    assert_eq!(wizard.page(), Page::ChooseWorkspace);
    assert!(wizard.form().get_text("status").contains("not empty"));
}

#[test]
fn upgrade_data_directory() {
    // A workspace with an old data directory ("v0.1") only: the data is
    // imported into the new data directory by copying.
    let tmp = tempfile::tempdir().unwrap();
    let path = FilePath::new(tmp.path().join("ws")).unwrap();
    Workspace::create(&path).unwrap();
    let old = path.path_to("v0.1");
    std::fs::create_dir_all(old.path_to("libraries").as_path()).unwrap();
    std::fs::write(
        old.path_to("settings.lp").as_path(),
        "(librepcb_workspace_settings\n)\n",
    )
    .unwrap();
    let mut wizard = InitializeWorkspaceWizard::new(Some(path.clone()), false);
    assert_eq!(wizard.page(), Page::Upgrade);
    let (src, dst) = wizard.status().upgrade.clone().unwrap();
    assert_eq!(src, "v0.1");
    let ok = wizard.options().ok_text.unwrap();
    assert_eq!(ok, "Upgrade");
    // "Switch Workspace" is offered.
    assert!(
        wizard
            .options()
            .extra_buttons
            .contains(&"Switch Workspace".to_owned())
    );
    wizard.finish().unwrap();
    assert!(path.path_to(&dst).path_to("settings.lp").is_existing_file());
}

#[test]
fn wizard_before_the_application() {
    crate::common::with_headless(W, H, |headless| {
        let save = |name: &str| {
            let pixels = headless.settle(20);
            let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
            write_png(&path, W, H, &pixels).unwrap();
            println!("screenshot: {}", path.display());
        };
        librepcb_i18n::set_language("en").unwrap();
        let window = ui::AppWindow::new().unwrap();
        slint::select_bundled_translation("en").unwrap();
        window.show().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let path = FilePath::new(tmp.path().join("ws")).unwrap();
        let wizard = InitializeWorkspaceWizard::new(None, false);
        let finished = std::rc::Rc::new(std::cell::Cell::new(false));
        let f = finished.clone();
        let dialog = ModalDialog::show(&window, Box::new(wizard), move || f.set(true));
        let dialogs = window.global::<ui::Dialogs>();
        headless.settle(5);
        assert_eq!(dialogs.get_form_title(), "LibrePCB Workspace Setup");
        save("workspace_wizard_welcome.png");
        dialogs.invoke_form_button(1); // Next
        headless.settle(5);
        ui_edit(&dialogs, "path", |f| f.text = path.to_native().into());
        headless.settle(5);
        save("workspace_wizard_choose.png");
        dialogs.invoke_form_button(2); // Next (after "Back")
        headless.settle(5);
        save("workspace_wizard_settings.png");
        dialogs.invoke_form_button(-1); // Finish
        headless.settle(5);
        assert!(finished.get());
        assert_eq!(
            dialog.result(),
            ModalResult::Accepted(Some(AppRequest::WorkspaceChosen(path.clone())))
        );
        assert!(Workspace::check_compatibility(&path).is_ok());
    });
}
