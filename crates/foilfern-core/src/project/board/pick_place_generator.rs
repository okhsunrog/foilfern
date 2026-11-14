//! Port of libs/librepcb/core/project/board/boardpickplacegenerator.{h,cpp}.
//!
//! Differences to upstream: [`BoardPickPlaceGenerator::new()`] fails if the
//! board or a library element does not exist (upstream gets a valid
//! `Board&`); [`export_pick_place_csv()`] is a convenience function for
//! CLI/MCP use.

use std::collections::HashSet;

use super::board_context::BoardContext;
use super::export_error::BoardExportResult;
use super::gerber_export::simplified;
use crate::attribute;
use crate::export::{
    BoardSide, PickPlaceCsvWriter, PickPlaceData, PickPlaceDataItem, PickPlaceSides, PickPlaceType,
    Timestamp,
};
use crate::fileio::FilePath;
use crate::library::LibraryBaseElement;
use crate::library::pkg::AssemblyType;
use crate::project::Project;
use crate::project::error::{EntityKind, Error};
use crate::project::id::{AssemblyVariantId, BoardId};
use crate::types::Layer;

/// Generates the pick&place data of a board for an assembly variant.
#[derive(Debug, Clone)]
pub struct BoardPickPlaceGenerator<'a> {
    ctx: BoardContext<'a>,
    assembly_variant: AssemblyVariantId,
}

impl<'a> BoardPickPlaceGenerator<'a> {
    /// Creates a generator for a board of a project.
    ///
    /// Fails if the board or a library element used by it does not exist.
    pub fn new(
        project: &'a Project,
        board: BoardId,
        assembly_variant: AssemblyVariantId,
    ) -> BoardExportResult<Self> {
        let b = project.board(board).ok_or(Error::NotFound {
            kind: EntityKind::Board,
            uuid: board.0,
        })?;
        Ok(Self {
            ctx: BoardContext::new(project, b)?,
            assembly_variant,
        })
    }

    /// Generates the data (upstream `generate()`).
    pub fn generate(&self) -> BoardExportResult<PickPlaceData> {
        let ctx = &self.ctx;
        let project = ctx.project;
        let mut data = PickPlaceData::new(
            project.metadata().name.to_string(),
            project.metadata().version.to_string(),
            ctx.board.properties().name.to_string(),
        );
        let locale = &project.settings().locale_order;

        let mut exported_designators: HashSet<String> = HashSet::new();
        for dev in &ctx.devices {
            let part = dev.parts(Some(self.assembly_variant)).into_iter().next();
            let attributes = ctx.device_lookup(dev, part.as_ref());
            let lookup = |key: &str| attributes.value(key);
            let mut items = Vec::new();
            let designator = dev.designator();
            let value = simplified(&attribute::substitute(
                "{{MPN or VALUE or DEVICE}}",
                lookup,
                None,
            ));
            let device_name = dev.lib_device.metadata().names().value(locale).to_string();
            let package_name = dev.package.metadata().names().value(locale).to_string();

            // Determine fiducials to be exported.
            for pad in ctx.device_pads(dev)? {
                if pad.properties.function_is_fiducial() {
                    let mut sides = Vec::new();
                    if pad.is_on_layer(Layer::TOP_COPPER) {
                        sides.push(BoardSide::Top);
                    }
                    if pad.is_on_layer(Layer::BOT_COPPER) {
                        sides.push(BoardSide::Bottom);
                    }
                    let rotation = if pad.mirrored {
                        -pad.rotation
                    } else {
                        pad.rotation
                    };
                    for side in sides {
                        items.push(PickPlaceDataItem {
                            designator: designator.to_owned(),
                            value: value.clone(),
                            device_name: device_name.clone(),
                            package_name: package_name.clone(),
                            position: pad.position,
                            rotation,
                            board_side: side,
                            item_type: PickPlaceType::Fiducial,
                            mount: true,
                        });
                    }
                }
            }

            // Ensure unique designators for pad items if there are multiple.
            if items.len() > 1 {
                for (i, item) in items.iter_mut().enumerate() {
                    item.designator = format!("{}:{}", item.designator, i + 1);
                }
            }

            // Export device only if it is contained in the assembly variant.
            let rotation = if dev.device.mirrored() {
                -dev.device.rotation()
            } else {
                dev.device.rotation()
            };
            let board_side = if dev.device.mirrored() {
                BoardSide::Bottom
            } else {
                BoardSide::Top
            };
            let item_type = match dev.package.resolved_assembly_type() {
                AssemblyType::Tht => PickPlaceType::Tht,
                AssemblyType::Smt => PickPlaceType::Smt,
                AssemblyType::Mixed => PickPlaceType::Mixed,
                _ => PickPlaceType::Other,
            };
            items.push(PickPlaceDataItem {
                designator: designator.to_owned(),
                value,
                device_name,
                package_name,
                position: dev.device.position(),
                rotation,
                board_side,
                item_type,
                mount: part.is_some(),
            });
            exported_designators.insert(designator.to_owned());

            // Add all items.
            for item in items {
                data.add_item(item);
            }
        }

        // Export board-level fiducials.
        let mut fiducial_number = 0;
        for segment in ctx.board.net_segments().values() {
            for pad in ctx.segment_pads(segment) {
                if pad.properties.function_is_fiducial() {
                    let mut sides = Vec::new();
                    if pad.is_on_layer(Layer::TOP_COPPER) {
                        sides.push(BoardSide::Top);
                    }
                    if pad.is_on_layer(Layer::BOT_COPPER) {
                        sides.push(BoardSide::Bottom);
                    }

                    // Determine a unique designator.
                    let designator = loop {
                        fiducial_number += 1;
                        let designator = format!("FID{fiducial_number}");
                        if !exported_designators.contains(&designator) {
                            break designator;
                        }
                    };

                    let rotation = if pad.mirrored {
                        -pad.rotation
                    } else {
                        pad.rotation
                    };
                    for side in sides {
                        data.add_item(PickPlaceDataItem {
                            designator: designator.clone(),
                            value: String::new(),
                            device_name: String::new(),
                            package_name: String::new(),
                            position: pad.position,
                            rotation,
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

/// Generates the pick&place data of a board for an assembly variant and
/// writes the given sides as CSV file with metadata comment (like
/// `librepcb-cli --export-pnp-top/--export-pnp-bottom <file>.csv`).
pub fn export_pick_place_csv(
    project: &Project,
    board: BoardId,
    assembly_variant: AssemblyVariantId,
    sides: PickPlaceSides,
    file_path: &FilePath,
    app_version: &str,
    generation_date: Timestamp,
) -> BoardExportResult<()> {
    let data = BoardPickPlaceGenerator::new(project, board, assembly_variant)?.generate()?;
    let mut writer = PickPlaceCsvWriter::new(&data, app_version);
    writer.set_generation_date(generation_date);
    writer.set_include_metadata_comment(true);
    writer.set_board_sides(sides);
    writer.generate_csv()?.save_to_file(file_path)?;
    Ok(())
}
