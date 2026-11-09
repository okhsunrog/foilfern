//! Color schemes of the workspace settings: the base schemes, user defined
//! schemes and the active scheme of the schematic and board editors.
//!
//! Port of libs/librepcb/core/workspace/{basecolorscheme,usercolorscheme,
//! colorrole,workspacesettingsitem_colorschemes}.{h,cpp} and of
//! libs/librepcb/editor/modelview/colorschememodel.{h,cpp} (the rows of
//! the color scheme dialog). The core keeps the settings items
//! `schematic_color_schemes` and `board_color_schemes` as raw
//! S-expressions ([`RawSettings`]); [`ColorSchemes`] interprets and writes
//! them like upstream (`(active <uuid> "<name>")` and one
//! `(scheme <uuid> "<name>" (base <uuid> "<name>") (color <role> (primary
//! "#aarrggbb") (secondary "#aarrggbb")) ...)` per user scheme; nodes which
//! are not modified are written back as they were read). The 3D view color
//! schemes are not ported (there is no 3D view yet); their settings entry is
//! kept by the core.
//!
//! The base scheme tables below are generated from upstream's
//! `basecolorscheme.cpp` and `colorrole.cpp`.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::serialization::{List, SExpression};
use librepcb_core::types::Uuid;
use librepcb_core::workspace::RawSettings;
use librepcb_i18n::tr;

/// The color role names are translated with upstream's context.
const ROLE_CTX: &str = "ColorRole";

/// Number of inner copper layers (upstream `Layer::innerCopperCount()`).
const INNER_COPPER_COUNT: usize = 62;

/// A built-in color scheme (upstream `BaseColorScheme`): roles with their
/// (untranslated) names and primary and secondary colors (`#aarrggbb`).
#[derive(Debug)]
pub struct BaseScheme {
    /// UUID.
    pub uuid: &'static str,
    /// Name (not translated, like upstream).
    pub name: &'static str,
    /// Roles: identifier, name, primary and secondary color.
    pub colors: &'static [(&'static str, &'static str, &'static str, &'static str)],
    /// Colors of the inner copper layers (repeated cyclically), inserted
    /// before the last role (bottom copper).
    pub inner_copper: &'static [(&'static str, &'static str)],
}

/// Colors of a role.
#[derive(Debug, Clone, PartialEq)]
pub struct RoleColors {
    /// Role identifier, e.g. `"schematic_wires"`.
    pub role: String,
    /// Translated role name.
    pub name: String,
    /// Primary color (ARGB).
    pub primary: u32,
    /// Secondary (highlight) color (ARGB).
    pub secondary: u32,
    /// Whether the primary color is overridden by the user scheme.
    pub primary_overridden: bool,
    /// Whether the secondary color is overridden by the user scheme.
    pub secondary_overridden: bool,
}

impl BaseScheme {
    /// The UUID.
    pub fn uuid(&self) -> Uuid {
        // Invariant: the tables contain valid UUIDs (checked by a test).
        self.uuid
            .parse()
            .expect("valid UUID in the base scheme table")
    }

    /// All roles in upstream's order (`getAllColors()`).
    pub fn roles(&self) -> Vec<RoleColors> {
        let mut out = Vec::new();
        let n = self.colors.len();
        for (i, (role, name, primary, secondary)) in self.colors.iter().enumerate() {
            if i + 1 == n && !self.inner_copper.is_empty() {
                for number in 1..=INNER_COPPER_COUNT {
                    let (p, s) = self.inner_copper[(number - 1) % self.inner_copper.len()];
                    out.push(RoleColors {
                        role: format!("board_copper_inner_{number}"),
                        name: format!("{} {number}", tr!(ROLE_CTX, "Inner Copper")),
                        primary: parse_argb(p).unwrap_or(0),
                        secondary: parse_argb(s).unwrap_or(0),
                        primary_overridden: false,
                        secondary_overridden: false,
                    });
                }
            }
            out.push(RoleColors {
                role: (*role).to_owned(),
                name: librepcb_i18n::translate(ROLE_CTX, name).into_owned(),
                primary: parse_argb(primary).unwrap_or(0),
                secondary: parse_argb(secondary).unwrap_or(0),
                primary_overridden: false,
                secondary_overridden: false,
            });
        }
        out
    }
}

/// Parses `#aarrggbb` or `#rrggbb` (Qt `QColor` names).
pub fn parse_argb(s: &str) -> Option<u32> {
    let hex = s.trim().strip_prefix('#')?;
    let v = u32::from_str_radix(hex, 16).ok()?;
    match hex.len() {
        8 => Some(v),
        6 => Some(0xff00_0000 | v),
        _ => None,
    }
}

/// Formats `#aarrggbb` (Qt `QColor::name(QColor::HexArgb)`).
pub fn format_argb(c: u32) -> String {
    format!("#{c:08x}")
}

/// Formats `#rrggbb[aa]` (upstream `toHexRgba()` of the color scheme
/// dialog: the alpha is appended only if not opaque).
pub fn format_rgba(c: u32) -> String {
    let [a, r, g, b] = c.to_be_bytes();
    if a == 0xff {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

/// Parses `#rrggbb[aa]` (upstream `fromHexRgba()`).
pub fn parse_rgba(s: &str) -> Option<u32> {
    let hex = s.trim().strip_prefix('#')?;
    let v = match hex.len() {
        6 => (u32::from_str_radix(hex, 16).ok()? << 8) | 0xff,
        8 => u32::from_str_radix(hex, 16).ok()?,
        _ => return None,
    };
    let [r, g, b, a] = v.to_be_bytes();
    Some(u32::from_be_bytes([a, r, g, b]))
}

/// HSV (all 0..1) of an ARGB color (upstream `QColor::hueF()` etc.; the
/// hue of gray colors is 0).
pub fn to_hsva(c: u32) -> (f32, f32, f32, f32) {
    let [a, r, g, b] = c.to_be_bytes();
    let (r, g, b) = (
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    );
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta <= 0.0 {
        0.0
    } else if max == r {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };
    let saturation = if max <= 0.0 { 0.0 } else { delta / max };
    (hue.clamp(0.0, 1.0), saturation, max, f32::from(a) / 255.0)
}

/// ARGB color of HSV values (all 0..1, upstream `QColor::fromHsvF()`).
pub fn from_hsva(h: f32, s: f32, v: f32, a: f32) -> u32 {
    let h = (h.clamp(0.0, 1.0) * 6.0) % 6.0;
    let (s, v) = (s.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let to = |f: f32| ((f + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    let a = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
    u32::from_be_bytes([a, to(r), to(g), to(b)])
}

/// A user defined color scheme (upstream `UserColorScheme`).
#[derive(Debug, Clone, PartialEq)]
pub struct UserScheme {
    /// UUID.
    pub uuid: Uuid,
    /// Name.
    pub name: String,
    /// UUID of the base scheme.
    pub base: Uuid,
    loaded_nodes: Vec<SExpression>,
    modified: BTreeSet<String>,
    primary: BTreeMap<String, u32>,
    secondary: BTreeMap<String, u32>,
}

impl UserScheme {
    fn new(uuid: Uuid, name: String, base: &BaseScheme) -> Self {
        let mut node = List::new("base");
        node.append_value(&base.uuid());
        node.append_value(&base.name.to_owned());
        Self {
            uuid,
            name,
            base: base.uuid(),
            loaded_nodes: vec![SExpression::List(node)],
            modified: BTreeSet::new(),
            primary: BTreeMap::new(),
            secondary: BTreeMap::new(),
        }
    }

    fn load(node: &SExpression) -> Option<Self> {
        let uuid: Uuid = node.child_value("@0").ok()?;
        let name = node.child("@1")?.value().ok()?.to_owned();
        let base: Uuid = node.child_value("base/@0").ok()?;
        let mut primary = BTreeMap::new();
        let mut secondary = BTreeMap::new();
        for color in node.children_named("color") {
            let Some(role) = color.child("@0").and_then(|r| r.value().ok()) else {
                continue;
            };
            if let Some(c) = color
                .child("primary/@0")
                .and_then(|c| c.value().ok())
                .and_then(parse_argb)
            {
                primary.insert(role.to_owned(), c);
            }
            if let Some(c) = color
                .child("secondary/@0")
                .and_then(|c| c.value().ok())
                .and_then(parse_argb)
            {
                secondary.insert(role.to_owned(), c);
            }
        }
        Some(Self {
            uuid,
            name,
            base,
            loaded_nodes: node
                .children()
                .iter()
                .filter(|c| c.is_list())
                .cloned()
                .collect(),
            modified: BTreeSet::new(),
            primary,
            secondary,
        })
    }

    /// Upstream `UserColorScheme::serialize()`.
    fn serialize(&self) -> SExpression {
        let mut nodes = self.loaded_nodes.clone();
        if !self.modified.is_empty() {
            nodes.retain(|n| {
                !(n.name().is_ok_and(|name| name == "color")
                    && n.child("@0")
                        .and_then(|r| r.value().ok())
                        .is_some_and(|r| self.modified.contains(r)))
            });
            for role in &self.modified {
                let mut node = List::new("color");
                node.push(SExpression::token(role.clone()));
                let (p, s) = (self.primary.get(role), self.secondary.get(role));
                if let Some(p) = p {
                    node.append_child("primary", &format_argb(*p));
                }
                if let Some(s) = s {
                    node.append_child("secondary", &format_argb(*s));
                }
                if p.is_some() || s.is_some() {
                    nodes.push(SExpression::List(node));
                }
            }
            nodes.sort();
        }
        let mut root = List::new("scheme");
        root.append_value(&self.uuid);
        root.append_value(&self.name);
        for node in nodes {
            root.ensure_line_break();
            root.push(node);
        }
        root.ensure_line_break();
        SExpression::List(root)
    }

    /// Sets (`Some`) or restores (`None`) a color override.
    fn set_color(&mut self, role: &str, secondary: bool, color: Option<u32>) {
        let map = if secondary {
            &mut self.secondary
        } else {
            &mut self.primary
        };
        let changed = match color {
            Some(c) => map.insert(role.to_owned(), c) != Some(c),
            None => map.remove(role).is_some(),
        };
        if changed {
            self.modified.insert(role.to_owned());
        }
    }
}

/// Which editors a set of color schemes is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemeKind {
    /// Schematics (and symbols).
    Schematic,
    /// Boards (and footprints).
    Board,
}

impl SchemeKind {
    /// The base schemes, the default first (upstream constructor of
    /// `WorkspaceSettingsItem_ColorSchemes`).
    pub fn bases(self) -> &'static [&'static BaseScheme] {
        match self {
            Self::Schematic => &[&SCHEMATIC_LIGHT, &SCHEMATIC_DARK, &SCHEMATIC_SOLARIZED],
            Self::Board => &[&BOARD_DARK],
        }
    }
}

/// The color schemes of one kind (upstream
/// `WorkspaceSettingsItem_ColorSchemes`).
#[derive(Debug, Clone, PartialEq)]
pub struct ColorSchemes {
    kind: SchemeKind,
    active: Uuid,
    /// User schemes by UUID (upstream `QMap`, ordered by UUID).
    users: BTreeMap<Uuid, UserScheme>,
}

impl ColorSchemes {
    /// The default: no user schemes, the first base scheme active.
    pub fn new(kind: SchemeKind) -> Self {
        Self {
            kind,
            active: kind.bases()[0].uuid(),
            users: BTreeMap::new(),
        }
    }

    /// Reads the settings item (upstream `loadImpl()`; invalid schemes are
    /// skipped).
    pub fn load(kind: SchemeKind, raw: &RawSettings) -> Self {
        let mut this = Self::new(kind);
        for node in &raw.0 {
            match node.name() {
                Ok("scheme") => match UserScheme::load(node) {
                    Some(s) => {
                        this.users.insert(s.uuid, s);
                    }
                    None => log::warn!("Invalid color scheme in the workspace settings."),
                },
                Ok("active") => {
                    if let Ok(uuid) = node.child_value::<Uuid>("@0") {
                        this.active = uuid;
                    }
                }
                _ => {}
            }
        }
        this
    }

    /// The settings item content (upstream `serializeImpl()`); empty (the
    /// default) without user schemes and with the default scheme active.
    pub fn to_raw(&self) -> RawSettings {
        if self.users.is_empty() && self.active == self.kind.bases()[0].uuid() {
            return RawSettings::default();
        }
        let mut root = List::new("item");
        root.ensure_line_break();
        let active = root.append_child("active", &self.active);
        active.append_value(&self.active_name());
        for scheme in self.users.values() {
            root.ensure_line_break();
            root.push(scheme.serialize());
        }
        root.ensure_line_break();
        RawSettings(root.children().to_vec())
    }

    /// The kind.
    pub fn kind(&self) -> SchemeKind {
        self.kind
    }

    /// The UUID of the active scheme.
    pub fn active(&self) -> Uuid {
        self.active
    }

    /// Activates a scheme.
    pub fn set_active(&mut self, uuid: Uuid) {
        self.active = uuid;
    }

    /// All schemes: `(uuid, name, user defined)`, the base schemes first.
    pub fn all(&self) -> Vec<(Uuid, String, bool)> {
        self.kind
            .bases()
            .iter()
            .map(|b| (b.uuid(), b.name.to_owned(), false))
            .chain(self.users.values().map(|u| (u.uuid, u.name.clone(), true)))
            .collect()
    }

    /// Whether a scheme is user defined (editable).
    pub fn is_user(&self, uuid: Uuid) -> bool {
        self.users.contains_key(&uuid)
    }

    fn base_of(&self, uuid: Uuid) -> &'static BaseScheme {
        let bases = self.kind.bases();
        let base_uuid = self.users.get(&uuid).map_or(uuid, |u| u.base);
        bases
            .iter()
            .find(|b| b.uuid() == base_uuid)
            .copied()
            .unwrap_or(bases[0])
    }

    /// The name of the active scheme (the first base scheme if the active
    /// UUID is unknown, like upstream `updateActive()`).
    pub fn active_name(&self) -> String {
        self.users.get(&self.active).map_or_else(
            || self.base_of(self.active).name.to_owned(),
            |u| u.name.clone(),
        )
    }

    /// The colors of a scheme (upstream `ColorSchemeModel` rows).
    pub fn colors(&self, uuid: Uuid) -> Vec<RoleColors> {
        let mut roles = self.base_of(uuid).roles();
        if let Some(user) = self.users.get(&uuid) {
            for r in &mut roles {
                if let Some(c) = user.primary.get(&r.role) {
                    r.primary = *c;
                    r.primary_overridden = true;
                }
                if let Some(c) = user.secondary.get(&r.role) {
                    r.secondary = *c;
                    r.secondary_overridden = true;
                }
            }
        }
        roles
    }

    /// The active scheme for the scenes (primary colors).
    pub fn scene_scheme(&self) -> librepcb_scene::ColorScheme {
        let colors = self.colors(self.active).into_iter().map(|r| {
            let [a, red, g, b] = r.primary.to_be_bytes();
            (
                r.role,
                librepcb_canvas::peniko::Color::from_rgba8(red, g, b, a),
            )
        });
        librepcb_scene::ColorScheme::custom(self.active_name(), colors)
    }

    /// Duplicates the active scheme as a new user scheme named
    /// `Copy of <name>` and activates it; returns its UUID.
    pub fn duplicate_active(&mut self) -> Uuid {
        let uuid = Uuid::new_random();
        let name = tr!(
            "librepcb::editor::WorkspaceSettingsDialog",
            "Copy of {0}",
            self.active_name()
        );
        let scheme = match self.users.get(&self.active) {
            Some(user) => UserScheme {
                uuid,
                name,
                ..user.clone()
            },
            None => UserScheme::new(uuid, name, self.base_of(self.active)),
        };
        self.users.insert(uuid, scheme);
        self.active = uuid;
        uuid
    }

    /// Removes a user scheme; its base scheme becomes active.
    pub fn remove(&mut self, uuid: Uuid) {
        if let Some(user) = self.users.remove(&uuid)
            && self.active == uuid
        {
            self.active = user.base;
        }
    }

    /// Renames a user scheme.
    pub fn rename(&mut self, uuid: Uuid, name: &str) {
        if let Some(user) = self.users.get_mut(&uuid) {
            user.name = name.to_owned();
        }
    }

    /// The name of a scheme.
    pub fn name(&self, uuid: Uuid) -> String {
        self.all()
            .into_iter()
            .find(|(u, _, _)| *u == uuid)
            .map(|(_, n, _)| n)
            .unwrap_or_default()
    }

    /// Sets (`Some`) or restores (`None`) a color of a user scheme.
    pub fn set_color(&mut self, uuid: Uuid, role: &str, secondary: bool, color: Option<u32>) {
        if let Some(user) = self.users.get_mut(&uuid) {
            user.set_color(role, secondary, color);
        }
    }
}

/// Upstream `BaseColorScheme::schematicLibrePcbLight()`.
pub const SCHEMATIC_LIGHT: BaseScheme = BaseScheme {
    uuid: "9121eabe-55b3-4a7c-bffe-20115b8ad314",
    name: "LibrePCB Light",
    colors: &[
        (
            "schematic_background",
            "Background/Grid",
            "#ffffffff",
            "#ffa0a0a4",
        ),
        ("schematic_overlays", "Overlays", "#78ffffff", "#ff000000"),
        ("schematic_info_box", "Info Box", "#82ffffff", "#ff000000"),
        ("schematic_selection", "Selection", "#ff78aaff", "#5096c8ff"),
        (
            "schematic_references",
            "References",
            "#32000000",
            "#80000000",
        ),
        ("schematic_frames", "Frames", "#ff000000", "#ff808080"),
        ("schematic_wires", "Wires", "#ff008000", "#ff00ff00"),
        (
            "schematic_net_labels",
            "Net Labels",
            "#ff008000",
            "#ff00ff00",
        ),
        ("schematic_buses", "Buses", "#ff008eff", "#ff72c0ff"),
        (
            "schematic_bus_labels",
            "Bus Labels",
            "#ff008eff",
            "#ff72c0ff",
        ),
        (
            "schematic_image_borders",
            "Image Borders",
            "#ff808080",
            "#ffa0a0a4",
        ),
        (
            "schematic_documentation",
            "Documentation",
            "#ff808080",
            "#ffa0a0a4",
        ),
        ("schematic_comments", "Comments", "#ff000080", "#ff0000ff"),
        ("schematic_guide", "Guide", "#ff808000", "#ffffff00"),
        ("schematic_outlines", "Outlines", "#ff800000", "#ffff0000"),
        (
            "schematic_grab_areas",
            "Grab Areas",
            "#ffffffe1",
            "#ffffffcd",
        ),
        (
            "schematic_hidden_grab_areas",
            "Hidden Grab Areas",
            "#1e0000ff",
            "#320000ff",
        ),
        ("schematic_names", "Names", "#ff202020", "#ff808080"),
        ("schematic_values", "Values", "#ff505050", "#ffa0a0a4"),
        (
            "schematic_optional_pins",
            "Optional Pins",
            "#ff00ff00",
            "#7f00ff00",
        ),
        (
            "schematic_required_pins",
            "Required Pins",
            "#ffff0000",
            "#7fff0000",
        ),
        ("schematic_pin_lines", "Pin Lines", "#ff800000", "#ffff0000"),
        ("schematic_pin_names", "Pin Names", "#ff404040", "#ffa0a0a4"),
        (
            "schematic_pin_numbers",
            "Pin Numbers",
            "#ff404040",
            "#ffa0a0a4",
        ),
    ],
    inner_copper: &[],
};

/// Upstream `BaseColorScheme::schematicLibrePcbDark()`.
pub const SCHEMATIC_DARK: BaseScheme = BaseScheme {
    uuid: "e761ed45-b99d-4309-aa07-04bba9a80443",
    name: "LibrePCB Dark",
    colors: &[
        (
            "schematic_background",
            "Background/Grid",
            "#ff161616",
            "#ff474747",
        ),
        ("schematic_overlays", "Overlays", "#78000000", "#ffffff00"),
        ("schematic_info_box", "Info Box", "#82000000", "#ffffff00"),
        ("schematic_selection", "Selection", "#ff78aaff", "#5096c8ff"),
        (
            "schematic_references",
            "References",
            "#54ffffff",
            "#9fffffff",
        ),
        ("schematic_frames", "Frames", "#ff9e0000", "#ffff0000"),
        ("schematic_wires", "Wires", "#ff009a00", "#ff00ff00"),
        (
            "schematic_net_labels",
            "Net Labels",
            "#ff009a00",
            "#ff00ff00",
        ),
        ("schematic_buses", "Buses", "#ff0068ba", "#ff59b5ff"),
        (
            "schematic_bus_labels",
            "Bus Labels",
            "#ff0068ba",
            "#ff59b5ff",
        ),
        (
            "schematic_image_borders",
            "Image Borders",
            "#ff5b5b5b",
            "#ffbababa",
        ),
        (
            "schematic_documentation",
            "Documentation",
            "#ffc0bfbc",
            "#ffffffff",
        ),
        ("schematic_comments", "Comments", "#ffb67700", "#ffffdb4b"),
        ("schematic_guide", "Guide", "#ffc0c000", "#ffffff00"),
        ("schematic_outlines", "Outlines", "#ff9e0000", "#ffff0000"),
        (
            "schematic_grab_areas",
            "Grab Areas",
            "#ff393939",
            "#ff535353",
        ),
        (
            "schematic_hidden_grab_areas",
            "Hidden Grab Areas",
            "#28ffffff",
            "#39ffffff",
        ),
        ("schematic_names", "Names", "#ffdeddda", "#ffffffff"),
        ("schematic_values", "Values", "#ffc0c0c0", "#ffffffff"),
        (
            "schematic_optional_pins",
            "Optional Pins",
            "#ff009a00",
            "#ff00ff00",
        ),
        (
            "schematic_required_pins",
            "Required Pins",
            "#ff9e0000",
            "#ffff0000",
        ),
        ("schematic_pin_lines", "Pin Lines", "#ff9e0000", "#ffff0000"),
        ("schematic_pin_names", "Pin Names", "#ffe3e3e3", "#ffffffff"),
        (
            "schematic_pin_numbers",
            "Pin Numbers",
            "#ff9e0000",
            "#ffff0000",
        ),
    ],
    inner_copper: &[],
};

/// Upstream `BaseColorScheme::schematicSolarizedDark()`.
pub const SCHEMATIC_SOLARIZED: BaseScheme = BaseScheme {
    uuid: "63bb162f-032d-4aeb-bef9-9546d8e15eaa",
    name: "Solarized Dark",
    colors: &[
        (
            "schematic_background",
            "Background/Grid",
            "#ff002b36",
            "#ff5b5b5b",
        ),
        ("schematic_overlays", "Overlays", "#78000000", "#ffffff00"),
        ("schematic_info_box", "Info Box", "#82000000", "#ffffff00"),
        ("schematic_selection", "Selection", "#846c71c4", "#506c71c4"),
        (
            "schematic_references",
            "References",
            "#72657b83",
            "#ffeee8d5",
        ),
        ("schematic_frames", "Frames", "#ff839496", "#ffeee8d5"),
        ("schematic_wires", "Wires", "#ff859900", "#ffdeff00"),
        (
            "schematic_net_labels",
            "Net Labels",
            "#ff859900",
            "#ffdeff00",
        ),
        ("schematic_buses", "Buses", "#ff268bd2", "#ff00ffff"),
        (
            "schematic_bus_labels",
            "Bus Labels",
            "#ff268bd2",
            "#ff00ffff",
        ),
        (
            "schematic_image_borders",
            "Image Borders",
            "#ff657b83",
            "#ffeee8d5",
        ),
        (
            "schematic_documentation",
            "Documentation",
            "#ffc0bcaf",
            "#ffffffff",
        ),
        ("schematic_comments", "Comments", "#ffd33682", "#ffff88c1"),
        ("schematic_guide", "Guide", "#ff2aa198", "#ff00ffec"),
        ("schematic_outlines", "Outlines", "#ffcb4b16", "#ffff7e49"),
        (
            "schematic_grab_areas",
            "Grab Areas",
            "#ff134653",
            "#ff3d5b65",
        ),
        (
            "schematic_hidden_grab_areas",
            "Hidden Grab Areas",
            "#79586e75",
            "#bb586e75",
        ),
        ("schematic_names", "Names", "#ff93a1a1", "#fffdf6e3"),
        ("schematic_values", "Values", "#ff839496", "#fffdf6e3"),
        (
            "schematic_optional_pins",
            "Optional Pins",
            "#ff859900",
            "#ff00ff00",
        ),
        (
            "schematic_required_pins",
            "Required Pins",
            "#ffdc322f",
            "#ffff0000",
        ),
        ("schematic_pin_lines", "Pin Lines", "#ffcb4b16", "#ffff7e49"),
        ("schematic_pin_names", "Pin Names", "#ffb58900", "#fffcff69"),
        (
            "schematic_pin_numbers",
            "Pin Numbers",
            "#ff93a1a1",
            "#fffdf6e3",
        ),
    ],
    inner_copper: &[],
};

/// Upstream `BaseColorScheme::boardLibrePcbDark()`.
pub const BOARD_DARK: BaseScheme = BaseScheme {
    uuid: "c605f278-5210-472e-a59f-1b73a15aee2d",
    name: "LibrePCB Dark",
    colors: &[
        (
            "board_background",
            "Background/Grid",
            "#ff000000",
            "#ffa0a0a4",
        ),
        ("board_overlays", "Overlays", "#78000000", "#ffffff00"),
        ("board_info_box", "Info Box", "#82000000", "#ffffff00"),
        ("board_drc_marker", "DRC Marker", "#00000000", "#ffff7f00"),
        ("board_selection", "Selection", "#ff78aaff", "#5096c8ff"),
        ("board_frames", "Frames", "#96e0e0e0", "#ffffffff"),
        ("board_outlines", "Outlines", "#c8ffffff", "#ffffffff"),
        (
            "board_plated_cutouts",
            "Plated Cutouts",
            "#c800ddff",
            "#ff00ffff",
        ),
        ("board_holes", "Holes", "#c8ffffff", "#ffffffff"),
        ("board_pads", "Pads", "#966db515", "#b44efc14"),
        ("board_vias", "Vias", "#966db515", "#b44efc14"),
        ("board_zones", "Zones", "#80494949", "#a0666666"),
        ("board_airwires", "Air Wires", "#ffffff00", "#ffffff00"),
        ("board_measures", "Measures", "#ff808000", "#ffa3b200"),
        ("board_alignment", "Alignment", "#b4e59500", "#dcffbf00"),
        (
            "board_documentation",
            "Documentation",
            "#76fbc697",
            "#b6fbc697",
        ),
        ("board_comments", "Comments", "#b4e59500", "#dcffbf00"),
        ("board_guide", "Guide", "#ff808000", "#ffa3b200"),
        ("board_names_top", "Names Top", "#96edffd8", "#dce0e0e0"),
        (
            "board_names_bottom",
            "Names Bottom",
            "#96edffd8",
            "#dce0e0e0",
        ),
        ("board_values_top", "Values Top", "#96d8f2ff", "#dce0e0e0"),
        (
            "board_values_bottom",
            "Values Bottom",
            "#96d8f2ff",
            "#dce0e0e0",
        ),
        ("board_legend_top", "Legend Top", "#bbffffff", "#ffffffff"),
        (
            "board_legend_bottom",
            "Legend Bottom",
            "#bbffffff",
            "#ffffffff",
        ),
        (
            "board_documentation_top",
            "Documentation Top",
            "#76fbc697",
            "#b6fbc697",
        ),
        (
            "board_documentation_bottom",
            "Documentation Bottom",
            "#76fbc697",
            "#b6fbc697",
        ),
        (
            "board_package_outlines_top",
            "Package Outlines Top",
            "#c000ffff",
            "#ff00ffff",
        ),
        (
            "board_package_outlines_bottom",
            "Package Outlines Bottom",
            "#c000ffff",
            "#ff00ffff",
        ),
        (
            "board_courtyard_top",
            "Courtyard Top",
            "#c0ff00ff",
            "#ffff00ff",
        ),
        (
            "board_courtyard_bottom",
            "Courtyard Bottom",
            "#c0ff00ff",
            "#ffff00ff",
        ),
        (
            "board_grab_areas_top",
            "Grab Areas Top",
            "#14ffffff",
            "#32ffffff",
        ),
        (
            "board_grab_areas_bottom",
            "Grab Areas Bottom",
            "#14ffffff",
            "#32ffffff",
        ),
        (
            "board_hidden_grab_areas_top",
            "Hidden Grab Areas Top",
            "#28ffffff",
            "#46ffffff",
        ),
        (
            "board_hidden_grab_areas_bottom",
            "Hidden Grab Areas Bottom",
            "#28ffffff",
            "#46ffffff",
        ),
        (
            "board_references_top",
            "References Top",
            "#64ffffff",
            "#b4ffffff",
        ),
        (
            "board_references_bottom",
            "References Bottom",
            "#64ffffff",
            "#b4ffffff",
        ),
        (
            "board_stop_mask_top",
            "Stop Mask Top",
            "#30ffffff",
            "#60ffffff",
        ),
        (
            "board_stop_mask_bottom",
            "Stop Mask Bottom",
            "#30ffffff",
            "#60ffffff",
        ),
        (
            "board_solder_paste_top",
            "Solder Paste Top",
            "#20e0e0e0",
            "#40e0e0e0",
        ),
        (
            "board_solder_paste_bottom",
            "Solder Paste Bottom",
            "#20e0e0e0",
            "#40e0e0e0",
        ),
        ("board_finish_top", "Finish Top", "#82ff0000", "#82ff0000"),
        (
            "board_finish_bottom",
            "Finish Bottom",
            "#82ff0000",
            "#82ff0000",
        ),
        ("board_glue_top", "Glue Top", "#64e0e0e0", "#78e0e0e0"),
        ("board_glue_bottom", "Glue Bottom", "#64e0e0e0", "#78e0e0e0"),
        ("board_copper_top", "Copper Top", "#96cc0802", "#c0ff0800"),
        // Inner copper layers follow (see `inner_copper`).
        (
            "board_copper_bottom",
            "Copper Bottom",
            "#964578cc",
            "#c00a66fc",
        ),
    ],
    inner_copper: &[
        ("#96cc57ff", "#c0da84ff"),
        ("#96e50063", "#c0e50063"),
        ("#96ee5c9b", "#c0ff4c99"),
        ("#96e2a1ff", "#c0e9baff"),
        ("#96a70049", "#c0cc0058"),
        ("#967b20a3", "#c09739bf"),
    ],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_scheme_uuids() {
        for kind in [SchemeKind::Schematic, SchemeKind::Board] {
            for base in kind.bases() {
                assert!(base.uuid.parse::<Uuid>().is_ok(), "{}", base.uuid);
            }
        }
        let roles = BOARD_DARK.roles();
        assert_eq!(roles.len(), BOARD_DARK.colors.len() + INNER_COPPER_COUNT);
        assert_eq!(roles.last().unwrap().role, "board_copper_bottom");
        assert_eq!(roles[roles.len() - 2].role, "board_copper_inner_62");
    }

    #[test]
    fn color_conversions() {
        assert_eq!(parse_argb("#80ff0000"), Some(0x80ff_0000));
        assert_eq!(parse_argb("#ff0000"), Some(0xffff_0000));
        assert_eq!(format_argb(0x80ff_0000), "#80ff0000");
        assert_eq!(format_rgba(0x80ff_0000), "#ff000080");
        assert_eq!(format_rgba(0xffff_0000), "#ff0000");
        assert_eq!(parse_rgba("#ff000080"), Some(0x80ff_0000));
        assert_eq!(parse_rgba("#00ff00"), Some(0xff00_ff00));
        let (h, s, v, a) = to_hsva(0xff00_ff00);
        assert_eq!(from_hsva(h, s, v, a), 0xff00_ff00);
        assert!((h - 1.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn user_schemes_roundtrip() {
        let mut schemes = ColorSchemes::new(SchemeKind::Schematic);
        assert_eq!(schemes.to_raw(), RawSettings::default());
        let uuid = schemes.duplicate_active();
        assert_eq!(schemes.active_name(), "Copy of LibrePCB Light");
        schemes.set_color(uuid, "schematic_wires", false, Some(0xff12_3456));
        let colors = schemes.colors(uuid);
        let wires = colors.iter().find(|c| c.role == "schematic_wires").unwrap();
        assert_eq!(wires.primary, 0xff12_3456);
        assert!(wires.primary_overridden && !wires.secondary_overridden);
        let raw = schemes.to_raw();
        let mut root = List::new("schematic_color_schemes");
        for n in &raw.0 {
            root.push(n.clone());
        }
        let text = String::from_utf8(
            SExpression::List(root)
                .to_byte_array(librepcb_core::serialization::Mode::LibrePcb)
                .unwrap(),
        )
        .unwrap();
        assert!(
            text.contains(&format!("(active {uuid} \"Copy of LibrePCB Light\")")),
            "{text}"
        );
        assert!(
            text.contains("(base 9121eabe-55b3-4a7c-bffe-20115b8ad314 \"LibrePCB Light\")"),
            "{text}"
        );
        assert!(
            text.contains("(color schematic_wires (primary \"#ff123456\"))"),
            "{text}"
        );
        let loaded = ColorSchemes::load(SchemeKind::Schematic, &raw);
        assert_eq!(loaded.to_raw(), raw);
        assert_eq!(loaded.colors(uuid), colors);
        let scene = loaded.scene_scheme();
        assert_eq!(
            scene.color("schematic_wires"),
            Some(librepcb_canvas::peniko::Color::from_rgba8(
                0x12, 0x34, 0x56, 0xff
            ))
        );
        let mut removed = loaded.clone();
        removed.remove(uuid);
        assert_eq!(removed.to_raw(), RawSettings::default());
    }
}
