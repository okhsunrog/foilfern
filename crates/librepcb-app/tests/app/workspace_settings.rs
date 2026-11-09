//! The workspace settings dialog (M3c): all pages, keyboard shortcut
//! overrides, color schemes with the color scheme editor (applied to the
//! scenes) and saving the settings. Screenshots `settings_*.png` are
//! written to `$CARGO_TARGET_TMPDIR`.

use std::path::PathBuf;

use librepcb_app::color_schemes::SchemeKind;
use librepcb_app::screenshot::write_png;
use librepcb_app::{App, startup, ui};
use slint::{ComponentHandle, Model};

use crate::lifecycle::ui_edit;

const W: u32 = 1400;
const H: u32 = 900;

#[test]
fn workspace_settings_dialog() {
    crate::common::with_headless(W, H, |headless| {
        let save = |name: &str| {
            let pixels = headless.settle(20);
            let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
            write_png(&path, W, H, &pixels).unwrap();
            println!("screenshot: {}", path.display());
        };
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("workspace");
        let workspace = startup::open_workspace(&ws_dir).unwrap();
        let settings_file = workspace.data_path().path_to("settings.lp");
        librepcb_i18n::set_language("en").unwrap();
        let window = ui::AppWindow::new().unwrap();
        slint::select_bundled_translation("en").unwrap();
        let app = App::new(window, workspace);
        app.window().show().unwrap();
        let dialogs = app.window().global::<ui::Dialogs>();
        let backend = app.window().global::<ui::Backend>();
        let editor = app.window().global::<ui::ColorSchemeEditor>();
        headless.settle(5);
        // A project with a schematic tab (to see the color scheme).
        let dir = tmp.path().join("project");
        std::fs::create_dir_all(&dir).unwrap();
        let lpp = {
            let (project, _, _) = crate::common::create_project(&dir);
            project.path().clone()
        };
        assert_eq!(app.open_project(lpp.as_path()), Some(0));
        headless.settle(5);

        backend.invoke_trigger(ui::Action::WorkspaceSettings);
        headless.settle(5);
        assert!(dialogs.get_form_shown());
        assert_eq!(dialogs.get_form_title(), "Workspace Settings");
        let pages = dialogs.get_form_pages();
        assert_eq!(pages.row_count(), 6);
        for (i, name) in [
            "general",
            "appearance",
            "library",
            "external",
            "shortcuts",
            "internet",
        ]
        .iter()
        .enumerate()
        {
            app.state().borrow_mut().show_form_dialog_page(i as i32);
            save(&format!("settings_{name}.png"));
        }

        // General.
        app.state().borrow_mut().show_form_dialog_page(0);
        ui_edit(&dialogs, "user_name", |f| f.text = "Jane Doe".into());
        ui_edit(&dialogs, "autosave", |f| f.text = "120".into());
        headless.settle(2);

        // Keyboard shortcuts: override "save".
        app.state().borrow_mut().show_form_dialog_page(4);
        ui_edit(&dialogs, "shortcut_filter", |f| f.text = "save".into());
        headless.settle(2);
        let rows = |id: &str| {
            let fields = dialogs.get_form_fields();
            (0..fields.row_count())
                .filter_map(|i| fields.row_data(i))
                .find(|f| f.id == id)
                .map(|f| {
                    (0..f.items.row_count())
                        .filter_map(|i| f.items.row_data(i))
                        .map(|i| i.cells.iter().map(|c| c.to_string()).collect::<Vec<_>>())
                        .collect::<Vec<_>>()
                })
                .unwrap()
        };
        let list = rows("shortcuts");
        let row = list.iter().position(|r| r[0] == "Save").expect("save");
        assert_eq!(list[row][1], "Ctrl+S");
        ui_edit(&dialogs, "shortcuts", |f| {
            f.action = ui::FormFieldAction::Select;
            f.action_row = row as i32;
        });
        headless.settle(2);
        ui_edit(&dialogs, "shortcut_edit", |f| {
            f.text = "Ctrl+Shift+S".into()
        });
        headless.settle(2);
        let list = rows("shortcuts");
        assert_eq!(list[row][1], "Ctrl+Shift+S");
        save("settings_shortcuts_edited.png");

        // Appearance: duplicate the schematic color scheme and edit a color.
        app.state().borrow_mut().show_form_dialog_page(1);
        ui_edit(&dialogs, "sch_duplicate", |f| {
            f.action = ui::FormFieldAction::Clicked;
        });
        headless.settle(5);
        assert!(editor.get_shown());
        assert_eq!(editor.get_name(), "Copy of LibrePCB Light");
        let (kind, _) = app.state().borrow().color_scheme_editor().unwrap();
        assert_eq!(kind, SchemeKind::Schematic);
        let model = editor.get_model();
        assert_eq!(model.row_data(0).unwrap().name, "Background/Grid");
        let mut bg = model.row_data(0).unwrap();
        bg.primary.hex = "#102030".into();
        bg.primary.action = ui::ColorSchemeColorAction::SetHex;
        model.set_row_data(0, bg);
        headless.settle(5);
        let bg = editor.get_model().row_data(0).unwrap();
        assert_eq!(bg.primary.hex, "#102030");
        assert!(bg.primary.overridden);
        editor.set_name("My Colors".into());
        save("settings_color_scheme.png");
        editor.invoke_close_requested();
        headless.settle(5);
        assert!(!editor.get_shown());
        save("settings_appearance_user_scheme.png");

        // OK saves and applies the settings.
        dialogs.invoke_form_button(-1);
        headless.settle(10);
        assert!(!dialogs.get_form_shown(), "{}", dialogs.get_form_error());
        {
            let s = app.state().borrow();
            let ws = s.workspace().lock();
            let settings = ws.settings();
            assert_eq!(settings.user_name.get(), "Jane Doe");
            assert_eq!(*settings.project_autosave_interval_seconds.get(), 120);
            assert_eq!(
                settings.keyboard_shortcuts.get().overrides().get("save"),
                Some(&vec!["Ctrl+Shift+S".to_owned()])
            );
        }
        let content = std::fs::read_to_string(settings_file.as_path()).unwrap();
        assert!(content.contains("(user \"Jane Doe\")"), "{content}");
        assert!(
            content.contains("(project_autosave_interval 120)"),
            "{content}"
        );
        assert!(content.contains("\"My Colors\""), "{content}");
        assert!(content.contains("(primary \"#ff102030\")"), "{content}");
        assert!(content.contains("Ctrl+Shift+S"), "{content}");
        // The schematic tab uses the new color scheme.
        app.show_tab(librepcb_app::InitialTab::Schematic);
        let section = app
            .window()
            .global::<ui::Data>()
            .get_sections()
            .row_data(0)
            .unwrap();
        let tab = section.current_tab_index as usize;
        let sch = section.schematic_tabs.row_data(tab).unwrap();
        assert_eq!(
            sch.background_color.color(),
            slint::Color::from_rgb_u8(0x10, 0x20, 0x30)
        );
        save("settings_applied_schematic.png");
        // The overrides are global: do not affect other tests.
        librepcb_app::shortcuts::set_overrides(&Default::default());
    });
}
