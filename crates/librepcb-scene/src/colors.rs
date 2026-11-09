//! Port of the default color schemes of
//! libs/librepcb/core/workspace/basecolorscheme.{h,cpp} (and the role
//! identifiers of `colorrole.{h,cpp}`).
//!
//! Only the primary colors are ported (the secondary colors are highlight
//! colors of the interactive editor): the schematic schemes "LibrePCB Light"
//! (upstream default) and "LibrePCB Dark", and the board scheme "LibrePCB
//! Dark" (upstream default). Colors are looked up by the color role
//! identifier, which is also what [`Layer::color_role()`] returns. Custom
//! schemes ([`ColorScheme::custom()`]) carry the colors of graphics exports.
//!
//! [`Layer::color_role()`]: librepcb_core::types::Layer::color_role

use std::borrow::Cow;

use librepcb_canvas::peniko::Color;

/// A color scheme: color role identifier → color.
///
/// Either one of upstream's default schemes (the constants) or a custom set
/// of colors ([`ColorScheme::custom()`], e.g. those of a graphics export).
#[derive(Debug, Clone, PartialEq)]
pub struct ColorScheme {
    name: Cow<'static, str>,
    colors: Colors,
}

#[derive(Debug, Clone, PartialEq)]
enum Colors {
    Builtin {
        colors: &'static [(&'static str, u32)],
        inner_copper: &'static [u32],
    },
    Custom(Vec<(String, Color)>),
}

impl ColorScheme {
    /// Upstream `schematicLibrePcbLight()` (the default schematic scheme).
    pub const SCHEMATIC_LIGHT: Self = Self::builtin("LibrePCB Light", SCHEMATIC_LIGHT, &[]);

    /// Upstream `schematicLibrePcbDark()`.
    pub const SCHEMATIC_DARK: Self = Self::builtin("LibrePCB Dark", SCHEMATIC_DARK, &[]);

    /// Upstream `boardLibrePcbDark()` (the default board scheme).
    pub const BOARD_DARK: Self = Self::builtin("LibrePCB Dark", BOARD_DARK, BOARD_DARK_INNER);

    const fn builtin(
        name: &'static str,
        colors: &'static [(&'static str, u32)],
        inner_copper: &'static [u32],
    ) -> Self {
        Self {
            name: Cow::Borrowed(name),
            colors: Colors::Builtin {
                colors,
                inner_copper,
            },
        }
    }

    /// A custom scheme; roles which are not contained have no color (their
    /// items are not drawn).
    pub fn custom(
        name: impl Into<String>,
        colors: impl IntoIterator<Item = (String, Color)>,
    ) -> Self {
        Self {
            name: Cow::Owned(name.into()),
            colors: Colors::Custom(colors.into_iter().collect()),
        }
    }

    /// The (untranslated) name of the scheme.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The primary color of a color role (e.g. `"board_copper_top"`), `None`
    /// if the scheme does not define the role.
    pub fn color(&self, role: &str) -> Option<Color> {
        match &self.colors {
            Colors::Custom(colors) => colors.iter().find(|(r, _)| r == role).map(|(_, c)| *c),
            Colors::Builtin {
                colors,
                inner_copper,
            } => {
                if let Some(number) = role.strip_prefix("board_copper_inner_") {
                    let n: usize = number.parse().ok()?;
                    if n == 0 || inner_copper.is_empty() {
                        return None;
                    }
                    // Upstream repeats the inner layer colors cyclically.
                    return Some(argb(inner_copper[(n - 1) % inner_copper.len()]));
                }
                colors
                    .iter()
                    .find(|(r, _)| *r == role)
                    .map(|(_, c)| argb(*c))
            }
        }
    }

    /// Like [`color()`](Self::color), but transparent for unknown roles.
    pub fn color_or_transparent(&self, role: &str) -> Color {
        self.color(role).unwrap_or(Color::TRANSPARENT)
    }
}

/// Converts upstream's `#AARRGGBB` notation.
fn argb(v: u32) -> Color {
    let [a, r, g, b] = v.to_be_bytes();
    Color::from_rgba8(r, g, b, a)
}

const SCHEMATIC_LIGHT: &[(&str, u32)] = &[
    ("schematic_background", 0xffffffff),
    ("schematic_overlays", 0x78ffffff),
    ("schematic_info_box", 0x82ffffff),
    ("schematic_selection", 0xff78aaff),
    ("schematic_references", 0x32000000),
    ("schematic_frames", 0xff000000),
    ("schematic_wires", 0xff008000),
    ("schematic_net_labels", 0xff008000),
    ("schematic_buses", 0xff008eff),
    ("schematic_bus_labels", 0xff008eff),
    ("schematic_image_borders", 0xff808080),
    ("schematic_documentation", 0xff808080),
    ("schematic_comments", 0xff000080),
    ("schematic_guide", 0xff808000),
    ("schematic_outlines", 0xff800000),
    ("schematic_grab_areas", 0xffffffe1),
    ("schematic_hidden_grab_areas", 0x1e0000ff),
    ("schematic_names", 0xff202020),
    ("schematic_values", 0xff505050),
    ("schematic_optional_pins", 0xff00ff00),
    ("schematic_required_pins", 0xffff0000),
    ("schematic_pin_lines", 0xff800000),
    ("schematic_pin_names", 0xff404040),
    ("schematic_pin_numbers", 0xff404040),
];

const SCHEMATIC_DARK: &[(&str, u32)] = &[
    ("schematic_background", 0xff161616),
    ("schematic_overlays", 0x78000000),
    ("schematic_info_box", 0x82000000),
    ("schematic_selection", 0xff78aaff),
    ("schematic_references", 0x54ffffff),
    ("schematic_frames", 0xff9e0000),
    ("schematic_wires", 0xff009a00),
    ("schematic_net_labels", 0xff009a00),
    ("schematic_buses", 0xff0068ba),
    ("schematic_bus_labels", 0xff0068ba),
    ("schematic_image_borders", 0xff5b5b5b),
    ("schematic_documentation", 0xffc0bfbc),
    ("schematic_comments", 0xffb67700),
    ("schematic_guide", 0xffc0c000),
    ("schematic_outlines", 0xff9e0000),
    ("schematic_grab_areas", 0xff393939),
    ("schematic_hidden_grab_areas", 0x28ffffff),
    ("schematic_names", 0xffdeddda),
    ("schematic_values", 0xffc0c0c0),
    ("schematic_optional_pins", 0xff009a00),
    ("schematic_required_pins", 0xff9e0000),
    ("schematic_pin_lines", 0xff9e0000),
    ("schematic_pin_names", 0xffe3e3e3),
    ("schematic_pin_numbers", 0xff9e0000),
];

const BOARD_DARK: &[(&str, u32)] = &[
    ("board_background", 0xff000000),
    ("board_overlays", 0x78000000),
    ("board_info_box", 0x82000000),
    ("board_drc_marker", 0x00000000),
    ("board_selection", 0xff78aaff),
    ("board_frames", 0x96e0e0e0),
    ("board_outlines", 0xc8ffffff),
    ("board_plated_cutouts", 0xc800ddff),
    ("board_holes", 0xc8ffffff),
    ("board_pads", 0x966db515),
    ("board_vias", 0x966db515),
    ("board_zones", 0x80494949),
    ("board_airwires", 0xffffff00),
    ("board_measures", 0xff808000),
    ("board_alignment", 0xb4e59500),
    ("board_documentation", 0x76fbc697),
    ("board_comments", 0xb4e59500),
    ("board_guide", 0xff808000),
    ("board_names_top", 0x96edffd8),
    ("board_names_bottom", 0x96edffd8),
    ("board_values_top", 0x96d8f2ff),
    ("board_values_bottom", 0x96d8f2ff),
    ("board_legend_top", 0xbbffffff),
    ("board_legend_bottom", 0xbbffffff),
    ("board_documentation_top", 0x76fbc697),
    ("board_documentation_bottom", 0x76fbc697),
    ("board_package_outlines_top", 0xc000ffff),
    ("board_package_outlines_bottom", 0xc000ffff),
    ("board_courtyard_top", 0xc0ff00ff),
    ("board_courtyard_bottom", 0xc0ff00ff),
    ("board_grab_areas_top", 0x14ffffff),
    ("board_grab_areas_bottom", 0x14ffffff),
    ("board_hidden_grab_areas_top", 0x28ffffff),
    ("board_hidden_grab_areas_bottom", 0x28ffffff),
    ("board_references_top", 0x64ffffff),
    ("board_references_bottom", 0x64ffffff),
    ("board_stop_mask_top", 0x30ffffff),
    ("board_stop_mask_bottom", 0x30ffffff),
    ("board_solder_paste_top", 0x20e0e0e0),
    ("board_solder_paste_bottom", 0x20e0e0e0),
    ("board_finish_top", 0x82ff0000),
    ("board_finish_bottom", 0x82ff0000),
    ("board_glue_top", 0x64e0e0e0),
    ("board_glue_bottom", 0x64e0e0e0),
    ("board_copper_top", 0x96cc0802),
    ("board_copper_bottom", 0x964578cc),
];

const BOARD_DARK_INNER: &[u32] = &[
    0x96cc57ff, 0x96e50063, 0x96ee5c9b, 0x96e2a1ff, 0x96a70049, 0x967b20a3,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup() {
        let s = ColorScheme::BOARD_DARK;
        assert_eq!(
            s.color("board_copper_top"),
            Some(Color::from_rgba8(0xcc, 0x08, 0x02, 0x96))
        );
        assert_eq!(
            s.color("board_copper_inner_1"),
            s.color("board_copper_inner_7")
        );
        assert_eq!(s.color("board_copper_inner_0"), None);
        assert_eq!(s.color("nope"), None);
        assert_eq!(
            ColorScheme::SCHEMATIC_LIGHT.color("schematic_background"),
            Some(Color::WHITE)
        );
    }
}
