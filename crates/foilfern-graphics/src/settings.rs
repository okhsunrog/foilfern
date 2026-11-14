//! Port of libs/librepcb/core/export/graphicsexportsettings.{h,cpp}.
use crate::{Error, Result};
use foilfern_core::{
    export::{GraphicsExportSettings, PageOrientation},
    job::GraphicsContent,
    types::{Color, Layer},
};
use std::collections::BTreeMap;
/// Page and palette settings, in millimetres and dots per inch.
#[derive(Debug, Clone)]
pub struct GraphicsSettings {
    pub paper: Option<String>,
    pub orientation: PageOrientation,
    /// Left, top, right, bottom.
    pub margins: [f64; 4],
    pub rotate: bool,
    pub mirror: bool,
    pub scale: Option<f64>,
    pub dpi: u32,
    pub monochrome: bool,
    pub background: Color,
    pub min_line_width: f64,
    /// Absence means that a role must not be painted.
    pub colors: BTreeMap<String, Color>,
    /// Foreground first; painters traverse this in reverse for layer stacking.
    pub paint_order: Vec<String>,
}
impl Default for GraphicsSettings {
    fn default() -> Self {
        let mut colors = BTreeMap::new();
        let mut paint_order = Vec::new();
        for role in [
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
        ] {
            if let Some((_, value)) = crate::palette::SCHEMATIC.iter().find(|(r, _)| *r == role) {
                colors.insert(role.into(), *value);
                paint_order.push(role.into());
            }
        }
        let board = [
            "board_guide",
            "board_comments",
            "board_documentation",
            "board_alignment",
            "board_measures",
            "board_frames",
            "board_outlines",
            "board_holes",
            "board_plated_cutouts",
            "board_pads",
            "board_vias",
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
        let adjust = |c: Color| Color::rgba(c.r / 2, c.g / 2, c.b / 2, c.a / 2 + 127);
        for role in board {
            if let Some((_, value)) = crate::palette::BOARD.iter().find(|(r, _)| *r == role) {
                colors.insert(role.into(), adjust(*value));
                paint_order.push(role.into());
            }
        }
        for layer in Layer::all().iter().copied().filter(|l| l.is_inner()) {
            let values = crate::palette::INNER;
            if !values.is_empty() {
                colors.insert(
                    layer.color_role().into(),
                    adjust(values[(layer.copper_number() - 1) % values.len()]),
                );
                paint_order.push(layer.color_role().into());
            }
        }
        for role in [
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
        ] {
            if let Some((_, value)) = crate::palette::BOARD.iter().find(|(r, _)| *r == role) {
                colors.insert(role.into(), adjust(*value));
                paint_order.push(role.into());
            }
        }
        Self {
            paper: None,
            orientation: PageOrientation::Auto,
            margins: [10.; 4],
            rotate: false,
            mirror: false,
            scale: None,
            dpi: 600,
            monochrome: false,
            background: Color::rgba(0, 0, 0, 0),
            min_line_width: 0.1,
            colors,
            paint_order,
        }
    }
}
impl GraphicsSettings {
    pub fn from_content(c: &GraphicsContent) -> Self {
        let mut s = Self {
            paper: c.page_size.clone(),
            orientation: c.orientation,
            margins: [c.margin_left, c.margin_top, c.margin_right, c.margin_bottom]
                .map(|l| l.to_mm()),
            rotate: c.rotate,
            mirror: c.mirror,
            scale: c.scale.map(|r| r.to_normalized()),
            dpi: c.pixmap_dpi,
            monochrome: c.monochrome,
            background: c.background_color,
            min_line_width: c.min_line_width.to_mm(),
            ..Default::default()
        };
        s.colors = c.layers.clone();
        s.paint_order.retain(|r| s.colors.contains_key(r));
        s
    }
    pub fn from_export_settings(c: &GraphicsExportSettings) -> Self {
        Self {
            paper: c.page_size.clone(),
            orientation: c.orientation,
            margins: [c.margin_left, c.margin_top, c.margin_right, c.margin_bottom]
                .map(|l| l.to_mm()),
            rotate: c.rotate,
            mirror: c.mirror,
            scale: c.scale.map(|r| r.to_normalized()),
            dpi: c.pixmap_dpi,
            monochrome: c.black_white,
            background: c.background_color,
            min_line_width: c.min_line_width.to_mm(),
            colors: c.colors.iter().cloned().collect(),
            paint_order: c.colors.iter().map(|(role, _)| role.clone()).collect(),
        }
    }
    pub fn color(&self, role: &str) -> Option<Color> {
        self.colors.get(role).copied().map(|c| {
            if self.monochrome {
                if self.background == Color::BLACK {
                    Color::WHITE
                } else {
                    Color::BLACK
                }
            } else {
                c
            }
        })
    }
    pub fn fill(&self, role: &str, filled: bool, grab: bool) -> Option<Color> {
        if filled {
            return self.color(role);
        }
        if !grab {
            return None;
        }
        let role = match role {
            "schematic_outlines" => "schematic_grab_areas",
            "board_legend_top" => "board_grab_areas_top",
            "board_legend_bottom" => "board_grab_areas_bottom",
            _ => return None,
        };
        self.colors.get(role).copied().map(|c| {
            if self.monochrome {
                let gray =
                    ((u32::from(c.r) * 11 + u32::from(c.g) * 16 + u32::from(c.b) * 5) / 32) as u8;
                Color::rgba(gray, gray, gray, c.a)
            } else {
                c
            }
        })
    }
    pub(crate) fn paper_size(&self) -> Result<Option<(f64, f64)>> {
        let Some(p) = self.paper.as_deref() else {
            return Ok(None);
        };
        Ok(Some(match p {
            "A0" => (841., 1189.),
            "A1" => (594., 841.),
            "A2" => (420., 594.),
            "A3" => (297., 420.),
            "A4" => (210., 297.),
            "A5" => (148., 210.),
            "A6" => (105., 148.),
            "Letter" => (215.9, 279.4),
            "Legal" => (215.9, 355.6),
            "Tabloid" | "Ledger" => (279.4, 431.8),
            _ => return Err(Error::UnsupportedPaper(p.into())),
        }))
    }
}
