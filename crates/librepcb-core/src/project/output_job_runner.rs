//! Port of libs/librepcb/core/project/outputjobrunner.{h,cpp}.
//! Synchronous execution uses existing format generators and indexed output
//! ownership. Application version/time are explicit; observer callbacks
//! replace Qt signals. Graphics and CAD encoding are provided by the caller.

use super::board::{
    BoardD356NetlistExport, BoardGerberExport, BoardInteractiveBomGenerator,
    BoardPickPlaceGenerator,
};
use super::{
    AssemblyVariantId, BoardId, BomGenerator, Project, ProjectAttributeLookup, ProjectJsonExport,
};
use crate::export::{
    BoardSide, BomCsvWriter, PickPlaceCsvWriter, PickPlaceSides, PickPlaceType, Timestamp,
};
use crate::fileio::{
    self, CleanFileNameOptions, FilePath, FileSystem, OutputDirectoryObserver,
    OutputDirectoryWriter, TransactionalFileSystem, ZipWriter,
};
use crate::job::{Error, GraphicsContentType, GraphicsJob, JobKind, ObjectSet, OutputJob, Result};
use crate::types::Uuid;
use librepcb_i18n::tr;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Lifecycle notifications. File operations have a separate observer on the
/// indexed writer, so callers can report the exact paths before each write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputJobEvent {
    Started(Uuid),
    Warning(String),
}
/// An encoded graphics file, named relative to the indexed output directory.
#[derive(Debug, Clone)]
pub struct GraphicsJobFile {
    pub name: String,
    pub contents: Vec<u8>,
}
/// Encoding lives outside core; the runner retains ownership of all writes.
pub type GraphicsJobExporter =
    Box<dyn Fn(&Project, &GraphicsJob) -> Result<Vec<GraphicsJobFile>> + Send>;
/// A prepared STEP export. Core owns naming, indexing, archives and cleanup.
pub struct BoardStepOutput {
    pub contents: Vec<u8>,
    pub warnings: Vec<String>,
}
pub type BoardStepExporter = Box<
    dyn Fn(&Project, &super::board::Board, Option<AssemblyVariantId>) -> Result<BoardStepOutput>
        + Send,
>;
/// Executes jobs in the supplied order. Archive dependencies must precede
/// their consumers, as upstream. A failure stops the run and retains files
/// already written; obsolete files are removed only after job success.
pub struct OutputJobRunner<'a> {
    project: &'a mut Project,
    writer: OutputDirectoryWriter,
    app_version: String,
    timestamp: Timestamp,
    graphics_exporter: Option<GraphicsJobExporter>,
    step_exporter: Option<BoardStepExporter>,
}
static_assertions::assert_impl_all!(OutputJobRunner<'_>:Send);
impl<'a> OutputJobRunner<'a> {
    pub fn new(
        project: &'a mut Project,
        output: &FilePath,
        app_version: impl Into<String>,
        timestamp: Timestamp,
    ) -> Self {
        Self {
            project,
            writer: OutputDirectoryWriter::new(output),
            app_version: app_version.into(),
            timestamp,
            graphics_exporter: None,
            step_exporter: None,
        }
    }
    pub fn directory_path(&self) -> &FilePath {
        self.writer.directory_path()
    }
    pub fn written_files(&self) -> &HashMap<Uuid, Vec<FilePath>> {
        self.writer.written_files()
    }
    pub fn set_file_observer(&mut self, observer: Option<OutputDirectoryObserver>) {
        self.writer.set_observer(observer);
    }
    pub fn set_graphics_exporter(&mut self, exporter: Option<GraphicsJobExporter>) {
        self.graphics_exporter = exporter;
    }
    pub fn set_step_exporter(&mut self, exporter: Option<BoardStepExporter>) {
        self.step_exporter = exporter;
    }
    pub fn find_unknown_files(&self, known: &HashSet<Uuid>) -> Result<Vec<FilePath>> {
        Ok(self.writer.find_unknown_files(known)?)
    }
    pub fn remove_unknown_files(&mut self, files: &[FilePath]) -> Result<()> {
        Ok(self.writer.remove_unknown_files(files)?)
    }
    pub fn run(&mut self, jobs: &[OutputJob]) -> Result<()> {
        self.run_with_observer(jobs, |_| {})
    }
    pub fn run_with_observer(
        &mut self,
        jobs: &[OutputJob],
        mut observer: impl FnMut(OutputJobEvent),
    ) -> Result<()> {
        self.writer.load_index();
        for job in jobs {
            observer(OutputJobEvent::Started(job.uuid()));
            let before = self
                .writer
                .written_files()
                .get(&job.uuid())
                .map_or(0, Vec::len);
            self.run_job(job, &mut observer)?;
            self.writer.remove_obsolete_files(&job.uuid())?;
            if self
                .writer
                .written_files()
                .get(&job.uuid())
                .map_or(0, Vec::len)
                <= before
            {
                observer(OutputJobEvent::Warning(tr!(
                    "OutputJobRunner",
                    "No output files were generated, check the job configuration."
                )));
            }
        }
        Ok(self.writer.store_index()?)
    }
    fn path(
        &self,
        template: &str,
        board: Option<BoardId>,
        variant: Option<AssemblyVariantId>,
    ) -> Result<String> {
        let board = board.and_then(|id| self.project.board(id));
        let variant = variant.and_then(|id| self.project.circuit().assembly_variant(id));
        let lookup = if let Some(board) = board {
            ProjectAttributeLookup::board(self.project, board, variant)
        } else {
            ProjectAttributeLookup::project(self.project, variant)
        };
        let mut error = None;
        let result = crate::attribute::substitute(
            template,
            |key| match lookup.value(key) {
                Ok(v) => v,
                Err(e) => {
                    error = Some(e);
                    None
                }
            },
            Some(&mut |s| {
                FilePath::clean_file_name(
                    s,
                    CleanFileNameOptions {
                        replace_spaces: true,
                        ..Default::default()
                    },
                    120,
                )
            }),
        );
        if let Some(e) = error {
            return Err(e.into());
        }
        Ok(result)
    }
    fn write(&mut self, job: &OutputJob, name: &str, contents: &[u8]) -> Result<()> {
        let path = self.writer.begin_writing_file(&job.uuid(), name)?;
        Ok(fileio::file_utils::write_file(&path, contents)?)
    }
    fn board(&self, id: BoardId) -> Result<&super::board::Board> {
        self.project.board(id).ok_or(Error::BoardNotFound(id.0))
    }
    fn boards(&self, set: &ObjectSet<Option<Uuid>>) -> Result<Vec<Option<BoardId>>> {
        select_job_boards(self.project, set)
    }
    fn required_boards(&self, set: &ObjectSet<Uuid>) -> Result<Vec<BoardId>> {
        Ok(self
            .boards(&optional_set(set))?
            .into_iter()
            .flatten()
            .collect())
    }
    fn variants(&self, set: &ObjectSet<Option<Uuid>>) -> Result<Vec<Option<AssemblyVariantId>>> {
        select_job_variants(self.project, set)
    }
    fn required_variants(&self, set: &ObjectSet<Uuid>) -> Result<Vec<AssemblyVariantId>> {
        Ok(self
            .variants(&optional_set(set))?
            .into_iter()
            .flatten()
            .collect())
    }
    fn rebuild_planes(&mut self, id: BoardId) -> Result<()> {
        // Rebuild all copper planes even if a caller constructed detached
        // items without dirty flags, so stale fills can never be exported.
        Ok(self.project.rebuild_planes(id, None)?)
    }
    fn run_job(
        &mut self,
        job: &OutputJob,
        observer: &mut impl FnMut(OutputJobEvent),
    ) -> Result<()> {
        match &job.kind {
            JobKind::ProjectJson { output } => {
                let path = self.path(output, None, None)?;
                let bytes = ProjectJsonExport::to_utf8(self.project)?;
                self.write(job, &path, &bytes)?;
            }
            JobKind::Bom {
                custom_attributes,
                boards,
                variants,
                output,
            } => {
                for board in self.boards(boards)? {
                    for variant in self.required_variants(variants)? {
                        let path = self.path(output, board, Some(variant))?;
                        format(&path, "csv", "BOM")?;
                        let mut gen_ = BomGenerator::new(self.project);
                        gen_.set_additional_attributes(custom_attributes.clone());
                        let bom =
                            gen_.generate(board.and_then(|id| self.project.board(id)), variant)?;
                        let csv = BomCsvWriter::new(&bom).generate_csv()?.to_csv_string()?;
                        self.write(job, &path, csv.as_bytes())?;
                    }
                }
            }
            JobKind::Netlist { boards, output } => {
                for board in self.required_boards(boards)? {
                    let path = self.path(output, Some(board), None)?;
                    format(&path, "d356", "netlist")?;
                    let text = BoardD356NetlistExport::new(
                        self.project,
                        self.board(board)?,
                        &self.app_version,
                        self.timestamp,
                    )
                    .generate()?;
                    self.write(job, &path, text.as_bytes())?;
                }
            }
            JobKind::GerberExcellon { boards, settings } => {
                for board in self.required_boards(boards)? {
                    self.rebuild_planes(board)?;
                    let value = self
                        .project
                        .board(board)
                        .ok_or(Error::BoardNotFound(board.0))?;
                    let exporter = BoardGerberExport::new(
                        self.project,
                        value,
                        &self.app_version,
                        self.timestamp,
                    );
                    let writer = &mut self.writer;
                    exporter.generate_pcb_layers_with_callback(settings, |path, text| {
                        let path = writer.begin_writing_file(&job.uuid(), path)?;
                        Ok(fileio::file_utils::write_file(&path, text.as_bytes())?)
                    })?;
                }
            }
            JobKind::PickPlace(j) => {
                let types: Vec<_> = [
                    (j.tht, PickPlaceType::Tht),
                    (j.smt, PickPlaceType::Smt),
                    (j.mixed, PickPlaceType::Mixed),
                    (j.fiducial, PickPlaceType::Fiducial),
                    (j.other, PickPlaceType::Other),
                ]
                .into_iter()
                .filter_map(|(enabled, t)| enabled.then_some(t))
                .collect();
                if types.is_empty() {
                    observer(OutputJobEvent::Warning(tr!(
                        "OutputJobRunner",
                        "No technologies selected, thus the output files won't contain any entries."
                    )));
                }
                for board in self.required_boards(&j.boards)? {
                    for variant in self.required_variants(&j.variants)? {
                        let data =
                            BoardPickPlaceGenerator::new(self.project, self.board(board)?, variant)
                                .generate()?;
                        for (side, output) in [
                            (PickPlaceSides::Top, &j.top),
                            (PickPlaceSides::Bottom, &j.bottom),
                            (PickPlaceSides::Both, &j.both),
                        ] {
                            if !output.create {
                                continue;
                            }
                            let path = self.path(&output.output, Some(board), Some(variant))?;
                            format(&path, "csv", "pick&place")?;
                            let mut writer = PickPlaceCsvWriter::new(&data, &self.app_version);
                            writer.set_generation_date(self.timestamp);
                            writer.set_include_metadata_comment(j.comment);
                            writer.set_board_sides(side);
                            writer.set_type_filter(types.iter().copied());
                            let text = writer.generate_csv()?.to_csv_string()?;
                            self.write(job, &path, text.as_bytes())?;
                        }
                    }
                }
            }
            JobKind::GerberX3(j) => {
                for board in self.required_boards(&j.boards)? {
                    for variant in self.required_variants(&j.variants)? {
                        for (side, glue, output) in [
                            (BoardSide::Top, false, &j.components_top),
                            (BoardSide::Bottom, false, &j.components_bottom),
                            (BoardSide::Top, true, &j.glue_top),
                            (BoardSide::Bottom, true, &j.glue_bottom),
                        ] {
                            if !output.create {
                                continue;
                            }
                            let path = self.path(&output.output, Some(board), Some(variant))?;
                            let exporter = BoardGerberExport::new(
                                self.project,
                                self.board(board)?,
                                &self.app_version,
                                self.timestamp,
                            );
                            let text = if glue {
                                exporter.generate_glue_layer(side, variant)?
                            } else {
                                exporter.generate_component_layer(side, variant)?
                            };
                            self.write(job, &path, text.as_bytes())?;
                        }
                    }
                }
            }
            JobKind::Copy {
                substitute_variables,
                boards,
                variants,
                input,
                output,
            } => {
                for board in self.boards(boards)? {
                    for variant in self.variants(variants)? {
                        let input = self.path(input, board, variant)?;
                        let path = self.path(output, board, variant)?;
                        if std::path::Path::new(&input).is_absolute() {
                            return Err(Error::InvalidCopyInput);
                        }
                        let input = TransactionalFileSystem::clean_path(&input);
                        if input.is_empty() || input.starts_with("..") {
                            return Err(Error::InvalidCopyInput);
                        }
                        let mut bytes = self.project.directory().read(&input)?;
                        if *substitute_variables {
                            let v =
                                variant.and_then(|id| self.project.circuit().assembly_variant(id));
                            let lookup = if let Some(b) = board {
                                ProjectAttributeLookup::board(self.project, self.board(b)?, v)
                            } else {
                                ProjectAttributeLookup::project(self.project, v)
                            };
                            bytes = lookup
                                .substitute(&String::from_utf8_lossy(&bytes))?
                                .into_bytes();
                        }
                        self.write(job, &path, &bytes)?;
                    }
                }
            }
            JobKind::Archive { inputs, output } => {
                let path = self.path(output, None, None)?;
                format(&path, "zip", "archive")?;
                let mut files = BTreeMap::new();
                for (uuid, destination) in inputs {
                    let paths = self
                        .writer
                        .written_files()
                        .get(uuid)
                        .ok_or(Error::ArchiveDependencyNotRun)?;
                    // Upstream QMultiHash returns files in reverse insertion order.
                    for path in paths.iter().rev() {
                        let name = TransactionalFileSystem::clean_path(&format!(
                            "{destination}/{}",
                            path.file_name()
                        ));
                        if name.starts_with("..") {
                            return Err(fileio::Error::SandboxBreakout(name).into());
                        }
                        files.insert(name, fileio::file_utils::read_file(path)?);
                    }
                }
                if inputs.is_empty() {
                    observer(OutputJobEvent::Warning(tr!(
                        "OutputJobRunner",
                        "No input jobs selected, thus the resulting archive will be empty."
                    )));
                }
                let mut zip = ZipWriter::new_in_memory();
                for (name, data) in files {
                    zip.write_file(&name, &data, 0o644)?;
                }
                self.write(job, &path, &zip.finish()?)?;
            }
            JobKind::Lppz { output } => {
                let path = self.path(output, None, None)?;
                if crate::application::is_file_format_stable() {
                    self.project.save()?;
                }
                let bytes = self
                    .project
                    .directory()
                    .file_system()
                    .export_to_zip(Some(&|p| {
                        !p.starts_with("output/")
                            && !p.starts_with("logs/")
                            && !p.ends_with(".user.lp")
                    }))?;
                self.write(job, &path, &bytes)?;
            }
            JobKind::Unknown(_) => return Err(Error::UnknownJob(job.kind.type_name().into())),
            JobKind::Graphics(settings) => {
                if self.graphics_exporter.is_none() {
                    return Err(Error::DeferredJob(job.kind.type_name().into()));
                }
                let mut boards = BTreeSet::new();
                for content in &settings.content {
                    if matches!(
                        content.kind,
                        GraphicsContentType::Board | GraphicsContentType::BoardRendering
                    ) {
                        boards.extend(self.boards(&content.boards)?.into_iter().flatten());
                    }
                }
                for board in boards {
                    self.rebuild_planes(board)?;
                }
                // The presence was checked before rebuilding; no user input can
                // change the callback while this synchronous method runs.
                let exporter = self.graphics_exporter.as_ref().expect("checked above");
                let files = exporter(self.project, settings)?;
                for file in files {
                    self.write(job, &file.name, &file.contents)?;
                }
            }
            JobKind::InteractiveBom(settings) => {
                let boards = self.required_boards(&settings.boards)?;
                let variants = self.required_variants(&settings.variants)?;
                for board in boards {
                    self.rebuild_planes(board)?;
                    for variant in variants.iter().copied() {
                        let path = self.path(&settings.output, Some(board), Some(variant))?;
                        let suffix = std::path::Path::new(&path)
                            .extension()
                            .and_then(|s| s.to_str())
                            .unwrap_or_default();
                        if !["html", "htm", "xhtml"]
                            .iter()
                            .any(|s| suffix.eq_ignore_ascii_case(s))
                        {
                            return Err(Error::UnsupportedFormat {
                                kind: "interactive BOM",
                                suffix: suffix.into(),
                            });
                        }
                        let board = self.board(board)?;
                        let mut generator =
                            BoardInteractiveBomGenerator::new(self.project, board, variant);
                        generator.set_custom_attributes(settings.custom_attributes.clone());
                        generator.set_component_order(settings.component_order.clone());
                        let mut data = generator.generate(self.timestamp)?;
                        data.view_mode = match settings.view_mode {
                            crate::job::BomViewMode::BomOnly => {
                                interactive_html_bom::ViewMode::BomOnly
                            }
                            crate::job::BomViewMode::LeftRight => {
                                interactive_html_bom::ViewMode::LeftRight
                            }
                            crate::job::BomViewMode::TopBottom => {
                                interactive_html_bom::ViewMode::TopBottom
                            }
                        };
                        data.highlight_pin1 = match settings.highlight_pin1 {
                            crate::job::HighlightPin1::None => {
                                interactive_html_bom::HighlightPin1Mode::None
                            }
                            crate::job::HighlightPin1::Selected => {
                                interactive_html_bom::HighlightPin1Mode::Selected
                            }
                            crate::job::HighlightPin1::All => {
                                interactive_html_bom::HighlightPin1Mode::All
                            }
                        };
                        data.dark_mode = settings.dark_mode;
                        data.board_rotation = settings.rotation.to_deg() as f32;
                        data.offset_back_rotation = settings.offset_back_rotation;
                        data.show_silkscreen = settings.show_silkscreen;
                        data.show_fabrication = settings.show_fabrication;
                        data.show_pads = settings.show_pads;
                        data.checkboxes = settings.checkboxes.clone();
                        // Source jobs currently leave show_tracks/show_zones
                        // unused; preserve the complete copper data like upstream.
                        let html = data.generate_html().map_err(Error::InteractiveBom)?;
                        self.write(job, &path, html.as_bytes())?;
                    }
                }
            }
            JobKind::Board3d {
                boards,
                variants,
                output,
            } => {
                if self.step_exporter.is_none() {
                    return Err(Error::DeferredJob(job.kind.type_name().into()));
                }
                for board in self.required_boards(boards)? {
                    self.rebuild_planes(board)?;
                    for variant in self.variants(variants)? {
                        let path = self.path(output, Some(board), variant)?;
                        let suffix = std::path::Path::new(&path)
                            .extension()
                            .and_then(|s| s.to_str())
                            .unwrap_or_default();
                        if !["step", "stp"]
                            .iter()
                            .any(|s| suffix.eq_ignore_ascii_case(s))
                        {
                            return Err(Error::UnsupportedFormat {
                                kind: "STEP",
                                suffix: suffix.into(),
                            });
                        }
                        let exporter = self
                            .step_exporter
                            .as_ref()
                            .expect("exporter presence checked before synchronous execution");
                        let prepared = exporter(self.project, self.board(board)?, variant)?;
                        for warning in prepared.warnings {
                            observer(OutputJobEvent::Warning(warning));
                        }
                        self.write(job, &path, &prepared.contents)?;
                    }
                }
            }
        }
        Ok(())
    }
}
/// Resolves nullable boards in project order. `All` excludes the null choice;
/// `Default` is the first board, or null for projects without any boards.
pub fn select_job_boards(
    project: &Project,
    set: &ObjectSet<Option<Uuid>>,
) -> Result<Vec<Option<BoardId>>> {
    select(
        set,
        project.boards().iter().map(|b| (b.uuid(), b.id())),
        false,
        Error::BoardNotFound,
    )
}
/// Resolves nullable assembly variants with the same semantics as boards.
pub fn select_job_variants(
    project: &Project,
    set: &ObjectSet<Option<Uuid>>,
) -> Result<Vec<Option<AssemblyVariantId>>> {
    select(
        set,
        project
            .circuit()
            .assembly_variants()
            .iter()
            .map(|v| (v.uuid(), AssemblyVariantId(v.uuid()))),
        false,
        Error::VariantNotFound,
    )
}
fn optional_set(set: &ObjectSet<Uuid>) -> ObjectSet<Option<Uuid>> {
    match set {
        ObjectSet::All => ObjectSet::All,
        ObjectSet::Default => ObjectSet::Default,
        ObjectSet::Custom(v) => ObjectSet::Custom(v.iter().copied().map(Some).collect()),
    }
}
fn select<I: Copy>(
    set: &ObjectSet<Option<Uuid>>,
    objects: impl Iterator<Item = (Uuid, I)>,
    include_null: bool,
    error: impl FnOnce(Uuid) -> Error,
) -> Result<Vec<Option<I>>> {
    let objects: Vec<_> = objects.collect();
    match set {
        ObjectSet::All => Ok(include_null
            .then_some(None)
            .into_iter()
            .chain(objects.into_iter().map(|(_, v)| Some(v)))
            .collect()),
        ObjectSet::Default => Ok(vec![objects.first().map(|(_, id)| *id)]),
        ObjectSet::Custom(values) => {
            let mut remaining: BTreeSet<_> = values.iter().copied().flatten().collect();
            let mut result = Vec::new();
            if values.contains(&None) {
                result.push(None);
            }
            for (uuid, id) in objects {
                if remaining.remove(&uuid) {
                    result.push(Some(id));
                }
            }
            if let Some(uuid) = remaining.first() {
                return Err(error(*uuid));
            }
            Ok(result)
        }
    }
}
fn format(path: &str, expected: &str, kind: &'static str) -> Result<()> {
    let suffix = std::path::Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if !suffix.eq_ignore_ascii_case(expected) {
        return Err(Error::UnsupportedFormat {
            kind,
            suffix: suffix.into(),
        });
    }
    Ok(())
}
