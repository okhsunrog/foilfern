//! Port of libs/librepcb/core/project/schematic/schematicpainter.{h,cpp}.
//! A detached display list retains no model references or cached live state.
use crate::{Drawing, Error, GraphicsSettings, Result, library::shapes};
use librepcb_core::{
    fileio::FileSystem,
    geometry::{Image, NetLineAnchor},
    library::{LibraryBaseElement, sym::SymbolPin},
    project::{Project, ProjectAttributeLookup, schematic::Schematic},
    types::{Alignment, Angle, HAlign, Point, VAlign},
    utils::transform::Transform,
};
fn placed_image(
    drawing: &mut Drawing,
    image: &Image,
    bytes: &[u8],
    transform: &Transform,
    settings: &GraphicsSettings,
) -> Result<()> {
    let mut image = image.clone();
    let mut position = transform.map(&image.position());
    let mut rotation = transform.map_non_mirrorable(image.rotation());
    if transform.mirrored {
        position += Point::new(-*image.width(), librepcb_core::types::Length::ZERO)
            .rotated(-image.rotation() + transform.rotation, Point::ORIGIN);
        rotation += Angle::DEG180;
    }
    image.set_position(position);
    image.set_rotation(rotation);
    drawing.image(&image, bytes, settings)
}
/// Builds a schematic's symbol, text, image, net and bus display list.
pub fn schematic_drawing(
    project: &Project,
    schematic: &Schematic,
    settings: &GraphicsSettings,
) -> Result<Drawing> {
    let mut drawing = Drawing::new();
    let ctx = project.view();
    let mut pin_junctions = Vec::new();
    for symbol in schematic.symbols().values() {
        let resolved = symbol.resolve(ctx)?;
        let transform = symbol.transform();
        let lib = resolved.lib_symbol;
        let polygons: Vec<_> = lib.polygons().iter().collect();
        let circles: Vec<_> = lib.circles().iter().collect();
        shapes(&mut drawing, &polygons, &circles, &transform, settings, 0);
        shapes(&mut drawing, &polygons, &circles, &transform, settings, 1);
        for image in lib.images().iter() {
            placed_image(
                &mut drawing,
                image,
                &lib.directory().read(image.file_name())?,
                &transform,
                settings,
            )?;
        }
        shapes(&mut drawing, &polygons, &circles, &transform, settings, 2);
        let primary = project
            .boards()
            .iter()
            .find_map(|b| b.device(symbol.component()));
        for pin in symbol.pins(ctx)? {
            let lib = pin.lib_pin();
            let mut length = *lib.length();
            if let Some(net) = pin.net().and_then(|id| project.circuit().net_signal(id))
                && pin.lib_signal().is_net_signal_name_forced()
            {
                let forced = ProjectAttributeLookup::for_component(
                    project,
                    resolved.component,
                    primary,
                    None,
                )
                .substitute(pin.lib_signal().forced_net_name());
                if !forced.is_empty() && net.name().as_str() != forced {
                    length /= 2;
                }
            }
            let rotation = transform.map_non_mirrorable(lib.rotation());
            drawing.line(
                pin.position(),
                pin.position()
                    + Point::new(length, librepcb_core::types::Length::ZERO)
                        .rotated(rotation, Point::ORIGIN),
                0.15875,
                settings.color("schematic_pin_lines"),
                settings,
            );
            let name_align = if transform.mirrored {
                lib.name_alignment().mirrored_v()
            } else {
                lib.name_alignment()
            };
            drawing.markup_label(
                transform.map(
                    &(lib.position() + lib.name_position().rotated(lib.rotation(), Point::ORIGIN)),
                ),
                transform.map_non_mirrorable(lib.rotation() + lib.name_rotation()),
                lib.name_height().to_mm(),
                name_align,
                pin.name(),
                settings.color("schematic_pin_names"),
                true,
                false,
                false,
            );
            let mut numbers = Vec::new();
            if let Some(device) = primary {
                for pad in device.pads(project.library(), project.circuit())? {
                    if pad.component_signal() == Some(pin.signal())
                        && let Some(pad) = pad.package_pad()
                    {
                        numbers.push(pad.name().to_string());
                    }
                }
            }
            numbers.sort_by(|a, b| librepcb_core::utils::toolbox::compare_numeric(a, b));
            let numbers = pin.numbers_text(&numbers);
            drawing.markup_label(
                transform.map(
                    &(lib.position()
                        + pin
                            .numbers_position()
                            .rotated(lib.rotation(), Point::ORIGIN)),
                ),
                rotation,
                SymbolPin::numbers_height().to_mm(),
                pin.numbers_alignment(),
                &numbers,
                settings.color("schematic_pin_numbers"),
                true,
                false,
                false,
            );
            if schematic
                .net_segments()
                .values()
                .any(|segment| segment.is_visible_junction(pin.anchor()))
            {
                pin_junctions.push(pin.position());
            }
        }
    }
    for image in schematic.images().values() {
        let bytes = project.directory().read(&format!(
            "schematics/{}/{}",
            schematic.directory_name(),
            image.file_name()
        ))?;
        drawing.image(image, &bytes, settings)?;
    }
    let identity = Transform::new(Point::ORIGIN, Angle::DEG0, false);
    for polygon in schematic.polygons().values() {
        drawing.polygon(polygon, &identity, settings);
    }
    for symbol in schematic.symbols().values() {
        let device = project
            .boards()
            .iter()
            .find_map(|b| b.device(symbol.component()));
        let lookup =
            ProjectAttributeLookup::for_symbol(project, schematic, symbol, device, None, None);
        for text in symbol.texts().values() {
            drawing.text(text, &lookup.substitute(text.text()), settings);
        }
    }
    for text in schematic.texts().values() {
        let value =
            ProjectAttributeLookup::for_schematic(project, schematic, None).substitute(text.text());
        drawing.text(text, &value, settings);
    }
    for segment in schematic.net_segments().values() {
        for line in segment.lines().values() {
            let position = |anchor| {
                schematic
                    .net_line_anchor_position(segment.id(), anchor, ctx)
                    .ok_or_else(|| Error::Model("Missing net line anchor".into()))
            };
            drawing.line(
                position(line.p1())?,
                position(line.p2())?,
                line.width().to_mm(),
                settings.color("schematic_wires"),
                settings,
            );
        }
    }
    for point in pin_junctions {
        drawing.junction(point, settings.color("schematic_wires"), settings);
    }
    for segment in schematic.net_segments().values() {
        for (uuid, junction) in segment.junctions() {
            if segment.is_visible_junction(NetLineAnchor::Junction(*uuid)) {
                drawing.junction(
                    junction.position(),
                    settings.color("schematic_wires"),
                    settings,
                );
            }
        }
    }
    for segment in schematic.net_segments().values() {
        let name = project
            .circuit()
            .net_signal(segment.net())
            .ok_or_else(|| Error::Model("Missing net signal".into()))?
            .name();
        for label in segment.labels().values() {
            drawing.markup_label(
                label.position(),
                label.rotation(),
                4. * 25.4 / 72.,
                Alignment::new(
                    if label.mirrored() {
                        HAlign::Right
                    } else {
                        HAlign::Left
                    },
                    VAlign::Bottom,
                ),
                name.as_str(),
                settings.color("schematic_net_labels"),
                true,
                false,
                true,
            );
        }
    }
    for segment in schematic.bus_segments().values() {
        for line in segment.lines().values() {
            let p1 = segment
                .junction_position(line.p1())
                .ok_or_else(|| Error::Model("Missing bus line anchor".into()))?;
            let p2 = segment
                .junction_position(line.p2())
                .ok_or_else(|| Error::Model("Missing bus line anchor".into()))?;
            drawing.line(
                p1,
                p2,
                line.width().to_mm(),
                settings.color("schematic_buses"),
                settings,
            );
        }
        for (uuid, junction) in segment.junctions() {
            if segment.is_visible_junction(*uuid) {
                drawing.junction(
                    junction.position(),
                    settings.color("schematic_buses"),
                    settings,
                );
            }
        }
        let name = project
            .circuit()
            .bus(segment.bus())
            .ok_or_else(|| Error::Model("Missing bus".into()))?
            .name();
        for label in segment.labels().values() {
            drawing.markup_label(
                label.position(),
                label.rotation(),
                4. * 25.4 / 72.,
                Alignment::new(
                    if label.mirrored() {
                        HAlign::Right
                    } else {
                        HAlign::Left
                    },
                    VAlign::Bottom,
                ),
                name.as_str(),
                settings.color("schematic_bus_labels"),
                true,
                false,
                true,
            );
        }
    }
    Ok(drawing)
}
