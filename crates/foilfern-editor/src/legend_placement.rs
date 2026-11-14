//! Geometric placement checks for board legend text (no upstream counterpart).
//!
//! Uses the native DRC extractor and exact regions, including native stroke
//! fonts and mask openings. Search callers provide bounded poses and checkpoints;
//! they still run full DRC on the selected patch before publishing it.

use foilfern_core::geometry::Path;
use foilfern_core::project::board::drc::{BoardDrcData, DrcStrokeText, GeometryGenerator, Result};
use foilfern_core::project::board::{
    Board, BoardDevice, BoardPhysicalGeometry, BoardStrokeTextData,
};
use foilfern_core::project::{BoardId, Project};
use foilfern_core::types::{Layer, Length, UnsignedLength, Uuid};
use foilfern_core::utils::rational_union::Region;
use serde::Serialize;

/// Cached board obstacles for a single text search. Coordinates are board nm.
pub struct LegendObstacles {
    board: Region,
    mask: Region,
    courtyards: Vec<(Uuid, Region)>,
    other_texts: Vec<Region>,
    clearance: Length,
    /// Devices without courtyard geometry; outlines are used as a fallback.
    pub outline_fallbacks: Vec<Uuid>,
    /// Devices without either courtyard or outline geometry.
    pub unknown_devices: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct LegendCheck {
    pub outside_board: bool,
    pub mask_overlap: bool,
    pub courtyard_overlaps: Vec<Uuid>,
    pub text_overlap: bool,
}

impl LegendCheck {
    pub fn clear(&self) -> bool {
        !self.outside_board
            && !self.mask_overlap
            && self.courtyard_overlaps.is_empty()
            && !self.text_overlap
    }
}

fn generator() -> GeometryGenerator {
    GeometryGenerator::new(BoardPhysicalGeometry::arc_tolerance())
}

fn text_region(text: &DrcStrokeText, offset: Length) -> Result<Region> {
    let mut geometry = generator();
    geometry.add_stroke_text(text, offset)?;
    geometry.take_region()
}

fn add_area(geometry: &mut GeometryGenerator, path: &Path) -> Result<()> {
    geometry.add_polygon(path, UnsignedLength::ZERO, true, Length::ZERO)
}

impl LegendObstacles {
    pub fn new(
        project: &Project,
        board: &Board,
        text: &BoardStrokeTextData,
        device: Option<&BoardDevice>,
    ) -> Result<Self> {
        let data = BoardDrcData::new(
            project,
            BoardId(board.uuid()),
            &board.settings().drc_settings,
            false,
        )?;
        let top = data.silkscreen_layers_top.contains(&text.layer());
        let (mask_layer, courtyard_layer, outline_layer, layers) = if top {
            (
                Layer::TOP_STOP_MASK,
                Layer::TOP_COURTYARD,
                Layer::TOP_PACKAGE_OUTLINES,
                &data.silkscreen_layers_top,
            )
        } else {
            (
                Layer::BOT_STOP_MASK,
                Layer::BOT_COURTYARD,
                Layer::BOT_PACKAGE_OUTLINES,
                &data.silkscreen_layers_bot,
            )
        };
        let mut body = generator();
        let mut cutouts = generator();
        let mut add_boundary = |layer, path: &Path| -> Result<()> {
            if layer == Layer::BOARD_OUTLINES {
                add_area(&mut body, path)?;
            }
            if matches!(layer, Layer::BOARD_CUTOUTS | Layer::BOARD_PLATED_CUTOUTS) {
                add_area(&mut cutouts, path)?;
            }
            Ok(())
        };
        for polygon in &data.polygons {
            add_boundary(polygon.layer, &polygon.path)?;
        }
        for d in data.devices.values() {
            for polygon in &d.polygons {
                add_boundary(
                    d.transform().map(&polygon.layer),
                    &d.transform().map(&polygon.path),
                )?;
            }
            for circle in &d.circles {
                add_boundary(
                    d.transform().map(&circle.layer),
                    &Path::circle(circle.diameter).translated(d.transform().map(&circle.center)),
                )?;
            }
        }
        let board_area = body.take_region()?.difference(&cutouts.take_region()?)?;
        let mut mask = generator();
        mask.add_stop_mask_openings(&data, mask_layer, Length::ZERO)?;
        let mask = mask.take_region()?;
        let mut courtyards = Vec::new();
        let mut outline_fallbacks = Vec::new();
        let mut unknown_devices = Vec::new();
        let mut other_texts = Vec::new();
        let target_device = device.map(|d| d.component().0);
        for d in data.devices.values() {
            let area = |layer| -> Result<Region> {
                let mut geometry = generator();
                let transform = d.transform();
                for polygon in &d.polygons {
                    if transform.map(&polygon.layer) == layer {
                        add_area(&mut geometry, &transform.map(&polygon.path))?;
                    }
                }
                for circle in &d.circles {
                    if transform.map(&circle.layer) == layer {
                        add_area(
                            &mut geometry,
                            &Path::circle(circle.diameter)
                                .translated(transform.map(&circle.center)),
                        )?;
                    }
                }
                geometry.take_region()
            };
            let mut region = area(courtyard_layer)?;
            if region.is_empty() {
                region = area(outline_layer)?;
                // Ignore opposite-side devices, whose geometry is on the other side.
                let same_side = d.mirror != top;
                if same_side {
                    if region.is_empty() {
                        unknown_devices.push(d.uuid);
                    } else {
                        outline_fallbacks.push(d.uuid);
                    }
                }
            }
            if !region.is_empty() {
                courtyards.push((d.uuid, region));
            }
            for other in &d.stroke_texts {
                if layers.contains(&other.layer)
                    && !(target_device == Some(d.uuid) && other.uuid == text.uuid())
                {
                    other_texts.push(text_region(other, Length::ZERO)?);
                }
            }
        }
        for other in &data.stroke_texts {
            if layers.contains(&other.layer)
                && !(target_device.is_none() && other.uuid == text.uuid())
            {
                other_texts.push(text_region(other, Length::ZERO)?);
            }
        }
        Ok(Self {
            board: board_area,
            mask,
            courtyards,
            other_texts,
            clearance: *data.settings.min_silkscreen_stopmask_clearance(),
            outline_fallbacks,
            unknown_devices,
        })
    }

    pub fn check(
        &self,
        project: &Project,
        board: &Board,
        text: &BoardStrokeTextData,
        device: Option<&BoardDevice>,
    ) -> Result<LegendCheck> {
        let source = DrcStrokeText {
            uuid: text.uuid(),
            position: text.position(),
            rotation: text.rotation(),
            mirror: text.mirrored(),
            layer: text.layer(),
            stroke_width: text.stroke_width(),
            height: text.height(),
            paths: text.paths(project, board, device)?,
        };
        let ink = text_region(&source, Length::ZERO)?;
        let clearance = text_region(&source, self.clearance)?;
        let mut courtyard_overlaps = Vec::new();
        for (uuid, region) in &self.courtyards {
            if !ink.intersection(region)?.is_empty() {
                courtyard_overlaps.push(*uuid);
            }
        }
        let mut text_overlap = false;
        for region in &self.other_texts {
            if !ink.intersection(region)?.is_empty() {
                text_overlap = true;
                break;
            }
        }
        Ok(LegendCheck {
            outside_board: !clearance.difference(&self.board)?.is_empty(),
            mask_overlap: !clearance.intersection(&self.mask)?.is_empty(),
            courtyard_overlaps,
            text_overlap,
        })
    }
}
