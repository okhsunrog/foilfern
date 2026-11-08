//! Port of InteractiveHtmlBom::addFootprint() and its resolved pad transforms.
use super::{BoardInteractiveBomGenerator, point};
use crate::{
    geometry::Path,
    project::{
        Result,
        board::{BoardDevice, FootprintPadView},
    },
    types::{Angle, Layer, Length, Orientation, Point, UnsignedLength},
    utils::{toolbox, transform::Transform},
};
use interactive_html_bom as ib;

impl BoardInteractiveBomGenerator<'_> {
    pub(super) fn footprint(
        &self,
        data: &mut ib::InteractiveHtmlBom,
        device: &BoardDevice,
        fields: &[String],
        mount: bool,
    ) -> Result<usize> {
        let footprint = device.footprint(self.project.library())?;
        let (low, high) = footprint.calculate_bounding_rect(true);
        let transform = device.transform();
        let mut views = device.pads(self.project.library(), self.project.circuit())?;
        views.sort_by_key(FootprintPadView::uuid);
        let pads = views
            .iter()
            .map(|pad| self.pad(pad, views.len()))
            .collect::<Result<Vec<_>>>()?;
        let mut origin = device.position();
        if device.mirrored() {
            origin -=
                Point::new(high.x + low.x, Length::ZERO).rotated(device.rotation(), Point::ORIGIN);
        }
        let id = data.add_footprint(ib::Footprint::new(
            if device.mirrored() {
                ib::Layer::Back
            } else {
                ib::Layer::Front
            },
            point(origin),
            device.rotation().to_deg() as f32,
            (low.x.to_mm() as f32, -high.y.to_mm() as f32),
            (high.x.to_mm() as f32, -low.y.to_mm() as f32),
            fields,
            &pads,
            mount,
        ));
        for polygon in footprint.polygons().iter() {
            let mut path = polygon.path().clone();
            if polygon.layer().polygons_represent_areas() {
                path.close();
            }
            self.add_drawing(
                data,
                transform.map(&polygon.layer()),
                &transform.map(&path),
                polygon.line_width(),
                polygon.is_filled(),
            );
        }
        for circle in footprint.circles().iter() {
            self.add_drawing(
                data,
                transform.map(&circle.layer()),
                &transform.map(&Path::circle(circle.diameter()).translated(circle.center())),
                circle.line_width(),
                circle.is_filled(),
            );
        }
        for text in device.stroke_texts().values() {
            self.add_text(data, Some(device), text)?;
        }
        for hole in footprint.holes().iter() {
            for outline in hole.path().to_outline_strokes(hole.diameter()) {
                self.add_drawing(
                    data,
                    Layer::BOARD_CUTOUTS,
                    &transform.map(&outline),
                    UnsignedLength::ZERO,
                    false,
                );
            }
        }
        Ok(id)
    }
    fn pad(&self, pad: &FootprintPadView<'_>, count: usize) -> Result<ib::Pad> {
        let mut offset = Point::ORIGIN;
        let mut rotation = Angle::DEG0;
        let drill = pad.properties().holes().iter().next().map(|hole| {
            let vertices = hole.path().vertices();
            if hole.is_slot() {
                let (p0, p1) = (vertices[0].pos, vertices[1].pos);
                rotation = toolbox::angle_between_points(p0, p1);
                if pad.mirrored() {
                    rotation = -rotation;
                }
                offset = (p0 + p1) / 2;
                (
                    ((p1 - p0).length() + *hole.diameter()).to_mm() as f32,
                    hole.diameter().to_mm() as f32,
                )
            } else {
                offset = vertices[0].pos;
                (
                    hole.diameter().to_mm() as f32,
                    hole.diameter().to_mm() as f32,
                )
            }
        });
        let geometries = pad.geometries(
            &self.board.copper_layers(),
            self.board.design_rules(),
            &self.board.anchor_trace_layers(pad.trace_anchor()),
        );
        let mut outline =
            if let Some(geometry) = geometries.get(&pad.solder_layer()).and_then(|g| g.first()) {
                geometry
                    .to_outlines()?
                    .into_iter()
                    .next()
                    .unwrap_or_default()
            } else {
                Path::default()
            };
        outline.close();
        outline = outline.translated(-offset);
        if pad.mirrored() {
            outline = outline.mirrored(Orientation::Horizontal, Point::ORIGIN);
        }
        outline = outline.rotated(-rotation, Point::ORIGIN);
        let transform = Transform::new(pad.position(), pad.rotation(), pad.mirrored());
        let layers = [
            (Layer::TOP_COPPER, ib::Layer::Front),
            (Layer::BOT_COPPER, ib::Layer::Back),
        ]
        .into_iter()
        .filter(|(layer, _)| pad.is_on_layer(*layer))
        .map(|(_, side)| side)
        .collect::<Vec<_>>();
        let pin1 = count > 1
            && pad
                .package_pad()
                .is_some_and(|p| matches!(p.name().as_str(), "1" | "A"));
        Ok(ib::Pad::new(
            &layers,
            point(transform.map(&offset)),
            (pad.rotation() + rotation).to_deg() as f32,
            &outline.to_svg_path_mm(),
            drill,
            self.net(pad.net()),
            pin1,
        ))
    }
}
