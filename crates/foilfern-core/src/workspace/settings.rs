//! Port of libs/librepcb/core/workspace/workspacesettings.{h,cpp} and the
//! settings items (workspacesettingsitem*.{h,cpp}).
//!
//! The workspace settings file (`data/settings.lp`) only contains settings
//! which the user changed; unknown entries are kept (so switching between
//! application versions doesn't lose settings), and entries at their
//! default value are removed.
//!
//! Differences to upstream:
//! - Settings items are a generic [`SettingsItem<T>`] (upstream: a class
//!   hierarchy with `QObject` parent registration); the value type defines
//!   the serialization through [`SettingsValue`]. There are no `edited()`
//!   signals.
//! - Color schemes (`schematic_color_schemes`, `board_color_schemes`,
//!   `3d_color_schemes`) are kept as raw S-expressions ([`RawSettings`]),
//!   since the color scheme logic (UI) is not ported. Keyboard shortcuts
//!   ([`KeyboardShortcuts`]) keep their key sequences as strings (Qt
//!   "portable text") without parsing them.
//! - The migration of legacy `themes` creates the user color schemes as raw
//!   S-expressions, the same content upstream's `UserColorScheme` writes.
//! - API endpoint URLs are stored verbatim ([`ApiEndpointSettings`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::application;
use crate::serialization::{self, FromSExpression, List, Mode, SExpression};
use crate::types::{AutoUpdateMode, Color, GridStyle, LengthUnit, Uuid, Version};

/// URL of the official LibrePCB API server.
pub const OFFICIAL_API_URL: &str = "https://api.librepcb.org";

/// Value types of settings items: defines how the content of the item node
/// `(key ...)` is read and written.
pub trait SettingsValue: Clone + PartialEq + fmt::Debug + Send + Sync {
    /// Reads the value from the item node (upstream `loadImpl()`).
    /// `item_key` is the name of the child nodes of list items.
    fn load(node: &SExpression, item_key: &str) -> serialization::Result<Self>;

    /// Appends the value to the item node (upstream `serializeImpl()`).
    fn serialize(&self, root: &mut List, item_key: &str);
}

macro_rules! single_value {
    ($($ty:ty),*) => {$(
        impl SettingsValue for $ty {
            fn load(node: &SExpression, _item_key: &str) -> serialization::Result<Self> {
                node.child_value("@0")
            }
            fn serialize(&self, root: &mut List, _item_key: &str) {
                root.append_value(self);
            }
        }
    )*};
}
single_value!(String, bool, u32, LengthUnit, GridStyle, AutoUpdateMode);

/// Items of list settings (upstream `WorkspaceSettingsItem_GenericValueList`).
pub trait ListItem: Clone + PartialEq + fmt::Debug + Send + Sync {
    /// Reads an item from its node `(item_key ...)`.
    fn load(node: &SExpression) -> serialization::Result<Self>;
    /// Appends the item node `(item_key ...)` to `root`.
    fn serialize(&self, root: &mut List, item_key: &str);
}

impl ListItem for String {
    fn load(node: &SExpression) -> serialization::Result<Self> {
        node.child_value("@0")
    }
    fn serialize(&self, root: &mut List, item_key: &str) {
        root.append_child(item_key, self);
    }
}

impl<T: ListItem> SettingsValue for Vec<T> {
    fn load(node: &SExpression, item_key: &str) -> serialization::Result<Self> {
        node.children_named(item_key).map(T::load).collect()
    }
    fn serialize(&self, root: &mut List, item_key: &str) {
        for item in self {
            root.ensure_line_break();
            item.serialize(root, item_key);
        }
        root.ensure_line_break();
    }
}

impl<T: ListItem + Ord> SettingsValue for BTreeSet<T> {
    fn load(node: &SExpression, item_key: &str) -> serialization::Result<Self> {
        node.children_named(item_key).map(T::load).collect()
    }
    fn serialize(&self, root: &mut List, item_key: &str) {
        // Sorted to make the file format canonical.
        for item in self {
            root.ensure_line_break();
            item.serialize(root, item_key);
        }
        root.ensure_line_break();
    }
}

/// An API endpoint (upstream `WorkspaceSettings::ApiEndpoint`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ApiEndpointSettings {
    /// Server URL (stored verbatim).
    pub url: String,
    /// Use for libraries (can be set on 0..n endpoints).
    pub use_for_libraries: bool,
    /// Use for parts information (can be set on 0..1 endpoints).
    pub use_for_parts_info: bool,
    /// Use for ordering PCBs (can be set on 0..1 endpoints).
    pub use_for_order: bool,
}

impl ApiEndpointSettings {
    /// Returns the official LibrePCB API server, used for everything (the
    /// default endpoint).
    pub fn official() -> Self {
        Self {
            url: OFFICIAL_API_URL.to_owned(),
            use_for_libraries: true,
            use_for_parts_info: true,
            use_for_order: true,
        }
    }
}

impl ListItem for ApiEndpointSettings {
    fn load(node: &SExpression) -> serialization::Result<Self> {
        let url: String = node.child_value("@0")?;
        let is_official = url == OFFICIAL_API_URL;
        let flag = |name: &str, default: bool| {
            node.child(name)
                .map_or(Ok(default), |child| child.child_value::<bool>("@0"))
        };
        Ok(Self {
            use_for_libraries: flag("libraries", true)?,
            use_for_parts_info: flag("parts", is_official)?,
            use_for_order: flag("order", is_official)?,
            url,
        })
    }

    fn serialize(&self, root: &mut List, item_key: &str) {
        let node = root.append_list(item_key);
        node.append_value(&self.url);
        node.append_child("libraries", &self.use_for_libraries);
        node.append_child("parts", &self.use_for_parts_info);
        node.append_child("order", &self.use_for_order);
    }
}

/// The kinds of color schemes (upstream
/// `WorkspaceSettingsItem_ColorSchemes::Kind`).
#[derive(Debug, Clone, Copy)]
enum ColorSchemeKind {
    Schematic,
    Board,
    View3d,
}

/// The colors of a legacy theme to migrate.
struct LegacyColors<'a> {
    primary: &'a BTreeMap<String, Color>,
    secondary: &'a BTreeMap<String, Color>,
    name: &'a str,
    active: bool,
}

/// Settings kept as raw S-expression content (the children of the item
/// node), for settings whose logic is not ported (color schemes).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawSettings(pub Vec<SExpression>);

impl SettingsValue for RawSettings {
    fn load(node: &SExpression, _item_key: &str) -> serialization::Result<Self> {
        Ok(Self(node.children().to_vec()))
    }
    fn serialize(&self, root: &mut List, _item_key: &str) {
        for child in &self.0 {
            root.push(child.clone());
        }
    }
}

/// Overridden keyboard shortcuts (upstream
/// `WorkspaceSettingsItem_KeyboardShortcuts`): action identifier → key
/// sequences (Qt "portable text", e.g. `"Ctrl+S"`; an empty list disables
/// the shortcut).
///
/// The nodes of unmodified shortcuts are written back as they were read.
#[derive(Debug, Clone, Default)]
pub struct KeyboardShortcuts {
    overrides: BTreeMap<String, Vec<String>>,
    nodes: BTreeMap<String, SExpression>,
}

impl KeyboardShortcuts {
    /// Returns the overrides.
    pub fn overrides(&self) -> &BTreeMap<String, Vec<String>> {
        &self.overrides
    }

    /// Returns a copy with the given overrides, keeping the nodes of
    /// unmodified shortcuts (upstream `set()`).
    pub fn with_overrides(&self, overrides: BTreeMap<String, Vec<String>>) -> Self {
        let nodes = overrides
            .iter()
            .map(|(key, sequences)| {
                let node = match (self.overrides.get(key), self.nodes.get(key)) {
                    (Some(old), Some(node)) if old == sequences => node.clone(),
                    _ => {
                        let mut list = List::new("shortcut");
                        list.push(SExpression::token(key.clone()));
                        for sequence in sequences {
                            list.append_value(sequence);
                        }
                        SExpression::List(list)
                    }
                };
                (key.clone(), node)
            })
            .collect();
        Self { overrides, nodes }
    }
}

impl PartialEq for KeyboardShortcuts {
    fn eq(&self, other: &Self) -> bool {
        self.overrides == other.overrides
    }
}

impl SettingsValue for KeyboardShortcuts {
    fn load(node: &SExpression, item_key: &str) -> serialization::Result<Self> {
        let mut result = Self::default();
        for child in node.children_named(item_key) {
            let identifier = child.required_child("@0")?.value()?.to_owned();
            let sequences = child
                .children()
                .iter()
                .filter(|c| c.is_string())
                .map(|c| c.value().map(str::to_owned))
                .collect::<serialization::Result<Vec<_>>>()?;
            result.nodes.insert(identifier.clone(), child.clone());
            result.overrides.insert(identifier, sequences);
        }
        Ok(result)
    }

    fn serialize(&self, root: &mut List, _item_key: &str) {
        for node in self.nodes.values() {
            root.ensure_line_break();
            root.push(node.clone());
        }
        root.ensure_line_break();
    }
}

/// A settings item (upstream `WorkspaceSettingsItem` and its generic
/// subclasses).
#[derive(Debug, Clone)]
pub struct SettingsItem<T> {
    key: &'static str,
    item_key: &'static str,
    default: T,
    current: T,
    is_default: bool,
    edited: bool,
}

impl<T: SettingsValue> SettingsItem<T> {
    fn new(key: &'static str, item_key: &'static str, default: T) -> Self {
        Self {
            key,
            item_key,
            current: default.clone(),
            default,
            is_default: true,
            edited: false,
        }
    }

    /// Returns the key used in the settings file.
    pub fn key(&self) -> &'static str {
        self.key
    }

    /// Returns the current value.
    pub fn get(&self) -> &T {
        &self.current
    }

    /// Returns the default value.
    pub fn default_value(&self) -> &T {
        &self.default
    }

    /// Sets the value; returns whether it was changed.
    pub fn set(&mut self, value: T) -> bool {
        if value == self.current {
            return false;
        }
        self.current = value;
        self.is_default = false;
        self.edited = true;
        true
    }

    /// Returns whether the setting is at its default value, i.e. it has not
    /// been loaded or set (or has been restored).
    pub fn is_default_value(&self) -> bool {
        self.is_default
    }

    /// Returns whether the setting was edited since the last load or save.
    pub fn is_edited(&self) -> bool {
        self.edited
    }

    /// Clears the edited flag.
    pub fn clear_edited_flag(&mut self) {
        self.edited = false;
    }

    /// Restores the default value.
    pub fn restore_default(&mut self) {
        self.set(self.default.clone());
        self.is_default = true;
        self.edited = true;
    }
}

impl<T: ListItem> SettingsItem<Vec<T>> {
    /// Returns whether the list contains `item`.
    pub fn contains(&self, item: &T) -> bool {
        self.current.contains(item)
    }

    /// Appends an item.
    pub fn add(&mut self, item: T) -> bool {
        let mut value = self.current.clone();
        value.push(item);
        self.set(value)
    }
}

impl<T: ListItem + Ord> SettingsItem<BTreeSet<T>> {
    /// Returns whether the set contains `item`.
    pub fn contains(&self, item: &T) -> bool {
        self.current.contains(item)
    }

    /// Adds an item.
    pub fn add(&mut self, item: T) -> bool {
        let mut value = self.current.clone();
        value.insert(item);
        self.set(value)
    }
}

/// Object safe interface of all settings items.
trait AnySettingsItem {
    fn key(&self) -> &'static str;
    fn is_default_value(&self) -> bool;
    fn is_edited(&self) -> bool;
    fn restore_default(&mut self);
    fn load(&mut self, node: &SExpression) -> serialization::Result<()>;
    fn serialize(&mut self) -> SExpression;
}

impl<T: SettingsValue> AnySettingsItem for SettingsItem<T> {
    fn key(&self) -> &'static str {
        self.key
    }
    fn is_default_value(&self) -> bool {
        self.is_default
    }
    fn is_edited(&self) -> bool {
        self.edited
    }
    fn restore_default(&mut self) {
        SettingsItem::restore_default(self);
    }
    fn load(&mut self, node: &SExpression) -> serialization::Result<()> {
        let value = T::load(node, self.item_key)?;
        self.set(value);
        self.is_default = false;
        self.edited = false;
        Ok(())
    }
    fn serialize(&mut self) -> SExpression {
        let mut node = List::new(self.key);
        self.current.serialize(&mut node, self.item_key);
        self.edited = false;
        SExpression::List(node)
    }
}

/// The workspace settings (see the [module docs](self)).
///
/// The items are public fields like upstream.
#[derive(Debug, Clone)]
pub struct WorkspaceSettings {
    file_content: BTreeMap<String, SExpression>,
    upgrade_required: bool,
    /// UI theme (`ui_theme`).
    pub ui_theme: SettingsItem<String>,
    /// Application locale (`application_locale`, empty = system locale).
    pub application_locale: SettingsItem<String>,
    /// Default length unit (`default_length_unit`).
    pub default_length_unit: SettingsItem<LengthUnit>,
    /// Project autosave interval in seconds (`project_autosave_interval`).
    pub project_autosave_interval_seconds: SettingsItem<u32>,
    /// Whether OpenGL is used (`use_opengl`).
    pub use_opengl: SettingsItem<bool>,
    /// User name (`user`).
    pub user_name: SettingsItem<String>,
    /// Library locale order (`library_locale_order`).
    pub library_locale_order: SettingsItem<Vec<String>>,
    /// Library norm order (`library_norm_order`).
    pub library_norm_order: SettingsItem<Vec<String>>,
    /// API endpoints (`api_endpoints`).
    pub api_endpoints: SettingsItem<Vec<ApiEndpointSettings>>,
    /// Library update mode (`library_updates`).
    pub libraries_auto_update_mode: SettingsItem<AutoUpdateMode>,
    /// Whether live part information is fetched automatically
    /// (`autofetch_live_part_information`).
    pub autofetch_live_part_information: SettingsItem<bool>,
    /// External web browser commands (`external_web_browser`).
    pub external_web_browser_commands: SettingsItem<Vec<String>>,
    /// External file manager commands (`external_file_manager`).
    pub external_file_manager_commands: SettingsItem<Vec<String>>,
    /// External PDF reader commands (`external_pdf_reader`).
    pub external_pdf_reader_commands: SettingsItem<Vec<String>>,
    /// Keyboard shortcuts (`keyboard_shortcuts`).
    pub keyboard_shortcuts: SettingsItem<KeyboardShortcuts>,
    /// Schematic grid style (`schematic_grid_style`).
    pub schematic_grid_style: SettingsItem<GridStyle>,
    /// Board grid style (`board_grid_style`).
    pub board_grid_style: SettingsItem<GridStyle>,
    /// Schematic color schemes (`schematic_color_schemes`, raw).
    pub schematic_color_schemes: SettingsItem<RawSettings>,
    /// Board color schemes (`board_color_schemes`, raw).
    pub board_color_schemes: SettingsItem<RawSettings>,
    /// 3D view color schemes (`3d_color_schemes`, raw).
    pub view_3d_color_schemes: SettingsItem<RawSettings>,
    /// Dismissed messages (`dismissed_messages`).
    pub dismissed_messages: SettingsItem<BTreeSet<String>>,
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceSettings {
    /// Creates the default settings.
    pub fn new() -> Self {
        Self {
            file_content: BTreeMap::new(),
            upgrade_required: false,
            ui_theme: SettingsItem::new("ui_theme", "", String::new()),
            application_locale: SettingsItem::new("application_locale", "", String::new()),
            default_length_unit: SettingsItem::new(
                "default_length_unit",
                "",
                LengthUnit::Millimeters,
            ),
            project_autosave_interval_seconds: SettingsItem::new(
                "project_autosave_interval",
                "",
                600,
            ),
            use_opengl: SettingsItem::new("use_opengl", "", false),
            user_name: SettingsItem::new("user", "", String::new()),
            library_locale_order: SettingsItem::new("library_locale_order", "locale", Vec::new()),
            library_norm_order: SettingsItem::new("library_norm_order", "norm", Vec::new()),
            api_endpoints: SettingsItem::new(
                "api_endpoints",
                "endpoint",
                vec![ApiEndpointSettings::official()],
            ),
            libraries_auto_update_mode: SettingsItem::new(
                "library_updates",
                "",
                AutoUpdateMode::Install,
            ),
            autofetch_live_part_information: SettingsItem::new(
                "autofetch_live_part_information",
                "",
                true,
            ),
            external_web_browser_commands: SettingsItem::new(
                "external_web_browser",
                "command",
                Vec::new(),
            ),
            external_file_manager_commands: SettingsItem::new(
                "external_file_manager",
                "command",
                Vec::new(),
            ),
            external_pdf_reader_commands: SettingsItem::new(
                "external_pdf_reader",
                "command",
                Vec::new(),
            ),
            keyboard_shortcuts: SettingsItem::new(
                "keyboard_shortcuts",
                "shortcut",
                KeyboardShortcuts::default(),
            ),
            schematic_grid_style: SettingsItem::new("schematic_grid_style", "", GridStyle::Lines),
            board_grid_style: SettingsItem::new("board_grid_style", "", GridStyle::Lines),
            schematic_color_schemes: SettingsItem::new(
                "schematic_color_schemes",
                "",
                RawSettings::default(),
            ),
            board_color_schemes: SettingsItem::new(
                "board_color_schemes",
                "",
                RawSettings::default(),
            ),
            view_3d_color_schemes: SettingsItem::new(
                "3d_color_schemes",
                "",
                RawSettings::default(),
            ),
            dismissed_messages: SettingsItem::new("dismissed_messages", "message", BTreeSet::new()),
        }
    }

    fn items_mut(&mut self) -> [&mut dyn AnySettingsItem; 21] {
        [
            &mut self.ui_theme,
            &mut self.application_locale,
            &mut self.default_length_unit,
            &mut self.project_autosave_interval_seconds,
            &mut self.use_opengl,
            &mut self.user_name,
            &mut self.library_locale_order,
            &mut self.library_norm_order,
            &mut self.api_endpoints,
            &mut self.libraries_auto_update_mode,
            &mut self.autofetch_live_part_information,
            &mut self.external_web_browser_commands,
            &mut self.external_file_manager_commands,
            &mut self.external_pdf_reader_commands,
            &mut self.keyboard_shortcuts,
            &mut self.schematic_grid_style,
            &mut self.board_grid_style,
            &mut self.schematic_color_schemes,
            &mut self.board_color_schemes,
            &mut self.view_3d_color_schemes,
            &mut self.dismissed_messages,
        ]
    }

    fn items(&self) -> [&dyn AnySettingsItem; 21] {
        [
            &self.ui_theme,
            &self.application_locale,
            &self.default_length_unit,
            &self.project_autosave_interval_seconds,
            &self.use_opengl,
            &self.user_name,
            &self.library_locale_order,
            &self.library_norm_order,
            &self.api_endpoints,
            &self.libraries_auto_update_mode,
            &self.autofetch_live_part_information,
            &self.external_web_browser_commands,
            &self.external_file_manager_commands,
            &self.external_pdf_reader_commands,
            &self.keyboard_shortcuts,
            &self.schematic_grid_style,
            &self.board_grid_style,
            &self.schematic_color_schemes,
            &self.board_color_schemes,
            &self.view_3d_color_schemes,
            &self.dismissed_messages,
        ]
    }

    /// Loads the settings from the root node of `settings.lp`, written in
    /// the file format `file_format` (upstream `load()`).
    ///
    /// Items which cannot be loaded are logged and keep their default
    /// value. If the file format is outdated, unknown entries will be
    /// removed on the next [`serialize()`](Self::serialize) and all
    /// non-default settings are written.
    pub fn load(&mut self, node: &SExpression, file_format: &Version) {
        for child in node.children().iter().filter(|c| c.is_list()) {
            if let Ok(name) = child.name() {
                self.file_content.insert(name.to_owned(), child.clone());
            }
        }

        // Migrate the grid styles of legacy themes (upstream: should be moved
        // to the file format migration of file format v3).
        if let Some(themes) = self.file_content.get("themes").cloned()
            && let Err(e) = self.migrate_legacy_themes(&themes)
        {
            log::error!("Could not migrate old workspace settings: {e}");
        }

        let file_content = self.file_content.clone();
        for item in self.items_mut() {
            if let Some(node) = file_content.get(item.key())
                && let Err(e) = item.load(node)
            {
                log::error!("Could not load workspace settings item: {e}");
            }
        }
        if *file_format < application::file_format_version() {
            self.file_content.clear();
            self.upgrade_required = true;
        }
    }

    /// Migrates legacy themes to the grid style and color scheme settings
    /// (upstream `load()`): the grid styles of the active theme, and the
    /// colors of each theme as user color schemes (only for color scheme
    /// kinds without settings yet).
    fn migrate_legacy_themes(&mut self, themes: &SExpression) -> serialization::Result<()> {
        let active: Uuid = themes.child_value("active/@0")?;
        for theme in themes.children_named("theme") {
            let uuid: Uuid = theme.child_value("@0")?;
            if uuid == active {
                if let Some(node) = theme.child("schematic_grid_style/@0") {
                    self.schematic_grid_style
                        .set(GridStyle::from_sexpression(node)?);
                    self.schematic_grid_style.clear_edited_flag();
                }
                if let Some(node) = theme.child("board_grid_style/@0") {
                    self.board_grid_style
                        .set(GridStyle::from_sexpression(node)?);
                    self.board_grid_style.clear_edited_flag();
                }
            }
            let name = theme.required_child("@1")?.value()?.to_owned();
            if let Some(colors) = theme.child("colors") {
                let mut primary = BTreeMap::new();
                let mut secondary = BTreeMap::new();
                for node in colors.children().iter().filter(|c| c.is_list()) {
                    let role = node.name()?.to_owned();
                    if let Some(color) = node.child("primary/@0") {
                        primary.insert(role.clone(), Color::from_sexpression(color)?);
                    }
                    if let Some(color) = node.child("secondary/@0") {
                        secondary.insert(role, Color::from_sexpression(color)?);
                    }
                }
                let legacy = LegacyColors {
                    primary: &primary,
                    secondary: &secondary,
                    name: &name,
                    active: uuid == active,
                };
                for kind in [
                    ColorSchemeKind::Schematic,
                    ColorSchemeKind::Board,
                    ColorSchemeKind::View3d,
                ] {
                    self.migrate_legacy_colors(kind, &legacy);
                }
            }
        }
        Ok(())
    }

    /// Creates a user color scheme from the colors of a legacy theme
    /// (upstream `migrateColors` lambda and `UserColorScheme::serialize()`).
    fn migrate_legacy_colors(&mut self, kind: ColorSchemeKind, legacy: &LegacyColors<'_>) {
        let (item, prefix, (base_uuid, base_name)) = match kind {
            ColorSchemeKind::Schematic => (
                &mut self.schematic_color_schemes,
                "schematic_",
                ("9121eabe-55b3-4a7c-bffe-20115b8ad314", "LibrePCB Light"),
            ),
            ColorSchemeKind::Board => (
                &mut self.board_color_schemes,
                "board_",
                ("c605f278-5210-472e-a59f-1b73a15aee2d", "LibrePCB Dark"),
            ),
            ColorSchemeKind::View3d => (
                &mut self.view_3d_color_schemes,
                "3d_",
                ("b3c5c2ed-45bc-47f5-b639-3713c6db826d", "LibrePCB Light"),
            ),
        };
        if self.file_content.contains_key(item.key()) {
            return; // Already migrated.
        }
        let roles: BTreeSet<&String> = legacy
            .primary
            .keys()
            .chain(legacy.secondary.keys())
            .filter(|role| role.starts_with(prefix))
            .collect();
        if roles.is_empty() {
            return;
        }

        // The new scheme: the base node and one node per modified color,
        // sorted.
        let mut nodes = Vec::new();
        let mut base = List::new("base");
        base.push(SExpression::token(base_uuid));
        base.append_value(base_name);
        nodes.push(SExpression::List(base));
        for role in roles {
            let mut node = List::new("color");
            node.push(SExpression::token(role.clone()));
            if let Some(color) = legacy.primary.get(role) {
                node.append_child("primary", color);
            }
            if let Some(color) = legacy.secondary.get(role) {
                node.append_child("secondary", color);
            }
            nodes.push(SExpression::List(node));
        }
        nodes.sort();
        let uuid = Uuid::new_random();
        let mut scheme = List::new("scheme");
        scheme.append_value(&uuid);
        scheme.append_value(legacy.name);
        for node in nodes {
            scheme.ensure_line_break();
            scheme.push(node);
        }
        scheme.ensure_line_break();

        // Add it to the schemes migrated from previous themes (ordered by
        // UUID like upstream's `QMap`).
        let mut schemes: BTreeMap<Uuid, SExpression> = item
            .get()
            .0
            .iter()
            .filter(|c| c.name().is_ok_and(|n| n == "scheme"))
            .filter_map(|c| Some((c.child_value::<Uuid>("@0").ok()?, c.clone())))
            .collect();
        schemes.insert(uuid, SExpression::List(scheme));
        let previous_active = item
            .get()
            .0
            .iter()
            .find(|c| c.name().is_ok_and(|n| n == "active"))
            .and_then(|c| c.child_value::<Uuid>("@0").ok());
        let active = if legacy.active {
            Some(uuid)
        } else {
            previous_active
        };
        let active_name = active
            .and_then(|a| schemes.get(&a))
            .and_then(|s| s.child("@1"))
            .and_then(|n| n.value().ok())
            .unwrap_or(base_name)
            .to_owned();
        let mut active_node = List::new("active");
        match active {
            Some(uuid) if schemes.contains_key(&uuid) => {
                active_node.append_value(&uuid);
            }
            _ => {
                active_node.push(SExpression::token(base_uuid));
            }
        }
        active_node.append_value(&active_name);
        let mut children = vec![SExpression::LineBreak, SExpression::List(active_node)];
        for scheme in schemes.into_values() {
            children.push(SExpression::LineBreak);
            children.push(scheme);
        }
        children.push(SExpression::LineBreak);
        item.set(RawSettings(children));
        item.clear_edited_flag();
    }

    /// Restores all default values and removes unknown entries (upstream
    /// `restoreDefaults()`).
    pub fn restore_defaults(&mut self) {
        for item in self.items_mut() {
            item.restore_default();
        }
        self.file_content.clear(); // Remove even unknown settings!
    }

    /// Returns whether any setting was edited since loading or saving.
    pub fn is_edited(&self) -> bool {
        self.items().iter().any(|item| item.is_edited())
    }

    /// Returns the root node of `settings.lp` (upstream `serialize()`),
    /// updating the file content with the edited settings.
    pub fn serialize(&mut self) -> SExpression {
        let upgrade_required = self.upgrade_required;
        let mut updates = Vec::new();
        for item in self.items_mut() {
            if item.is_edited() || upgrade_required {
                if item.is_default_value() {
                    updates.push((item.key(), None));
                } else {
                    updates.push((item.key(), Some(item.serialize())));
                }
            }
        }
        for (key, node) in updates {
            match node {
                Some(node) => {
                    self.file_content.insert(key.to_owned(), node);
                }
                None => {
                    self.file_content.remove(key);
                }
            }
        }
        let mut root = List::new("librepcb_workspace_settings");
        for child in self.file_content.values() {
            root.ensure_line_break();
            root.push(child.clone());
        }
        root.ensure_line_break();
        SExpression::List(root)
    }

    /// Serializes the settings to the content of `settings.lp`.
    pub fn to_bytes(&mut self) -> serialization::Result<Vec<u8>> {
        self.serialize().to_byte_array(Mode::LibrePcb)
    }

    /// Returns the first endpoint used for parts information (upstream
    /// `getApiEndpointForPartsInfo()`).
    pub fn api_endpoint_for_parts_info(&self) -> Option<&ApiEndpointSettings> {
        self.api_endpoints
            .get()
            .iter()
            .find(|ep| ep.use_for_parts_info && !ep.url.is_empty())
    }

    /// Returns the first endpoint used for ordering PCBs (upstream
    /// `getApiEndpointForOrder()`).
    pub fn api_endpoint_for_order(&self) -> Option<&ApiEndpointSettings> {
        self.api_endpoints
            .get()
            .iter()
            .find(|ep| ep.use_for_order && !ep.url.is_empty())
    }

    /// Returns all endpoints used for libraries (non-empty URLs).
    pub fn api_endpoints_for_libraries(&self) -> impl Iterator<Item = &ApiEndpointSettings> {
        self.api_endpoints
            .get()
            .iter()
            .filter(|ep| ep.use_for_libraries && !ep.url.is_empty())
    }
}
