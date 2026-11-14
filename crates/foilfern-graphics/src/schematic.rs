//! Port of libs/librepcb/core/project/schematic/schematicpainter.{h,cpp}.
//! A detached display list retains no model references or cached live state.
use crate::{Drawing, Error, GraphicsSettings, Result, drawing::svg_options, library::shapes};
use foilfern_canvas::{Item, Style};
use foilfern_core::{
    fileio::FileSystem,
    geometry::{Image, NetLineAnchor},
    library::{LibraryBaseElement, sym::SymbolPin},
    project::{
        Project, ProjectAttributeLookup, SymbolRef,
        schematic::{
            Schematic,
            import_appearance::{ImportAppearance, cross, symbol_cross},
        },
    },
    types::{Alignment, Angle, Color, HAlign, Point, VAlign},
    utils::transform::Transform,
};
use kurbo::{Point as GraphicsPoint, Rect};
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
        position += Point::new(-*image.width(), foilfern_core::types::Length::ZERO)
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
    build_schematic_drawing(project, schematic, settings, None)
}

/// One placed field and the other visible elements of the exported schematic.
pub struct SchematicFieldGeometry {
    field: Rect,
    text_and_images: Vec<Rect>,
    shapes: Vec<Item>,
}

impl SchematicFieldGeometry {
    /// Tests a translation in sheet millimeters using the export font and geometry.
    pub fn fits(&self, from: Point, to: Point, min: Point, max: Point, clearance_mm: f64) -> bool {
        let (x, y) = from.to_mm();
        let (tx, ty) = to.to_mm();
        let bounds = self.field + kurbo::Vec2::new(tx - x, ty - y);
        let region = Rect::from_points(min.to_mm(), max.to_mm());
        let clearance = bounds.inflate(clearance_mm, clearance_mm);
        region.contains_rect(bounds)
            && !self
                .text_and_images
                .iter()
                .any(|other| other.overlaps(clearance))
            && !self
                .shapes
                .iter()
                .any(|item| item.intersects_rect(clearance))
    }
}

/// Resolves the same visible geometry and font as the schematic PNG exporter.
pub fn schematic_field_geometry(
    project: &Project,
    schematic: &Schematic,
    symbol: SymbolRef,
    text: foilfern_core::types::Uuid,
    settings: &GraphicsSettings,
) -> Result<Option<SchematicFieldGeometry>> {
    let drawing = build_schematic_drawing(project, schematic, settings, Some((symbol, text)))?;
    let source = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"1000\" height=\"1000\">{}</svg>",
        drawing.body()
    );
    let tree = resvg::usvg::Tree::from_str(&source, &svg_options())
        .map_err(|error| Error::Svg(error.to_string()))?;
    let bounds = |node: &resvg::usvg::Node| {
        let r = node.abs_stroke_bounding_box();
        Rect::new(
            f64::from(r.left()),
            -f64::from(r.bottom()),
            f64::from(r.right()),
            -f64::from(r.top()),
        )
    };
    let Some(field) = tree.node_by_id("placement_field").map(bounds) else {
        return Ok(None);
    };
    let mut text_and_images = Vec::new();
    let mut shapes = Vec::new();
    fn add_obstacle(
        node: &resvg::usvg::Node,
        bounds: &impl Fn(&resvg::usvg::Node) -> Rect,
        text_and_images: &mut Vec<Rect>,
        shapes: &mut Vec<Item>,
    ) {
        if node.id() == "placement_field" {
            return;
        }
        use resvg::{tiny_skia::PathSegment, usvg::Node};
        match node {
            Node::Group(group) => {
                for child in group.children() {
                    add_obstacle(child, bounds, text_and_images, shapes);
                }
            }
            Node::Path(path) => {
                if !path.is_visible() || (path.fill().is_none() && path.stroke().is_none()) {
                    return;
                }
                let xf = path.abs_transform();
                let map = |mut point: resvg::tiny_skia::Point| {
                    xf.map_point(&mut point);
                    GraphicsPoint::new(f64::from(point.x), -f64::from(point.y))
                };
                let mut geometry = kurbo::BezPath::new();
                for segment in path.data().segments() {
                    match segment {
                        PathSegment::MoveTo(p) => geometry.move_to(map(p)),
                        PathSegment::LineTo(p) => geometry.line_to(map(p)),
                        PathSegment::QuadTo(p, q) => geometry.quad_to(map(p), map(q)),
                        PathSegment::CubicTo(p, q, r) => geometry.curve_to(map(p), map(q), map(r)),
                        PathSegment::Close => geometry.close_path(),
                    }
                }
                let mut style = path.stroke().map_or_else(Style::default, |stroke| {
                    let (sx, sy) = xf.get_scale();
                    Style::stroke(f64::from(stroke.width().get() * sx.max(sy)))
                });
                if path.fill().is_some() {
                    style.fill = Style::fill().fill;
                }
                shapes.push(Item::new(Default::default(), geometry, style));
            }
            Node::Image(image) if image.is_visible() => text_and_images.push(bounds(node)),
            Node::Text(_) => text_and_images.push(bounds(node)),
            Node::Image(_) => {}
        }
    }
    for node in tree.root().children() {
        add_obstacle(node, &bounds, &mut text_and_images, &mut shapes);
    }
    Ok(Some(SchematicFieldGeometry {
        field,
        text_and_images,
        shapes,
    }))
}

fn build_schematic_drawing(
    project: &Project,
    schematic: &Schematic,
    settings: &GraphicsSettings,
    placement: Option<(SymbolRef, foilfern_core::types::Uuid)>,
) -> Result<Drawing> {
    let mut drawing = Drawing::new();
    let ctx = project.view();
    let appearance = ImportAppearance::load(project, schematic);
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
                    + Point::new(length, foilfern_core::types::Length::ZERO)
                        .rotated(rotation, Point::ORIGIN),
                0.15875,
                settings.color("schematic_pin_lines"),
                settings,
            );
            let heights = appearance.pin_text_heights(symbol.uuid(), pin.uuid());
            let name_position = appearance.pin_name_position(&pin, &transform);
            drawing.markup_label(
                name_position.position,
                name_position.rotation,
                heights
                    .name
                    .map_or(*lib.name_height(), |height| *height)
                    .to_mm(),
                name_position.alignment,
                appearance
                    .pin_names
                    .get(&symbol.uuid())
                    .and_then(|pins| pins.get(&pin.uuid()))
                    .map_or_else(|| pin.name(), String::as_str),
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
            numbers.sort_by(|a, b| foilfern_core::utils::toolbox::compare_numeric(a, b));
            let numbers = appearance
                .pin_numbers
                .get(&symbol.uuid())
                .and_then(|pins| pins.get(&pin.uuid()))
                .cloned()
                .unwrap_or_else(|| pin.numbers_text(&numbers));
            let number_position = appearance.pin_number_position(&pin, &transform);
            drawing.markup_label(
                number_position.position,
                number_position.rotation,
                heights
                    .number
                    .map_or(*SymbolPin::numbers_height(), |height| *height)
                    .to_mm(),
                number_position.alignment,
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
            if pin.net().is_none() && appearance.is_no_connect(symbol.uuid(), pin.uuid()) {
                let delta = Point::from_nm(635_000, 635_000);
                for (start, end) in cross(pin.position() - delta, pin.position() + delta) {
                    drawing.line(
                        start,
                        end,
                        0.2,
                        Some(Color::rgba(0, 90, 180, 255)),
                        settings,
                    );
                }
            }
        }
        if appearance.unpopulated.contains(&symbol.uuid()) {
            for (start, end) in symbol_cross(lib, &transform) {
                drawing.line(
                    start,
                    end,
                    0.35,
                    Some(Color::rgba(190, 0, 0, 255)),
                    settings,
                );
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
            let start = drawing.elements.len();
            drawing.text(text, &lookup.substitute(text.text()), settings);
            if placement
                .is_some_and(|(target, uuid)| target.symbol == symbol.id() && uuid == text.uuid())
                && drawing.elements.len() > start
            {
                let elements = drawing.elements.split_off(start).join("\n");
                drawing
                    .elements
                    .push(format!("<g id=\"placement_field\">{elements}</g>"));
            }
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
            if let Some(style) = appearance.global_labels.get(&label.uuid()) {
                let (path, position, rotation, alignment) = style.geometry(label, name.as_str());
                let color = settings.color("schematic_outlines");
                drawing.path(
                    foilfern_canvas::convert::path(&path),
                    0.15,
                    color,
                    None,
                    settings,
                );
                drawing.markup_label(
                    position,
                    rotation,
                    style.height.to_mm(),
                    alignment,
                    name.as_str(),
                    color,
                    false,
                    false,
                    true,
                );
                continue;
            }
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
