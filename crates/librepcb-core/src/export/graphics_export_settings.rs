//! Port of libs/librepcb/core/export/graphicsexportsettings.{h,cpp}.
//!
//! Plain data only (used by the graphics output job); the painting related
//! methods `getFillColor()` and `convertImageColors()` are not ported.
//!
//! Differences to upstream:
//! - The page size is stored as its `QPageSize` key (e.g. `"A4"`), like
//!   the graphics output job stores it in files, instead of a `QPageSize`.
//! - Color schemes (`ColorScheme`, `ColorRole` of `core/workspace`) are not
//!   available in core yet, so [`GraphicsExportSettings::load_default_colors()`]
//!   replaces `loadColorsFromScheme()` with the two schemes upstream's
//!   constructor uses (`BaseColorScheme::schematicLibrePcbLight()` and
//!   `boardLibrePcbDark()`), whose primary colors are embedded here.
//! - The color adjustment of board layers for white backgrounds
//!   (`QColor::hsvHue()`, `fromHsv()`, ...) is ported from Qt on purpose:
//!   the resulting colors are written to `jobs.lp` by the default graphics
//!   output jobs, so they must be identical.

use std::fmt;
use std::str::FromStr;

use crate::serialization::{self, FromSExpression, SExpression, ToSExpression};
use crate::types::{self, Color, Layer, Length, UnsignedLength, UnsignedRatio};

/// Page orientation of graphics exports (upstream
/// `GraphicsExportSettings::Orientation`).
///
/// Serde: the file format token (`"landscape"`, `"portrait"`, `"auto"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PageOrientation {
    /// Landscape.
    Landscape,
    /// Portrait.
    Portrait,
    /// Chosen automatically depending on the content.
    #[default]
    Auto,
}

impl PageOrientation {
    /// Returns the file format token.
    pub fn to_str(self) -> &'static str {
        match self {
            Self::Landscape => "landscape",
            Self::Portrait => "portrait",
            Self::Auto => "auto",
        }
    }
}

impl fmt::Display for PageOrientation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.to_str())
    }
}

impl FromStr for PageOrientation {
    type Err = types::Error;
    fn from_str(s: &str) -> Result<Self, types::Error> {
        [Self::Landscape, Self::Portrait, Self::Auto]
            .into_iter()
            .find(|v| v.to_str() == s)
            .ok_or_else(|| types::Error::InvalidPageOrientation(s.to_owned()))
    }
}

crate::utils::serde_string::serde_string!(PageOrientation);

impl ToSExpression for PageOrientation {
    fn to_sexpression(&self) -> SExpression {
        SExpression::token(self.to_str())
    }
}

impl FromSExpression for PageOrientation {
    fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
        Ok(node.value()?.parse()?)
    }
}

/// Serializes an optional scale (`None` = fit into page) as `auto` or the
/// ratio (upstream `serialize<std::optional<UnsignedRatio>>()`).
pub(crate) fn scale_to_sexpression(scale: Option<UnsignedRatio>) -> SExpression {
    match scale {
        Some(scale) => scale.to_sexpression(),
        None => SExpression::token("auto"),
    }
}

/// Parses an optional scale written by [`scale_to_sexpression()`].
pub(crate) fn scale_from_sexpression(
    node: &SExpression,
) -> serialization::Result<Option<UnsignedRatio>> {
    if node.value()? == "auto" {
        Ok(None)
    } else {
        UnsignedRatio::from_sexpression(node).map(Some)
    }
}

/// Settings of a graphics (PDF/SVG/image) export.
///
/// Serde: an object with the fields below.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GraphicsExportSettings {
    /// Page size key (e.g. `"A4"`), `None` = automatic (fit to content).
    pub page_size: Option<String>,
    /// Page orientation.
    pub orientation: PageOrientation,
    /// Left page margin.
    pub margin_left: UnsignedLength,
    /// Top page margin.
    pub margin_top: UnsignedLength,
    /// Right page margin.
    pub margin_right: UnsignedLength,
    /// Bottom page margin.
    pub margin_bottom: UnsignedLength,
    /// Whether the content is rotated by 90°.
    pub rotate: bool,
    /// Whether the content is mirrored.
    pub mirror: bool,
    /// Scale factor, `None` = fit into page.
    pub scale: Option<UnsignedRatio>,
    /// Resolution of pixmap exports.
    pub pixmap_dpi: u32,
    /// Whether everything is exported in black/white.
    pub black_white: bool,
    /// Background color.
    pub background_color: Color,
    /// Minimum line width.
    pub min_line_width: UnsignedLength,
    /// Colors per color role ID (e.g. `"board_outlines"`), in reverse paint
    /// order (the first one is painted on top).
    pub colors: Vec<(String, Color)>,
}

/// Creates an unsigned length from a positive nanometer constant.
fn unsigned_nm(nm: i64) -> UnsignedLength {
    // Only called with positive constants.
    UnsignedLength::new(Length::new(nm)).expect("positive constant")
}

impl Default for GraphicsExportSettings {
    /// Upstream default constructor: automatic page size and orientation,
    /// 10mm margins, fit into page, 600 DPI, transparent background and the
    /// default colors of all schematic and board layers.
    fn default() -> Self {
        let mut settings = Self {
            page_size: None,
            orientation: PageOrientation::Auto,
            margin_left: unsigned_nm(10_000_000), // 10mm
            margin_top: unsigned_nm(10_000_000),
            margin_right: unsigned_nm(10_000_000),
            margin_bottom: unsigned_nm(10_000_000),
            rotate: false,
            mirror: false,
            scale: None,
            pixmap_dpi: 600,
            black_white: false,
            background_color: Color::TRANSPARENT,
            min_line_width: unsigned_nm(100_000),
            colors: Vec::new(),
        };
        settings.load_default_colors(true, true, Layer::INNER_COPPER_COUNT);
        settings
    }
}

impl GraphicsExportSettings {
    /// Returns the color role IDs in paint order (bottom first).
    pub fn paint_order(&self) -> Vec<&str> {
        self.colors
            .iter()
            .rev()
            .map(|(role, _)| role.as_str())
            .collect()
    }

    /// Returns the color of a role, `None` if it shall not be painted. In
    /// black/white mode, the color is white on black background and black
    /// otherwise.
    pub fn color(&self, role: &str) -> Option<Color> {
        let color = self
            .colors
            .iter()
            .find(|(r, _)| r == role)
            .map(|(_, c)| *c)?;
        if self.black_white {
            Some(if self.background_color == Color::BLACK {
                Color::WHITE
            } else {
                Color::BLACK
            })
        } else {
            Some(color)
        }
    }

    /// Replaces the colors by those of the default color schemes (upstream
    /// `loadColorsFromScheme()` with `BaseColorScheme::schematicLibrePcbLight()`
    /// and `boardLibrePcbDark()`): schematic layers if `schematic`, board
    /// layers (adjusted for white background) if `board`, with
    /// `inner_layer_count` inner copper layers.
    pub fn load_default_colors(&mut self, schematic: bool, board: bool, inner_layer_count: usize) {
        self.colors.clear();
        if schematic {
            for role in SCHEMATIC_ROLES {
                let color = schematic_light_color(role);
                self.colors.push(((*role).to_owned(), color));
            }
        }
        if board {
            let mut add = |role: String| {
                let color = adjust_for_white_background(board_dark_color(&role));
                self.colors.push((role, color));
            };
            for role in BOARD_ROLES_BEFORE_INNER {
                add((*role).to_owned());
            }
            for i in 1..=inner_layer_count.min(Layer::INNER_COPPER_COUNT) {
                add(board_copper_inner_id(i));
            }
            for role in BOARD_ROLES_AFTER_INNER {
                add((*role).to_owned());
            }
        }
    }

    /// Replaces the colors by the few layers used for realistic board
    /// rendering (upstream `loadBoardRenderingColors()`). Transparent stop
    /// mask and legend colors mean to take the colors of the board.
    pub fn load_board_rendering_colors(&mut self, inner_layer_count: usize) {
        const COPPER: Color = Color::rgb(188, 156, 105);
        const PASTE: Color = Color::rgb(128, 128, 128); // Qt::darkGray
        const GLUE: Color = Color::rgba(200, 50, 50, 80);
        self.colors.clear();
        let mut add = |role: &str, color: Color| self.colors.push((role.to_owned(), color));
        add("board_outlines", Color::rgb(70, 80, 70));
        add("board_copper_top", COPPER);
        add("board_stop_mask_top", Color::TRANSPARENT);
        add("board_legend_top", Color::TRANSPARENT);
        add("board_solder_paste_top", PASTE);
        add("board_glue_top", GLUE);
        for i in 1..=inner_layer_count.min(Layer::INNER_COPPER_COUNT) {
            add(&board_copper_inner_id(i), COPPER);
        }
        add("board_copper_bottom", COPPER);
        add("board_stop_mask_bottom", Color::TRANSPARENT);
        add("board_legend_bottom", Color::TRANSPARENT);
        add("board_solder_paste_bottom", PASTE);
        add("board_glue_bottom", GLUE);
    }
}

/// Upstream `ColorRole::boardCopperInnerId()`.
fn board_copper_inner_id(number: usize) -> String {
    format!("board_copper_inner_{number}")
}

/// Schematic color roles in the order of `loadColorsFromScheme()`.
const SCHEMATIC_ROLES: &[&str] = &[
    "schematic_frames",
    "schematic_outlines",
    "schematic_grab_areas",
    "schematic_pin_lines",
    "schematic_pin_names",
    "schematic_pin_numbers",
    "schematic_names",
    "schematic_values",
    "schematic_wires",
    "schematic_net_labels",
    "schematic_buses",
    "schematic_bus_labels",
    "schematic_image_borders",
    "schematic_documentation",
    "schematic_comments",
    "schematic_guide",
];

/// Board color roles in the order of `loadColorsFromScheme()`, before the
/// inner copper layers.
const BOARD_ROLES_BEFORE_INNER: &[&str] = &[
    // Asymmetric board layers.
    "board_guide",
    "board_comments",
    "board_documentation",
    "board_alignment",
    "board_measures",
    "board_frames",
    "board_airwires",
    "board_outlines",
    "board_holes",
    "board_plated_cutouts",
    "board_pads",
    "board_vias",
    // Symmetric board layers in logical order.
    "board_documentation_top",
    "board_names_top",
    "board_values_top",
    "board_courtyard_top",
    "board_grab_areas_top",
    "board_legend_top",
    "board_glue_top",
    "board_solder_paste_top",
    "board_stop_mask_top",
    "board_copper_top",
];

/// Board color roles in the order of `loadColorsFromScheme()`, after the
/// inner copper layers.
const BOARD_ROLES_AFTER_INNER: &[&str] = &[
    "board_copper_bottom",
    "board_stop_mask_bottom",
    "board_solder_paste_bottom",
    "board_glue_bottom",
    "board_legend_bottom",
    "board_grab_areas_bottom",
    "board_courtyard_bottom",
    "board_values_bottom",
    "board_names_bottom",
    "board_documentation_bottom",
];

/// Creates a color from `0xAARRGGBB`.
const fn argb(v: u32) -> Color {
    let [a, r, g, b] = v.to_be_bytes();
    Color::rgba(r, g, b, a)
}

/// Primary color of a role in `BaseColorScheme::schematicLibrePcbLight()`
/// (only the roles of [`SCHEMATIC_ROLES`]).
fn schematic_light_color(role: &str) -> Color {
    match role {
        "schematic_frames" => argb(0xff000000),
        "schematic_wires" | "schematic_net_labels" => argb(0xff008000),
        "schematic_buses" | "schematic_bus_labels" => argb(0xff008eff),
        "schematic_image_borders" | "schematic_documentation" => argb(0xff808080),
        "schematic_comments" => argb(0xff000080),
        "schematic_guide" => argb(0xff808000),
        "schematic_outlines" | "schematic_pin_lines" => argb(0xff800000),
        "schematic_grab_areas" => argb(0xffffffe1),
        "schematic_names" => argb(0xff202020),
        "schematic_values" => argb(0xff505050),
        "schematic_pin_names" | "schematic_pin_numbers" => argb(0xff404040),
        _ => Color::TRANSPARENT,
    }
}

/// Primary color of a role in `BaseColorScheme::boardLibrePcbDark()` (only
/// the roles loaded by [`GraphicsExportSettings::load_default_colors()`]).
fn board_dark_color(role: &str) -> Color {
    /// Colors of the inner copper layers, repeated cyclically.
    const INNER: [u32; 6] = [
        0x96cc57ff, 0x96e50063, 0x96ee5c9b, 0x96e2a1ff, 0x96a70049, 0x967b20a3,
    ];
    if let Some(n) = role
        .strip_prefix("board_copper_inner_")
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| *n >= 1)
    {
        return argb(INNER[(n - 1) % INNER.len()]);
    }
    argb(match role {
        "board_frames" => 0x96e0e0e0,
        "board_outlines" | "board_holes" => 0xc8ffffff,
        "board_plated_cutouts" => 0xc800ddff,
        "board_pads" | "board_vias" => 0x966db515,
        "board_airwires" => 0xffffff00,
        "board_measures" | "board_guide" => 0xff808000,
        "board_alignment" | "board_comments" => 0xb4e59500,
        "board_documentation" | "board_documentation_top" | "board_documentation_bottom" => {
            0x76fbc697
        }
        "board_names_top" | "board_names_bottom" => 0x96edffd8,
        "board_values_top" | "board_values_bottom" => 0x96d8f2ff,
        "board_legend_top" | "board_legend_bottom" => 0xbbffffff,
        "board_courtyard_top" | "board_courtyard_bottom" => 0xc0ff00ff,
        "board_grab_areas_top" | "board_grab_areas_bottom" => 0x14ffffff,
        "board_stop_mask_top" | "board_stop_mask_bottom" => 0x30ffffff,
        "board_solder_paste_top" | "board_solder_paste_bottom" => 0x20e0e0e0,
        "board_glue_top" | "board_glue_bottom" => 0x64e0e0e0,
        "board_copper_top" => 0x96cc0802,
        "board_copper_bottom" => 0x964578cc,
        _ => 0x00000000,
    })
}

/// Makes a board layer color look better on white background (upstream
/// lambda in `loadColorsFromScheme()`): half the HSV value, alpha mapped to
/// 127..254.
///
/// Ported from `QColor::hsvHue()`, `hsvSaturation()`, `value()`,
/// `QColor::fromHsv()` and `QColor::toRgb()` (Qt 6, single precision
/// floats, 16 bit channels) because the resulting colors are written to
/// files and must be identical.
fn adjust_for_white_background(color: Color) -> Color {
    let (hue, saturation, value) = qt_rgb_to_hsv(color);
    let h = hue.map(|h| h / 100); // hsvHue(), None = -1 (achromatic)
    let s = qt_div_257(saturation); // hsvSaturation()
    let v = qt_div_257(value) / 2; // value() / 2: avoid white colors
    let a = u32::from(color.a) / 2 + 127; // avoid transparent colors
    qt_hsv_to_rgb(h, s, v, a)
}

/// Qt's conversion of a 16 bit color channel to 8 bits (`qt_div_257()`,
/// used by `QColor::red()`, `hsvSaturation()`, ...).
fn qt_div_257(x: u32) -> u32 {
    (x - (x >> 8) + 0x80) >> 8
}

/// Upstream `qRound()` for non-negative floats.
fn q_round(value: f32) -> u32 {
    // Only called with values in 0..=65535 (or 0..=36000), so the cast
    // cannot truncate.
    (value + 0.5) as u32
}

/// `qFuzzyCompare(float, float)`.
fn q_fuzzy_compare(a: f32, b: f32) -> bool {
    (a - b).abs() * 100_000.0 <= a.abs().min(b.abs())
}

/// `QColor::toHsv()`: returns the hue in 1/100 degrees (`None` if
/// achromatic), the saturation and the value, each with 16 bits.
fn qt_rgb_to_hsv(color: Color) -> (Option<u32>, u32, u32) {
    let channel = |c: u8| f32::from(u16::from(c) * 0x101) / 65535.0;
    let (r, g, b) = (channel(color.r), channel(color.g), channel(color.b));
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let value = q_round(max * 65535.0);
    if delta.abs() <= 0.00001 {
        // Achromatic case, hue is undefined.
        return (None, 0, value);
    }
    let saturation = q_round((delta / max) * 65535.0);
    let mut hue = if q_fuzzy_compare(r, max) {
        (g - b) / delta
    } else if q_fuzzy_compare(g, max) {
        2.0 + (b - r) / delta
    } else {
        4.0 + (r - g) / delta
    };
    hue *= 6000.0;
    if hue < 0.0 {
        hue += 36000.0;
    }
    (Some(q_round(hue)), saturation, value)
}

/// `QColor::fromHsv(h, s, v, a).toRgb()` with `h` in degrees (`None` =
/// achromatic) and `s`, `v`, `a` in 0..=255.
fn qt_hsv_to_rgb(h: Option<u32>, s: u32, v: u32, a: u32) -> Color {
    let hue = h.map(|h| (h % 360) * 100);
    let saturation = s * 0x101;
    let value = v * 0x101;
    // `qt_div_257()` of a 16 bit value is <= 255.
    let to8 = |v16: u32| qt_div_257(v16.min(65535)) as u8;
    let alpha = to8(a * 0x101);
    let hue = match hue {
        Some(hue) if saturation != 0 => hue,
        _ => {
            let c = to8(value);
            return Color::rgba(c, c, c, alpha);
        }
    };
    let h = if hue == 36000 {
        0.0
    } else {
        hue as f32 / 6000.0
    };
    let s = saturation as f32 / 65535.0;
    let v = value as f32 / 65535.0;
    let i = h as u32;
    let f = h - i as f32;
    let p = v * (1.0 - s);
    let (r, g, b) = if i & 1 == 1 {
        let q = v * (1.0 - (s * f));
        match i {
            1 => (q, v, p),
            3 => (p, q, v),
            _ => (v, p, q),
        }
    } else {
        let t = v * (1.0 - (s * (1.0 - f)));
        match i {
            0 => (v, t, p),
            2 => (p, v, t),
            _ => (t, p, v),
        }
    };
    let c = |x: f32| to8(q_round(x * 65535.0));
    Color::rgba(c(r), c(g), c(b), alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_background_adjustment() {
        // Colors as written by upstream into jobs.lp (tests/data/projects/v1).
        let cases = [
            (0xb4e59500, 0xd9724a00), // board_comments
            (0x96cc0802, 0xca660201), // board_copper_top
            (0x964578cc, 0xca223c66), // board_copper_bottom
            (0x76fbc697, 0xba7d624b), // board_documentation
            (0x96e0e0e0, 0xca707070), // board_frames
            (0x14ffffff, 0x897f7f7f), // board_grab_areas
            (0xff808000, 0xfe404000), // board_guide
            (0xc8ffffff, 0xe37f7f7f), // board_holes
            (0xbbffffff, 0xdc7f7f7f), // board_legend
            (0x96edffd8, 0xca767f6c), // board_names
            (0x966db515, 0xca365a0b), // board_pads
            (0xc800ddff, 0xe3006e7f), // board_plated_cutouts
            (0x96d8f2ff, 0xca6c797f), // board_values
        ];
        for (input, expected) in cases {
            assert_eq!(
                adjust_for_white_background(argb(input)),
                argb(expected),
                "{input:08x}"
            );
        }
    }

    #[test]
    fn default_colors() {
        let s = GraphicsExportSettings::default();
        assert_eq!(s.colors.len(), 16 + 22 + 62 + 10);
        assert_eq!(s.color("schematic_wires"), Some(argb(0xff008000)));
        assert_eq!(s.color("board_airwires"), Some(argb(0xfe7f7f00)));
        assert_eq!(s.color("foo"), None);
        assert_eq!(s.paint_order().first(), Some(&"board_documentation_bottom"));
    }

    #[test]
    fn orientation_tokens() {
        for o in [
            PageOrientation::Landscape,
            PageOrientation::Portrait,
            PageOrientation::Auto,
        ] {
            assert_eq!(o.to_string().parse::<PageOrientation>().unwrap(), o);
        }
        assert!("foo".parse::<PageOrientation>().is_err());
    }
}
