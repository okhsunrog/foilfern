//! Port of BoardGerberExport's Gerber X3 component export in
//! libs/librepcb/core/project/board/boardgerberexport.cpp.

use super::{BoardGerberExport, drawing::layer_width};
use crate::export::{
    ApertureFunction, BoardSide, ComponentAttributes, GerberGenerator, MountType, ObjectAttributes,
};
use crate::geometry::Path;
use crate::library::{LibraryBaseElement, pkg::AssemblyType};
use crate::project::board::{BoardDevice, PadOnBoard, ResolvedDevice};
use crate::project::{AssemblyVariantId, Error, ProjectAttributeLookup, Result};
use crate::types::Layer;

impl BoardGerberExport<'_> {
    /// Generates a Gerber X3 assembly layer including body/courtyard outlines,
    /// pins, attributes, fiducials and board edges.
    pub fn generate_component_layer(
        &self,
        side: BoardSide,
        variant: AssemblyVariantId,
    ) -> Result<String> {
        let bottom = side == BoardSide::Bottom;
        let mut generator = GerberGenerator::new(&self.info);
        generator.set_file_function_component(if bottom { self.bottom_number() } else { 1 }, side);
        for polygon in self
            .board
            .polygons()
            .values()
            .filter(|p| p.layer().is_board_edge())
        {
            generator.draw_path_outline(
                polygon.path(),
                layer_width(polygon.line_width(), polygon.layer()),
                &ObjectAttributes {
                    function: Some(ApertureFunction::Profile),
                    ..Default::default()
                },
            );
        }
        for device in self
            .board
            .devices()
            .values()
            .filter(|d| d.mirrored() == bottom)
        {
            let parts = device.parts(self.project.circuit(), Some(variant))?;
            let Some(part) = parts.first() else {
                continue;
            };
            let resolved = ResolvedDevice::resolve(device, self.project.library())?;
            let component = self
                .project
                .circuit()
                .component_instance(device.component())
                .ok_or(Error::InexistentComponent(device.component().0))?;
            let lookup =
                ProjectAttributeLookup::device(self.project, self.board, device, Some(part));
            let value = lookup
                .substitute(&lookup.value("VALUE")?.unwrap_or_default())?
                .trim()
                .to_owned();
            let mpn = simplified(&lookup.value("MPN")?.unwrap_or_default());
            let manufacturer = simplified(&lookup.value("MANUFACTURER")?.unwrap_or_default());
            let attributes = ComponentAttributes {
                designator: component.name().as_str(),
                value: &value,
                mount_type: match resolved.package.resolved_assembly_type() {
                    AssemblyType::Tht | AssemblyType::Mixed => MountType::Tht,
                    AssemblyType::Smt => MountType::Smt,
                    _ => MountType::Other,
                },
                manufacturer: &manufacturer,
                mpn: &mpn,
                footprint: resolved.package.metadata().names().default_value().as_str(),
            };
            generator.flash_component(device.position(), device.rotation(), &attributes);
            let mut body = self.component_outlines(
                device,
                if bottom {
                    Layer::BOT_PACKAGE_OUTLINES
                } else {
                    Layer::TOP_PACKAGE_OUTLINES
                },
            )?;
            if body.is_empty() {
                body = self.component_outlines(
                    device,
                    if bottom {
                        Layer::BOT_DOCUMENTATION
                    } else {
                        Layer::TOP_DOCUMENTATION
                    },
                )?;
            }
            for path in body {
                generator.draw_component_outline(
                    &path,
                    device.rotation(),
                    &attributes,
                    Some(ApertureFunction::ComponentOutlineBody),
                );
            }
            for path in self.component_outlines(
                device,
                if bottom {
                    Layer::BOT_COURTYARD
                } else {
                    Layer::TOP_COURTYARD
                },
            )? {
                generator.draw_component_outline(
                    &path,
                    device.rotation(),
                    &attributes,
                    Some(ApertureFunction::ComponentOutlineCourtyard),
                );
            }
            for pad in self.pads(device)? {
                if pad.properties().function_is_fiducial() {
                    continue;
                }
                let pin = pad
                    .package_pad()
                    .map(|p| p.name().as_str())
                    .unwrap_or_default();
                generator.flash_component_pin(
                    pad.position(),
                    device.rotation(),
                    &attributes,
                    pin,
                    self.signal_name(pad.component_signal())?,
                    pin == "1",
                );
            }
        }
        let layer = if bottom {
            Layer::BOT_COPPER
        } else {
            Layer::TOP_COPPER
        };
        for device in self.board.devices().values() {
            let mut number = 0;
            for pad in self.pads(device)? {
                if !pad.properties().function_is_fiducial() || !pad.is_on_layer(layer) {
                    continue;
                }
                number += 1;
                let resolved = ResolvedDevice::resolve(device, self.project.library())?;
                let component = self
                    .project
                    .circuit()
                    .component_instance(device.component())
                    .ok_or(Error::InexistentComponent(device.component().0))?;
                let lookup = ProjectAttributeLookup::device(self.project, self.board, device, None);
                let value =
                    simplified(&lookup.substitute(&lookup.value("VALUE")?.unwrap_or_default())?);
                let designator = format!("{}:{number}", component.name());
                generator.flash_component(
                    pad.position(),
                    pad.rotation(),
                    &ComponentAttributes {
                        designator: &designator,
                        value: &value,
                        mount_type: MountType::Fiducial,
                        manufacturer: "",
                        mpn: "",
                        footprint: resolved.package.metadata().names().default_value().as_str(),
                    },
                );
            }
        }
        for segment in self.board.net_segments().values() {
            for pad in segment.pads().values() {
                let p = pad.pad();
                if p.function_is_fiducial()
                    && (PadOnBoard {
                        pad: p,
                        mirrored: false,
                    })
                    .is_on_layer(layer)
                {
                    generator.flash_component(
                        p.position(),
                        p.rotation(),
                        &ComponentAttributes {
                            designator: "",
                            value: "",
                            mount_type: MountType::Fiducial,
                            manufacturer: "",
                            mpn: "",
                            footprint: "",
                        },
                    );
                }
            }
        }
        Ok(generator.generate())
    }
    fn component_outlines(&self, device: &BoardDevice, layer: Layer) -> Result<Vec<Path>> {
        let footprint = ResolvedDevice::resolve(device, self.project.library())?.footprint;
        let transform = device.transform();
        let mut paths = Vec::new();
        let mut polygons: Vec<_> = footprint.polygons().iter().collect();
        polygons.sort_unstable_by_key(|p| p.uuid());
        for polygon in polygons {
            if (!polygon.layer().polygons_represent_areas() && !polygon.path().is_closed())
                || polygon.is_filled()
                || transform.map(&polygon.layer()) != layer
            {
                continue;
            }
            paths.push(transform.map(&polygon.path_for_rendering()));
        }
        let mut circles: Vec<_> = footprint.circles().iter().collect();
        circles.sort_unstable_by_key(|p| p.uuid());
        for circle in circles {
            if !circle.is_filled() && transform.map(&circle.layer()) == layer {
                // Upstream omits the circle center here. Retain that rule to
                // keep component-layer files interoperable byte for byte.
                paths.push(transform.map(&Path::circle(circle.diameter())));
            }
        }
        Ok(paths)
    }
}
fn simplified(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
