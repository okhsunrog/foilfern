//! Port of libs/librepcb/core/project/board/boardd356netlistexport.{h,cpp}.
//!
//! Resolves board vias and pads into the existing IPC-D-356A generator.
//! Application version and creation time are caller supplied, like the
//! other manufacturing generators. No file I/O or board mutation occurs.

use super::{Board, FootprintPadView, ViaView};
use crate::export::{D356Land, D356NetlistGenerator, Timestamp};
use crate::geometry::{ComponentSide, Pad};
use crate::project::circuit::Circuit;
use crate::project::{Error, NetSignalId, Project, Result};
use crate::types::Angle;

/// Exports the board's physical connectivity for bare-board electrical tests.
#[derive(Debug, Clone)]
pub struct BoardD356NetlistExport<'a> {
    project: &'a Project,
    board: &'a Board,
    app_version: String,
    creation_date: Timestamp,
}

impl<'a> BoardD356NetlistExport<'a> {
    /// Creates an exporter with explicit file metadata.
    pub fn new(
        project: &'a Project,
        board: &'a Board,
        app_version: impl Into<String>,
        creation_date: Timestamp,
    ) -> Self {
        Self {
            project,
            board,
            app_version: app_version.into(),
            creation_date,
        }
    }

    /// Generates IPC-D-356A text. Like upstream, vias use their original
    /// logical start/end copper numbers here, rather than clipped drill spans.
    pub fn generate(&self) -> Result<String> {
        let mut generator = D356NetlistGenerator::new(
            &self.app_version,
            self.project.metadata().name.as_str(),
            self.project.metadata().version.as_str(),
            self.board.name().as_str(),
            &self.creation_date,
        );
        let circuit = self.project.circuit();
        for segment in self.board.net_segments().values() {
            let net = net_name(circuit, segment.net())?;
            for via in segment.vias().values() {
                let geometry = ViaView::new(via, self.board, segment.net(), circuit)?;
                let covered = geometry.stop_mask_diameter_top().is_none()
                    && geometry.stop_mask_diameter_bottom().is_none();
                let land = D356Land {
                    position: via.position(),
                    width: geometry.size(),
                    height: geometry.size(),
                    rotation: Angle::DEG0,
                };
                let start = via.start_layer().copper_number() as i32 + 1;
                let end = via.end_layer().copper_number() as i32 + 1;
                if via.is_blind() {
                    generator.blind_via(net, &land, geometry.drill_diameter(), start, end, covered);
                } else if via.is_buried() {
                    generator.buried_via(
                        net,
                        via.position(),
                        geometry.drill_diameter(),
                        start,
                        end,
                    );
                } else {
                    generator.through_via(net, &land, geometry.drill_diameter(), covered);
                }
            }
        }
        for device in self.board.devices().values() {
            let component = circuit
                .component_instance(device.component())
                .ok_or(Error::InexistentComponent(device.component().0))?;
            let mut pads = device.pads(self.project.library(), circuit)?;
            pads.sort_unstable_by_key(FootprintPadView::uuid);
            for pad in pads {
                let pad_name = pad
                    .package_pad()
                    .map(|p| p.name().as_str())
                    .unwrap_or_default();
                let rotation = if pad.mirrored() {
                    pad.rotation() + Angle::DEG180
                } else {
                    pad.rotation()
                };
                let export = PadExport {
                    properties: pad.properties(),
                    land: D356Land {
                        position: pad.position(),
                        width: pad.properties().width(),
                        height: pad.properties().height(),
                        rotation,
                    },
                    net: pad.net(),
                    component_name: component.name().as_str(),
                    pad_name,
                    side: pad.component_side(),
                };
                export.write(
                    &mut generator,
                    circuit,
                    self.board.settings().inner_layer_count,
                )?;
            }
        }
        let mut board_pad_number = 0;
        for segment in self.board.net_segments().values() {
            for pad in segment.pads().values() {
                board_pad_number += 1;
                let properties = pad.pad();
                let name = format!("P{board_pad_number}");
                let export = PadExport {
                    properties,
                    land: D356Land {
                        position: properties.position(),
                        width: properties.width(),
                        height: properties.height(),
                        rotation: properties.rotation(),
                    },
                    net: segment.net(),
                    component_name: "BOARD",
                    pad_name: &name,
                    side: properties.component_side(),
                };
                export.write(
                    &mut generator,
                    circuit,
                    self.board.settings().inner_layer_count,
                )?;
            }
        }
        Ok(generator.generate())
    }
}

struct PadExport<'a> {
    properties: &'a Pad,
    land: D356Land,
    net: Option<NetSignalId>,
    component_name: &'a str,
    pad_name: &'a str,
    side: ComponentSide,
}
impl PadExport<'_> {
    fn write(
        &self,
        generator: &mut D356NetlistGenerator,
        circuit: &Circuit,
        inner_layers: u32,
    ) -> Result<()> {
        let net = net_name(circuit, self.net)?;
        if let Some(hole) = self.properties.holes().first() {
            // Upstream exports only the first hole, including for slots.
            generator.tht_pad(
                net,
                self.component_name,
                self.pad_name,
                &self.land,
                hole.diameter(),
            );
        } else {
            let layer = if self.side == ComponentSide::Top {
                1
            } else {
                inner_layers as i32 + 2
            };
            generator.smt_pad(net, self.component_name, self.pad_name, &self.land, layer);
        }
        Ok(())
    }
}

fn net_name(circuit: &Circuit, net: Option<NetSignalId>) -> Result<&str> {
    net.map(|id| {
        circuit
            .net_signal(id)
            .map(|net| net.name().as_str())
            .ok_or(Error::InexistentNetSignal(id.0))
    })
    .transpose()
    .map(|name| name.unwrap_or_default())
}
