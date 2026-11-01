//! Port of BoardGerberExport's geometry drawing in
//! libs/librepcb/core/project/board/boardgerberexport.cpp.

use super::BoardGerberExport;
use crate::export::{ApertureFunction as Function, GerberGenerator, ObjectAttributes};
use crate::geometry::{
    Pad, PadFunction, PadGeometry, PadGeometryShape, Path, StraightAreaPath, TraceAnchor,
};
use crate::project::board::{
    BoardDevice, BoardStrokeTextData, PadOnBoard, ResolvedDevice, ViaView,
};
use crate::project::{ComponentSignalRef, Error, NetSignalId, Result};
use crate::types::{Angle, Layer, Length, Orientation, Point, PositiveLength, UnsignedLength};
use crate::utils::transform::Transform;

impl BoardGerberExport<'_> {
    pub(super) fn net_name(&self, net: Option<NetSignalId>) -> Result<Option<&str>> {
        net.map(|id| {
            self.project
                .circuit()
                .net_signal(id)
                .map(|n| n.name().as_str())
                .ok_or(Error::InexistentNetSignal(id.0))
        })
        .transpose()
    }
    pub(super) fn signal_name(&self, signal: Option<ComponentSignalRef>) -> Result<&str> {
        let Some(signal) = signal else {
            return Ok("");
        };
        let component = self
            .project
            .circuit()
            .component_instance(signal.component)
            .ok_or(Error::InexistentComponent(signal.component.0))?;
        let library = self
            .project
            .library()
            .component(&component.lib_component())
            .ok_or(Error::MissingLibraryComponent(component.lib_component()))?;
        library
            .signals()
            .by_uuid(&signal.signal)
            .map(|s| s.name().as_str())
            .ok_or(Error::InexistentComponentSignal(signal.signal))
    }
    pub(super) fn draw_layer(&self, generator: &mut GerberGenerator, layer: Layer) -> Result<()> {
        for device in self.board.devices().values() {
            self.draw_device(generator, device, layer)?;
        }
        self.draw_board(generator, layer)
    }
    pub(super) fn draw_board(&self, generator: &mut GerberGenerator, layer: Layer) -> Result<()> {
        let copper = self.board.copper_layers();
        for segment in self.board.net_segments().values() {
            let net = self.net_name(segment.net())?.unwrap_or("N/C");
            for pad in segment.pads().values() {
                let p = pad.pad();
                let geometries = PadOnBoard {
                    pad: p,
                    mirrored: false,
                }
                .geometries(
                    &copper,
                    self.board.design_rules(),
                    &self.board.anchor_trace_layers(TraceAnchor::Pad(pad.uuid())),
                );
                for geometry in geometries.get(&layer).into_iter().flatten() {
                    let attributes = pad_attributes(p, layer, net, "", "", "");
                    self.draw_pad(
                        generator,
                        geometry,
                        Transform::new(p.position(), p.rotation(), false),
                        &attributes,
                    )?;
                }
            }
            for via in segment.vias().values() {
                let view = ViaView::new(via, self.board, segment.net(), self.project.circuit())?;
                let mask = if layer.is_stop_mask() {
                    if layer.is_top() {
                        view.stop_mask_diameter_top()
                    } else {
                        view.stop_mask_diameter_bottom()
                    }
                } else {
                    None
                };
                let copper = via.is_on_layer(layer);
                if copper || mask.is_some() {
                    let attrs = if copper {
                        ObjectAttributes {
                            function: Some(Function::ViaPad),
                            net: Some(net),
                            ..Default::default()
                        }
                    } else {
                        ObjectAttributes::default()
                    };
                    generator.flash_circle(via.position(), mask.unwrap_or(view.size()), &attrs);
                }
            }
            for trace in segment.traces().values() {
                if trace.layer() != layer {
                    continue;
                }
                let position = |anchor| {
                    self.board
                        .anchor_position(
                            segment,
                            anchor,
                            self.project.library(),
                            self.project.circuit(),
                        )
                        .ok_or(Error::InexistentTraceAnchor {
                            anchor,
                            device_missing: false,
                        })
                };
                generator.draw_line(
                    position(trace.p1())?,
                    position(trace.p2())?,
                    trace.width().into(),
                    &ObjectAttributes {
                        function: Some(Function::Conductor),
                        net: Some(net),
                        ..Default::default()
                    },
                );
            }
        }
        for plane in self.board.planes().values().filter(|p| p.layer() == layer) {
            let attrs = ObjectAttributes {
                function: Some(Function::Conductor),
                net: self.net_name(plane.net())?,
                ..Default::default()
            };
            for fragment in self.board.derived().fragments_of(plane.id()) {
                generator.draw_path_area(fragment, &attrs);
            }
        }
        let attrs = graphics_attributes(layer, "");
        for polygon in self
            .board
            .polygons()
            .values()
            .filter(|p| p.layer() == layer)
        {
            draw_polygon(
                generator,
                layer,
                polygon.path(),
                polygon.line_width(),
                polygon.is_filled(),
                &attrs,
            );
        }
        for text in self
            .board
            .stroke_texts()
            .values()
            .filter(|t| t.layer() == layer)
        {
            self.draw_text(generator, text, None, attrs)?;
        }
        if layer.is_stop_mask() {
            for hole in self.board.holes().values() {
                if let Some(offset) = hole.stop_mask_offset(self.board.design_rules()) {
                    draw_hole_mask(
                        generator,
                        &cleaned(hole.path()),
                        *hole.diameter() + offset * 2,
                    )?;
                }
            }
        }
        Ok(())
    }
    pub(super) fn draw_device(
        &self,
        generator: &mut GerberGenerator,
        device: &BoardDevice,
        layer: Layer,
    ) -> Result<()> {
        let component = self
            .project
            .circuit()
            .component_instance(device.component())
            .ok_or(Error::InexistentComponent(device.component().0))?;
        let attrs = graphics_attributes(layer, component.name().as_str());
        let resolved = ResolvedDevice::resolve(device, self.project.library())?;
        let transform = device.transform();
        for pad in self.pads(device)? {
            let geometries = pad.geometries(
                &self.board.copper_layers(),
                self.board.design_rules(),
                &self.board.anchor_trace_layers(pad.trace_anchor()),
            );
            for geometry in geometries.get(&layer).into_iter().flatten() {
                let attributes = pad_attributes(
                    pad.properties(),
                    layer,
                    self.net_name(pad.net())?.unwrap_or("N/C"),
                    component.name().as_str(),
                    pad.package_pad()
                        .map(|p| p.name().as_str())
                        .unwrap_or_default(),
                    self.signal_name(pad.component_signal())?,
                );
                self.draw_pad(
                    generator,
                    geometry,
                    Transform::new(pad.position(), pad.rotation(), pad.mirrored()),
                    &attributes,
                )?;
            }
        }
        let mut polygons: Vec<_> = resolved.footprint.polygons().iter().collect();
        polygons.sort_unstable_by_key(|p| p.uuid());
        for polygon in polygons {
            if transform.map(&polygon.layer()) == layer {
                draw_polygon(
                    generator,
                    layer,
                    &transform.map(polygon.path()),
                    polygon.line_width(),
                    polygon.is_filled(),
                    &attrs,
                );
            }
        }
        let mut circles: Vec<_> = resolved.footprint.circles().iter().collect();
        circles.sort_unstable_by_key(|p| p.uuid());
        for circle in circles {
            if transform.map(&circle.layer()) != layer {
                continue;
            }
            let position = transform.map(&circle.center());
            if circle.is_filled() {
                generator.draw_path_area(
                    &Path::circle(circle.diameter() + circle.line_width()).translated(position),
                    &attrs,
                );
            } else {
                generator.draw_path_outline(
                    &Path::circle(circle.diameter()).translated(position),
                    layer_width(circle.line_width(), layer),
                    &attrs,
                );
            }
        }
        for text in device
            .stroke_texts()
            .values()
            .filter(|t| t.layer() == layer)
        {
            self.draw_text(generator, text, Some(device), attrs)?;
        }
        if layer.is_stop_mask() {
            let offsets =
                BoardDevice::hole_stop_mask_offsets(resolved.footprint, self.board.design_rules());
            let mut holes: Vec<_> = resolved.footprint.holes().iter().collect();
            holes.sort_unstable_by_key(|h| h.uuid());
            for hole in holes {
                if let Some(Some(offset)) = offsets.get(&hole.uuid()) {
                    draw_hole_mask(
                        generator,
                        &transform.map(&cleaned(hole.path())),
                        *hole.diameter() + *offset * 2,
                    )?;
                }
            }
        }
        Ok(())
    }
    fn draw_text(
        &self,
        generator: &mut GerberGenerator,
        text: &BoardStrokeTextData,
        device: Option<&BoardDevice>,
        mut attrs: ObjectAttributes<'_>,
    ) -> Result<()> {
        attrs.function = text.layer().is_copper().then_some(Function::NonConductor);
        for path in text.paths(self.project, self.board, device)? {
            generator.draw_path_outline(
                &text.transform().map(&path),
                layer_width(text.stroke_width(), text.layer()),
                &attrs,
            );
        }
        Ok(())
    }
    fn draw_pad(
        &self,
        generator: &mut GerberGenerator,
        geometry: &PadGeometry,
        transform: Transform,
        attrs: &ObjectAttributes<'_>,
    ) -> Result<()> {
        let outline = |generator: &mut GerberGenerator| -> Result<()> {
            for path in geometry.to_outlines()? {
                let mut path = path.flattened_arcs(PositiveLength::new(Length::new(5000))?);
                if transform.mirrored {
                    path = path.mirrored(Orientation::Horizontal, Point::ORIGIN);
                }
                generator.flash_outline(
                    transform.position,
                    &StraightAreaPath::new(path)?,
                    transform.rotation,
                    attrs,
                );
            }
            Ok(())
        };
        let width = geometry.width();
        match geometry.shape() {
            PadGeometryShape::RoundedRect | PadGeometryShape::RoundedOctagon => {
                if width > Length::ZERO && geometry.height() > Length::ZERO {
                    let width = PositiveLength::new(width)?;
                    let height = PositiveLength::new(geometry.height())?;
                    if geometry.shape() == PadGeometryShape::RoundedRect {
                        generator.flash_rect(
                            transform.position,
                            width,
                            height,
                            geometry.corner_radius(),
                            transform.rotation,
                            attrs,
                        );
                    } else {
                        generator.flash_octagon(
                            transform.position,
                            width,
                            height,
                            geometry.corner_radius(),
                            transform.rotation,
                            attrs,
                        );
                    }
                }
            }
            PadGeometryShape::Stroke => {
                if width > Length::ZERO && !geometry.path().vertices().is_empty() {
                    let path = transform.map(geometry.path());
                    let vertices = path.vertices();
                    let height = PositiveLength::new(width)?;
                    if vertices.len() == 1 {
                        generator.flash_circle(vertices[0].pos, height, attrs);
                    } else if vertices.len() == 2 && vertices[0].angle == Angle::DEG0 {
                        let p0 = vertices[0].pos;
                        let p1 = vertices[1].pos;
                        let delta = p1 - p0;
                        let rotation =
                            Angle::from_rad(libm::atan2(delta.y.to_mm(), delta.x.to_mm()))?;
                        generator.flash_obround(
                            (p0 + p1) / 2,
                            height + delta.length(),
                            height,
                            rotation,
                            attrs,
                        );
                    } else {
                        outline(generator)?;
                    }
                }
            }
            PadGeometryShape::Custom => outline(generator)?,
        }
        Ok(())
    }
}

pub(super) fn layer_width(width: UnsignedLength, layer: Layer) -> UnsignedLength {
    if layer.is_board_edge() {
        width.max(UnsignedLength::new(Length::new(1000)).expect("positive constant"))
    } else {
        width
    }
}
fn graphics_attributes(layer: Layer, component: &str) -> ObjectAttributes<'_> {
    ObjectAttributes {
        function: if layer.is_board_edge() {
            Some(Function::Profile)
        } else if layer.is_copper() {
            Some(Function::Conductor)
        } else {
            None
        },
        net: layer.is_copper().then_some(""),
        component,
        ..Default::default()
    }
}
fn pad_attributes<'a>(
    pad: &Pad,
    layer: Layer,
    net: &'a str,
    component: &'a str,
    pin: &'a str,
    signal: &'a str,
) -> ObjectAttributes<'a> {
    let mut attrs = ObjectAttributes {
        component,
        ..Default::default()
    };
    if layer.is_copper() {
        attrs.function = Some(match pad.function() {
            PadFunction::ThermalPad => Function::HeatsinkPad,
            PadFunction::BgaPad => Function::BgaPadCopperDefined,
            PadFunction::EdgeConnectorPad => Function::ConnectorPad,
            PadFunction::TestPad => Function::TestPad,
            PadFunction::LocalFiducial => Function::FiducialPadLocal,
            PadFunction::GlobalFiducial => Function::FiducialPadGlobal,
            _ if pad.is_tht() => Function::ComponentPad,
            _ => Function::SmdPadCopperDefined,
        });
        attrs.net = Some(net);
        attrs.pin = pin;
        attrs.signal = signal;
    }
    attrs
}
fn draw_polygon(
    generator: &mut GerberGenerator,
    layer: Layer,
    path: &Path,
    width: UnsignedLength,
    filled: bool,
    attrs: &ObjectAttributes<'_>,
) {
    if width > UnsignedLength::ZERO || !filled || !path.is_closed() {
        generator.draw_path_outline(path, layer_width(width, layer), attrs);
    }
    if filled && path.is_closed() {
        generator.draw_path_area(path, attrs);
    }
}
fn draw_hole_mask(generator: &mut GerberGenerator, path: &Path, diameter: Length) -> Result<()> {
    if diameter > Length::ZERO {
        if path.vertices().len() == 1 {
            generator.flash_circle(
                path.vertices()[0].pos,
                PositiveLength::new(diameter)?,
                &ObjectAttributes::default(),
            );
        } else {
            generator.draw_path_outline(
                path,
                UnsignedLength::new(diameter)?,
                &ObjectAttributes::default(),
            );
        }
    }
    Ok(())
}

fn cleaned(path: &Path) -> Path {
    let mut result = path.clone();
    result.clean();
    result
}
