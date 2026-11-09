//! The workspace settings dialog.
//!
//! Port of libs/librepcb/editor/workspace/workspacesettingsdialog.{h,cpp,ui}
//! with the pages (tabs) of upstream: general (language, length unit, user
//! name, autosave interval, rendering method, dismissed messages),
//! appearance (theme, color schemes and grid styles of schematics and
//! boards), library (locale and norm order), external applications,
//! keyboard shortcuts and internet access (API endpoints, automatic library
//! updates, live part information).
//!
//! Differences to upstream (see COMPAT.md): the settings are applied with
//! "OK"/"Apply" only (upstream applies the theme, language, grid styles and
//! color schemes immediately and reverts them on "Cancel"); the keyboard
//! shortcuts are edited as text (Qt portable key sequences, one per line)
//! instead of recording key presses; the desktop integration and the 3D
//! view color schemes are not available.

use std::collections::BTreeMap;

use librepcb_core::types::{AutoUpdateMode, GridStyle, LengthUnit, Uuid};
use librepcb_core::workspace::{ApiEndpointSettings, RawSettings, WorkspaceSettings};
use librepcb_i18n::{tr, trn};

use super::{
    AppRequest, Applied, ButtonResult, DialogContext, DialogKind, DialogOptions, FieldEvent, Form,
    FormDialog, ListAction, ListButtons, ListItem,
};
use crate::color_schemes::{ColorSchemes, SchemeKind};
use crate::shortcuts;
use crate::theme::UiTheme;

const CTX: &str = "librepcb::editor::WorkspaceSettingsDialog";

/// The external applications of upstream's list: (name, example
/// executable, default argument).
const EXTERNAL_APPS: [(&str, &str, &str); 3] = [
    ("Web Browser", "firefox", "\"{{URL}}\""),
    ("File Manager", "explorer", "\"{{FILEPATH}}\""),
    ("PDF Reader", "evince", "\"{{FILEPATH}}\""),
];

const GRID_STYLES: [GridStyle; 3] = [GridStyle::None, GridStyle::Dots, GridStyle::Lines];

const UPDATE_MODES: [AutoUpdateMode; 4] = [
    AutoUpdateMode::Disabled,
    AutoUpdateMode::Check,
    AutoUpdateMode::Notify,
    AutoUpdateMode::Install,
];

/// The workspace settings dialog (upstream `WorkspaceSettingsDialog`).
pub struct WorkspaceSettingsDialog {
    form: Form,
    settings: WorkspaceSettings,
    schematic_schemes: ColorSchemes,
    board_schemes: ColorSchemes,
    locales: Vec<String>,
    external_app: usize,
    external_commands: [Vec<String>; 3],
    shortcut_overrides: BTreeMap<String, Vec<String>>,
    shortcut_rows: Vec<usize>,
    shortcut_current: Option<usize>,
    api_current: Option<usize>,
    restore_defaults_asked: bool,
    request: Option<AppRequest>,
}

fn grid_style_names() -> Vec<String> {
    vec![tr!(CTX, "None"), tr!(CTX, "Dots"), tr!(CTX, "Lines")]
}

impl WorkspaceSettingsDialog {
    /// Creates the dialog for (a copy of) the workspace settings.
    pub fn new(settings: &WorkspaceSettings) -> Self {
        let mut locales: Vec<String> = librepcb_i18n::available_languages()
            .iter()
            .map(|l| (*l).to_owned())
            .collect();
        locales.sort();
        let external_commands = [
            settings.external_web_browser_commands.get().clone(),
            settings.external_file_manager_commands.get().clone(),
            settings.external_pdf_reader_commands.get().clone(),
        ];
        let mut dialog = Self {
            form: Form::new(LengthUnit::Millimeters),
            schematic_schemes: ColorSchemes::load(
                SchemeKind::Schematic,
                settings.schematic_color_schemes.get(),
            ),
            board_schemes: ColorSchemes::load(
                SchemeKind::Board,
                settings.board_color_schemes.get(),
            ),
            shortcut_overrides: settings.keyboard_shortcuts.get().overrides().clone(),
            settings: settings.clone(),
            locales,
            external_app: 0,
            external_commands,
            shortcut_rows: Vec::new(),
            shortcut_current: None,
            api_current: None,
            restore_defaults_asked: false,
            request: None,
        };
        dialog.build();
        dialog
    }

    /// The color schemes of a kind (edited in the color scheme dialog).
    pub fn color_schemes_mut(&mut self, kind: SchemeKind) -> &mut ColorSchemes {
        match kind {
            SchemeKind::Schematic => &mut self.schematic_schemes,
            SchemeKind::Board => &mut self.board_schemes,
        }
    }

    /// The color schemes of a kind.
    pub fn color_schemes(&self, kind: SchemeKind) -> &ColorSchemes {
        match kind {
            SchemeKind::Schematic => &self.schematic_schemes,
            SchemeKind::Board => &self.board_schemes,
        }
    }

    /// The keyboard shortcut overrides being edited.
    pub fn shortcut_overrides(&self) -> &BTreeMap<String, Vec<String>> {
        &self.shortcut_overrides
    }

    fn build(&mut self) {
        let s = self.settings.clone();
        self.form.clear();

        // General.
        self.form.page(tr!(CTX, "General"));
        let mut languages = vec![tr!(CTX, "System Language")];
        languages.extend(self.locales.iter().cloned());
        let locale = s.application_locale.get();
        let locale_index = self
            .locales
            .iter()
            .position(|l| l == locale)
            .map_or(0, |i| i + 1);
        let units: Vec<String> = LengthUnit::ALL.iter().map(|u| u.to_string_tr()).collect();
        let unit_index = LengthUnit::ALL
            .iter()
            .position(|u| u == s.default_length_unit.get());
        self.form
            .choice(
                "app_locale",
                tr!(CTX, "Language:"),
                &languages,
                Some(locale_index),
            )
            .choice("length_unit", tr!(CTX, "Length Unit:"), &units, unit_index)
            .text("user_name", tr!(CTX, "User Name:"), s.user_name.get())
            .note(
                "user_name_hint",
                tr!(
                    CTX,
                    "This name will be used as author when creating new projects or libraries."
                ),
            )
            .text(
                "autosave",
                tr!(CTX, "Autosave Interval:"),
                s.project_autosave_interval_seconds.get().to_string(),
            )
            .checkbox(
                "use_opengl",
                tr!(CTX, "Rendering Method:"),
                tr!(CTX, "Use OpenGL Hardware Acceleration"),
                *s.use_opengl.get(),
            )
            .button("reset_dismissed", tr!(CTX, "Dismissed Messages:"), "");
        self.form
            .set_hint("user_name", tr!(CTX, "e.g. \"John Doe\""));
        self.form
            .set_hint("autosave", tr!(CTX, "Seconds (0 = disable autosave)"));
        self.update_dismissed_messages();

        // Appearance.
        self.form.page(tr!(CTX, "Appearance"));
        let mut themes = vec![tr!(CTX, "System Theme")];
        themes.extend(UiTheme::ALL.iter().map(UiTheme::name_tr));
        let theme_index = UiTheme::ALL
            .iter()
            .position(|t| t.id == s.ui_theme.get())
            .map_or(0, |i| i + 1);
        self.form
            .choice("ui_theme", tr!(CTX, "Theme:"), &themes, Some(theme_index));
        for (prefix, label, grid) in [
            (
                "sch",
                tr!(CTX, "Schematics:"),
                *s.schematic_grid_style.get(),
            ),
            ("brd", tr!(CTX, "Boards:"), *s.board_grid_style.get()),
        ] {
            self.form
                .header(label.trim_end_matches(':'))
                .choice(&format!("{prefix}_scheme"), label, &[], None)
                .button(
                    &format!("{prefix}_modify"),
                    "",
                    tr!(CTX, "Modify (only for user-defined color schemes)"),
                )
                .button(&format!("{prefix}_duplicate"), "", tr!(CTX, "Duplicate"))
                .button(
                    &format!("{prefix}_remove"),
                    "",
                    tr!(CTX, "Remove (only for user-defined color schemes)"),
                )
                .choice(
                    &format!("{prefix}_grid"),
                    tr!(CTX, "Grid:"),
                    &grid_style_names(),
                    GRID_STYLES.iter().position(|g| *g == grid),
                );
        }
        self.update_color_schemes();

        // Library.
        self.form.page(tr!(CTX, "Library"));
        let order_buttons = ListButtons {
            add: true,
            remove: true,
            move_: true,
            duplicate: false,
        };
        let locales: Vec<ListItem> = s
            .library_locale_order
            .get()
            .iter()
            .map(ListItem::text)
            .collect();
        let norms: Vec<ListItem> = s
            .library_norm_order
            .get()
            .iter()
            .map(ListItem::text)
            .collect();
        self.form
            .list(
                "locale_order",
                tr!(CTX, "Preferred Languages:\n(Highest priority at top)"),
                &[],
                &locales,
                5,
                order_buttons,
            )
            .list(
                "norm_order",
                tr!(CTX, "Preferred Norms:\n(Highest priority at top)"),
                &[],
                &norms,
                5,
                order_buttons,
            );
        self.form
            .set_hint("locale_order", tr!(CTX, "Click here to add a locale"));
        self.form
            .set_hint("norm_order", tr!(CTX, "Click here to add a norm"));

        // External applications.
        self.form.page(tr!(CTX, "External Applications"));
        let apps: Vec<ListItem> = EXTERNAL_APPS
            .iter()
            .map(|(name, _, _)| ListItem::text(tr!(CTX, name)))
            .collect();
        self.form
            .list(
                "external_apps",
                "",
                &[],
                &apps,
                3,
                ListButtons::default(),
            )
            .multiline(
                "external_commands",
                tr!(CTX, "Custom command(s):"),
                "",
                4,
            )
            .note("external_placeholders", "")
            .note(
                "external_note",
                tr!(
                    CTX,
                    "You can add multiple commands to make the same settings working on multiple computers. LibrePCB will iterate through the list of commands until one of them succeeds. If none succeeds, the system's default application will be used."
                ),
            );
        self.update_external_app();

        // Keyboard shortcuts.
        self.form.page(tr!(CTX, "Keyboard Shortcuts"));
        self.form
            .text("shortcut_filter", "", "")
            .list(
                "shortcuts",
                "",
                &[tr!(CTX, "Command"), tr!(CTX, "Shortcuts")],
                &[],
                9,
                ListButtons::default(),
            )
            .multiline("shortcut_edit", tr!(CTX, "Shortcuts:"), "", 2)
            .button(
                "shortcut_default",
                "",
                tr!(
                    "librepcb::editor::KeySequencesEditorWidget",
                    "Restore default shortcut(s)"
                ),
            )
            .button("shortcut_none", "", tr!(CTX, "No Shortcut"));
        self.form
            .set_hint("shortcut_filter", tr!(CTX, "Type to filter..."));
        self.form.set_hint(
            "shortcut_edit",
            tr!(CTX, "One key sequence per line, e.g. \"Ctrl+Shift+S\""),
        );
        self.update_shortcuts();

        // Internet access.
        self.form.page(tr!(CTX, "Internet Access"));
        self.form
            .note(
                "api_info",
                tr!(
                    CTX,
                    "API servers provide online libraries, live part information and the PCB ordering service."
                ),
            )
            .list(
                "api_endpoints",
                tr!(CTX, "API Servers:"),
                &[
                    tr!("librepcb::editor::ApiEndpointListModelLegacy", "URL"),
                    tr!("librepcb::editor::ApiEndpointListModelLegacy", "Libraries"),
                    tr!("librepcb::editor::ApiEndpointListModelLegacy", "Parts Info"),
                    tr!("librepcb::editor::ApiEndpointListModelLegacy", "Order PCB"),
                ],
                &[],
                5,
                order_buttons,
            )
            .checkbox("api_libraries", "", tr!(CTX, "Use for libraries"), false)
            .checkbox("api_parts", "", tr!(CTX, "Use for parts information"), false)
            .checkbox("api_order", "", tr!(CTX, "Use for PCB ordering"), false);
        let modes = vec![
            tr!(CTX, "Disabled"),
            tr!(CTX, "Check (Silent)"),
            tr!(CTX, "Check & Notify"),
            tr!(CTX, "Check & Install"),
        ];
        self.form
            .choice(
                "auto_update",
                tr!(CTX, "Automatic Library Updates:"),
                &modes,
                UPDATE_MODES
                    .iter()
                    .position(|m| m == s.libraries_auto_update_mode.get()),
            )
            .checkbox(
                "autofetch",
                "",
                tr!(CTX, "Auto-Fetch Live Part Information"),
                *s.autofetch_live_part_information.get(),
            )
            .note(
                "autofetch_note",
                tr!(
                    CTX,
                    "Allow the editors to automatically display live information about parts (lifecycle status, stock availability, price, ...) by requesting it from the configured API endpoints."
                ),
            );
        self.update_api_endpoints();
    }

    fn update_dismissed_messages(&self) {
        let count = self.settings.dismissed_messages.get().len();
        self.form.set_text(
            "reset_dismissed",
            format!("{} ({count})", tr!(CTX, "Reset")),
        );
        self.form.set_hint(
            "reset_dismissed",
            trn!(
                CTX,
                "Currently there are {0} dismissed message(s).",
                "Currently there are {0} dismissed message(s).",
                count,
                count
            ),
        );
    }

    /// Updates the color scheme fields (after editing a scheme).
    pub fn update_color_schemes(&self) {
        for (prefix, schemes) in [
            ("sch", &self.schematic_schemes),
            ("brd", &self.board_schemes),
        ] {
            let all = schemes.all();
            // Upstream marks user defined schemes with "*".
            let names: Vec<String> = all
                .iter()
                .map(|(_, n, user)| if *user { format!("*{n}") } else { n.clone() })
                .collect();
            let index = all.iter().position(|(u, _, _)| *u == schemes.active());
            self.form
                .set_options(&format!("{prefix}_scheme"), &names, index);
            let editable = schemes.is_user(schemes.active());
            self.form.set_enabled(&format!("{prefix}_modify"), editable);
            self.form.set_enabled(&format!("{prefix}_remove"), editable);
        }
    }

    fn update_external_app(&self) {
        let (_, exe, arg) = EXTERNAL_APPS[self.external_app];
        let items: Vec<ListItem> = EXTERNAL_APPS
            .iter()
            .map(|(name, _, _)| ListItem::text(tr!(CTX, name)))
            .collect();
        self.form
            .set_items("external_apps", &items, Some(self.external_app));
        self.form.set_text(
            "external_commands",
            self.external_commands[self.external_app].join("\n"),
        );
        self.form.set_hint(
            "external_commands",
            format!("{} {exe} {arg}", tr!(CTX, "Example:")),
        );
        let placeholders: Vec<String> = if self.external_app == 0 {
            vec![format!("{{{{URL}}}}: {}", tr!(CTX, "Website URL to open"))]
        } else {
            vec![
                format!(
                    "{{{{FILEPATH}}}}: {}",
                    tr!(CTX, "Absolute path to the file to open")
                ),
                format!(
                    "{{{{URL}}}}: {}",
                    tr!(CTX, "URL to the file to open (file://)")
                ),
            ]
        };
        self.form.set_text(
            "external_placeholders",
            format!("Available placeholders:\n{}", placeholders.join("\n")),
        );
    }

    /// The shortcuts of a command (overrides or the default).
    fn shortcuts_of(&self, index: usize) -> Vec<String> {
        let cmd = &shortcuts::commands()[index];
        match self.shortcut_overrides.get(&cmd.id) {
            Some(list) => list.clone(),
            None if cmd.default_shortcut.is_empty() => Vec::new(),
            None => vec![cmd.default_shortcut.clone()],
        }
    }

    fn update_shortcuts(&mut self) {
        let filter = self.form.get_text("shortcut_filter").to_lowercase();
        let cmds = shortcuts::commands();
        self.shortcut_rows = (0..cmds.len())
            .filter(|&i| {
                filter.is_empty()
                    || cmds[i].text_tr().to_lowercase().contains(&filter)
                    || cmds[i].id.contains(&filter)
                    || self
                        .shortcuts_of(i)
                        .join(" ")
                        .to_lowercase()
                        .contains(&filter)
            })
            .collect();
        let items: Vec<ListItem> = self
            .shortcut_rows
            .iter()
            .map(|&i| {
                let mut item =
                    ListItem::row(vec![cmds[i].text_tr(), self.shortcuts_of(i).join(", ")]);
                // Overridden shortcuts are shown like upstream in bold;
                // here with a color swatch.
                if self.shortcut_overrides.contains_key(&cmds[i].id) {
                    item.color = Some(slint::Color::from_rgb_u8(0x29, 0xd6, 0x82));
                }
                item
            })
            .collect();
        let row = self
            .shortcut_current
            .and_then(|c| self.shortcut_rows.iter().position(|&i| i == c));
        self.form.set_items("shortcuts", &items, row);
        let current = row.map(|r| self.shortcut_rows[r]);
        self.form.set_text(
            "shortcut_edit",
            current
                .map(|i| self.shortcuts_of(i).join("\n"))
                .unwrap_or_default(),
        );
        for id in ["shortcut_edit", "shortcut_default", "shortcut_none"] {
            self.form.set_enabled(id, current.is_some());
        }
    }

    fn set_shortcut_override(&mut self, text: &str) -> Result<(), String> {
        let Some(index) = self.shortcut_current else {
            return Ok(());
        };
        let sequences: Vec<String> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect();
        if let Some(invalid) = sequences
            .iter()
            .find(|s| shortcuts::parse_key_sequence(s).is_none())
        {
            return Err(tr!(CTX, "Invalid key sequence: {0}", invalid));
        }
        let cmd = &shortcuts::commands()[index];
        let default: Vec<String> = if cmd.default_shortcut.is_empty() {
            Vec::new()
        } else {
            vec![cmd.default_shortcut.clone()]
        };
        if sequences == default {
            self.shortcut_overrides.remove(&cmd.id);
        } else {
            self.shortcut_overrides.insert(cmd.id.clone(), sequences);
        }
        Ok(())
    }

    fn api_endpoints(&self) -> Vec<ApiEndpointSettings> {
        self.settings.api_endpoints.get().clone()
    }

    fn update_api_endpoints(&self) {
        let check = |b: bool| if b { "✔" } else { "" }.to_owned();
        let items: Vec<ListItem> = self
            .api_endpoints()
            .iter()
            .map(|e| {
                ListItem::row(vec![
                    e.url.clone(),
                    check(e.use_for_libraries),
                    check(e.use_for_parts_info),
                    check(e.use_for_order),
                ])
            })
            .collect();
        self.form
            .set_items("api_endpoints", &items, self.api_current);
        let current = self
            .api_current
            .and_then(|i| self.api_endpoints().get(i).cloned());
        for (id, value) in [
            (
                "api_libraries",
                current.as_ref().map(|e| e.use_for_libraries),
            ),
            ("api_parts", current.as_ref().map(|e| e.use_for_parts_info)),
            ("api_order", current.as_ref().map(|e| e.use_for_order)),
        ] {
            self.form.set_checked(id, value.unwrap_or(false));
            self.form.set_enabled(id, value.is_some());
        }
    }

    fn edit_api_endpoints(&mut self, f: impl FnOnce(&mut Vec<ApiEndpointSettings>)) {
        let mut list = self.api_endpoints();
        f(&mut list);
        // Parts information and ordering: at most one endpoint (upstream
        // `ApiEndpointListModelLegacy`).
        self.settings.api_endpoints.set(list);
        self.update_api_endpoints();
    }

    fn edit_order(&mut self, id: &str, action: &ListAction) {
        let item = if id == "locale_order" {
            &mut self.settings.library_locale_order
        } else {
            &mut self.settings.library_norm_order
        };
        let mut list = item.get().clone();
        match action {
            ListAction::Add(text) => {
                let text = text.trim();
                if !text.is_empty() && !list.iter().any(|l| l == text) {
                    list.push(text.to_owned());
                }
            }
            ListAction::Remove(i) if *i < list.len() => {
                list.remove(*i);
            }
            ListAction::MoveUp(i) if *i > 0 && *i < list.len() => list.swap(*i, *i - 1),
            ListAction::MoveDown(i) if *i + 1 < list.len() => list.swap(*i, *i + 1),
            _ => return,
        }
        item.set(list.clone());
        let items: Vec<ListItem> = list.iter().map(ListItem::text).collect();
        self.form.set_items(id, &items, None);
    }

    /// Reads the simple fields into the settings copy.
    fn store(&mut self) -> Result<(), String> {
        let f = &self.form;
        let s = &mut self.settings;
        let locale = f
            .get_index("app_locale")
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| self.locales.get(i).cloned())
            .unwrap_or_default();
        s.application_locale.set(locale);
        if let Some(unit) = f
            .get_index("length_unit")
            .and_then(|i| LengthUnit::ALL.get(i))
        {
            s.default_length_unit.set(*unit);
        }
        s.user_name.set(f.get_text("user_name").trim().to_owned());
        let autosave = f.get_text("autosave").trim().parse::<u32>().map_err(|_| {
            format!(
                "{} {}",
                tr!(CTX, "Autosave Interval:"),
                f.get_text("autosave")
            )
        })?;
        s.project_autosave_interval_seconds.set(autosave);
        s.use_opengl.set(f.get_checked("use_opengl"));
        let theme = f
            .get_index("ui_theme")
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| UiTheme::ALL.get(i))
            .map_or(String::new(), |t| t.id.to_owned());
        s.ui_theme.set(theme);
        if let Some(g) = f.get_index("sch_grid").and_then(|i| GRID_STYLES.get(i)) {
            s.schematic_grid_style.set(*g);
        }
        if let Some(g) = f.get_index("brd_grid").and_then(|i| GRID_STYLES.get(i)) {
            s.board_grid_style.set(*g);
        }
        if let Some(m) = f.get_index("auto_update").and_then(|i| UPDATE_MODES.get(i)) {
            s.libraries_auto_update_mode.set(*m);
        }
        s.autofetch_live_part_information
            .set(f.get_checked("autofetch"));
        Ok(())
    }

    /// Writes the edited settings into `target` (only the items this dialog
    /// shows; others, e.g. modified meanwhile, are kept).
    pub fn apply_to(&mut self, target: &mut WorkspaceSettings) -> Result<(), String> {
        self.store()?;
        let s = &self.settings;
        target
            .application_locale
            .set(s.application_locale.get().clone());
        target.default_length_unit.set(*s.default_length_unit.get());
        target.user_name.set(s.user_name.get().clone());
        target
            .project_autosave_interval_seconds
            .set(*s.project_autosave_interval_seconds.get());
        target.use_opengl.set(*s.use_opengl.get());
        target.ui_theme.set(s.ui_theme.get().clone());
        target
            .schematic_grid_style
            .set(*s.schematic_grid_style.get());
        target.board_grid_style.set(*s.board_grid_style.get());
        target
            .library_locale_order
            .set(s.library_locale_order.get().clone());
        target
            .library_norm_order
            .set(s.library_norm_order.get().clone());
        target.api_endpoints.set(s.api_endpoints.get().clone());
        target
            .libraries_auto_update_mode
            .set(*s.libraries_auto_update_mode.get());
        target
            .autofetch_live_part_information
            .set(*s.autofetch_live_part_information.get());
        target
            .external_web_browser_commands
            .set(self.external_commands[0].clone());
        target
            .external_file_manager_commands
            .set(self.external_commands[1].clone());
        target
            .external_pdf_reader_commands
            .set(self.external_commands[2].clone());
        let shortcuts = target
            .keyboard_shortcuts
            .get()
            .with_overrides(self.shortcut_overrides.clone());
        target.keyboard_shortcuts.set(shortcuts);
        for (item, schemes) in [
            (&mut target.schematic_color_schemes, &self.schematic_schemes),
            (&mut target.board_color_schemes, &self.board_schemes),
        ] {
            let raw = schemes.to_raw();
            if raw == RawSettings::default() {
                if !item.is_default_value() {
                    item.restore_default();
                }
            } else {
                item.set(raw);
            }
        }
        if s.dismissed_messages.is_default_value() && !target.dismissed_messages.is_default_value()
        {
            target.dismissed_messages.restore_default();
        }
        Ok(())
    }

    fn scheme_event(&mut self, kind: SchemeKind, action: &str) {
        let schemes = self.color_schemes_mut(kind);
        match action {
            "scheme" => {}
            "modify" => {
                if schemes.is_user(schemes.active()) {
                    self.request = Some(AppRequest::ShowDialog(DialogKind::ColorScheme(kind)));
                }
            }
            "duplicate" => {
                schemes.duplicate_active();
                self.request = Some(AppRequest::ShowDialog(DialogKind::ColorScheme(kind)));
            }
            "remove" => {
                let active = schemes.active();
                schemes.remove(active);
            }
            _ => {}
        }
        self.update_color_schemes();
    }

    fn scheme_selected(&mut self, kind: SchemeKind, id: &str) {
        let index = self.form.get_index(id);
        let schemes = self.color_schemes_mut(kind);
        if let Some((uuid, _, _)) = index.and_then(|i| schemes.all().get(i).cloned()) {
            schemes.set_active(uuid);
        }
        self.update_color_schemes();
    }

    /// The UUID of the active scheme of a kind (tests).
    pub fn active_scheme(&self, kind: SchemeKind) -> Uuid {
        self.color_schemes(kind).active()
    }
}

impl FormDialog for WorkspaceSettingsDialog {
    fn title(&self) -> String {
        tr!(CTX, "Workspace Settings")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        DialogOptions {
            extra_buttons: vec![tr!("QDialogButtonBox", "Restore Defaults")],
            width: 720.0,
            label_width: 190.0,
            ..DialogOptions::default()
        }
    }

    fn field_event(&mut self, _ctx: &DialogContext<'_>, id: &str, event: FieldEvent) {
        match (id, &event) {
            ("reset_dismissed", FieldEvent::Clicked) => {
                self.settings.dismissed_messages.restore_default();
                self.update_dismissed_messages();
            }
            ("sch_scheme", _) => self.scheme_selected(SchemeKind::Schematic, id),
            ("brd_scheme", _) => self.scheme_selected(SchemeKind::Board, id),
            (_, FieldEvent::Clicked) if id.starts_with("sch_") => {
                self.scheme_event(SchemeKind::Schematic, &id[4..]);
            }
            (_, FieldEvent::Clicked) if id.starts_with("brd_") => {
                self.scheme_event(SchemeKind::Board, &id[4..]);
            }
            ("locale_order" | "norm_order", FieldEvent::List(action)) => {
                self.edit_order(id, action);
            }
            ("external_apps", FieldEvent::List(ListAction::Select(row))) => {
                if *row < EXTERNAL_APPS.len() {
                    self.external_app = *row;
                    self.update_external_app();
                }
            }
            ("external_commands", _) => {
                self.external_commands[self.external_app] = self
                    .form
                    .get_text("external_commands")
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
            ("shortcut_filter", _) => self.update_shortcuts(),
            ("shortcuts", FieldEvent::List(ListAction::Select(row))) => {
                self.shortcut_current = self.shortcut_rows.get(*row).copied();
                self.update_shortcuts();
            }
            ("shortcut_edit", FieldEvent::Edited) => {
                let text = self.form.get_text("shortcut_edit");
                match self.set_shortcut_override(&text) {
                    Ok(()) => {
                        self.form.set_error("shortcut_edit", "");
                        let edit = self.form.get_text("shortcut_edit");
                        self.update_shortcuts();
                        // Keep the text as typed (incomplete lines).
                        self.form.set_text("shortcut_edit", edit);
                    }
                    Err(e) => self.form.set_error("shortcut_edit", e),
                }
            }
            ("shortcut_default", FieldEvent::Clicked) => {
                if let Some(i) = self.shortcut_current {
                    self.shortcut_overrides.remove(&shortcuts::commands()[i].id);
                    self.update_shortcuts();
                }
            }
            ("shortcut_none", FieldEvent::Clicked) => {
                if let Some(i) = self.shortcut_current {
                    self.shortcut_overrides
                        .insert(shortcuts::commands()[i].id.clone(), Vec::new());
                    self.update_shortcuts();
                }
            }
            ("api_endpoints", FieldEvent::List(action)) => {
                let action = action.clone();
                match action {
                    ListAction::Select(row) => {
                        self.api_current = Some(row);
                        self.update_api_endpoints();
                    }
                    ListAction::Add(url) => {
                        let url = url.trim().to_owned();
                        if !url.is_empty() {
                            self.edit_api_endpoints(|l| {
                                l.push(ApiEndpointSettings {
                                    url,
                                    use_for_libraries: true,
                                    use_for_parts_info: false,
                                    use_for_order: false,
                                });
                            });
                        }
                    }
                    ListAction::Remove(row) => {
                        self.api_current = None;
                        self.edit_api_endpoints(|l| {
                            if row < l.len() {
                                l.remove(row);
                            }
                        });
                    }
                    ListAction::MoveUp(row) => {
                        self.api_current = None;
                        self.edit_api_endpoints(|l| {
                            if row > 0 && row < l.len() {
                                l.swap(row, row - 1);
                            }
                        });
                    }
                    ListAction::MoveDown(row) => {
                        self.api_current = None;
                        self.edit_api_endpoints(|l| {
                            if row + 1 < l.len() {
                                l.swap(row, row + 1);
                            }
                        });
                    }
                    _ => {}
                }
            }
            ("api_libraries" | "api_parts" | "api_order", FieldEvent::Edited) => {
                let Some(row) = self.api_current else { return };
                let checked = self.form.get_checked(id);
                let id = id.to_owned();
                self.edit_api_endpoints(|l| {
                    for (i, e) in l.iter_mut().enumerate() {
                        match id.as_str() {
                            "api_libraries" if i == row => e.use_for_libraries = checked,
                            // At most one endpoint for parts information
                            // and ordering (upstream radio buttons).
                            "api_parts" => e.use_for_parts_info = checked && i == row,
                            "api_order" => e.use_for_order = checked && i == row,
                            _ => {}
                        }
                    }
                });
            }
            _ => {}
        }
    }

    fn take_request(&mut self) -> Option<AppRequest> {
        self.request.take()
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn button(&mut self, ctx: &DialogContext<'_>, _index: usize) -> Result<ButtonResult, String> {
        // Upstream asks "Are you sure to reset all settings to their default
        // values?" in a message box; here the first click asks and the
        // second one restores the defaults.
        if !self.restore_defaults_asked {
            self.restore_defaults_asked = true;
            return Err(tr!(
                CTX,
                "Are you sure to reset all settings to their default values?\n\nAttention: This will be applied immediately and cannot be undone!"
            ));
        }
        // The defaults are applied and saved immediately (like upstream,
        // "Cancel" does not revert them).
        let Some(workspace) = ctx.workspace else {
            return Ok(ButtonResult::Keep);
        };
        let settings = {
            let mut ws = workspace.lock();
            ws.settings_mut().restore_defaults();
            ws.save_settings().map_err(|e| e.to_string())?;
            ws.settings().clone()
        };
        *self = Self::new(&settings);
        self.request = Some(AppRequest::WorkspaceSettingsChanged);
        Ok(ButtonResult::Keep)
    }

    fn apply(&mut self, ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let Some(workspace) = ctx.workspace else {
            return Ok(Applied::Nothing);
        };
        {
            let mut ws = workspace.lock();
            self.apply_to(ws.settings_mut())?;
            ws.save_settings().map_err(|e| e.to_string())?;
        }
        Ok(Applied::App(AppRequest::WorkspaceSettingsChanged))
    }
}
