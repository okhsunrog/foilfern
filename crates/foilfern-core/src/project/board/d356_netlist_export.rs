//! Port of libs/librepcb/core/project/board/boardd356netlistexport.{h,cpp}.
//!
//! Differences to upstream: the application version and the creation date
//! are passed in ([`ExportInfo`]) instead of taken from
//! `Application::getVersion()` and `QDateTime::currentDateTime()`;
//! [`BoardD356NetlistExport::new()`] fails if the board or a library element
//! does not exist; [`export_d356_netlist()`] is a convenience function for
//! CLI/MCP use.

use super::board_context::{BoardContext, ContextPad};
use super::export_error::BoardExportResult;
use super::gerber_export::ExportInfo;
use crate::export::{D356Land, D356NetlistGenerator};
use crate::fileio::{FilePath, file_utils};
use crate::geometry::ComponentSide;
use crate::project::Project;
use crate::project::error::{EntityKind, Error};
use crate::project::id::BoardId;
use crate::types::Angle;

/// Exports the IPC-D-356A netlist of a board.
#[derive(Debug, Clone)]
pub struct BoardD356NetlistExport<'a> {
    ctx: BoardContext<'a>,
    info: ExportInfo,
}

impl<'a> BoardD356NetlistExport<'a> {
    /// Creates an exporter for a board of a project.
    ///
    /// Fails if the board or a library element used by it does not exist.
    pub fn new(project: &'a Project, board: BoardId, info: &ExportInfo) -> BoardExportResult<Self> {
        let b = project.board(board).ok_or(Error::NotFound {
            kind: EntityKind::Board,
            uuid: board.0,
        })?;
        Ok(Self {
            ctx: BoardContext::new(project, b)?,
            info: info.clone(),
        })
    }

    /// Generates the file content (upstream `generate()`).
    pub fn generate(&self) -> BoardExportResult<String> {
        let ctx = &self.ctx;
        let project = ctx.project;
        let board = ctx.board;
        let mut g = D356NetlistGenerator::new(
            &self.info.app_version,
            project.metadata().name.as_str(),
            project.metadata().version.as_str(),
            board.properties().name.as_str(),
            &self.info.creation_date,
        );

        // Vias.
        for segment in board.net_segments().values() {
            let net_name = ctx.net_name(segment.net()).unwrap_or_default();
            for via in ctx.segment_vias(segment) {
                let solder_mask_covered = via.props.stop_mask_diameter_top.is_none()
                    && via.props.stop_mask_diameter_bot.is_none();
                let land = D356Land {
                    position: via.via.position(),
                    width: via.props.size,
                    height: via.props.size,
                    rotation: Angle::DEG0,
                };
                let start = via.via.start_layer().copper_number() as i32 + 1;
                let end = via.via.end_layer().copper_number() as i32 + 1;
                if via.via.is_blind() {
                    g.blind_via(
                        net_name,
                        &land,
                        via.props.drill_diameter,
                        start,
                        end,
                        solder_mask_covered,
                    );
                } else if via.via.is_buried() {
                    g.buried_via(
                        net_name,
                        via.via.position(),
                        via.props.drill_diameter,
                        start,
                        end,
                    );
                } else {
                    g.through_via(
                        net_name,
                        &land,
                        via.props.drill_diameter,
                        solder_mask_covered,
                    );
                }
            }
        }

        // Helper to add a pad.
        let mut board_pad_number = 1;
        let inner_layer_count = board.settings().inner_layer_count as i32;
        let mut add_pad = |g: &mut D356NetlistGenerator, pad: &ContextPad<'_>| {
            let net_name = ctx.net_name(pad.net).unwrap_or_default();
            let cmp_name = pad.device.map_or("BOARD", |d| d.designator());
            let pad_name = match (pad.package_pad_name, pad.device) {
                (Some(name), _) => name.to_owned(),
                (None, None) => {
                    let name = format!("P{board_pad_number}");
                    board_pad_number += 1;
                    name
                }
                (None, Some(_)) => String::new(),
            };
            let rotation = if pad.mirrored {
                pad.rotation + Angle::DEG180
            } else {
                pad.rotation
            };
            let land = D356Land {
                position: pad.position,
                width: pad.properties.width(),
                height: pad.properties.height(),
                rotation,
            };
            if let Some(hole) = pad.properties.holes().iter().next() {
                // THT pad. Not sure if we really need to export all holes, if
                // there are multiple. Also slots are probably not supported
                // by IPC-D-356A. I suspect it's good enough to export only a
                // single, circular hole?
                g.tht_pad(net_name, cmp_name, &pad_name, &land, hole.diameter());
            } else {
                // SMT pad.
                let layer_number = if pad.component_side() == ComponentSide::Top {
                    1
                } else {
                    inner_layer_count + 2
                };
                g.smt_pad(net_name, cmp_name, &pad_name, &land, layer_number);
            }
        };

        // Footprint pads.
        for dev in &ctx.devices {
            for pad in ctx.device_pads(dev)? {
                add_pad(&mut g, &pad);
            }
        }

        // Board pads.
        for segment in board.net_segments().values() {
            for pad in ctx.segment_pads(segment) {
                add_pad(&mut g, &pad);
            }
        }

        Ok(g.generate())
    }
}

/// Exports the IPC-D-356A netlist of a board into a file (like
/// `librepcb-cli --export-netlist <file>.d356`).
pub fn export_d356_netlist(
    project: &Project,
    board: BoardId,
    file_path: &FilePath,
    info: &ExportInfo,
) -> BoardExportResult<()> {
    let content = BoardD356NetlistExport::new(project, board, info)?.generate()?;
    file_utils::write_file(file_path, content.as_bytes())?;
    Ok(())
}
