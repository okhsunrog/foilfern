//! Port of libs/librepcb/core/project/board/realisticboardpainter.{h,cpp}:
//! a realistic top or bottom view of a board for graphics exports (board
//! body, copper, solder resist, silkscreen and solder paste as filled
//! areas).
//!
//! Upstream computes the areas from the board's 3D scene data
//! (`SceneData3D`, not ported); here they are taken from the items of a
//! [`BoardScene`] (fills as they are, strokes converted to their outlines)
//! and combined with Clipper like upstream:
//!
//! - holes: board cutouts, plated cutouts and all drills (upstream keeps
//!   the drills of plated holes on the viewed side out of the body cutout
//!   and subtracts them from the copper only, which looks the same);
//! - body: board outlines (even-odd) minus holes;
//! - copper: body ∩ copper items of the layer (traces, pads, vias,
//!   planes, polygons, texts);
//! - solder resist: board outlines minus stop mask openings and cutouts,
//!   in the board's solder resist color if the configured color is
//!   transparent;
//! - silkscreen: the board's silkscreen layers ∩ solder resist, in the
//!   board's silkscreen color if the configured color is transparent;
//! - solder paste: paste items ∩ body.

use clipper::{IntPoint, PolyFillType};
use librepcb_canvas::kurbo::{self, BezPath, PathEl, StrokeOpts};
use librepcb_canvas::{Brush, LayerId};
use librepcb_core::export::GraphicsExportSettings;
use librepcb_core::project::{BoardId, Project};
use librepcb_core::types::Layer;
use librepcb_core::utils::clipper_helpers::{self, ClipperPaths};

use super::{Drawing, PrimitivePaint, kurbo_stroke, to_color};
use crate::board::{BoardScene, BoardSceneLayer, BoardSide};
use crate::colors::ColorScheme;
use crate::error::{Error, Result};

/// Flattening tolerance in millimeters (upstream `mMaxArcTolerance`, 5 µm).
const TOLERANCE: f64 = 0.005;

/// Builds the drawing of a realistic board rendering.
pub fn drawing(
    project: &Project,
    id: BoardId,
    settings: &GraphicsExportSettings,
) -> Result<Drawing> {
    let board = project.board(id).ok_or(Error::BoardNotFound(id))?;
    let scene = BoardScene::build(project, id, BoardSide::Top, &ColorScheme::BOARD_DARK)?;
    let min_line_width = settings.min_line_width.to_mm();
    let items = |layers: &[BoardSceneLayer], as_outline: bool| -> ClipperPaths {
        let ids: Vec<LayerId> = layers.iter().map(|l| l.id()).collect();
        let mut paths = ClipperPaths::new();
        for (_, item) in scene.scene().items() {
            if !ids.contains(&item.layer) {
                continue;
            }
            let path = item.geometry.to_path();
            // Grab area fills (solid colors) are no areas.
            if as_outline || item.style.fill == Some(Brush::Layer) {
                paths.extend(to_clipper(&path));
            }
            if !as_outline
                && let Some(style) = &item.style.stroke
                && !style.is_hairline()
            {
                let stroke = kurbo_stroke(style, min_line_width);
                let outline = kurbo::stroke(
                    path.elements().iter().copied(),
                    &stroke,
                    &StrokeOpts::default(),
                    TOLERANCE,
                );
                paths.extend(to_clipper(&outline));
            }
        }
        paths
    };
    let color = |role: &str| settings.color(role);
    let mut drawing = Drawing::new();
    let mut push = |paths: &ClipperPaths, color: librepcb_core::types::Color| {
        drawing.push(from_clipper(paths), PrimitivePaint::Fill(to_color(color)));
    };

    // Holes/cutouts.
    let mut holes = items(
        &[
            BoardSceneLayer::Board(Layer::BOARD_CUTOUTS),
            BoardSceneLayer::Board(Layer::BOARD_PLATED_CUTOUTS),
        ],
        true,
    );
    holes.extend(items(&[BoardSceneLayer::HoleFills], false));
    clipper_helpers::unite(&mut holes, PolyFillType::NonZero)?;

    // Board outlines/area.
    let outlines = items(&[BoardSceneLayer::Board(Layer::BOARD_OUTLINES)], true);
    let mut area = outlines.clone();
    clipper_helpers::subtract(
        &mut area,
        &holes,
        PolyFillType::EvenOdd,
        PolyFillType::NonZero,
    )?;

    // Board body.
    if let Some(c) = color("board_outlines").filter(|c| c.a > 0) {
        push(&area, c);
    }

    // Copper.
    for role in settings.paint_order() {
        let Some(layer) = Layer::all()
            .iter()
            .copied()
            .find(|l| l.is_copper() && l.color_role() == role)
        else {
            continue;
        };
        if let Some(c) = color(role).filter(|c| c.a > 0) {
            let mut paths = area.clone();
            let copper = items(
                &[
                    BoardSceneLayer::Board(layer),
                    BoardSceneLayer::ThtPads(layer),
                    BoardSceneLayer::Vias(layer),
                ],
                false,
            );
            clipper_helpers::intersect(
                &mut paths,
                &copper,
                PolyFillType::EvenOdd,
                PolyFillType::NonZero,
            )?;
            push(&paths, c);
        }
    }

    // Solder resist.
    let cutouts = items(
        &[
            BoardSceneLayer::Board(Layer::BOARD_CUTOUTS),
            BoardSceneLayer::Board(Layer::BOARD_PLATED_CUTOUTS),
        ],
        true,
    );
    let solder_resist = |bottom: bool| -> Result<ClipperPaths> {
        let mask = if bottom {
            Layer::BOT_STOP_MASK
        } else {
            Layer::TOP_STOP_MASK
        };
        let mut openings = items(&[BoardSceneLayer::Board(mask)], false);
        openings.extend(cutouts.iter().cloned());
        let mut paths = outlines.clone();
        clipper_helpers::subtract(
            &mut paths,
            &openings,
            PolyFillType::EvenOdd,
            PolyFillType::NonZero,
        )?;
        Ok(paths)
    };
    let board_settings = board.settings();
    for (role, bottom) in [
        ("board_stop_mask_top", false),
        ("board_stop_mask_bottom", true),
    ] {
        let mut c = color(role);
        if c.is_some_and(|c| c.a == 0)
            && let Some(resist) = board_settings.solder_resist
        {
            c = Some(resist.to_solder_resist_color());
        }
        if let Some(c) = c.filter(|c| c.a > 0) {
            push(&solder_resist(bottom)?, c);
        }
    }

    // Silkscreen.
    for (role, bottom) in [("board_legend_top", false), ("board_legend_bottom", true)] {
        let silkscreen = if bottom {
            board_settings.silkscreen_color_bot()
        } else {
            board_settings.silkscreen_color_top()
        };
        let mut c = color(role);
        if c.is_some_and(|c| c.a == 0)
            && let Some(silkscreen) = silkscreen
        {
            c = Some(silkscreen.to_silkscreen_color());
        }
        if let Some(c) = c.filter(|c| c.a > 0) {
            let layers: Vec<BoardSceneLayer> = if bottom {
                &board_settings.silkscreen_layers_bot
            } else {
                &board_settings.silkscreen_layers_top
            }
            .iter()
            .map(|l| BoardSceneLayer::Board(*l))
            .collect();
            let mut paths = items(&layers, false);
            clipper_helpers::intersect(
                &mut paths,
                &solder_resist(bottom)?,
                PolyFillType::NonZero,
                PolyFillType::EvenOdd,
            )?;
            push(&paths, c);
        }
    }

    // Solder paste.
    for (role, layer) in [
        ("board_solder_paste_top", Layer::TOP_SOLDER_PASTE),
        ("board_solder_paste_bottom", Layer::BOT_SOLDER_PASTE),
    ] {
        if let Some(c) = color(role).filter(|c| c.a > 0) {
            let mut paths = items(&[BoardSceneLayer::Board(layer)], false);
            clipper_helpers::intersect(
                &mut paths,
                &area,
                PolyFillType::NonZero,
                PolyFillType::EvenOdd,
            )?;
            push(&paths, c);
        }
    }
    Ok(drawing)
}

/// Flattens a path into Clipper polygons (nanometers).
fn to_clipper(path: &BezPath) -> ClipperPaths {
    let mut paths = ClipperPaths::new();
    let mut current: Vec<IntPoint> = Vec::new();
    let point =
        |p: kurbo::Point| IntPoint::new((p.x * 1e6).round() as i64, (p.y * 1e6).round() as i64);
    kurbo::flatten(path.elements().iter().copied(), TOLERANCE, |el| match el {
        PathEl::MoveTo(p) => {
            if current.len() > 2 {
                paths.push(std::mem::take(&mut current));
            }
            current.clear();
            current.push(point(p));
        }
        PathEl::LineTo(p) => current.push(point(p)),
        PathEl::ClosePath => {
            if current.len() > 2 {
                paths.push(std::mem::take(&mut current));
            }
            current.clear();
        }
        // `flatten()` only emits lines.
        PathEl::QuadTo(..) | PathEl::CurveTo(..) => {}
    });
    if current.len() > 2 {
        paths.push(current);
    }
    paths
}

/// Converts Clipper polygons (nanometers) into a path (millimeters).
fn from_clipper(paths: &ClipperPaths) -> BezPath {
    let mut out = BezPath::new();
    for path in paths {
        let mut points = path
            .iter()
            .map(|p| kurbo::Point::new(p.x as f64 / 1e6, p.y as f64 / 1e6));
        if let Some(first) = points.next() {
            out.move_to(first);
            for p in points {
                out.line_to(p);
            }
            out.close_path();
        }
    }
    out
}
