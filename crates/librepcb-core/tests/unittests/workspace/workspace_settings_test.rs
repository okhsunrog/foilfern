//! Port of tests/unittests/core/workspace/workspacesettingstest.cpp.
//!
//! `testBaseColorSchemes` is not ported (color schemes are not ported).

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::application::file_format_version;
use librepcb_core::serialization::{Mode, SExpression};
use librepcb_core::types::{GridStyle, LengthUnit};
use librepcb_core::workspace::{ApiEndpointSettings, WorkspaceSettings};

fn parse(s: &str) -> SExpression {
    SExpression::parse(s.as_bytes(), None, Mode::LibrePcb).unwrap()
}

fn to_string(root: &SExpression) -> String {
    String::from_utf8(root.to_byte_array(Mode::LibrePcb).unwrap()).unwrap()
}

fn ep(url: &str, libraries: bool, parts: bool, order: bool) -> ApiEndpointSettings {
    ApiEndpointSettings {
        url: url.to_owned(),
        use_for_libraries: libraries,
        use_for_parts_info: parts,
        use_for_order: order,
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn test_load_from_sexpression() {
    let root = parse(
        "(librepcb_workspace_settings\n\
         \x20(ui_theme \"dark\")\n\
         \x20(user \"Foo Bar\")\n\
         \x20(application_locale \"de_CH\")\n\
         \x20(default_length_unit micrometers)\n\
         \x20(project_autosave_interval 120)\n\
         \x20(use_opengl true)\n\
         \x20(library_locale_order\n\
         \x20 (locale \"de_DE\")\n\
         \x20)\n\
         \x20(library_norm_order\n\
         \x20 (norm \"IEC 60617\")\n\
         \x20)\n\
         \x20(api_endpoints\n\
         \x20 (endpoint \"https://api.librepcb.org\" (libraries true) (parts false) (order true))\n\
         \x20)\n\
         \x20(external_web_browser\n\
         \x20 (command \"firefox \\\"{{URL}}\\\"\")\n\
         \x20)\n\
         \x20(external_file_manager\n\
         \x20 (command \"nautilus \\\"{{FILEPATH}}\\\"\")\n\
         \x20)\n\
         \x20(external_pdf_reader\n\
         \x20 (command \"evince \\\"{{FILEPATH}}\\\"\")\n\
         \x20)\n\
         \x20(schematic_grid_style dots)\n\
         \x20(board_grid_style none)\n\
         \x20(dismissed_messages\n\
         \x20 (message \"SOME_MESSAGE: foo\")\n\
         \x20 (message \"SOME_MESSAGE: bar\")\n\
         \x20)\n\
         )",
    );

    let mut obj = WorkspaceSettings::new();
    obj.load(&root, &file_format_version());
    assert_eq!(obj.ui_theme.get(), "dark");
    assert_eq!(obj.user_name.get(), "Foo Bar");
    assert_eq!(obj.application_locale.get(), "de_CH");
    assert_eq!(*obj.default_length_unit.get(), LengthUnit::Micrometers);
    assert_eq!(*obj.project_autosave_interval_seconds.get(), 120);
    assert!(*obj.use_opengl.get());
    assert_eq!(obj.library_locale_order.get(), &strings(&["de_DE"]));
    assert_eq!(obj.library_norm_order.get(), &strings(&["IEC 60617"]));
    assert_eq!(
        obj.api_endpoints.get(),
        &vec![ep("https://api.librepcb.org", true, false, true)]
    );
    assert_eq!(
        obj.external_web_browser_commands.get(),
        &strings(&["firefox \"{{URL}}\""])
    );
    assert_eq!(
        obj.external_file_manager_commands.get(),
        &strings(&["nautilus \"{{FILEPATH}}\""])
    );
    assert_eq!(
        obj.external_pdf_reader_commands.get(),
        &strings(&["evince \"{{FILEPATH}}\""])
    );
    assert_eq!(*obj.schematic_grid_style.get(), GridStyle::Dots);
    assert_eq!(*obj.board_grid_style.get(), GridStyle::None);
    assert_eq!(
        obj.dismissed_messages.get(),
        &BTreeSet::from([
            "SOME_MESSAGE: foo".to_owned(),
            "SOME_MESSAGE: bar".to_owned()
        ])
    );
}

#[test]
fn test_store_and_load() {
    // Store
    let mut obj1 = WorkspaceSettings::new();
    obj1.ui_theme.set("light".to_owned());
    obj1.user_name.set("foo bar".to_owned());
    obj1.application_locale.set("de_CH".to_owned());
    obj1.default_length_unit.set(LengthUnit::Nanometers);
    obj1.project_autosave_interval_seconds.set(1234);
    let opengl = !*obj1.use_opengl.get();
    obj1.use_opengl.set(opengl);
    obj1.library_locale_order.set(strings(&["de_CH", "en_US"]));
    obj1.library_norm_order.set(strings(&["foo", "bar"]));
    obj1.api_endpoints.set(vec![
        ep("https://foo", true, false, true),
        ep("https://bar", false, true, false),
    ]);
    obj1.external_web_browser_commands
        .set(strings(&["foo", "bar"]));
    obj1.external_file_manager_commands
        .set(strings(&["file", "manager"]));
    obj1.external_pdf_reader_commands
        .set(strings(&["pdf", "reader"]));
    obj1.schematic_grid_style.set(GridStyle::None);
    obj1.board_grid_style.set(GridStyle::Lines);
    obj1.dismissed_messages
        .set(BTreeSet::from(["foo".to_owned(), "bar".to_owned()]));
    let root1 = obj1.serialize();

    // Load
    let mut obj2 = WorkspaceSettings::new();
    obj2.load(&root1, &file_format_version());
    assert_eq!(obj1.ui_theme.get(), obj2.ui_theme.get());
    assert_eq!(obj1.user_name.get(), obj2.user_name.get());
    assert_eq!(obj1.application_locale.get(), obj2.application_locale.get());
    assert_eq!(
        obj1.default_length_unit.get(),
        obj2.default_length_unit.get()
    );
    assert_eq!(
        obj1.project_autosave_interval_seconds.get(),
        obj2.project_autosave_interval_seconds.get()
    );
    assert_eq!(obj1.use_opengl.get(), obj2.use_opengl.get());
    assert_eq!(
        obj1.library_locale_order.get(),
        obj2.library_locale_order.get()
    );
    assert_eq!(obj1.library_norm_order.get(), obj2.library_norm_order.get());
    assert_eq!(obj1.api_endpoints.get(), obj2.api_endpoints.get());
    assert_eq!(
        obj1.external_web_browser_commands.get(),
        obj2.external_web_browser_commands.get()
    );
    assert_eq!(
        obj1.external_file_manager_commands.get(),
        obj2.external_file_manager_commands.get()
    );
    assert_eq!(
        obj1.external_pdf_reader_commands.get(),
        obj2.external_pdf_reader_commands.get()
    );
    assert_eq!(
        obj1.schematic_grid_style.get(),
        obj2.schematic_grid_style.get()
    );
    assert_eq!(obj1.board_grid_style.get(), obj2.board_grid_style.get());
    assert_eq!(obj1.dismissed_messages.get(), obj2.dismissed_messages.get());
    let root2 = obj2.serialize();

    // Check if serialization of loaded settings leads to same file content.
    assert_eq!(to_string(&root1), to_string(&root2));
}

// Verify that serializing does only overwrite modified settings, but keeps
// unknown file entries and does not add new entries for default settings.
#[test]
fn test_save_only_modified_settings() {
    let root = parse(
        "(librepcb_workspace_settings\n\
         \x20(project_autosave_interval 1234)\n\
         \x20(unknown_item \"Foo Bar\")\n\
         \x20(unknown_list\n\
         \x20 (unknown_list_item 42)\n\
         \x20)\n\
         )\n",
    );

    let mut obj = WorkspaceSettings::new();
    obj.load(&root, &file_format_version());
    assert_eq!(*obj.project_autosave_interval_seconds.get(), 1234);
    obj.project_autosave_interval_seconds.set(42);
    let root2 = obj.serialize();

    assert_eq!(
        to_string(&root2),
        "(librepcb_workspace_settings\n\
         \x20(project_autosave_interval 42)\n\
         \x20(unknown_item \"Foo Bar\")\n\
         \x20(unknown_list\n\
         \x20 (unknown_list_item 42)\n\
         \x20)\n\
         )\n"
    );
}

// Saving a default-constructed object shall create a file without entries.
#[test]
fn test_default_serialize_empty() {
    let mut obj = WorkspaceSettings::new();
    assert_eq!(
        to_string(&obj.serialize()),
        "(librepcb_workspace_settings\n)\n"
    );
}

// Restoring all default values also removes unknown entries.
#[test]
fn test_restore_defaults_clears_file() {
    let root = parse(
        "(librepcb_workspace_settings\n\
         \x20(project_autosave_interval 1234)\n\
         \x20(unknown_value \"Foo Bar\")\n\
         \x20(unknown_list\n\
         \x20 (unknown_list_item 42)\n\
         \x20)\n\
         )\n",
    );

    let mut obj = WorkspaceSettings::new();
    obj.load(&root, &file_format_version());
    obj.restore_defaults();
    assert_eq!(
        to_string(&obj.serialize()),
        "(librepcb_workspace_settings\n)\n"
    );
}

// Unknown (obsolete) settings are removed when upgrading the file format.
#[test]
fn test_upgrade_file_format() {
    let root = parse(
        "(librepcb_workspace_settings\n\
         \x20(dismissed_messages\n\
         \x20 (message \"SOME_MESSAGE: foo\")\n\
         \x20 (message \"SOME_MESSAGE: bar\")\n\
         \x20)\n\
         \x20(external_pdf_reader\n\
         \x20 (command \"evince \\\"{{FILEPATH}}\\\"\")\n\
         \x20)\n\
         \x20(keyboard_shortcuts\n\
         \x20 (shortcut file_manager \"F1\")\n\
         \x20 (shortcut foo_bar \"F2\")\n\
         \x20)\n\
         \x20(project_autosave_interval 1234)\n\
         \x20(unknown_item \"Foo Bar\")\n\
         \x20(unknown_list\n\
         \x20 (unknown_list_item 42)\n\
         \x20)\n\
         )\n",
    );

    let mut obj = WorkspaceSettings::new();
    obj.load(&root, &"0.1".parse().unwrap());
    assert_eq!(*obj.project_autosave_interval_seconds.get(), 1234);
    obj.project_autosave_interval_seconds.set(42);
    assert_eq!(
        to_string(&obj.serialize()),
        "(librepcb_workspace_settings\n\
         \x20(dismissed_messages\n\
         \x20 (message \"SOME_MESSAGE: bar\")\n\
         \x20 (message \"SOME_MESSAGE: foo\")\n\
         \x20)\n\
         \x20(external_pdf_reader\n\
         \x20 (command \"evince \\\"{{FILEPATH}}\\\"\")\n\
         \x20)\n\
         \x20(keyboard_shortcuts\n\
         \x20 (shortcut file_manager \"F1\")\n\
         \x20 (shortcut foo_bar \"F2\")\n\
         \x20)\n\
         \x20(project_autosave_interval 42)\n\
         )\n"
    );
}

// Tests below are not ported from upstream.

#[test]
fn test_keyboard_shortcuts() {
    let root = parse(
        "(librepcb_workspace_settings\n\
         \x20(keyboard_shortcuts\n\
         \x20 (shortcut zoom_in \"Ctrl++\" \"+\")\n\
         \x20 (shortcut file_manager \"F1\")\n\
         \x20)\n\
         )\n",
    );
    let mut obj = WorkspaceSettings::new();
    obj.load(&root, &file_format_version());
    let overrides = obj.keyboard_shortcuts.get().overrides().clone();
    assert_eq!(overrides["zoom_in"], strings(&["Ctrl++", "+"]));
    let mut new = overrides.clone();
    new.insert("save".to_owned(), strings(&[]));
    new.remove("file_manager");
    let value = obj.keyboard_shortcuts.get().with_overrides(new);
    assert!(obj.keyboard_shortcuts.set(value));
    assert_eq!(
        to_string(&obj.serialize()),
        "(librepcb_workspace_settings\n\
         \x20(keyboard_shortcuts\n\
         \x20 (shortcut save)\n\
         \x20 (shortcut zoom_in \"Ctrl++\" \"+\")\n\
         \x20)\n\
         )\n"
    );
    obj.keyboard_shortcuts
        .set(obj.keyboard_shortcuts.get().with_overrides(BTreeMap::new()));
    obj.keyboard_shortcuts.restore_default();
    assert_eq!(
        to_string(&obj.serialize()),
        "(librepcb_workspace_settings\n)\n"
    );
}

#[test]
fn test_raw_color_schemes_and_legacy_themes() {
    let content = "(librepcb_workspace_settings\n\
         \x20(board_color_schemes\n\
         \x20 (active b8c0b6e6-6cb1-4c02-9ac7-ae6f1a7e24a6 \"Foo\")\n\
         \x20 (scheme b8c0b6e6-6cb1-4c02-9ac7-ae6f1a7e24a6 (name \"Foo\"))\n\
         \x20)\n\
         \x20(themes\n\
         \x20 (active 5c6f2a4f-8c3d-4b5a-9e6f-1a2b3c4d5e6f)\n\
         \x20 (theme 5c6f2a4f-8c3d-4b5a-9e6f-1a2b3c4d5e6f \"Theme\"\n\
         \x20  (schematic_grid_style dots) (board_grid_style none)\n\
         \x20 )\n\
         \x20)\n\
         )\n";
    let mut obj = WorkspaceSettings::new();
    obj.load(&parse(content), &file_format_version());
    assert_eq!(*obj.schematic_grid_style.get(), GridStyle::Dots);
    assert_eq!(*obj.board_grid_style.get(), GridStyle::None);
    assert!(!obj.is_edited());
    // Nothing edited: the file is written back unchanged.
    assert_eq!(to_string(&obj.serialize()), content);

    // On upgrade, the raw color schemes are written back as they were, and
    // the migrated grid styles are written (like upstream), but the legacy
    // themes are removed.
    let mut obj = WorkspaceSettings::new();
    obj.load(&parse(content), &"1".parse().unwrap());
    assert_eq!(
        to_string(&obj.serialize()),
        "(librepcb_workspace_settings\n\
         \x20(board_color_schemes\n\
         \x20 (active b8c0b6e6-6cb1-4c02-9ac7-ae6f1a7e24a6 \"Foo\")\n\
         \x20 (scheme b8c0b6e6-6cb1-4c02-9ac7-ae6f1a7e24a6 (name \"Foo\"))\n\
         \x20)\n\
         \x20(board_grid_style none)\n\
         \x20(schematic_grid_style dots)\n\
         )\n"
    );
}

/// The colors of legacy themes become user color schemes (upstream
/// `WorkspaceSettings::load()` with `UserColorScheme::serialize()`), for
/// each kind that has no color scheme settings yet.
#[test]
fn test_legacy_theme_colors_migration() {
    let content = "(librepcb_workspace_settings\n\
         \x20(board_color_schemes\n\
         \x20 (active c605f278-5210-472e-a59f-1b73a15aee2d \"LibrePCB Dark\")\n\
         \x20)\n\
         \x20(themes\n\
         \x20 (active 5c6f2a4f-8c3d-4b5a-9e6f-1a2b3c4d5e6f)\n\
         \x20 (theme 11111111-8c3d-4b5a-9e6f-1a2b3c4d5e6f \"Other\"\n\
         \x20  (colors (schematic_wires (primary \"#ff00ff00\")))\n\
         \x20 )\n\
         \x20 (theme 5c6f2a4f-8c3d-4b5a-9e6f-1a2b3c4d5e6f \"My Theme\"\n\
         \x20  (colors\n\
         \x20   (schematic_background (primary \"#ff112233\") (secondary \"#80445566\"))\n\
         \x20   (board_copper_top (primary \"#ffaabbcc\"))\n\
         \x20   (schematic_frames (secondary \"#ff000000\"))\n\
         \x20  )\n\
         \x20 )\n\
         \x20)\n\
         )\n";
    let mut obj = WorkspaceSettings::new();
    obj.load(&parse(content), &"1".parse().unwrap());
    assert!(!obj.is_edited());
    let children = &obj.schematic_color_schemes.get().0;
    let schemes: Vec<_> = children
        .iter()
        .filter(|c| c.name().is_ok_and(|n| n == "scheme"))
        .collect();
    assert_eq!(schemes.len(), 2);
    let uuid_of = |name: &str| -> String {
        schemes
            .iter()
            .find(|s| s.child("@1").unwrap().value().unwrap() == name)
            .unwrap()
            .child("@0")
            .unwrap()
            .value()
            .unwrap()
            .to_owned()
    };
    let (mine, other) = (uuid_of("My Theme"), uuid_of("Other"));
    let scheme_mine = format!(
        "  (scheme {mine} \"My Theme\"\n\
         \x20  (base 9121eabe-55b3-4a7c-bffe-20115b8ad314 \"LibrePCB Light\")\n\
         \x20  (color schematic_background (primary \"#ff112233\") (secondary \"#80445566\"))\n\
         \x20  (color schematic_frames (secondary \"#ff000000\"))\n\
         \x20 )\n"
    );
    let scheme_other = format!(
        "  (scheme {other} \"Other\"\n\
         \x20  (base 9121eabe-55b3-4a7c-bffe-20115b8ad314 \"LibrePCB Light\")\n\
         \x20  (color schematic_wires (primary \"#ff00ff00\"))\n\
         \x20 )\n"
    );
    let (first, second) = if mine < other {
        (scheme_mine, scheme_other)
    } else {
        (scheme_other, scheme_mine)
    };
    // The board color schemes exist already, the themes have no 3D colors:
    // both are not migrated. The legacy themes are removed on upgrade.
    assert_eq!(
        to_string(&obj.serialize()),
        format!(
            "(librepcb_workspace_settings\n\
             \x20(board_color_schemes\n\
             \x20 (active c605f278-5210-472e-a59f-1b73a15aee2d \"LibrePCB Dark\")\n\
             \x20)\n\
             \x20(schematic_color_schemes\n\
             \x20 (active {mine} \"My Theme\")\n\
             {first}{second}\
             \x20)\n\
             )\n"
        )
    );
}

#[test]
fn test_api_endpoint_defaults() {
    let root = parse(
        "(librepcb_workspace_settings\n\
         \x20(api_endpoints\n\
         \x20 (endpoint \"https://api.librepcb.org\")\n\
         \x20 (endpoint \"https://example.com\")\n\
         \x20)\n\
         )\n",
    );
    let mut obj = WorkspaceSettings::new();
    obj.load(&root, &file_format_version());
    assert_eq!(
        obj.api_endpoints.get(),
        &vec![
            ep("https://api.librepcb.org", true, true, true),
            ep("https://example.com", true, false, false),
        ]
    );
    assert_eq!(
        obj.api_endpoint_for_order().unwrap().url,
        "https://api.librepcb.org"
    );
    assert_eq!(obj.api_endpoints_for_libraries().count(), 2);
}
