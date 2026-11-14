//! Port of libs/librepcb/core/project/board/boardgerberexport.{h,cpp}.
//!
//! Differences to upstream:
//! - The application version and creation date written into the files are
//!   passed in ([`ExportInfo`]) instead of taken from
//!   `Application::getVersion()` and `QDateTime::currentDateTime()`.
//! - The "before write" callback returns a `Result` to abort the export.
//! - Plane fragments are taken from the board's derived data; rebuild them
//!   before exporting (see [`Project::rebuild_planes()`]), like upstream's
//!   CLI and output job runner do.
//! - [`export_fabrication_data()`] bundles planes rebuild and export for
//!   CLI/MCP use.

use std::collections::BTreeMap;

use super::BoardFabricationOutputSettings;
use super::board_context::{BoardContext, ContextDevice, ContextPad, ContextVia};
use super::export_error::{BoardExportError, BoardExportResult};
use crate::attribute;
use crate::export::{
    ApertureFunction, BoardSide, ComponentAttributes, CopperSide, ExcellonGenerator,
    GerberFileInfo, GerberGenerator, MountType, ObjectAttributes, Plating, Polarity, Timestamp,
};
use crate::fileio::{CleanFileNameOptions, FileNameCase, FilePath, file_utils};
use crate::geometry::{Circle, PadFunction, PadGeometryShape, Path, Polygon, StraightAreaPath};
use crate::library::LibraryBaseElement;
use crate::library::pkg::AssemblyType;
use crate::project::Project;
use crate::project::error::{EntityKind, Error};
use crate::project::id::{AssemblyVariantId, BoardId};
use crate::types::{
    Angle, Layer, Length, Orientation, Point, PositiveLength, UnsignedLength, Uuid,
};

/// Metadata written into the exported files, which upstream takes from the
/// environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportInfo {
    /// Version of the generating application (upstream
    /// `Application::getVersion()`), e.g. `"2.1.1"`.
    pub app_version: String,
    /// Creation date of the files (upstream
    /// `QDateTime::currentDateTime()`).
    pub creation_date: Timestamp,
}

impl ExportInfo {
    /// Creates export metadata with the current local time.
    pub fn now(app_version: impl Into<String>) -> Self {
        Self {
            app_version: app_version.into(),
            creation_date: Timestamp::now(),
        }
    }
}

/// Callback called with the path of each file before it is written.
pub type BeforeWriteCallback<'a> = Box<dyn FnMut(&FilePath) -> BoardExportResult<()> + 'a>;

/// Exports the fabrication data (Gerber and Excellon files) and the
/// assembly layers of a board.
pub struct BoardGerberExport<'a> {
    ctx: BoardContext<'a>,
    info: GerberFileInfo,
    remove_obsolete_files: bool,
    before_write: Option<BeforeWriteCallback<'a>>,
    written_files: Vec<FilePath>,
    /// Attribute provider state for the output file paths.
    current_inner_copper_layer: usize,
    current_start_layer: Option<Layer>,
    current_end_layer: Option<Layer>,
}

impl std::fmt::Debug for BoardGerberExport<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoardGerberExport")
            .field("board", &self.ctx.board.id())
            .field("info", &self.info)
            .field("written_files", &self.written_files)
            .finish_non_exhaustive()
    }
}

/// Upstream `calcWidthOfLayer()`: outlines have a minimum width of 1um.
fn calc_width_of_layer(width: UnsignedLength, layer: Layer) -> UnsignedLength {
    if layer.is_board_edge() && (*width < Length::new(1000)) {
        UnsignedLength::new(Length::new(1000)).expect("constant is not negative")
    } else {
        width
    }
}

/// Upstream `QString::simplified()`.
pub(crate) fn simplified(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sorted_by_uuid<'b, T>(
    items: impl IntoIterator<Item = &'b T>,
    uuid: impl Fn(&T) -> Uuid,
) -> Vec<&'b T>
where
    T: 'b,
{
    let mut items: Vec<&T> = items.into_iter().collect();
    items.sort_by_key(|item| uuid(item));
    items
}

impl<'a> BoardGerberExport<'a> {
    /// Creates an exporter for a board of a project.
    ///
    /// Fails if the board or a library element used by it does not exist.
    pub fn new(project: &'a Project, board: BoardId, info: &ExportInfo) -> BoardExportResult<Self> {
        let b = project.board(board).ok_or(Error::NotFound {
            kind: EntityKind::Board,
            uuid: board.0,
        })?;
        let ctx = BoardContext::new(project, b)?;
        // If the project contains multiple boards, add the board name to
        // the Gerber file metadata as well to distinguish between the
        // different boards.
        let mut project_name = project.metadata().name.to_string();
        if project.boards().len() > 1 {
            project_name = format!("{project_name} ({})", b.properties().name);
        }
        let info = GerberFileInfo {
            app_version: info.app_version.clone(),
            creation_date: info.creation_date,
            project_name,
            project_uuid: b.uuid(),
            project_revision: project.metadata().version.to_string(),
        };
        Ok(Self {
            ctx,
            info,
            remove_obsolete_files: true,
            before_write: None,
            written_files: Vec::new(),
            current_inner_copper_layer: 0,
            current_start_layer: None,
            current_end_layer: None,
        })
    }

    /// Returns the output directory of the fabrication data (upstream
    /// `getOutputDirectory()`).
    pub fn output_directory(
        &self,
        settings: &BoardFabricationOutputSettings,
    ) -> BoardExportResult<FilePath> {
        // Use dummy suffix.
        let fp = self.output_file_path(&format!("{}dummy", settings.output_base_path))?;
        fp.parent_dir()
            .ok_or_else(|| BoardExportError::InvalidOutputPath(fp.to_native()))
    }

    /// Returns the files written by the last
    /// [`export_pcb_layers()`](Self::export_pcb_layers) and the other
    /// exports since then.
    pub fn written_files(&self) -> &[FilePath] {
        &self.written_files
    }

    /// Sets whether files of disabled outputs (e.g. the merged drill file
    /// when drills are not merged) are removed (default: `true`).
    pub fn set_remove_obsolete_files(&mut self, remove: bool) {
        self.remove_obsolete_files = remove;
    }

    /// Sets a callback called with each file path before it is written.
    pub fn set_before_write_callback(&mut self, cb: BeforeWriteCallback<'a>) {
        self.before_write = Some(cb);
    }

    /// Exports all Gerber and Excellon files according to `settings`
    /// (upstream `exportPcbLayers()`).
    pub fn export_pcb_layers(
        &mut self,
        settings: &BoardFabricationOutputSettings,
    ) -> BoardExportResult<()> {
        self.written_files.clear();
        self.export_drills_merged(settings)?;
        self.export_drills_npth(settings)?;
        self.export_drills_pth(settings)?;
        self.export_drills_blind_buried(settings)?;
        self.export_layer_board_outlines(settings)?;
        self.export_layer_top_copper(settings)?;
        self.export_layer_inner_copper(settings)?;
        self.export_layer_bottom_copper(settings)?;
        self.export_layer_solder_mask(settings, BoardSide::Top)?;
        self.export_layer_solder_mask(settings, BoardSide::Bottom)?;
        self.export_layer_silkscreen(settings, BoardSide::Top)?;
        self.export_layer_silkscreen(settings, BoardSide::Bottom)?;
        self.export_layer_solder_paste(settings, BoardSide::Top)?;
        self.export_layer_solder_paste(settings, BoardSide::Bottom)?;
        Ok(())
    }

    /// Exports the glue layer of one board side, containing the devices of
    /// an assembly variant (upstream `exportGlueLayer()`).
    pub fn export_glue_layer(
        &mut self,
        side: BoardSide,
        assembly_variant: AssemblyVariantId,
        file_path: &FilePath,
    ) -> BoardExportResult<()> {
        let mut g = GerberGenerator::new(&self.info);
        g.set_file_function_glue(side, Polarity::Positive);
        let layer = match side {
            BoardSide::Top => Layer::TOP_GLUE,
            BoardSide::Bottom => Layer::BOT_GLUE,
        };
        // Draw footprints incl. pads (only those contained in the assembly
        // variant).
        for dev in &self.ctx.devices {
            if dev.device.glue() && dev.is_in_assembly_variant(assembly_variant) {
                self.draw_device(&mut g, dev, layer)?;
            }
        }
        // Draw all non-footprint objects.
        self.draw_layer_except_devices(&mut g, layer)?;
        self.save_gerber(&g, file_path)
    }

    /// Exports the component (assembly) layer of one board side in Gerber
    /// X3 format, containing the devices of an assembly variant (upstream
    /// `exportComponentLayer()`).
    pub fn export_component_layer(
        &mut self,
        side: BoardSide,
        assembly_variant: AssemblyVariantId,
        file_path: &FilePath,
    ) -> BoardExportResult<()> {
        let ctx = &self.ctx;
        let board = ctx.board;
        let mut g = GerberGenerator::new(&self.info);
        match side {
            BoardSide::Top => g.set_file_function_component(1, BoardSide::Top),
            BoardSide::Bottom => g.set_file_function_component(
                board.settings().inner_layer_count as i32 + 2,
                BoardSide::Bottom,
            ),
        }

        // Export board outline since this is useful for manual review.
        for polygon in board.polygons().values() {
            if polygon.layer().is_board_edge() {
                let width = calc_width_of_layer(polygon.line_width(), polygon.layer());
                g.draw_path_outline(
                    polygon.path(),
                    width,
                    &ObjectAttributes {
                        function: Some(ApertureFunction::Profile),
                        ..Default::default()
                    },
                );
            }
        }

        // Export all components on the selected board side.
        for dev in &ctx.devices {
            if dev.device.mirrored() != (side == BoardSide::Bottom) {
                continue;
            }
            let Some(part) = dev.parts(Some(assembly_variant)).into_iter().next() else {
                continue; // Do not mount.
            };

            // Determine assembly type.
            let mount_type = match dev.package.resolved_assembly_type() {
                AssemblyType::None => {
                    log::warn!(
                        "Exported device with non-mountable package to Gerber X3: {}",
                        dev.designator()
                    );
                    MountType::Other
                }
                // Mixed: does this make sense?!
                AssemblyType::Tht | AssemblyType::Mixed => MountType::Tht,
                AssemblyType::Smt => MountType::Smt,
                AssemblyType::Other | AssemblyType::Auto => MountType::Other,
            };

            // Export component center and attributes.
            let attributes = ctx.device_lookup(dev, Some(&part));
            let lookup = |key: &str| attributes.value(key);
            let rotation = dev.device.rotation();
            let designator = dev.designator();
            let value = attribute::substitute(&lookup("VALUE").unwrap_or_default(), lookup, None)
                .trim()
                .to_owned();
            let mpn = simplified(&lookup("MPN").unwrap_or_default());
            let manufacturer = simplified(&lookup("MANUFACTURER").unwrap_or_default());
            // Note: Always use english locale to make PnP files portable.
            let footprint_name = dev.package.metadata().name().to_string();
            let component = ComponentAttributes {
                designator,
                value: &value,
                mount_type,
                manufacturer: &manufacturer,
                mpn: &mpn,
                footprint: &footprint_name,
            };
            g.flash_component(dev.device.position(), rotation, &component);

            // Export component body outlines.
            let (outlines_layer, documentation_layer, courtyard_layer) = match side {
                BoardSide::Top => (
                    Layer::TOP_PACKAGE_OUTLINES,
                    Layer::TOP_DOCUMENTATION,
                    Layer::TOP_COURTYARD,
                ),
                BoardSide::Bottom => (
                    Layer::BOT_PACKAGE_OUTLINES,
                    Layer::BOT_DOCUMENTATION,
                    Layer::BOT_COURTYARD,
                ),
            };
            let mut outlines: Vec<(ApertureFunction, Path)> =
                component_outlines(dev, outlines_layer)
                    .into_iter()
                    .map(|p| (ApertureFunction::ComponentOutlineBody, p))
                    .collect();
            if outlines.is_empty() {
                // Many packages probably don't have an explicit package
                // outline, thus using the documentation layer as a fallback.
                outlines.extend(
                    component_outlines(dev, documentation_layer)
                        .into_iter()
                        .map(|p| (ApertureFunction::ComponentOutlineBody, p)),
                );
            }
            outlines.extend(
                component_outlines(dev, courtyard_layer)
                    .into_iter()
                    .map(|p| (ApertureFunction::ComponentOutlineCourtyard, p)),
            );
            for (function, path) in &outlines {
                g.draw_component_outline(path, rotation, &component, Some(*function));
            }

            // Export component pins.
            for pad in ctx.device_pads(dev)? {
                if pad.properties.function_is_fiducial() {
                    continue;
                }
                let pin_name = pad.package_pad_name.unwrap_or_default();
                let pin_signal = pad.signal_name.unwrap_or_default();
                let is_pin1 = pin_name == "1"; // Very sophisticated algorithm ;-)
                g.flash_component_pin(
                    pad.position,
                    rotation,
                    &component,
                    pin_name,
                    pin_signal,
                    is_pin1,
                );
            }
        }

        // Export fiducials on the selected board side.
        let cu_layer = match side {
            BoardSide::Top => Layer::TOP_COPPER,
            BoardSide::Bottom => Layer::BOT_COPPER,
        };
        for dev in &ctx.devices {
            let mut pad_number = 1;
            for pad in ctx.device_pads(dev)? {
                if pad.properties.function_is_fiducial() && pad.is_on_layer(cu_layer) {
                    let attributes = ctx.device_lookup(dev, None);
                    let lookup = |key: &str| attributes.value(key);
                    let designator = format!("{}:{pad_number}", dev.designator());
                    let value = simplified(&attribute::substitute(
                        &lookup("VALUE").unwrap_or_default(),
                        lookup,
                        None,
                    ));
                    let footprint_name = dev.package.metadata().name().to_string();
                    g.flash_component(
                        pad.position,
                        pad.rotation,
                        &ComponentAttributes {
                            designator: &designator,
                            value: &value,
                            mount_type: MountType::Fiducial,
                            manufacturer: "",
                            mpn: "",
                            footprint: &footprint_name,
                        },
                    );
                    pad_number += 1;
                }
            }
        }
        for segment in board.net_segments().values() {
            for pad in ctx.segment_pads(segment) {
                if pad.properties.function_is_fiducial() && pad.is_on_layer(cu_layer) {
                    // Is there any useful attribute we could or should set?
                    g.flash_component(
                        pad.position,
                        pad.rotation,
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

        self.save_gerber(&g, file_path)
    }

    // --------------------------------------------------------------------
    // Drills
    // --------------------------------------------------------------------

    fn export_drills_merged(
        &mut self,
        settings: &BoardFabricationOutputSettings,
    ) -> BoardExportResult<()> {
        let fp = self.output_file_path(&format!(
            "{}{}",
            settings.output_base_path, settings.suffix_drills
        ))?;
        if settings.merge_drill_files {
            let mut g = self.create_excellon_generator(settings, Plating::Mixed);
            self.draw_pth_drills(&mut g)?;
            self.draw_npth_drills(&mut g);
            self.save_excellon(&g, &fp)
        } else {
            self.remove_obsolete(&fp)
        }
    }

    fn export_drills_npth(
        &mut self,
        settings: &BoardFabricationOutputSettings,
    ) -> BoardExportResult<()> {
        let fp = self.output_file_path(&format!(
            "{}{}",
            settings.output_base_path, settings.suffix_drills_npth
        ))?;
        if !settings.merge_drill_files {
            let mut g = self.create_excellon_generator(settings, Plating::No);
            self.draw_npth_drills(&mut g);
            // Note that separate NPTH drill files could lead to issues with
            // some PCB manufacturers, even if it's empty in many cases.
            // However, we generate the NPTH file even if there are no NPTH
            // drills since it could also lead to unexpected behavior if the
            // file is generated only conditionally. See
            // https://github.com/LibrePCB/LibrePCB/issues/998. If the PCB
            // manufacturer doesn't support a separate NPTH file, the user
            // shall enable the "merge PTH and NPTH drills" option.
            self.save_excellon(&g, &fp)
        } else {
            self.remove_obsolete(&fp)
        }
    }

    fn export_drills_pth(
        &mut self,
        settings: &BoardFabricationOutputSettings,
    ) -> BoardExportResult<()> {
        let fp = self.output_file_path(&format!(
            "{}{}",
            settings.output_base_path, settings.suffix_drills_pth
        ))?;
        if !settings.merge_drill_files {
            let mut g = self.create_excellon_generator(settings, Plating::Yes);
            self.draw_pth_drills(&mut g)?;
            self.save_excellon(&g, &fp)
        } else {
            self.remove_obsolete(&fp)
        }
    }

    fn export_drills_blind_buried(
        &mut self,
        settings: &BoardFabricationOutputSettings,
    ) -> BoardExportResult<()> {
        let mut vias: BTreeMap<(Layer, Layer), Vec<ContextVia<'a>>> = BTreeMap::new();
        for segment in self.ctx.board.net_segments().values() {
            for via in self.ctx.segment_vias(segment) {
                if (via.via.is_blind() || via.via.is_buried())
                    && let Some(span) = via.props.drill_layer_span
                {
                    vias.entry(span).or_default().push(via);
                }
            }
        }
        for ((start, end), vias) in vias {
            self.current_start_layer = Some(start);
            self.current_end_layer = Some(end);
            let fp = self.output_file_path(&format!(
                "{}{}",
                settings.output_base_path, settings.suffix_drills_blind_buried
            ));
            self.current_start_layer = None;
            self.current_end_layer = None;
            let fp = fp?;
            let mut g = self.create_excellon_generator(settings, Plating::Yes);
            for via in vias {
                g.drill(
                    via.via.position(),
                    via.props.drill_diameter,
                    true,
                    ApertureFunction::ViaDrill,
                );
            }
            self.save_excellon(&g, &fp)?;
        }
        Ok(())
    }

    fn draw_npth_drills(&self, g: &mut ExcellonGenerator) -> usize {
        let mut count = 0;
        // Footprint holes.
        for dev in &self.ctx.devices {
            let transform = dev.transform();
            for hole in dev.footprint.holes() {
                g.drill_path(
                    transform.map(hole.path()),
                    hole.diameter(),
                    false,
                    ApertureFunction::MechanicalDrill,
                );
                count += 1;
            }
        }
        // Board holes.
        for hole in self.ctx.board.holes().values() {
            g.drill_path(
                hole.path().clone(),
                hole.diameter(),
                false,
                ApertureFunction::MechanicalDrill,
            );
            count += 1;
        }
        count
    }

    fn draw_pth_drills(&self, g: &mut ExcellonGenerator) -> BoardExportResult<usize> {
        // Helper to draw a pad.
        let draw_pad = |g: &mut ExcellonGenerator, pad: &ContextPad<'_>| {
            let transform = pad.transform();
            let function = if pad.properties.function() == PadFunction::PressFitPad {
                ApertureFunction::ComponentDrillPressFit
            } else {
                ApertureFunction::ComponentDrill
            };
            for hole in pad.properties.holes() {
                g.drill_path(transform.map(hole.path()), hole.diameter(), true, function);
            }
            pad.properties.holes().len()
        };
        let mut count = 0;
        // Footprint pads.
        for dev in &self.ctx.devices {
            for pad in self.ctx.device_pads(dev)? {
                count += draw_pad(g, &pad);
            }
        }
        // Board pads & vias.
        for segment in self.ctx.board.net_segments().values() {
            for pad in self.ctx.segment_pads(segment) {
                count += draw_pad(g, &pad);
            }
            for via in self.ctx.segment_vias(segment) {
                if via.via.is_through() {
                    g.drill(
                        via.via.position(),
                        via.props.drill_diameter,
                        true,
                        ApertureFunction::ViaDrill,
                    );
                    count += 1;
                }
            }
        }
        Ok(count)
    }

    fn create_excellon_generator(
        &self,
        settings: &BoardFabricationOutputSettings,
        plating: Plating,
    ) -> ExcellonGenerator {
        let mut g = ExcellonGenerator::new(
            &self.info,
            plating,
            1,
            self.ctx.board.settings().inner_layer_count as i32 + 2,
        );
        g.set_use_g85_slots(settings.use_g85_slot_command);
        g
    }

    // --------------------------------------------------------------------
    // Layers
    // --------------------------------------------------------------------

    fn new_gerber(&self) -> GerberGenerator {
        GerberGenerator::new(&self.info)
    }

    fn export_layer_board_outlines(
        &mut self,
        settings: &BoardFabricationOutputSettings,
    ) -> BoardExportResult<()> {
        let fp = self.output_file_path(&format!(
            "{}{}",
            settings.output_base_path, settings.suffix_outlines
        ))?;
        let mut g = self.new_gerber();
        g.set_file_function_outlines(false);
        self.draw_layer(&mut g, Layer::BOARD_OUTLINES)?;
        self.draw_layer(&mut g, Layer::BOARD_CUTOUTS)?;
        // Note: Currently the "plated cutouts" layer is exported to the
        // normal board outlines Gerber file, which is not ideal but
        // unfortunately there doesn't exist a standardized way of exporting
        // plated cutouts :-( This way may still work fine if there is copper
        // around the plated cutout polygons, therefore we have implemented a
        // DRC warning if this is not the case.
        self.draw_layer(&mut g, Layer::BOARD_PLATED_CUTOUTS)?;
        self.save_gerber(&g, &fp)
    }

    fn export_layer_top_copper(
        &mut self,
        settings: &BoardFabricationOutputSettings,
    ) -> BoardExportResult<()> {
        let fp = self.output_file_path(&format!(
            "{}{}",
            settings.output_base_path, settings.suffix_copper_top
        ))?;
        let mut g = self.new_gerber();
        g.set_file_function_copper(1, CopperSide::Top, Polarity::Positive);
        self.draw_layer(&mut g, Layer::TOP_COPPER)?;
        self.save_gerber(&g, &fp)
    }

    fn export_layer_bottom_copper(
        &mut self,
        settings: &BoardFabricationOutputSettings,
    ) -> BoardExportResult<()> {
        let fp = self.output_file_path(&format!(
            "{}{}",
            settings.output_base_path, settings.suffix_copper_bot
        ))?;
        let mut g = self.new_gerber();
        g.set_file_function_copper(
            self.ctx.board.settings().inner_layer_count as i32 + 2,
            CopperSide::Bottom,
            Polarity::Positive,
        );
        self.draw_layer(&mut g, Layer::BOT_COPPER)?;
        self.save_gerber(&g, &fp)
    }

    fn export_layer_inner_copper(
        &mut self,
        settings: &BoardFabricationOutputSettings,
    ) -> BoardExportResult<()> {
        let count = self.ctx.board.settings().inner_layer_count as usize;
        for i in 1..=count {
            self.current_inner_copper_layer = i; // Used for attribute provider.
            let fp = self.output_file_path(&format!(
                "{}{}",
                settings.output_base_path, settings.suffix_copper_inner
            ));
            self.current_inner_copper_layer = 0;
            let fp = fp?;
            let mut g = self.new_gerber();
            g.set_file_function_copper(i as i32 + 1, CopperSide::Inner, Polarity::Positive);
            // The number of inner layers is limited when loading the board.
            let layer = Layer::inner_copper(i).ok_or(BoardExportError::InvalidOutputPath(
                format!("unknown inner copper layer {i}"),
            ))?;
            self.draw_layer(&mut g, layer)?;
            self.save_gerber(&g, &fp)?;
        }
        Ok(())
    }

    fn export_layer_solder_mask(
        &mut self,
        settings: &BoardFabricationOutputSettings,
        side: BoardSide,
    ) -> BoardExportResult<()> {
        let (suffix, layer) = match side {
            BoardSide::Top => (&settings.suffix_solder_mask_top, Layer::TOP_STOP_MASK),
            BoardSide::Bottom => (&settings.suffix_solder_mask_bot, Layer::BOT_STOP_MASK),
        };
        let fp = self.output_file_path(&format!("{}{suffix}", settings.output_base_path))?;
        if self.ctx.board.settings().solder_resist.is_some() {
            let mut g = self.new_gerber();
            g.set_file_function_solder_mask(side, Polarity::Negative);
            self.draw_layer(&mut g, layer)?;
            self.save_gerber(&g, &fp)
        } else {
            self.remove_obsolete(&fp)
        }
    }

    fn export_layer_silkscreen(
        &mut self,
        settings: &BoardFabricationOutputSettings,
        side: BoardSide,
    ) -> BoardExportResult<()> {
        let s = self.ctx.board.settings();
        let (suffix, layers, stop_mask) = match side {
            BoardSide::Top => (
                &settings.suffix_silkscreen_top,
                &s.silkscreen_layers_top,
                Layer::TOP_STOP_MASK,
            ),
            BoardSide::Bottom => (
                &settings.suffix_silkscreen_bot,
                &s.silkscreen_layers_bot,
                Layer::BOT_STOP_MASK,
            ),
        };
        let fp = self.output_file_path(&format!("{}{suffix}", settings.output_base_path))?;
        // Don't export silkscreen if no layers selected.
        if !layers.is_empty() {
            let mut g = self.new_gerber();
            g.set_file_function_legend(side, Polarity::Positive);
            for layer in layers {
                self.draw_layer(&mut g, *layer)?;
            }
            g.set_layer_polarity(Polarity::Negative);
            self.draw_layer(&mut g, stop_mask)?;
            self.save_gerber(&g, &fp)
        } else {
            self.remove_obsolete(&fp)
        }
    }

    fn export_layer_solder_paste(
        &mut self,
        settings: &BoardFabricationOutputSettings,
        side: BoardSide,
    ) -> BoardExportResult<()> {
        let (suffix, enabled, layer) = match side {
            BoardSide::Top => (
                &settings.suffix_solder_paste_top,
                settings.enable_solder_paste_top,
                Layer::TOP_SOLDER_PASTE,
            ),
            BoardSide::Bottom => (
                &settings.suffix_solder_paste_bot,
                settings.enable_solder_paste_bot,
                Layer::BOT_SOLDER_PASTE,
            ),
        };
        let fp = self.output_file_path(&format!("{}{suffix}", settings.output_base_path))?;
        if enabled {
            let mut g = self.new_gerber();
            g.set_file_function_paste(side, Polarity::Positive);
            self.draw_layer(&mut g, layer)?;
            self.save_gerber(&g, &fp)
        } else {
            self.remove_obsolete(&fp)
        }
    }

    // --------------------------------------------------------------------
    // Drawing
    // --------------------------------------------------------------------

    fn draw_layer(&self, g: &mut GerberGenerator, layer: Layer) -> BoardExportResult<()> {
        // Draw footprints incl. pads.
        for dev in &self.ctx.devices {
            self.draw_device(g, dev, layer)?;
        }
        // Draw all non-footprint objects.
        self.draw_layer_except_devices(g, layer)
    }

    fn draw_layer_except_devices(
        &self,
        g: &mut GerberGenerator,
        layer: Layer,
    ) -> BoardExportResult<()> {
        let ctx = &self.ctx;
        let board = ctx.board;

        // Draw board pads, vias and traces (grouped by net).
        for segment in board.net_segments().values() {
            // Anonymous net: "N/C" (reserved name by Gerber specs).
            let net = ctx.net_name(segment.net()).unwrap_or("N/C");
            for pad in ctx.segment_pads(segment) {
                draw_pad(g, ctx, &pad, layer)?;
            }
            for via in ctx.segment_vias(segment) {
                draw_via(g, &via, layer, net);
            }
            for trace in segment.traces().values() {
                if trace.layer() == layer {
                    let (Some(p1), Some(p2)) = (
                        ctx.anchor_position(segment, trace.p1()),
                        ctx.anchor_position(segment, trace.p2()),
                    ) else {
                        continue; // Invalid anchor, rejected by the mutations.
                    };
                    g.draw_line(
                        p1,
                        p2,
                        trace.width().into(),
                        &ObjectAttributes {
                            function: Some(ApertureFunction::Conductor),
                            net: Some(net),
                            ..Default::default()
                        },
                    );
                }
            }
        }

        // Draw planes.
        for plane in board.planes().values() {
            if plane.layer() == layer {
                let net = ctx.net_name(plane.net());
                for fragment in board.derived().fragments_of(plane.id()) {
                    g.draw_path_area(
                        fragment,
                        &ObjectAttributes {
                            function: Some(ApertureFunction::Conductor),
                            net,
                            ..Default::default()
                        },
                    );
                }
            }
        }

        // Draw polygons.
        let (graphics_function, graphics_net) = graphics_attributes(layer);
        for polygon in board.polygons().values() {
            if polygon.layer() == layer {
                draw_polygon(
                    g,
                    layer,
                    polygon.path(),
                    polygon.line_width(),
                    polygon.is_filled(),
                    graphics_function,
                    graphics_net,
                    "",
                );
            }
        }

        // Draw stroke texts.
        let text_function = layer.is_copper().then_some(ApertureFunction::NonConductor);
        for text in board.stroke_texts().values() {
            if text.layer() == layer {
                let width = calc_width_of_layer(text.stroke_width(), layer);
                let transform = BoardContext::stroke_text_transform(text);
                for path in transform.map(&ctx.stroke_text_paths(text, None)?) {
                    g.draw_path_outline(
                        &path,
                        width,
                        &ObjectAttributes {
                            function: text_function,
                            net: graphics_net,
                            ..Default::default()
                        },
                    );
                }
            }
        }

        // Draw holes.
        if layer.is_stop_mask() {
            let rules = board.design_rules();
            for hole in board.holes().values() {
                if let Some(offset) = hole.stop_mask_offset(rules) {
                    let diameter = *hole.diameter() + offset + offset;
                    let mut path = hole.path().get().clone();
                    path.clean();
                    draw_hole_stop_mask(g, &path, diameter);
                }
            }
        }
        Ok(())
    }

    fn draw_device(
        &self,
        g: &mut GerberGenerator,
        dev: &ContextDevice<'a>,
        layer: Layer,
    ) -> BoardExportResult<()> {
        let ctx = &self.ctx;
        let (graphics_function, graphics_net) = graphics_attributes(layer);
        let component = dev.designator();

        // Draw pads.
        for pad in ctx.device_pads(dev)? {
            draw_pad(g, ctx, &pad, layer)?;
        }

        // Draw polygons.
        let transform = dev.transform();
        for polygon in sorted_by_uuid(dev.footprint.polygons(), Polygon::uuid) {
            let polygon_layer = transform.map(&polygon.layer());
            if polygon_layer == layer {
                let path = transform.map(polygon.path());
                draw_polygon(
                    g,
                    layer,
                    &path,
                    polygon.line_width(),
                    polygon.is_filled(),
                    graphics_function,
                    graphics_net,
                    component,
                );
            }
        }

        // Draw circles.
        for circle in sorted_by_uuid(dev.footprint.circles(), Circle::uuid) {
            let circle_layer = transform.map(&circle.layer());
            if circle_layer == layer {
                let absolute_pos = transform.map(&circle.center());
                let attributes = ObjectAttributes {
                    function: graphics_function,
                    net: graphics_net,
                    component,
                    ..Default::default()
                };
                if circle.is_filled() {
                    let outer_dia = circle.diameter() + circle.line_width();
                    g.draw_path_area(
                        &Path::circle(outer_dia).translated(absolute_pos),
                        &attributes,
                    );
                } else {
                    let width = calc_width_of_layer(circle.line_width(), circle_layer);
                    g.draw_path_outline(
                        &Path::circle(circle.diameter()).translated(absolute_pos),
                        width,
                        &attributes,
                    );
                }
            }
        }

        // Draw stroke texts (from footprint instance, *NOT* from library
        // footprint!).
        let text_function = layer.is_copper().then_some(ApertureFunction::NonConductor);
        for text in dev.device.stroke_texts().values() {
            if text.layer() == layer {
                let width = calc_width_of_layer(text.stroke_width(), layer);
                let text_transform = BoardContext::stroke_text_transform(text);
                for path in text_transform.map(&ctx.stroke_text_paths(text, Some(dev))?) {
                    g.draw_path_outline(
                        &path,
                        width,
                        &ObjectAttributes {
                            function: text_function,
                            net: graphics_net,
                            component,
                            ..Default::default()
                        },
                    );
                }
            }
        }

        // Draw holes.
        if layer.is_stop_mask() {
            let offsets = dev.hole_stop_mask_offsets(ctx.board.design_rules());
            for hole in sorted_by_uuid(dev.footprint.holes(), |h| h.uuid()) {
                if let Some(Some(offset)) = offsets.get(&hole.uuid()) {
                    let diameter = *hole.diameter() + *offset + *offset;
                    if diameter > Length::ZERO {
                        let mut path = hole.path().get().clone();
                        path.clean();
                        draw_hole_stop_mask(g, &transform.map(&path), diameter);
                    }
                }
            }
        }
        Ok(())
    }

    // --------------------------------------------------------------------
    // Files
    // --------------------------------------------------------------------

    /// Returns the path of an output file with substituted attributes
    /// (upstream `getOutputFilePath()`).
    fn output_file_path(&self, path: &str) -> BoardExportResult<FilePath> {
        let mut filter = |s: &str| {
            FilePath::clean_file_name(s, CleanFileNameOptions::new(true, FileNameCase::Keep), 120)
        };
        let path = attribute::substitute(path, |key| self.attribute_value(key), Some(&mut filter));
        let result = if std::path::Path::new(&path).is_absolute() {
            FilePath::new(&path)
        } else {
            self.ctx.project.path().map(|dir| dir.path_to(&path))
        };
        result.ok_or(BoardExportError::InvalidOutputPath(path))
    }

    /// Upstream `getAttributeValue()`.
    fn attribute_value(&self, key: &str) -> Option<String> {
        let layer_name = |layer: Layer| {
            // No tr()!
            if layer.is_top() {
                "TOP".to_owned()
            } else if layer.is_bottom() {
                "BOTTOM".to_owned()
            } else {
                format!("IN{}", layer.copper_number())
            }
        };
        match (key, self.current_start_layer, self.current_end_layer) {
            ("CU_LAYER", _, _) if self.current_inner_copper_layer > 0 => {
                Some(self.current_inner_copper_layer.to_string())
            }
            ("START_LAYER", Some(start), _) => Some(layer_name(start)),
            ("END_LAYER", _, Some(end)) => Some(layer_name(end)),
            ("START_NUMBER", Some(start), _) => Some((start.copper_number() + 1).to_string()),
            ("END_NUMBER", _, Some(end)) => Some((end.copper_number() + 1).to_string()),
            _ => self.ctx.board_lookup().value(key),
        }
    }

    fn track_file_before_write(&mut self, fp: &FilePath) -> BoardExportResult<()> {
        if let Some(cb) = self.before_write.as_mut() {
            cb(fp)?;
        }
        self.written_files.push(fp.clone());
        Ok(())
    }

    fn save_gerber(&mut self, g: &GerberGenerator, fp: &FilePath) -> BoardExportResult<()> {
        let content = g.generate();
        self.track_file_before_write(fp)?;
        file_utils::write_file(fp, content.as_bytes())?;
        Ok(())
    }

    fn save_excellon(&mut self, g: &ExcellonGenerator, fp: &FilePath) -> BoardExportResult<()> {
        let content = g.generate()?;
        self.track_file_before_write(fp)?;
        file_utils::write_file(fp, content.as_bytes())?;
        Ok(())
    }

    fn remove_obsolete(&self, fp: &FilePath) -> BoardExportResult<()> {
        if self.remove_obsolete_files && fp.is_existing_file() && !self.written_files.contains(fp) {
            file_utils::remove_file(fp)?;
        }
        Ok(())
    }
}

/// Aperture function and net of graphical objects on a layer.
fn graphics_attributes(layer: Layer) -> (Option<ApertureFunction>, Option<&'static str>) {
    if layer.is_board_edge() {
        (Some(ApertureFunction::Profile), None)
    } else if layer.is_copper() {
        // Not connected to any net.
        (Some(ApertureFunction::Conductor), Some(""))
    } else {
        (None, None)
    }
}

/// Draws the stop mask opening of a hole.
fn draw_hole_stop_mask(g: &mut GerberGenerator, path: &Path, diameter: Length) {
    let (Ok(dia), Ok(width)) = (PositiveLength::new(diameter), UnsignedLength::new(diameter))
    else {
        return;
    };
    if let [vertex] = path.vertices() {
        g.flash_circle(vertex.pos, dia, &ObjectAttributes::default());
    } else {
        g.draw_path_outline(path, width, &ObjectAttributes::default());
    }
}

/// Upstream `drawVia()`.
fn draw_via(g: &mut GerberGenerator, via: &ContextVia<'_>, layer: Layer, net_name: &str) {
    let draw_copper = via.via.is_on_layer(layer);
    let stop_mask_diameter = if layer.is_stop_mask() {
        if layer.is_top() {
            via.props.stop_mask_diameter_top
        } else {
            via.props.stop_mask_diameter_bot
        }
    } else {
        None
    };
    if draw_copper || stop_mask_diameter.is_some() {
        // Via attributes (only on copper layers).
        let attributes = if draw_copper {
            ObjectAttributes {
                function: Some(ApertureFunction::ViaPad),
                net: Some(net_name),
                ..Default::default()
            }
        } else {
            ObjectAttributes::default()
        };
        let diameter = stop_mask_diameter.unwrap_or(via.props.size);
        g.flash_circle(via.via.position(), diameter, &attributes);
    }
}

/// Upstream `drawPad()`.
fn draw_pad(
    g: &mut GerberGenerator,
    ctx: &BoardContext<'_>,
    pad: &ContextPad<'_>,
    layer: Layer,
) -> BoardExportResult<()> {
    let component = pad.device.map(|d| d.designator()).unwrap_or_default();
    for geometry in pad.geometries_on(layer) {
        // Pad attributes (most of them only on copper layers).
        let mut attributes = ObjectAttributes {
            component,
            ..Default::default()
        };
        if layer.is_copper() {
            let function = match pad.properties.function() {
                PadFunction::ThermalPad => ApertureFunction::HeatsinkPad,
                PadFunction::BgaPad => ApertureFunction::BgaPadCopperDefined,
                PadFunction::EdgeConnectorPad => ApertureFunction::ConnectorPad,
                PadFunction::TestPad => ApertureFunction::TestPad,
                PadFunction::LocalFiducial => ApertureFunction::FiducialPadLocal,
                PadFunction::GlobalFiducial => ApertureFunction::FiducialPadGlobal,
                _ if pad.properties.is_tht() => ApertureFunction::ComponentPad,
                _ => ApertureFunction::SmdPadCopperDefined,
            };
            attributes.function = Some(function);
            // Anonymous net: "N/C" (reserved name by Gerber specs).
            attributes.net = Some(ctx.net_name(pad.net).unwrap_or("N/C"));
            attributes.pin = pad.package_pad_name.unwrap_or_default();
            attributes.signal = pad.signal_name.unwrap_or_default();
        }

        // Helper to flash a custom outline by flattening all arcs.
        let flash_pad_outline = |g: &mut GerberGenerator| -> BoardExportResult<()> {
            for outline in geometry.to_outlines()? {
                let mut outline = outline.flattened_arcs(
                    PositiveLength::new(Length::new(5000)).expect("constant is positive"),
                );
                if pad.mirrored {
                    outline = outline.mirrored(Orientation::Horizontal, Point::ORIGIN);
                }
                g.flash_outline(
                    pad.position,
                    &StraightAreaPath::new(outline)?,
                    pad.rotation,
                    &attributes,
                );
            }
            Ok(())
        };

        // Flash shape.
        let width = geometry.width();
        let height = geometry.height();
        match geometry.shape() {
            PadGeometryShape::RoundedRect | PadGeometryShape::RoundedOctagon => {
                if let (Ok(w), Ok(h)) = (PositiveLength::new(width), PositiveLength::new(height)) {
                    if geometry.shape() == PadGeometryShape::RoundedRect {
                        g.flash_rect(
                            pad.position,
                            w,
                            h,
                            geometry.corner_radius(),
                            pad.rotation,
                            &attributes,
                        );
                    } else {
                        g.flash_octagon(
                            pad.position,
                            w,
                            h,
                            geometry.corner_radius(),
                            pad.rotation,
                            &attributes,
                        );
                    }
                }
            }
            PadGeometryShape::Stroke => {
                let Ok(w) = PositiveLength::new(width) else {
                    continue;
                };
                if geometry.path().vertices().is_empty() {
                    continue;
                }
                let path = pad.transform().map(geometry.path());
                match path.vertices() {
                    [single] => {
                        // For maximum compatibility, convert the stroke to a
                        // circle.
                        g.flash_circle(single.pos, w, &attributes);
                    }
                    [v0, v1] if v0.angle == Angle::DEG0 => {
                        // For maximum compatibility, convert the stroke to an
                        // obround.
                        let (p0, p1) = (v0.pos, v1.pos);
                        let delta = p1 - p0;
                        let center = (p0 + p1) / 2;
                        let obround_height = w;
                        let obround_width = obround_height + delta.length();
                        let rotation =
                            Angle::from_rad(libm::atan2(delta.y.to_mm(), delta.x.to_mm()))
                                .unwrap_or_default();
                        g.flash_obround(
                            center,
                            obround_width,
                            obround_height,
                            rotation,
                            &attributes,
                        );
                    }
                    _ => {
                        // As a last resort, convert the outlines to straight
                        // path segments and flash them with outline
                        // apertures.
                        flash_pad_outline(g)?;
                    }
                }
            }
            PadGeometryShape::Custom => flash_pad_outline(g)?,
        }
    }
    Ok(())
}

/// Upstream `drawPolygon()`.
#[allow(clippy::too_many_arguments)]
fn draw_polygon(
    g: &mut GerberGenerator,
    layer: Layer,
    outline: &Path,
    line_width: UnsignedLength,
    fill: bool,
    function: Option<ApertureFunction>,
    net: Option<&str>,
    component: &str,
) {
    let attributes = ObjectAttributes {
        function,
        net,
        component,
        ..Default::default()
    };
    // Don't draw zero-width outlines if the path gets filled! They have no
    // purpose and Gerber states that zero-width strokes shall not be
    // created! However, if the path is not filled, let's draw the outline
    // anyway as this *might* lead to a warning during production to inform
    // the user about this shaky input data.
    if (*line_width > Length::ZERO) || !fill || !outline.is_closed() {
        g.draw_path_outline(outline, calc_width_of_layer(line_width, layer), &attributes);
    }
    // Only fill closed paths (for consistency with the appearance in the
    // board editor, and because Gerber expects area outlines as closed).
    if fill && outline.is_closed() {
        g.draw_path_area(outline, &attributes);
    }
}

/// Upstream `getComponentOutlines()`.
fn component_outlines(dev: &ContextDevice<'_>, layer: Layer) -> Vec<Path> {
    let transform = dev.transform();
    let mut result = Vec::new();
    for polygon in sorted_by_uuid(dev.footprint.polygons(), Polygon::uuid) {
        // Return only closed ones, since Gerber specs say that component
        // outlines must be closed.
        if !polygon.layer().polygons_represent_areas() && !polygon.path().is_closed() {
            continue;
        }
        if polygon.is_filled() {
            continue;
        }
        if transform.map(&polygon.layer()) != layer {
            continue;
        }
        result.push(transform.map(&polygon.path_for_rendering()));
    }
    for circle in sorted_by_uuid(dev.footprint.circles(), Circle::uuid) {
        if circle.is_filled() {
            continue;
        }
        if transform.map(&circle.layer()) != layer {
            continue;
        }
        // Note: Like upstream, the circle center is not taken into account.
        result.push(transform.map(&Path::circle(circle.diameter())));
    }
    result
}

/// Rebuilds the planes of a board and exports its fabrication data (Gerber
/// and Excellon files) according to `settings` (or the board's own
/// fabrication output settings if `None`), like `librepcb-cli
/// --export-pcb-fabrication-data`. Returns the written files.
pub fn export_fabrication_data(
    project: &mut Project,
    board: BoardId,
    settings: Option<&BoardFabricationOutputSettings>,
    info: &ExportInfo,
) -> BoardExportResult<Vec<FilePath>> {
    project.rebuild_planes(board, None)?;
    let settings = match settings {
        Some(s) => s.clone(),
        None => project
            .board(board)
            .map(|b| b.settings().fabrication_output_settings.clone())
            .unwrap_or_default(),
    };
    let mut export = BoardGerberExport::new(project, board, info)?;
    export.export_pcb_layers(&settings)?;
    Ok(export.written_files().to_vec())
}

/// Exports the Gerber X3 component (assembly) layer of one board side (like
/// `librepcb-cli --export-pnp-top/--export-pnp-bottom <file>.gbr`).
pub fn export_component_layer(
    project: &Project,
    board: BoardId,
    side: BoardSide,
    assembly_variant: AssemblyVariantId,
    file_path: &FilePath,
    info: &ExportInfo,
) -> BoardExportResult<()> {
    BoardGerberExport::new(project, board, info)?.export_component_layer(
        side,
        assembly_variant,
        file_path,
    )
}
