//! Port of libs/librepcb/core/library/{sym/symbolpainter,pkg/footprintpainter}.{h,cpp}.
use crate::{Drawing, GraphicsSettings, Result};
use librepcb_canvas::convert;
use librepcb_core::{
    fileio::FileSystem,
    font::StrokeFont,
    geometry::{Circle, Polygon, StrokeText},
    library::{
        LibraryBaseElement,
        pkg::Footprint,
        sym::{Symbol, SymbolPin},
    },
    types::{Angle, Point},
    utils::{toolbox, transform::Transform},
};
use std::sync::LazyLock;
/// Application stroke font, embedded from the upstream CC0-1.0 resource at
/// build time; exported files do not need the upstream checkout at runtime.
pub fn default_stroke_font() -> &'static StrokeFont {
    static FONT: LazyLock<StrokeFont> = LazyLock::new(|| {
        StrokeFont::new(
            include_bytes!(concat!(
                env!("LIBREPCB_UPSTREAM_DIR"),
                "/share/librepcb/fontobene/newstroke.bene"
            ))
            .as_slice(),
        )
    });
    &FONT
}
pub(crate) fn shapes(
    drawing: &mut Drawing,
    polygons: &[&Polygon],
    circles: &[&Circle],
    transform: &Transform,
    settings: &GraphicsSettings,
    pass: u8,
) {
    let draw = |filled: bool, grab: bool| match pass {
        0 => grab && !filled,
        1 => filled,
        _ => !grab && !filled,
    };
    for polygon in polygons {
        if draw(polygon.is_filled(), polygon.is_grab_area()) {
            drawing.polygon(polygon, transform, settings);
        }
    }
    for circle in circles {
        if draw(circle.is_filled(), circle.is_grab_area()) {
            drawing.circle(circle, transform, settings);
        }
    }
}
/// Draw a library symbol, including the source's preview pin numbers `1…`.
pub fn symbol_drawing(symbol: &Symbol, settings: &GraphicsSettings) -> Result<Drawing> {
    let mut drawing = Drawing::new();
    let transform = Transform::new(Point::ORIGIN, Angle::DEG0, false);
    let polygons: Vec<_> = symbol.polygons().iter().collect();
    let circles: Vec<_> = symbol.circles().iter().collect();
    shapes(&mut drawing, &polygons, &circles, &transform, settings, 0);
    shapes(&mut drawing, &polygons, &circles, &transform, settings, 1);
    for image in symbol.images().iter() {
        let bytes = symbol.directory().read(image.file_name())?;
        drawing.image(image, &bytes, settings)?;
    }
    shapes(&mut drawing, &polygons, &circles, &transform, settings, 2);
    for text in symbol.texts().iter() {
        drawing.text(text, text.text(), settings);
    }
    for pin in symbol.pins().iter() {
        drawing.line(
            pin.position(),
            pin.position()
                + Point::from_nm(pin.length().to_nm(), 0).rotated(pin.rotation(), Point::ORIGIN),
            0.15875,
            settings.color("schematic_pin_lines"),
            settings,
        );
        drawing.markup_label(
            pin.position() + pin.name_position().rotated(pin.rotation(), Point::ORIGIN),
            pin.rotation() + pin.name_rotation(),
            pin.name_height().to_mm(),
            pin.name_alignment(),
            pin.name().as_str(),
            settings.color("schematic_pin_names"),
            true,
            false,
            false,
        );
        let flipped = toolbox::is_text_upside_down(pin.rotation());
        drawing.label(
            pin.position()
                + pin
                    .numbers_position(flipped)
                    .rotated(pin.rotation(), Point::ORIGIN),
            pin.rotation(),
            SymbolPin::numbers_height().to_mm(),
            SymbolPin::numbers_alignment(flipped),
            "1…",
            settings.color("schematic_pin_numbers"),
            true,
            false,
            false,
        );
    }
    Ok(drawing)
}
pub(crate) fn stroke_text(
    drawing: &mut Drawing,
    text: &StrokeText,
    font: &StrokeFont,
    value: &str,
    settings: &GraphicsSettings,
) {
    let transform = Transform::new(text.position(), text.rotation(), text.mirrored());
    let path =
        convert::transform(&transform) * convert::paths(&text.generate_paths_for(font, value));
    drawing.path(
        path,
        text.stroke_width().to_mm(),
        settings.color(text.layer().color_role()),
        None,
        settings,
    );
}
/// Draw a detached footprint using the default application stroke font.
pub fn footprint_drawing(footprint: &Footprint, settings: &GraphicsSettings) -> Result<Drawing> {
    let mut drawing = Drawing::new();
    let transform = Transform::new(Point::ORIGIN, Angle::DEG0, false);
    let pad_holes =
        settings.colors.contains_key("board_holes") && !settings.colors.contains_key("board_pads");
    for role in settings.paint_order.iter().rev() {
        for pad in footprint.pads().iter().map(|p| p.pad()) {
            let pad_role = if pad.is_tht() {
                "board_pads"
            } else {
                pad.smt_layer().color_role()
            };
            if pad_role == role {
                let geometry = pad.geometry();
                let outlines = geometry.to_outlines()?;
                let mut path = convert::paths(&outlines);
                path.extend(
                    convert::paths(&geometry.to_hole_outlines())
                        .elements()
                        .iter()
                        .copied(),
                );
                let transform =
                    convert::transform(&Transform::new(pad.position(), pad.rotation(), false));
                drawing.area(transform * path, settings.color(role));
            }
        }
        for polygon in footprint
            .polygons()
            .iter()
            .filter(|p| p.layer().color_role() == role)
        {
            drawing.polygon(polygon, &transform, settings);
        }
        for circle in footprint
            .circles()
            .iter()
            .filter(|p| p.layer().color_role() == role)
        {
            drawing.circle(circle, &transform, settings);
        }
        if role == "board_holes" {
            for hole in footprint.holes().iter() {
                drawing.slot(
                    hole.path(),
                    hole.diameter(),
                    settings.color(role),
                    None,
                    settings,
                );
            }
            if pad_holes {
                for pad in footprint.pads().iter().map(|p| p.pad()) {
                    let transform = Transform::new(pad.position(), pad.rotation(), false);
                    for hole in pad.holes().iter() {
                        drawing.slot(
                            &transform.map(hole.path()),
                            hole.diameter(),
                            settings.color(role),
                            None,
                            settings,
                        );
                    }
                }
            }
        }
        for text in footprint
            .stroke_texts()
            .iter()
            .filter(|p| p.layer().color_role() == role)
        {
            stroke_text(
                &mut drawing,
                text,
                default_stroke_font(),
                text.text(),
                settings,
            );
        }
    }
    Ok(drawing)
}
