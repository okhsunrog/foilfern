//! Port of libs/librepcb/core/project/board/boardgerberexport.{h,cpp}.
//!
//! Resolves board geometry into the existing Gerber/Excellon generators.
//! Returns named file contents; file ownership, writing and obsolete-file
//! removal belong to the caller. Rebuild plane fragments before exporting.

mod component;
mod drawing;

use super::{
    Board, BoardDevice, BoardFabricationOutputSettings, FootprintPadView, ResolvedDevice, ViaView,
};
use crate::export::{
    ApertureFunction, BoardSide, CopperSide, ExcellonGenerator, GerberFileInfo, GerberGenerator,
    Plating, Polarity, Timestamp,
};
use crate::fileio::{CleanFileNameOptions, FilePath};
use crate::geometry::Pad;
use crate::project::{AssemblyVariantId, Error, Project, ProjectAttributeLookup, Result};
use crate::types::Layer;
use crate::utils::transform::Transform;
use std::collections::BTreeMap;

/// Fabrication and assembly exports for one board, with explicit metadata.
#[derive(Debug, Clone)]
pub struct BoardGerberExport<'a> {
    project: &'a Project,
    board: &'a Board,
    info: GerberFileInfo,
}

impl<'a> BoardGerberExport<'a> {
    /// Creates a read-only exporter. The UUID in file headers is the board UUID.
    pub fn new(
        project: &'a Project,
        board: &'a Board,
        app_version: impl Into<String>,
        creation_date: Timestamp,
    ) -> Self {
        let mut name = project.metadata().name.to_string();
        if project.boards().len() > 1 {
            name.push_str(&format!(" ({})", board.name()));
        }
        Self {
            project,
            board,
            info: GerberFileInfo {
                app_version: app_version.into(),
                creation_date,
                project_name: name,
                project_uuid: board.uuid(),
                project_revision: project.metadata().version.to_string(),
            },
        }
    }

    /// Generates configured PCB layers and drill files. Keys are expanded
    /// output paths (relative paths remain relative to the project directory).
    /// Disabled outputs are absent. Filename collisions retain the last output,
    /// matching upstream's sequence of writes.
    pub fn generate_pcb_layers(
        &self,
        settings: &BoardFabricationOutputSettings,
    ) -> Result<BTreeMap<String, String>> {
        self.generate_pcb_layers_with_callback(settings, |_, _| Ok(()))
    }

    /// Generates files in upstream write order, calling `on_file` before
    /// collecting each result. Indexed writers use this to reject duplicate
    /// paths even when two layers of the same job resolve to the same name.
    pub fn generate_pcb_layers_with_callback(
        &self,
        settings: &BoardFabricationOutputSettings,
        mut on_file: impl FnMut(&str, &str) -> Result<()>,
    ) -> Result<BTreeMap<String, String>> {
        let mut files = BTreeMap::new();
        let mut context = FileContext::default();
        let mut insert = |suffix: &str, data: String, ctx: &FileContext| -> Result<()> {
            let path = self.output_path(&format!("{}{suffix}", settings.output_base_path), ctx)?;
            on_file(&path, &data)?;
            files.insert(path, data);
            Ok(())
        };
        if settings.merge_drill_files {
            let mut generator = self.drill_generator(settings, Plating::Mixed);
            self.draw_pth(&mut generator)?;
            self.draw_npth(&mut generator)?;
            insert(&settings.suffix_drills, generator.generate()?, &context)?;
        } else {
            let mut generator = self.drill_generator(settings, Plating::No);
            self.draw_npth(&mut generator)?;
            insert(
                &settings.suffix_drills_npth,
                generator.generate()?,
                &context,
            )?;
            let mut generator = self.drill_generator(settings, Plating::Yes);
            self.draw_pth(&mut generator)?;
            insert(&settings.suffix_drills_pth, generator.generate()?, &context)?;
        }
        let mut blind = BTreeMap::<_, Vec<_>>::new();
        for segment in self.board.net_segments().values() {
            for via in segment.vias().values() {
                if via.is_blind() || via.is_buried() {
                    let view =
                        ViaView::new(via, self.board, segment.net(), self.project.circuit())?;
                    if let Some(span) = view.drill_layer_span() {
                        blind
                            .entry(span)
                            .or_default()
                            .push((via.position(), view.drill_diameter()));
                    }
                }
            }
        }
        for (span, vias) in blind {
            context.span = Some(span);
            let mut generator = self.drill_generator(settings, Plating::Yes);
            for (position, diameter) in vias {
                generator.drill(position, diameter, true, ApertureFunction::ViaDrill);
            }
            insert(
                &settings.suffix_drills_blind_buried,
                generator.generate()?,
                &context,
            )?;
        }
        let mut generator = GerberGenerator::new(&self.info);
        generator.set_file_function_outlines(false);
        for layer in [
            Layer::BOARD_OUTLINES,
            Layer::BOARD_CUTOUTS,
            Layer::BOARD_PLATED_CUTOUTS,
        ] {
            self.draw_layer(&mut generator, layer)?;
        }
        insert(&settings.suffix_outlines, generator.generate(), &context)?;
        let mut copper = vec![(
            Layer::TOP_COPPER,
            CopperSide::Top,
            1,
            settings.suffix_copper_top.as_str(),
        )];
        for n in 1..=self.board.settings().inner_layer_count {
            let layer = Layer::inner_copper(n as usize).ok_or(Error::InvalidFabricationLayer(n))?;
            copper.push((
                layer,
                CopperSide::Inner,
                n as i32 + 1,
                &settings.suffix_copper_inner,
            ));
        }
        copper.push((
            Layer::BOT_COPPER,
            CopperSide::Bottom,
            self.bottom_number(),
            &settings.suffix_copper_bot,
        ));
        for (layer, side, number, suffix) in copper {
            context.inner = if layer.is_inner() {
                layer.copper_number()
            } else {
                0
            };
            let mut generator = GerberGenerator::new(&self.info);
            generator.set_file_function_copper(number, side, Polarity::Positive);
            self.draw_layer(&mut generator, layer)?;
            insert(suffix, generator.generate(), &context)?;
        }
        context.inner = 0;
        // Preserve write order even when configured output names collide.
        for kind in 0..3 {
            for (
                side,
                mask,
                paste,
                mask_suffix,
                silk_suffix,
                paste_suffix,
                silk_layers,
                paste_enabled,
            ) in [
                (
                    BoardSide::Top,
                    Layer::TOP_STOP_MASK,
                    Layer::TOP_SOLDER_PASTE,
                    &settings.suffix_solder_mask_top,
                    &settings.suffix_silkscreen_top,
                    &settings.suffix_solder_paste_top,
                    &self.board.settings().silkscreen_layers_top,
                    settings.enable_solder_paste_top,
                ),
                (
                    BoardSide::Bottom,
                    Layer::BOT_STOP_MASK,
                    Layer::BOT_SOLDER_PASTE,
                    &settings.suffix_solder_mask_bot,
                    &settings.suffix_silkscreen_bot,
                    &settings.suffix_solder_paste_bot,
                    &self.board.settings().silkscreen_layers_bot,
                    settings.enable_solder_paste_bot,
                ),
            ] {
                if kind == 0 && self.board.settings().solder_resist.is_some() {
                    let mut generator = GerberGenerator::new(&self.info);
                    generator.set_file_function_solder_mask(side, Polarity::Negative);
                    self.draw_layer(&mut generator, mask)?;
                    insert(mask_suffix, generator.generate(), &context)?;
                }
                if kind == 1 && !silk_layers.is_empty() {
                    let mut generator = GerberGenerator::new(&self.info);
                    generator.set_file_function_legend(side, Polarity::Positive);
                    for &layer in silk_layers {
                        self.draw_layer(&mut generator, layer)?;
                    }
                    generator.set_layer_polarity(Polarity::Negative);
                    self.draw_layer(&mut generator, mask)?;
                    insert(silk_suffix, generator.generate(), &context)?;
                }
                if kind == 2 && paste_enabled {
                    let mut generator = GerberGenerator::new(&self.info);
                    generator.set_file_function_paste(side, Polarity::Positive);
                    self.draw_layer(&mut generator, paste)?;
                    insert(paste_suffix, generator.generate(), &context)?;
                }
            }
        }
        Ok(files)
    }

    /// Disabled drill/mask/legend/paste outputs which a direct export should
    /// remove. The caller must exclude paths written by this export, since
    /// suffixes can coincide. Indexed output jobs use their ownership index
    /// instead of this legacy cleanup, matching upstream.
    pub fn disabled_pcb_output_paths(
        &self,
        settings: &BoardFabricationOutputSettings,
    ) -> Result<Vec<String>> {
        let board = self.board.settings();
        let disabled = [
            (!settings.merge_drill_files, &settings.suffix_drills),
            (settings.merge_drill_files, &settings.suffix_drills_npth),
            (settings.merge_drill_files, &settings.suffix_drills_pth),
            (
                board.solder_resist.is_none(),
                &settings.suffix_solder_mask_top,
            ),
            (
                board.solder_resist.is_none(),
                &settings.suffix_solder_mask_bot,
            ),
            (
                board.silkscreen_layers_top.is_empty(),
                &settings.suffix_silkscreen_top,
            ),
            (
                board.silkscreen_layers_bot.is_empty(),
                &settings.suffix_silkscreen_bot,
            ),
            (
                !settings.enable_solder_paste_top,
                &settings.suffix_solder_paste_top,
            ),
            (
                !settings.enable_solder_paste_bot,
                &settings.suffix_solder_paste_bot,
            ),
        ];
        disabled
            .into_iter()
            .filter(|(disabled, _)| *disabled)
            .map(|(_, suffix)| {
                self.output_path(
                    &format!("{}{suffix}", settings.output_base_path),
                    &FileContext::default(),
                )
            })
            .collect()
    }

    /// Generates assembly-variant-filtered glue for one side.
    pub fn generate_glue_layer(
        &self,
        side: BoardSide,
        variant: AssemblyVariantId,
    ) -> Result<String> {
        let layer = if side == BoardSide::Top {
            Layer::TOP_GLUE
        } else {
            Layer::BOT_GLUE
        };
        let mut generator = GerberGenerator::new(&self.info);
        generator.set_file_function_glue(side, Polarity::Positive);
        for device in self.board.devices().values() {
            if device.glue()
                && !device
                    .parts(self.project.circuit(), Some(variant))?
                    .is_empty()
            {
                self.draw_device(&mut generator, device, layer)?;
            }
        }
        self.draw_board(&mut generator, layer)?;
        Ok(generator.generate())
    }

    fn bottom_number(&self) -> i32 {
        self.board.settings().inner_layer_count as i32 + 2
    }
    fn drill_generator(
        &self,
        settings: &BoardFabricationOutputSettings,
        plating: Plating,
    ) -> ExcellonGenerator {
        let mut generator = ExcellonGenerator::new(&self.info, plating, 1, self.bottom_number());
        generator.set_use_g85_slots(settings.use_g85_slot_command);
        generator
    }
    fn pads(&self, device: &'a BoardDevice) -> Result<Vec<FootprintPadView<'a>>> {
        let mut pads = device.pads(self.project.library(), self.project.circuit())?;
        pads.sort_unstable_by_key(FootprintPadView::uuid);
        Ok(pads)
    }
    fn draw_npth(&self, generator: &mut ExcellonGenerator) -> Result<()> {
        for device in self.board.devices().values() {
            for hole in ResolvedDevice::resolve(device, self.project.library())?
                .footprint
                .holes()
            {
                generator.drill_path(
                    device.transform().map(hole.path()),
                    hole.diameter(),
                    false,
                    ApertureFunction::MechanicalDrill,
                );
            }
        }
        for hole in self.board.holes().values() {
            generator.drill_path(
                hole.path().clone(),
                hole.diameter(),
                false,
                ApertureFunction::MechanicalDrill,
            );
        }
        Ok(())
    }
    fn draw_pth(&self, generator: &mut ExcellonGenerator) -> Result<()> {
        let draw = |generator: &mut ExcellonGenerator, pad: &Pad, transform: Transform| {
            let function = if pad.function() == crate::geometry::PadFunction::PressFitPad {
                ApertureFunction::ComponentDrillPressFit
            } else {
                ApertureFunction::ComponentDrill
            };
            for hole in pad.holes() {
                generator.drill_path(transform.map(hole.path()), hole.diameter(), true, function);
            }
        };
        for device in self.board.devices().values() {
            for pad in self.pads(device)? {
                draw(
                    generator,
                    pad.properties(),
                    Transform::new(pad.position(), pad.rotation(), pad.mirrored()),
                );
            }
        }
        for segment in self.board.net_segments().values() {
            for pad in segment.pads().values() {
                draw(
                    generator,
                    pad.pad(),
                    Transform::new(pad.pad().position(), pad.pad().rotation(), false),
                );
            }
            for via in segment.vias().values() {
                if via.is_through() {
                    let view =
                        ViaView::new(via, self.board, segment.net(), self.project.circuit())?;
                    generator.drill(
                        via.position(),
                        view.drill_diameter(),
                        true,
                        ApertureFunction::ViaDrill,
                    );
                }
            }
        }
        Ok(())
    }
    fn output_path(&self, template: &str, context: &FileContext) -> Result<String> {
        let lookup = ProjectAttributeLookup::board(self.project, self.board, None);
        let mut error = None;
        let path = crate::attribute::substitute(
            template,
            |key| {
                let layer_name = |layer: Layer| {
                    if layer.is_top() {
                        "TOP".into()
                    } else if layer.is_bottom() {
                        "BOTTOM".into()
                    } else {
                        format!("IN{}", layer.copper_number())
                    }
                };
                match (key, context.span) {
                    ("CU_LAYER", _) if context.inner > 0 => Some(context.inner.to_string()),
                    ("START_LAYER", Some((start, _))) => Some(layer_name(start)),
                    ("END_LAYER", Some((_, end))) => Some(layer_name(end)),
                    ("START_NUMBER", Some((start, _))) => {
                        Some((start.copper_number() + 1).to_string())
                    }
                    ("END_NUMBER", Some((_, end))) => Some((end.copper_number() + 1).to_string()),
                    _ => match lookup.value(key) {
                        Ok(value) => value,
                        Err(e) => {
                            error = Some(e);
                            None
                        }
                    },
                }
            },
            Some(&mut |value| {
                FilePath::clean_file_name(
                    value,
                    CleanFileNameOptions {
                        replace_spaces: true,
                        ..Default::default()
                    },
                    120,
                )
            }),
        );
        if let Some(error) = error {
            return Err(error);
        }
        Ok(path)
    }
}

#[derive(Default)]
struct FileContext {
    inner: usize,
    span: Option<(Layer, Layer)>,
}
