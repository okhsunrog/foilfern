//! Port of libs/librepcb/core/project/board/boardpickplacegenerator.{h,cpp}.
//!
//! Resolves the existing project, library and pad read views into the
//! existing pick&place data model. No separate serialization or geometry
//! algorithm is needed. Values use Rust whitespace simplification.

use super::{Board, PadOnBoard, ResolvedDevice};
use crate::export::{BoardSide, PickPlaceData, PickPlaceDataItem, PickPlaceType};
use crate::library::{LibraryBaseElement, pkg::AssemblyType};
use crate::project::{AssemblyVariantId, Error, Project, ProjectAttributeLookup, Result};
use crate::types::Layer;
use std::collections::BTreeSet;

/// Prepares assembly and fiducial placements for the CSV/Gerber generators.
#[derive(Debug, Clone, Copy)]
pub struct BoardPickPlaceGenerator<'a> {
    project: &'a Project,
    board: &'a Board,
    variant: AssemblyVariantId,
}

impl<'a> BoardPickPlaceGenerator<'a> {
    /// Creates a generator for one board and one assembly variant.
    pub fn new(project: &'a Project, board: &'a Board, variant: AssemblyVariantId) -> Self {
        Self {
            project,
            board,
            variant,
        }
    }

    /// Returns all placed devices (with their mount flags) and all
    /// fiducials. Writers can filter the resulting items by type and side.
    pub fn generate(&self) -> Result<PickPlaceData> {
        let mut data = PickPlaceData::new(
            self.project.metadata().name.as_str(),
            self.project.metadata().version.as_str(),
            self.board.name().as_str(),
        );
        let locale = &self.project.settings().locale_order;
        let mut exported_designators = BTreeSet::new();
        for device in self.board.devices().values() {
            let resolved = ResolvedDevice::resolve(device, self.project.library())?;
            let component = self
                .project
                .circuit()
                .component_instance(device.component())
                .ok_or(Error::InexistentComponent(device.component().0))?;
            let parts = device.parts(self.project.circuit(), Some(self.variant))?;
            let lookup =
                ProjectAttributeLookup::device(self.project, self.board, device, parts.first());
            let designator = component.name().to_string();
            let value = lookup
                .substitute("{{MPN or VALUE or DEVICE}}")?
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let device_name = resolved.device.metadata().names().value(locale).to_string();
            let package_name = resolved
                .package
                .metadata()
                .names()
                .value(locale)
                .to_string();
            let base = PickPlaceDataItem {
                designator: designator.clone(),
                value,
                device_name,
                package_name,
                position: device.position(),
                rotation: if device.mirrored() {
                    -device.rotation()
                } else {
                    device.rotation()
                },
                board_side: if device.mirrored() {
                    BoardSide::Bottom
                } else {
                    BoardSide::Top
                },
                item_type: match resolved.package.resolved_assembly_type() {
                    AssemblyType::Tht => PickPlaceType::Tht,
                    AssemblyType::Smt => PickPlaceType::Smt,
                    AssemblyType::Mixed => PickPlaceType::Mixed,
                    AssemblyType::Auto | AssemblyType::None | AssemblyType::Other => {
                        PickPlaceType::Other
                    }
                },
                mount: !parts.is_empty(),
            };
            let mut fiducials = Vec::new();
            let mut pads = device.pads(self.project.library(), self.project.circuit())?;
            // Upstream BI_Device stores its pads in a QMap keyed by UUID.
            pads.sort_unstable_by_key(|pad| pad.uuid());
            for pad in pads {
                if !pad.properties().function_is_fiducial() {
                    continue;
                }
                for (layer, side) in [
                    (Layer::TOP_COPPER, BoardSide::Top),
                    (Layer::BOT_COPPER, BoardSide::Bottom),
                ] {
                    if pad.is_on_layer(layer) {
                        fiducials.push(PickPlaceDataItem {
                            position: pad.position(),
                            rotation: if pad.mirrored() {
                                -pad.rotation()
                            } else {
                                pad.rotation()
                            },
                            board_side: side,
                            item_type: PickPlaceType::Fiducial,
                            mount: true,
                            ..base.clone()
                        });
                    }
                }
            }
            if fiducials.len() > 1 {
                for (i, item) in fiducials.iter_mut().enumerate() {
                    item.designator = format!("{}:{}", item.designator, i + 1);
                }
            }
            for item in fiducials {
                data.add_item(item);
            }
            data.add_item(base);
            exported_designators.insert(designator);
        }
        let mut fiducial_number = 0;
        for segment in self.board.net_segments().values() {
            for pad in segment.pads().values() {
                let properties = pad.pad();
                if !properties.function_is_fiducial() {
                    continue;
                }
                let designator = loop {
                    fiducial_number += 1;
                    let candidate = format!("FID{fiducial_number}");
                    if !exported_designators.contains(&candidate) {
                        break candidate;
                    }
                };
                let view = PadOnBoard {
                    pad: properties,
                    mirrored: false,
                };
                for (layer, side) in [
                    (Layer::TOP_COPPER, BoardSide::Top),
                    (Layer::BOT_COPPER, BoardSide::Bottom),
                ] {
                    if view.is_on_layer(layer) {
                        data.add_item(PickPlaceDataItem {
                            designator: designator.clone(),
                            value: String::new(),
                            device_name: String::new(),
                            package_name: String::new(),
                            position: properties.position(),
                            rotation: properties.rotation(),
                            board_side: side,
                            item_type: PickPlaceType::Fiducial,
                            mount: true,
                        });
                    }
                }
            }
        }
        Ok(data)
    }
}
