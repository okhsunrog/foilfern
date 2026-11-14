//! Port of libs/librepcb/core/project/board/realisticboardpainter.{h,cpp}.
//! Physical clipping lives in core's shared snapshot; this painter only
//! selects materials, colors and order. No meshes or component bodies.
use crate::{Drawing, GraphicsSettings};
use foilfern_canvas::convert;
use foilfern_core::{
    export::BoardSide,
    project::board::BoardPhysicalGeometry,
    types::{Color, Layer},
};

/// Builds the source's flat physical top/bottom view. `mirror` selects the
/// drill projection; the page encoder applies the actual coordinate mirror.
pub fn realistic_board_drawing(
    data: &BoardPhysicalGeometry,
    settings: &GraphicsSettings,
) -> Drawing {
    let mut drawing = Drawing::new();
    let view = data.surface(if settings.mirror {
        BoardSide::Bottom
    } else {
        BoardSide::Top
    });
    let mut area = |paths: &[foilfern_core::geometry::Path], color: Option<Color>| {
        if let Some(color) = color.filter(|color| color.a > 0)
            && !paths.is_empty()
        {
            drawing.area(convert::paths(paths), Some(color));
        }
    };
    area(&view.body, settings.color("board_outlines"));
    for role in settings.paint_order.iter().rev() {
        if let Some(layer) = Layer::all()
            .iter()
            .find(|layer| layer.is_copper() && layer.color_role() == role)
            && let Some(paths) = view.copper.get(layer)
        {
            area(paths, settings.color(role));
        }
    }
    // Transparent mask/legend entries request the board's material colors;
    // missing entries remain hidden. Monochrome is resolved by settings first.
    let material = |role: &str, fallback: Option<Color>| {
        settings.color(role).map(|color| {
            if color.a == 0 {
                fallback.unwrap_or(color)
            } else {
                color
            }
        })
    };
    for (side, role) in [
        (BoardSide::Top, "board_stop_mask_top"),
        (BoardSide::Bottom, "board_stop_mask_bottom"),
    ] {
        area(
            &data.surface(side).solder_resist,
            material(
                role,
                data.solder_resist
                    .map(|color| color.to_solder_resist_color()),
            ),
        );
    }
    for (side, role) in [
        (BoardSide::Top, "board_legend_top"),
        (BoardSide::Bottom, "board_legend_bottom"),
    ] {
        area(
            &data.surface(side).silkscreen,
            material(role, Some(data.silkscreen.to_silkscreen_color())),
        );
    }
    for layer in [Layer::TOP_SOLDER_PASTE, Layer::BOT_SOLDER_PASTE] {
        if let Some(paths) = view.solder_paste.get(&layer) {
            area(paths, settings.color(layer.color_role()));
        }
    }
    drawing
}
