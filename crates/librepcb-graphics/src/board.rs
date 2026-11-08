//! Port of libs/librepcb/core/project/board/boardpainter.{h,cpp}.
//! Owned layer batches reuse the core's manufacturing geometry and font paths.
use crate::{Drawing, Error, GraphicsSettings, Result};
use kurbo::{BezPath, Shape};
use librepcb_canvas::convert;
use librepcb_core::{
    geometry::{Circle, Pad, PadGeometry, Path, Polygon, TraceAnchor},
    project::{
        Project,
        board::{Board, BoardDevice, BoardStrokeTextData, ViaView},
    },
    types::{Angle, Layer, Length, Point, PositiveLength},
    utils::transform::Transform,
};
use std::collections::BTreeMap;

#[derive(Default)]
struct Content {
    areas: Vec<BezPath>,
    tht_pads: Vec<BezPath>,
    vias: Vec<BezPath>,
    traces: Vec<(Point, Point, f64)>,
    polygons: Vec<Polygon>,
    circles: Vec<Circle>,
    holes: Vec<(Path, PositiveLength)>,
    pad_holes: Vec<(Path, PositiveLength)>,
}
type Layers = BTreeMap<String, Content>;
fn layer<'a>(layers: &'a mut Layers, role: &str) -> &'a mut Content {
    layers.entry(role.into()).or_default()
}
fn geometry_path(geometry: &PadGeometry) -> Result<BezPath> {
    let mut path = convert::paths(&geometry.to_outlines()?);
    path.extend(
        convert::paths(&geometry.to_hole_outlines())
            .elements()
            .iter()
            .copied(),
    );
    Ok(path)
}
fn add_pad(
    layers: &mut Layers,
    pad: &Pad,
    transform: &Transform,
    geometries: BTreeMap<Layer, Vec<PadGeometry>>,
) -> Result<()> {
    for (on_layer, geometries) in geometries {
        for geometry in geometries {
            let path = convert::transform(transform) * geometry_path(&geometry)?;
            if pad.is_tht() && on_layer.is_copper() {
                let generic = &mut layer(layers, "board_pads").areas;
                if !generic.contains(&path) {
                    generic.push(path.clone());
                }
                layer(layers, on_layer.color_role()).tht_pads.push(path);
            } else {
                layer(layers, on_layer.color_role()).areas.push(path);
            }
        }
    }
    for hole in pad.holes().iter() {
        layer(layers, "board_holes")
            .pad_holes
            .push((transform.map(hole.path()), hole.diameter()));
    }
    Ok(())
}
fn add_hole(
    layers: &mut Layers,
    path: Path,
    diameter: PositiveLength,
    offset: Option<Length>,
) -> Result<()> {
    layer(layers, "board_holes")
        .holes
        .push((path.clone(), diameter));
    // BoardPainter includes the unexpanded hole even when the mask offset is
    // absent. Keep this source behavior for the 2D stop-mask layer drawing.
    let geometry = PadGeometry::stroke(
        diameter,
        path.try_into()
            .map_err(|_| Error::Model("Empty hole path".into()))?,
        Default::default(),
    )
    .with_offset(offset.unwrap_or(Length::ZERO));
    let path = geometry_path(&geometry)?;
    for role in ["board_stop_mask_top", "board_stop_mask_bottom"] {
        layer(layers, role).areas.push(path.clone());
    }
    Ok(())
}
fn add_text(
    layers: &mut Layers,
    project: &Project,
    board: &Board,
    device: Option<&BoardDevice>,
    text: &BoardStrokeTextData,
) -> Result<()> {
    for path in text.paths(project, board, device)? {
        layer(layers, text.layer().color_role())
            .polygons
            .push(Polygon::new(
                text.uuid(),
                text.layer(),
                text.stroke_width(),
                false,
                false,
                text.transform().map(&path),
            ));
    }
    Ok(())
}

/// Draws a board using its current plane fragments. Callers must rebuild planes
/// first when a current fill is required; the output job runner does this.
/// Building this detached drawing never changes the project or its journal.
pub fn board_drawing(
    project: &Project,
    board: &Board,
    settings: &GraphicsSettings,
) -> Result<Drawing> {
    let mut layers = Layers::new();
    let copper = board.copper_layers();
    let rules = board.design_rules();
    for device in board.devices().values() {
        let transform = device.transform();
        let footprint = device.footprint(project.library())?;
        for pad in device.pads(project.library(), project.circuit())? {
            add_pad(
                &mut layers,
                pad.properties(),
                &Transform::new(pad.position(), pad.rotation(), pad.mirrored()),
                pad.geometries(
                    &copper,
                    rules,
                    &board.anchor_trace_layers(pad.trace_anchor()),
                ),
            )?;
        }
        for polygon in footprint.polygons().iter() {
            let mut polygon = polygon.clone();
            polygon.set_layer(transform.map(&polygon.layer()));
            polygon.set_path(transform.map(polygon.path()));
            layer(&mut layers, polygon.layer().color_role())
                .polygons
                .push(polygon);
        }
        for circle in footprint.circles().iter() {
            let mut circle = circle.clone();
            circle.set_layer(transform.map(&circle.layer()));
            circle.set_center(transform.map(&circle.center()));
            layer(&mut layers, circle.layer().color_role())
                .circles
                .push(circle);
        }
        let masks = BoardDevice::hole_stop_mask_offsets(footprint, rules);
        for hole in footprint.holes().iter() {
            add_hole(
                &mut layers,
                transform.map(hole.path()),
                hole.diameter(),
                masks.get(&hole.uuid()).copied().flatten(),
            )?;
        }
        for text in device.stroke_texts().values() {
            add_text(&mut layers, project, board, Some(device), text)?;
        }
    }
    for (id, plane) in board.planes() {
        if let Some(fragments) = board.derived().plane_fragments().get(id) {
            for path in fragments {
                layer(&mut layers, plane.layer().color_role())
                    .areas
                    .push(convert::path(path));
            }
        }
    }
    for polygon in board.polygons().values() {
        layer(&mut layers, polygon.layer().color_role())
            .polygons
            .push(Polygon::new(
                polygon.uuid(),
                polygon.layer(),
                polygon.line_width(),
                polygon.is_filled(),
                polygon.is_grab_area(),
                polygon.path().clone(),
            ));
    }
    for text in board.stroke_texts().values() {
        add_text(&mut layers, project, board, None, text)?;
    }
    for hole in board.holes().values() {
        add_hole(
            &mut layers,
            (**hole.path()).clone(),
            hole.diameter(),
            hole.stop_mask_offset(rules),
        )?;
    }
    for segment in board.net_segments().values() {
        for pad in segment.pads().values() {
            let properties = pad.pad();
            add_pad(
                &mut layers,
                properties,
                &Transform::new(properties.position(), properties.rotation(), false),
                pad.geometries(
                    &copper,
                    rules,
                    &board.anchor_trace_layers(TraceAnchor::Pad(properties.uuid())),
                ),
            )?;
        }
        for via in segment.vias().values() {
            let view = ViaView::new(via, board, segment.net(), project.circuit())?;
            let center = convert::point(via.position());
            let mut path = kurbo::Circle::new(center, view.size().to_mm() / 2.).to_path(0.0001);
            path.extend(
                kurbo::Circle::new(center, view.drill_diameter().to_mm() / 2.).to_path(0.0001),
            );
            let mut on_board = false;
            for on_layer in &copper {
                if on_layer.copper_number() >= via.start_layer().copper_number()
                    && on_layer.copper_number() <= via.end_layer().copper_number()
                {
                    layer(&mut layers, on_layer.color_role())
                        .vias
                        .push(path.clone());
                    on_board = true;
                }
            }
            if on_board {
                let areas = &mut layer(&mut layers, "board_vias").areas;
                if !areas.contains(&path) {
                    areas.push(path);
                }
            }
            for (role, diameter) in [
                ("board_stop_mask_top", view.stop_mask_diameter_top()),
                ("board_stop_mask_bottom", view.stop_mask_diameter_bottom()),
            ] {
                if let Some(diameter) = diameter {
                    layer(&mut layers, role)
                        .areas
                        .push(kurbo::Circle::new(center, diameter.to_mm() / 2.).to_path(0.0001));
                }
            }
        }
        for trace in segment.traces().values() {
            let position = |anchor| {
                board
                    .anchor_position(segment, anchor, project.library(), project.circuit())
                    .ok_or_else(|| Error::Model("Missing trace anchor".into()))
            };
            layer(&mut layers, trace.layer().color_role()).traces.push((
                position(trace.p1())?,
                position(trace.p2())?,
                trace.width().to_mm(),
            ));
        }
    }
    let on_copper = settings
        .paint_order
        .iter()
        .any(|r| r.starts_with("board_copper_"));
    let pad_holes = settings.paint_order.iter().any(|r| r == "board_holes")
        && !settings.paint_order.iter().any(|r| r == "board_pads");
    let identity = Transform::new(Point::ORIGIN, Angle::DEG0, false);
    let mut drawing = Drawing::new();
    for role in settings.paint_order.iter().rev() {
        if on_copper && matches!(role.as_str(), "board_pads" | "board_vias") {
            continue;
        }
        let Some(content) = layers.get(role) else {
            continue;
        };
        for area in &content.areas {
            drawing.area(area.clone(), settings.color(role));
        }
        for area in &content.tht_pads {
            drawing.area(area.clone(), settings.color("board_pads"));
        }
        for area in &content.vias {
            drawing.area(area.clone(), settings.color("board_vias"));
        }
        for (a, b, w) in &content.traces {
            drawing.line(*a, *b, *w, settings.color(role), settings);
        }
        for polygon in &content.polygons {
            drawing.polygon(polygon, &identity, settings);
        }
        for circle in &content.circles {
            drawing.circle(circle, &identity, settings);
        }
        for (path, diameter) in content
            .holes
            .iter()
            .chain(content.pad_holes.iter().filter(|_| pad_holes))
        {
            drawing.slot(path, *diameter, settings.color(role), None, settings);
        }
    }
    Ok(drawing)
}
