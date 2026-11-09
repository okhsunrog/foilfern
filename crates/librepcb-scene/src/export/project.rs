//! Drawings of the pages of a graphics export: the export paths of
//! upstream's `SchematicPainter`, `BoardPainter`, `SymbolPainter` and
//! `FootprintPainter` (libs/librepcb/core/project/schematic/
//! schematicpainter.cpp, project/board/boardpainter.cpp,
//! library/sym/symbolpainter.cpp, library/pkg/footprintpainter.cpp): the
//! scenes built with the colors of the [`GraphicsExportSettings`], and the
//! [`GraphicsExporter`] of the core output job runner.

use librepcb_canvas::LayerId;
use librepcb_core::export::GraphicsExportSettings;
use librepcb_core::fileio::FilePath;
use librepcb_core::font::StrokeFont;
use librepcb_core::library::pkg::Footprint;
use librepcb_core::library::sym::Symbol;
use librepcb_core::project::{
    GraphicsExportResult, GraphicsExporter, GraphicsPage, GraphicsPageContent, Project,
};
use librepcb_core::types::Layer;

use super::{Drawing, ExportPage, GraphicsExport, board_rendering, to_color};
use crate::board::{BoardScene, BoardSceneLayer, BoardSide};
use crate::colors::ColorScheme;
use crate::error::Result;
use crate::footprint::FootprintScene;
use crate::schematic::SchematicScene;
use crate::symbol::SymbolScene;

/// Grab area roles (upstream `ColorRole::getGrabAreaRole()` targets).
const GRAB_AREA_ROLES: &[&str] = &[
    "schematic_grab_areas",
    "board_grab_areas_top",
    "board_grab_areas_bottom",
];

/// The colors of export settings as a color scheme: roles without color
/// are not painted, black/white mode is applied (upstream `getColor()`),
/// and the grab area fills are gray in black/white mode (upstream
/// `getFillColor()`).
pub fn export_color_scheme(settings: &GraphicsExportSettings) -> ColorScheme {
    let colors = settings.colors.iter().filter_map(|(role, raw)| {
        let color = if GRAB_AREA_ROLES.contains(&role.as_str()) {
            if settings.black_white {
                // `qGray()`.
                let gray = ((u32::from(raw.r) * 11 + u32::from(raw.g) * 16 + u32::from(raw.b) * 5)
                    / 32) as u8;
                librepcb_core::types::Color::rgba(gray, gray, gray, raw.a)
            } else {
                *raw
            }
        } else {
            settings.color(role)?
        };
        Some((role.clone(), to_color(color)))
    });
    ColorScheme::custom("Graphics Export", colors)
}

/// The drawing of a schematic, board or board rendering page.
pub fn drawing_for_page(project: &Project, page: &GraphicsPage) -> Result<Drawing> {
    let scheme = export_color_scheme(&page.settings);
    match page.content {
        GraphicsPageContent::Schematic(id) => {
            let scene = SchematicScene::build(project, id, &scheme)?;
            for w in scene.warnings() {
                log::warn!("{w}");
            }
            Ok(Drawing::from_scene(scene.scene()))
        }
        GraphicsPageContent::Board(id) => {
            let mut scene = BoardScene::build(project, id, BoardSide::Top, &scheme)?;
            for w in scene.warnings() {
                log::warn!("{w}");
            }
            let roles = apply_board_visibility(&mut scene, &scheme);
            // Paint in the order of the color roles (upstream paints role
            // by role).
            let order = page.settings.paint_order();
            let rank = |id: LayerId| {
                roles
                    .iter()
                    .find(|(l, _)| l.id() == id)
                    .and_then(|(_, role)| order.iter().position(|r| r == role))
                    .unwrap_or(usize::MAX)
            };
            Ok(Drawing::from_scene_sorted(scene.scene(), rank))
        }
        GraphicsPageContent::BoardRendering(id) => {
            board_rendering::drawing(project, id, &page.settings)
        }
    }
}

/// Shows the canvas layers of a board scene like upstream's
/// `BoardPainter`: board layers with a color, THT pads and vias only with
/// their copper layer if any copper layer is enabled (otherwise once, in
/// the pads/vias color), no zones, drill fills and air wires.
/// Returns the color role each visible layer is painted with.
fn apply_board_visibility(
    scene: &mut BoardScene,
    scheme: &ColorScheme,
) -> Vec<(BoardSceneLayer, &'static str)> {
    let has = |role: &str| scheme.color(role).is_some();
    let copper_enabled = scene
        .layers()
        .iter()
        .any(|l| matches!(l, BoardSceneLayer::Board(b) if b.is_copper() && has(b.color_role())));
    let layers = scene.layers().to_vec();
    let mut roles = Vec::new();
    for layer in layers {
        let role = match layer {
            BoardSceneLayer::Board(l) => l.color_role(),
            BoardSceneLayer::ThtPads(l) if copper_enabled => l.color_role(),
            BoardSceneLayer::ThtPads(_) => "board_pads",
            BoardSceneLayer::Vias(l) if copper_enabled => l.color_role(),
            BoardSceneLayer::Vias(_) => "board_vias",
            BoardSceneLayer::Zones(l) => l.color_role(),
            BoardSceneLayer::Holes | BoardSceneLayer::HoleFills => "board_holes",
            BoardSceneLayer::AirWires => "board_airwires",
        };
        roles.push((layer, role));
        let visible = match layer {
            BoardSceneLayer::Board(l) => has(l.color_role()),
            BoardSceneLayer::ThtPads(l) => {
                has("board_pads")
                    && if copper_enabled {
                        has(l.color_role())
                    } else {
                        l == Layer::TOP_COPPER
                    }
            }
            BoardSceneLayer::Vias(l) => {
                has("board_vias")
                    && if copper_enabled {
                        has(l.color_role())
                    } else {
                        l == Layer::TOP_COPPER
                    }
            }
            BoardSceneLayer::Holes => has("board_holes"),
            BoardSceneLayer::Zones(_) | BoardSceneLayer::HoleFills | BoardSceneLayer::AirWires => {
                false
            }
        };
        scene.set_scene_layer_visible(layer, visible);
    }
    roles
}

/// The drawing of a library symbol (upstream `SymbolPainter`).
pub fn symbol_drawing(
    symbol: &Symbol,
    font: Option<&StrokeFont>,
    settings: &GraphicsExportSettings,
) -> Drawing {
    let scene = SymbolScene::build(symbol, font, &export_color_scheme(settings));
    Drawing::from_scene(scene.scene())
}

/// The drawing of a library footprint (upstream `FootprintPainter`).
pub fn footprint_drawing(
    footprint: &Footprint,
    font: Option<&StrokeFont>,
    settings: &GraphicsExportSettings,
) -> Drawing {
    let scene = FootprintScene::build(footprint, font, &export_color_scheme(settings));
    Drawing::from_scene(scene.scene())
}

/// The [`GraphicsExporter`] of the output job runner: builds the page
/// drawings and exports them with [`GraphicsExport`].
#[derive(Debug, Clone)]
pub struct ProjectGraphicsExporter {
    creator: String,
}

impl ProjectGraphicsExporter {
    /// Creates the exporter; `creator` is written into PDF files (upstream
    /// `"LibrePCB <version>"`).
    pub fn new(creator: impl Into<String>) -> Self {
        Self {
            creator: creator.into(),
        }
    }
}

impl GraphicsExporter for ProjectGraphicsExporter {
    fn export(
        &self,
        project: &Project,
        pages: &[GraphicsPage],
        file_path: &FilePath,
        document_name: &str,
    ) -> GraphicsExportResult {
        let mut errors = Vec::new();
        let mut export_pages = Vec::new();
        for page in pages {
            match drawing_for_page(project, page) {
                Ok(drawing) => export_pages.push(ExportPage {
                    drawing,
                    settings: page.settings.clone(),
                }),
                Err(e) => errors.push(e.to_string()),
            }
        }
        let mut export = GraphicsExport::new(self.creator.clone());
        export.set_document_name(document_name);
        let mut result = export.export(&export_pages, file_path);
        errors.append(&mut result.errors);
        result.errors = errors;
        result
    }
}
