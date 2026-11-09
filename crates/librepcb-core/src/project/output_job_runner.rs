//! Port of libs/librepcb/core/project/outputjobrunner.{h,cpp}.
//!
//! Runs [output jobs](crate::job) of a project: substitutes the attributes
//! in the output paths, writes the files into the output directory through
//! an [`OutputDirectoryWriter`] (which detects files written multiple times
//! and removes obsolete files of a job) and reports progress through an
//! observer callback.
//!
//! Differences to upstream:
//! - The Qt signals (`jobStarted`, `aboutToWriteFile`, `aboutToRemoveFile`,
//!   `warning`) are one observer callback receiving [`OutputJobEvent`]s.
//! - The application version and the creation date written into the files
//!   are passed in ([`ExportInfo`]).
//! - Graphics (PDF/SVG/image) jobs: the runner builds the pages (upstream
//!   `buildPages()`, [`GraphicsPage`]) and hands them to a
//!   [`GraphicsExporter`] set by the caller
//!   ([`OutputJobRunner::set_graphics_exporter()`]), since the painters live
//!   in the scene crate (which depends on core). Without an exporter, these
//!   jobs fail with [`OutputJobError::Unsupported`].
//! - Mechanical STEP jobs use an injected pure-Rust CAD exporter. Core
//!   retains filenames, output ownership and archives without a CAD dependency.
//! - Archive jobs collect their input files in an in-memory transactional
//!   file system (upstream opens a writable one in a temporary directory,
//!   which is left behind); the archive content is the same.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use librepcb_i18n::tr;

use super::board::{
    BoardExportError, BoardFabricationOutputSettings, BoardGerberExport, ExportInfo,
};
use super::board::{
    BoardInteractiveHtmlBomGenerator, BoardPickPlaceGenerator, export_d356_netlist,
};
use super::circuit::AssemblyVariant;
use super::{
    AssemblyVariantId, BoardId, BomGenerator, Project, ProjectAttributeLookup, SchematicId,
    json_export,
};
use crate::application;
use crate::export::{
    BoardSide, BomCsvWriter, GraphicsExportSettings, PageSize, PickPlaceCsvWriter, PickPlaceSides,
    PickPlaceType,
};
use crate::fileio::{
    self, CleanFileNameOptions, FileNameCase, FilePath, FileSystem, OutputDirectoryEvent,
    OutputDirectoryWriter, TransactionalDirectory, file_utils,
};
use crate::job::{
    ArchiveOutputJob, Board3DOutputJob, BomOutputJob, CopyOutputJob, GerberExcellonOutputJob,
    GerberX3OutputJob, GraphicsContentType, GraphicsOutputJob, InteractiveHtmlBomOutputJob,
    LppzOutputJob, NetlistOutputJob, ObjectSet, OutputJob, OutputJobKind, PickPlaceOutputJob,
    ProjectJsonOutputJob,
};
use crate::types::Uuid;

/// Translation context of the upstream class.
const TR_CONTEXT: &str = "librepcb::OutputJobRunner";

/// Error of the [`OutputJobRunner`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OutputJobError {
    /// The job type is unknown to this version (upstream `LogicError`).
    #[error(
        "{} {}",
        tr!(TR_CONTEXT, "Unknown output job type '{0}'.", .0),
        tr!(TR_CONTEXT, "You may need a more recent LibrePCB version to run this job.")
    )]
    UnknownJobType(String),
    /// The job type is known, but not supported by this port yet (3D jobs,
    /// graphics jobs without exporter). Not an upstream error.
    #[error(
        "Output jobs of type '{0}' are not supported yet by this LibrePCB version (librepcb-rs)."
    )]
    Unsupported(String),
    /// A graphics job has a page size key unknown to Qt's `QPageSize`.
    #[error("Unsupported page size: '{0}'")]
    UnsupportedPageSize(String),
    /// A graphics job contains an assembly guide (upstream supports them only
    /// in later releases).
    #[error(
        "Assembly guide output jobs are not supported yet, you need to use a more recent release of LibrePCB."
    )]
    AssemblyGuideUnsupported,
    /// The graphics export failed (all error messages, joined by `"; "`).
    #[error("{0}")]
    Graphics(String),
    /// Mechanical model preparation or serialization failed.
    #[error("{0}")]
    Step(String),
    #[error("Unsupported STEP format: '{0}'")]
    UnsupportedStepFormat(String),
    /// A board of the job does not exist.
    #[error("Board does not exist: {0}")]
    BoardNotFound(Uuid),
    /// An assembly variant of the job does not exist.
    #[error("Assembly variant does not exist: {0}")]
    AssemblyVariantNotFound(Uuid),
    /// Unsupported output file extension of a pick&place job.
    #[error("Unsupported pick&place format: '{0}'")]
    UnsupportedPickPlaceFormat(String),
    /// Unsupported output file extension of a netlist job.
    #[error("Unsupported netlist format: '{0}'")]
    UnsupportedNetlistFormat(String),
    /// Unsupported output file extension of a BOM job.
    #[error("Unsupported BOM format: '{0}'")]
    UnsupportedBomFormat(String),
    /// Unsupported output file extension of an interactive BOM job.
    #[error("Unsupported interactive BOM format: '{0}'")]
    UnsupportedInteractiveBomFormat(String),
    /// Unsupported output file extension of an archive job.
    #[error("Unsupported archive format: '{0}'")]
    UnsupportedArchiveFormat(String),
    /// The input file of a copy job is not inside the project directory.
    #[error(
        "{}",
        tr!(
            TR_CONTEXT,
            "The input file must be located within the project directory, specified by a relative file path."
        )
    )]
    CopyInputOutsideProject,
    /// An archive job depends on a job which was not run before.
    #[error(
        "{}",
        tr!(
            TR_CONTEXT,
            "The archive job depends on files from another job which was not run yet. Note that archive jobs can only depend on jobs further ahead in the list so you might need to reorder them."
        )
    )]
    ArchiveDependencyNotRun,
    /// The project is not stored in a directory on disk (the output
    /// directory is relative to it).
    #[error("The project has no directory on disk, thus output jobs cannot be run.")]
    NoProjectDirectory,
    /// A file operation failed (including output directory errors).
    #[error(transparent)]
    FileIo(#[from] fileio::Error),
    /// A board export failed.
    #[error(transparent)]
    BoardExport(Box<BoardExportError>),
    /// An export generator failed.
    #[error(transparent)]
    Export(#[from] crate::export::Error),
    /// Saving the project failed (`*.lppz` export).
    #[error(transparent)]
    Project(Box<super::Error>),
}

impl From<BoardExportError> for OutputJobError {
    fn from(e: BoardExportError) -> Self {
        Self::BoardExport(Box::new(e))
    }
}

impl From<super::Error> for OutputJobError {
    fn from(e: super::Error) -> Self {
        Self::Project(Box::new(e))
    }
}

/// Result type of the [`OutputJobRunner`].
pub type OutputJobResult<T, E = OutputJobError> = std::result::Result<T, E>;

/// Progress reported by an [`OutputJobRunner`] (upstream signals).
#[derive(Debug, Clone, Copy)]
pub enum OutputJobEvent<'a> {
    /// A job is about to be run (upstream `jobStarted()`).
    JobStarted(&'a OutputJob),
    /// A file is about to be written (upstream `aboutToWriteFile()`).
    AboutToWriteFile(&'a FilePath),
    /// A file is about to be removed (upstream `aboutToRemoveFile()`).
    AboutToRemoveFile(&'a FilePath),
    /// A (non-fatal) problem with the job configuration (upstream
    /// `warning()`).
    Warning(&'a str),
}

/// What a page of a graphics export shows (upstream: the
/// `GraphicsPagePainter` subclass created by `buildPages()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsPageContent {
    /// A schematic page (upstream `SchematicPainter`).
    Schematic(SchematicId),
    /// A board (upstream `BoardPainter`).
    Board(BoardId),
    /// A realistic rendering of a board (upstream `RealisticBoardPainter`).
    BoardRendering(BoardId),
}

/// A page of a graphics export (upstream `GraphicsExport::Page`).
#[derive(Debug, Clone, PartialEq)]
pub struct GraphicsPage {
    /// What to paint.
    pub content: GraphicsPageContent,
    /// Page layout and colors.
    pub settings: GraphicsExportSettings,
}

/// Result of a graphics export (upstream `GraphicsExport::Result`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphicsExportResult {
    /// The written files (upstream reports the PDF file even if the export
    /// failed later).
    pub written_files: Vec<FilePath>,
    /// Error messages (of the export and of the painters, e.g. images
    /// which could not be loaded); empty on success.
    pub errors: Vec<String>,
}

/// Encoded graphics output, prepared before paths are reserved or written.
#[derive(Debug, Clone)]
pub struct GraphicsJobFile {
    pub path: FilePath,
    pub contents: Vec<u8>,
}

/// Exports pages of a project to a PDF, SVG or image file (upstream
/// `GraphicsExport::startExport()` + `waitForFinished()`). Implemented
/// outside of core (the painters need the scene crate).
pub trait GraphicsExporter: Send + Sync {
    /// Memory-first exporters let the runner check all paths before writing.
    /// The fallback supports existing exporters that own their writes.
    fn prepare(
        &self,
        _project: &Project,
        _pages: &[GraphicsPage],
        _file_path: &FilePath,
        _document_name: &str,
    ) -> Option<OutputJobResult<Vec<GraphicsJobFile>>> {
        None
    }
    /// Exports `pages` to `file_path` (the file type is taken from the
    /// suffix; for several pages of an image format, the page number is
    /// appended to the file name). `document_name` is the document title of
    /// PDF and SVG files.
    fn export(
        &self,
        project: &Project,
        pages: &[GraphicsPage],
        file_path: &FilePath,
        document_name: &str,
    ) -> GraphicsExportResult;
}

/// Prepared STEP bytes; the runner owns every filesystem write.
#[derive(Debug, Clone)]
pub struct BoardStepOutput {
    pub contents: Vec<u8>,
    pub warnings: Vec<String>,
}
pub type BoardStepExporter = Box<
    dyn Fn(
            &Project,
            &super::board::Board,
            Option<AssemblyVariantId>,
        ) -> OutputJobResult<BoardStepOutput>
        + Send,
>;

/// Observer callback of an [`OutputJobRunner`].
pub type OutputJobObserver = Box<dyn FnMut(OutputJobEvent<'_>) + Send>;

/// Shared slot of the observer (the output directory writer forwards its
/// events to it).
type SharedObserver = Arc<Mutex<Option<OutputJobObserver>>>;

fn notify(observer: &SharedObserver, event: OutputJobEvent<'_>) {
    let mut guard = observer.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(observer) = guard.as_mut() {
        observer(event);
    }
}

impl Project {
    /// Returns the output directory of the project (`output/` in the
    /// project directory, upstream `getOutputDir()`), or `None` if the
    /// project is not stored on disk.
    pub fn output_dir(&self) -> Option<FilePath> {
        self.path().map(|p| p.path_to("output"))
    }

    /// Returns the output directory of the current project version
    /// (`output/<version>/`, upstream `getCurrentOutputDir()`).
    pub fn current_output_dir(&self) -> Option<FilePath> {
        self.output_dir()
            .map(|p| p.path_to(self.metadata().version.as_ref()))
    }
}

/// Runs output jobs of a project (upstream `OutputJobRunner`).
pub struct OutputJobRunner<'a> {
    project: &'a mut Project,
    writer: OutputDirectoryWriter,
    info: ExportInfo,
    observer: SharedObserver,
    graphics_exporter: Option<Arc<dyn GraphicsExporter>>,
    step_exporter: Option<BoardStepExporter>,
}

impl std::fmt::Debug for OutputJobRunner<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutputJobRunner")
            .field("writer", &self.writer)
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

/// Filter of the substituted attribute values in output paths.
fn clean_file_name(value: &str) -> String {
    FilePath::clean_file_name(
        value,
        CleanFileNameOptions::new(true, FileNameCase::Keep),
        120,
    )
}

/// Substitutes the attributes in an output path.
fn output_path(lookup: &ProjectAttributeLookup<'_>, path: &str) -> String {
    lookup.substitute_filtered(path, &mut clean_file_name)
}

impl<'a> OutputJobRunner<'a> {
    /// Creates a runner writing into the current output directory of the
    /// project (see [`Project::current_output_dir()`]).
    pub fn new(project: &'a mut Project, info: ExportInfo) -> OutputJobResult<Self> {
        let dir = project
            .current_output_dir()
            .ok_or(OutputJobError::NoProjectDirectory)?;
        let observer: SharedObserver = Arc::new(Mutex::new(None));
        let mut runner = Self {
            project,
            writer: OutputDirectoryWriter::new(&dir),
            info,
            observer,
            graphics_exporter: None,
            step_exporter: None,
        };
        runner.set_output_directory(&dir);
        Ok(runner)
    }

    /// Sets the observer notified about the progress.
    pub fn set_observer(&mut self, observer: Option<OutputJobObserver>) {
        *self.observer.lock().unwrap_or_else(PoisonError::into_inner) = observer;
    }

    /// Sets the exporter which runs graphics (PDF/SVG/image) jobs; without
    /// one, they fail with [`OutputJobError::Unsupported`].
    pub fn set_graphics_exporter(&mut self, exporter: Option<Arc<dyn GraphicsExporter>>) {
        self.graphics_exporter = exporter;
    }

    pub fn set_step_exporter(&mut self, exporter: Option<BoardStepExporter>) {
        self.step_exporter = exporter;
    }

    /// Returns the output directory.
    pub fn output_directory(&self) -> &FilePath {
        self.writer.directory_path()
    }

    /// Returns the files written so far, by job UUID.
    pub fn written_files(&self) -> &HashMap<Uuid, Vec<FilePath>> {
        self.writer.written_files()
    }

    /// Sets the output directory (e.g. a custom directory given on the
    /// command line).
    pub fn set_output_directory(&mut self, dir: &FilePath) {
        let mut writer = OutputDirectoryWriter::new(dir);
        let observer = Arc::clone(&self.observer);
        writer.set_observer(Some(Box::new(move |event| match event {
            OutputDirectoryEvent::AboutToWriteFile(fp) => {
                notify(&observer, OutputJobEvent::AboutToWriteFile(fp));
            }
            OutputDirectoryEvent::AboutToRemoveFile(fp) => {
                notify(&observer, OutputJobEvent::AboutToRemoveFile(fp));
            }
        })));
        self.writer = writer;
    }

    /// Runs the given jobs in the given order.
    pub fn run(&mut self, jobs: &[OutputJob]) -> OutputJobResult<()> {
        self.writer.load_index();
        for job in jobs {
            notify(&self.observer, OutputJobEvent::JobStarted(job));
            self.run_job(job)?;
        }
        self.writer.store_index()?;
        Ok(())
    }

    /// Returns all files in the output directory which were not written by
    /// one of `known_jobs` (upstream `findUnknownFiles()`).
    pub fn find_unknown_files(&self, known_jobs: &HashSet<Uuid>) -> OutputJobResult<Vec<FilePath>> {
        Ok(self.writer.find_unknown_files(known_jobs)?)
    }

    /// Removes files found by
    /// [`find_unknown_files()`](Self::find_unknown_files).
    pub fn remove_unknown_files(&mut self, files: &[FilePath]) -> OutputJobResult<()> {
        Ok(self.writer.remove_unknown_files(files)?)
    }

    fn warning(&self, msg: &str) {
        notify(&self.observer, OutputJobEvent::Warning(msg));
    }

    fn written_count(&self, job: &Uuid) -> usize {
        self.writer.written_files().get(job).map_or(0, Vec::len)
    }

    fn run_job(&mut self, job: &OutputJob) -> OutputJobResult<()> {
        let uuid = job.uuid();
        let count_before = self.written_count(&uuid);
        match job.kind() {
            OutputJobKind::GerberExcellon(j) => self.run_gerber_excellon(uuid, j)?,
            OutputJobKind::PickPlace(j) => self.run_pick_place(uuid, j)?,
            OutputJobKind::GerberX3(j) => self.run_gerber_x3(uuid, j)?,
            OutputJobKind::Netlist(j) => self.run_netlist(uuid, j)?,
            OutputJobKind::Bom(j) => self.run_bom(uuid, j)?,
            OutputJobKind::ProjectJson(j) => self.run_project_json(uuid, j)?,
            OutputJobKind::Lppz(j) => self.run_lppz(uuid, j)?,
            OutputJobKind::Copy(j) => self.run_copy(uuid, j)?,
            OutputJobKind::Archive(j) => self.run_archive(uuid, j)?,
            OutputJobKind::Graphics(j) => match self.graphics_exporter.clone() {
                Some(exporter) => self.run_graphics(uuid, j, exporter.as_ref())?,
                None => {
                    return Err(OutputJobError::Unsupported(job.type_name().to_owned()));
                }
            },
            OutputJobKind::InteractiveHtmlBom(j) => self.run_interactive_html_bom(uuid, j)?,
            OutputJobKind::Board3D(j) => self.run_board_3d(uuid, j)?,
            OutputJobKind::Unknown(_) => {
                return Err(OutputJobError::UnknownJobType(job.type_name().to_owned()));
            } // `OutputJobKind` is non-exhaustive for other crates only.
        }
        let count_after = self.written_count(&uuid);
        self.writer.remove_obsolete_files(&uuid)?;
        if count_after <= count_before {
            self.warning(&tr!(
                TR_CONTEXT,
                "No output files were generated, check the job configuration."
            ));
        }
        Ok(())
    }

    /// Upstream `buildPages()`: the pages of a graphics job, rebuilding
    /// outdated planes of the exported boards if `rebuild_planes`.
    pub fn build_pages(
        &mut self,
        job: &GraphicsOutputJob,
        rebuild_planes: bool,
    ) -> OutputJobResult<Vec<GraphicsPage>> {
        let mut pages = Vec::new();
        for content in &job.content {
            let page_size = match &content.page_size {
                None => None,
                Some(key) if PageSize::is_known_key(key) => Some(key.clone()),
                Some(key) => return Err(OutputJobError::UnsupportedPageSize(key.clone())),
            };
            let mut settings = GraphicsExportSettings {
                page_size,
                orientation: content.orientation,
                margin_left: content.margin_left,
                margin_top: content.margin_top,
                margin_right: content.margin_right,
                margin_bottom: content.margin_bottom,
                rotate: content.rotate,
                mirror: content.mirror,
                scale: content.scale,
                pixmap_dpi: content.pixmap_dpi,
                black_white: content.monochrome,
                background_color: content.background_color,
                min_line_width: content.min_line_width,
                ..GraphicsExportSettings::default()
            };
            if content.content_type == GraphicsContentType::BoardRendering {
                settings.load_board_rendering_colors(crate::types::Layer::INNER_COPPER_COUNT);
            }
            settings.colors = settings
                .colors
                .iter()
                .filter_map(|(role, _)| {
                    content.layers.get(role).map(|color| (role.clone(), *color))
                })
                .collect();
            let boards = self.optional_boards(&content.boards, false)?;
            let variants = self.optional_assembly_variants(&content.assembly_variants, false)?;
            match content.content_type {
                GraphicsContentType::Schematic => {
                    // Upstream does not consider boards and assembly variants
                    // yet (TODO), but they multiply the pages.
                    for _ in &variants {
                        for _ in &boards {
                            for schematic in self.project.schematics() {
                                pages.push(GraphicsPage {
                                    content: GraphicsPageContent::Schematic(schematic.id()),
                                    settings: settings.clone(),
                                });
                            }
                        }
                    }
                }
                GraphicsContentType::Board | GraphicsContentType::BoardRendering => {
                    // Upstream would dereference a null board for "no
                    // board"; skip it.
                    for board in boards.into_iter().flatten() {
                        if rebuild_planes {
                            self.rebuild_outdated_planes(board)?;
                        }
                        let content = if content.content_type == GraphicsContentType::BoardRendering
                        {
                            GraphicsPageContent::BoardRendering(board)
                        } else {
                            GraphicsPageContent::Board(board)
                        };
                        for _ in &variants {
                            pages.push(GraphicsPage {
                                content,
                                settings: settings.clone(),
                            });
                        }
                    }
                }
                GraphicsContentType::AssemblyGuide => {
                    return Err(OutputJobError::AssemblyGuideUnsupported);
                }
            }
        }
        Ok(pages)
    }

    fn run_graphics(
        &mut self,
        uuid: Uuid,
        job: &GraphicsOutputJob,
        exporter: &dyn GraphicsExporter,
    ) -> OutputJobResult<()> {
        // Build pages.
        let pages = self.build_pages(job, true)?;

        // Determine lookup objects.
        let mut all_boards: BTreeSet<Option<BoardId>> = BTreeSet::new();
        let mut all_variants: BTreeSet<Option<AssemblyVariantId>> = BTreeSet::new();
        for content in &job.content {
            all_boards.extend(self.optional_boards(&content.boards, false)?);
            all_variants
                .extend(self.optional_assembly_variants(&content.assembly_variants, false)?);
        }

        // Determine output path.
        let project = &*self.project;
        let variant = match all_variants.iter().collect::<Vec<_>>().as_slice() {
            [Some(av)] => Some(required_variant(project, *av)?),
            _ => None,
        };
        let board = match all_boards.iter().collect::<Vec<_>>().as_slice() {
            [Some(b)] => Some(required_board(project, *b)?),
            _ => None,
        };
        let lookup = match board {
            Some(b) => ProjectAttributeLookup::for_board(project, b, variant),
            None => ProjectAttributeLookup::for_project(project, variant),
        };
        let output = output_path(&lookup, &job.output_path);
        let fp = self.writer.directory_path().path_to(&output);

        // Determine document name (`QString::simplified()`).
        let mut doc_name = job.document_title.as_str().to_owned();
        if doc_name.is_empty() {
            doc_name = "{{PROJECT}} {{VERSION}}".to_owned();
        }
        let doc_name = lookup
            .substitute(&doc_name)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");

        // Reserve all prepared files before the first write, including page
        // suffixes, so a collision cannot overwrite another job's output.
        if let Some(files) = exporter.prepare(project, &pages, &fp, &doc_name) {
            let files = files?;
            let paths = files
                .iter()
                .map(|file| {
                    let relative = file.path.to_relative(self.writer.directory_path());
                    self.writer.begin_writing_file(&uuid, &relative)
                })
                .collect::<fileio::Result<Vec<_>>>()?;
            for (file, path) in files.iter().zip(paths) {
                file_utils::write_file(&path, &file.contents)?;
            }
            return Ok(());
        }
        self.writer.begin_writing_file(&uuid, &output)?;
        // Perform the legacy exporter-owned write.
        let result = exporter.export(project, &pages, &fp, &doc_name);
        for written in &result.written_files {
            if *written != fp {
                // Track additional files.
                let rel = written.to_relative(self.writer.directory_path());
                self.writer.begin_writing_file(&uuid, &rel)?;
            }
        }
        if !result.errors.is_empty() {
            return Err(OutputJobError::Graphics(result.errors.join("; ")));
        }
        Ok(())
    }

    fn run_gerber_excellon(
        &mut self,
        uuid: Uuid,
        job: &GerberExcellonOutputJob,
    ) -> OutputJobResult<()> {
        let settings = BoardFabricationOutputSettings {
            output_base_path: format!(
                "{}/{}",
                self.writer.directory_path().as_str(),
                job.output_path
            ),
            suffix_drills: job.suffix_drills.clone(),
            suffix_drills_npth: job.suffix_drills_npth.clone(),
            suffix_drills_pth: job.suffix_drills_pth.clone(),
            suffix_drills_blind_buried: job.suffix_drills_blind_buried.clone(),
            suffix_outlines: job.suffix_outlines.clone(),
            suffix_copper_top: job.suffix_copper_top.clone(),
            suffix_copper_inner: job.suffix_copper_inner.clone(),
            suffix_copper_bot: job.suffix_copper_bot.clone(),
            suffix_solder_mask_top: job.suffix_solder_mask_top.clone(),
            suffix_solder_mask_bot: job.suffix_solder_mask_bot.clone(),
            suffix_silkscreen_top: job.suffix_silkscreen_top.clone(),
            suffix_silkscreen_bot: job.suffix_silkscreen_bot.clone(),
            suffix_solder_paste_top: job.suffix_solder_paste_top.clone(),
            suffix_solder_paste_bot: job.suffix_solder_paste_bot.clone(),
            merge_drill_files: job.merge_drill_files,
            use_g85_slot_command: job.use_g85_slot_command,
            enable_solder_paste_top: job.enable_solder_paste_top,
            enable_solder_paste_bot: job.enable_solder_paste_bot,
        };
        for board in self.boards(&job.boards)? {
            // Rebuild planes to be sure no outdated planes are exported!
            self.rebuild_outdated_planes(board)?;

            // Now actually export Gerber/Excellon.
            let writer = &mut self.writer;
            let mut export = BoardGerberExport::new(self.project, board, &self.info)?;
            export.set_remove_obsolete_files(false); // Must be done by this runner!
            export.set_before_write_callback(Box::new(|fp: &FilePath| {
                let rel = fp.to_relative(writer.directory_path());
                writer.begin_writing_file(&uuid, &rel)?;
                Ok(())
            }));
            export.export_pcb_layers(&settings)?;
        }
        Ok(())
    }

    fn run_pick_place(&mut self, uuid: Uuid, job: &PickPlaceOutputJob) -> OutputJobResult<()> {
        let boards = self.boards(&job.boards)?;
        let variants = self.assembly_variants(&job.assembly_variants)?;

        let mut sides = Vec::new();
        if job.create_top {
            sides.push((PickPlaceSides::Top, &job.output_path_top));
        }
        if job.create_bottom {
            sides.push((PickPlaceSides::Bottom, &job.output_path_bottom));
        }
        if job.create_both {
            sides.push((PickPlaceSides::Both, &job.output_path_both));
        }

        let t = &job.technologies;
        let type_filter: Vec<PickPlaceType> = [
            (t.tht, PickPlaceType::Tht),
            (t.smt, PickPlaceType::Smt),
            (t.mixed, PickPlaceType::Mixed),
            (t.fiducial, PickPlaceType::Fiducial),
            (t.other, PickPlaceType::Other),
        ]
        .into_iter()
        .filter_map(|(enabled, ty)| enabled.then_some(ty))
        .collect();
        if type_filter.is_empty() {
            self.warning(&tr!(
                TR_CONTEXT,
                "No technologies selected, thus the output files won't contain any entries."
            ));
        }

        let project = &*self.project;
        for board in &boards {
            let b = required_board(project, *board)?;
            for av in &variants {
                let variant = required_variant(project, *av)?;
                let data = BoardPickPlaceGenerator::new(project, *board, *av)?.generate()?;
                let lookup = ProjectAttributeLookup::for_board(project, b, Some(variant));
                for (side, path) in &sides {
                    let fp = self
                        .writer
                        .begin_writing_file(&uuid, &output_path(&lookup, path))?;
                    if fp.suffix().to_lowercase() == "csv" {
                        let mut writer = PickPlaceCsvWriter::new(&data, &self.info.app_version);
                        writer.set_generation_date(self.info.creation_date);
                        writer.set_include_metadata_comment(job.include_comment);
                        writer.set_board_sides(*side);
                        writer.set_type_filter(type_filter.iter().copied());
                        writer.generate_csv()?.save_to_file(&fp)?;
                    } else {
                        return Err(OutputJobError::UnsupportedPickPlaceFormat(
                            fp.suffix().to_owned(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn run_gerber_x3(&mut self, uuid: Uuid, job: &GerberX3OutputJob) -> OutputJobResult<()> {
        let boards = self.boards(&job.boards)?;
        let variants = self.assembly_variants(&job.assembly_variants)?;

        #[derive(Clone, Copy)]
        enum FileType {
            Components,
            Glue,
        }
        let mut files = Vec::new();
        if job.enable_components_top {
            files.push((
                FileType::Components,
                BoardSide::Top,
                &job.output_path_components_top,
            ));
        }
        if job.enable_components_bot {
            files.push((
                FileType::Components,
                BoardSide::Bottom,
                &job.output_path_components_bot,
            ));
        }
        if job.enable_glue_top {
            files.push((FileType::Glue, BoardSide::Top, &job.output_path_glue_top));
        }
        if job.enable_glue_bot {
            files.push((FileType::Glue, BoardSide::Bottom, &job.output_path_glue_bot));
        }

        let project = &*self.project;
        for board in &boards {
            let b = required_board(project, *board)?;
            for av in &variants {
                let variant = required_variant(project, *av)?;
                let lookup = ProjectAttributeLookup::for_board(project, b, Some(variant));
                for (ty, side, path) in &files {
                    let fp = self
                        .writer
                        .begin_writing_file(&uuid, &output_path(&lookup, path))?;
                    let mut export = BoardGerberExport::new(project, *board, &self.info)?;
                    match ty {
                        FileType::Components => export.export_component_layer(*side, *av, &fp)?,
                        FileType::Glue => export.export_glue_layer(*side, *av, &fp)?,
                    }
                }
            }
        }
        Ok(())
    }

    fn run_netlist(&mut self, uuid: Uuid, job: &NetlistOutputJob) -> OutputJobResult<()> {
        let boards = self.boards(&job.boards)?;
        let project = &*self.project;
        for board in boards {
            let b = required_board(project, board)?;
            let lookup = ProjectAttributeLookup::for_board(project, b, None);
            let fp = self
                .writer
                .begin_writing_file(&uuid, &output_path(&lookup, &job.output_path))?;
            if fp.suffix().to_lowercase() == "d356" {
                export_d356_netlist(project, board, &fp, &self.info)?;
            } else {
                return Err(OutputJobError::UnsupportedNetlistFormat(
                    fp.suffix().to_owned(),
                ));
            }
        }
        Ok(())
    }

    fn run_bom(&mut self, uuid: Uuid, job: &BomOutputJob) -> OutputJobResult<()> {
        let boards = self.optional_boards(&job.boards, false)?;
        let variants = self.assembly_variants(&job.assembly_variants)?;
        let project = &*self.project;
        for board in &boards {
            let b = board.map(|id| required_board(project, id)).transpose()?;
            for av in &variants {
                let variant = required_variant(project, *av)?;
                let lookup = match b {
                    Some(b) => ProjectAttributeLookup::for_board(project, b, Some(variant)),
                    None => ProjectAttributeLookup::for_project(project, Some(variant)),
                };
                let fp = self
                    .writer
                    .begin_writing_file(&uuid, &output_path(&lookup, &job.output_path))?;
                let mut generator = BomGenerator::new(project);
                generator.set_additional_attributes(job.custom_attributes.clone());
                let bom = generator.generate(b, *av);
                if fp.suffix().to_lowercase() == "csv" {
                    BomCsvWriter::new(&bom).generate_csv()?.save_to_file(&fp)?;
                } else {
                    return Err(OutputJobError::UnsupportedBomFormat(fp.suffix().to_owned()));
                }
            }
        }
        Ok(())
    }

    /// Upstream `runImpl(const Board3DOutputJob&)`, with an injected CAD kernel.
    fn run_board_3d(&mut self, uuid: Uuid, job: &Board3DOutputJob) -> OutputJobResult<()> {
        let boards = self.boards(&job.boards)?;
        let variants = self.optional_assembly_variants(&job.assembly_variants, false)?;
        for board in boards {
            // Rebuild planes to be sure no outdated planes are exported!
            self.rebuild_outdated_planes(board)?;

            let project = &*self.project;
            let b = required_board(project, board)?;
            // The first file fails, so only the first variant is reached.
            let Some(av) = variants.first() else {
                continue;
            };
            let variant = av.map(|av| required_variant(project, av)).transpose()?;
            let lookup = ProjectAttributeLookup::for_board(project, b, variant);
            let output = output_path(&lookup, &job.output_path);
            let suffix = self
                .writer
                .directory_path()
                .path_to(&output)
                .suffix()
                .to_lowercase();
            if !matches!(suffix.as_str(), "step" | "stp") {
                return Err(OutputJobError::UnsupportedStepFormat(suffix));
            }
            let exporter = self.step_exporter.as_ref().ok_or_else(|| {
                OutputJobError::Unsupported(
                    <Board3DOutputJob as crate::job::OutputJobType>::TYPE_NAME.to_owned(),
                )
            })?;
            // Prepare before reserving/writing output, so a failed CAD import
            // leaves both the previous file and output ownership intact.
            let prepared = exporter(project, b, *av)?;
            for warning in &prepared.warnings {
                notify(&self.observer, OutputJobEvent::Warning(warning));
            }
            let fp = self.writer.begin_writing_file(&uuid, &output)?;
            file_utils::write_file(&fp, &prepared.contents)?;
        }
        Ok(())
    }

    fn run_interactive_html_bom(
        &mut self,
        uuid: Uuid,
        job: &InteractiveHtmlBomOutputJob,
    ) -> OutputJobResult<()> {
        let boards = self.boards(&job.boards)?;
        let variants = self.assembly_variants(&job.assembly_variants)?;
        for board in boards {
            // Rebuild planes to be sure no outdated planes are exported!
            self.rebuild_outdated_planes(board)?;

            let project = &*self.project;
            let b = required_board(project, board)?;
            for av in &variants {
                let variant = required_variant(project, *av)?;
                let lookup = ProjectAttributeLookup::for_board(project, b, Some(variant));
                let fp = self
                    .writer
                    .begin_writing_file(&uuid, &output_path(&lookup, &job.output_path))?;
                let suffix = fp.suffix().to_lowercase();
                if !matches!(suffix.as_str(), "html" | "htm" | "xhtml") {
                    return Err(OutputJobError::UnsupportedInteractiveBomFormat(
                        fp.suffix().to_owned(),
                    ));
                }
                let mut generator = BoardInteractiveHtmlBomGenerator::new(project, board, *av)?;
                generator.set_custom_attributes(job.custom_attributes.clone());
                generator.set_component_order(job.component_order.clone());
                let mut ibom = generator.generate(self.info.creation_date)?;
                ibom.set_view_config(job.view_mode, job.highlight_pin1, job.dark_mode);
                ibom.set_board_rotation(job.board_rotation, job.offset_back_rotation);
                ibom.set_show_silkscreen(job.show_silkscreen);
                ibom.set_show_fabrication(job.show_fabrication);
                ibom.set_show_pads(job.show_pads);
                ibom.set_check_boxes(job.check_boxes.clone());
                let html = ibom.generate_html()?;
                file_utils::write_file(&fp, html.as_bytes())?;
            }
        }
        Ok(())
    }

    fn run_project_json(&mut self, uuid: Uuid, job: &ProjectJsonOutputJob) -> OutputJobResult<()> {
        let project = &*self.project;
        let lookup = ProjectAttributeLookup::for_project(project, None);
        let fp = self
            .writer
            .begin_writing_file(&uuid, &output_path(&lookup, &job.output_path))?;
        file_utils::write_file(&fp, &json_export::to_utf8(project))?;
        Ok(())
    }

    fn run_lppz(&mut self, uuid: Uuid, job: &LppzOutputJob) -> OutputJobResult<()> {
        let fp = {
            let lookup = ProjectAttributeLookup::for_project(self.project, None);
            self.writer
                .begin_writing_file(&uuid, &output_path(&lookup, &job.output_path))?
        };

        // Usually we save the project to the transactional file system (but
        // not to the disk!) before exporting the *.lppz since the user
        // probably expects that the current state of the project gets
        // exported. However, if the file format is unstable (i.e. on
        // development branches), this would lead in a *.lppz of an unstable
        // file format, which is not really useful. Therefore we don't save
        // the project on development branches.
        if application::is_file_format_stable() {
            self.project.save()?;
        }

        // Export project to ZIP, but without the output directory since this
        // can be quite large and usually does not make sense, especially
        // since *.lppz files might even be stored in this directory as well
        // because they are output files. Additionally, logs and user
        // settings will not be exported.
        let filter = |path: &str| {
            !path.starts_with("output/")
                && !path.starts_with("logs/")
                && !path.ends_with(".user.lp")
        };
        self.project
            .directory()
            .file_system()
            .export_to_zip_file(&fp, Some(&filter))?;
        Ok(())
    }

    fn run_copy(&mut self, uuid: Uuid, job: &CopyOutputJob) -> OutputJobResult<()> {
        let boards = self.optional_boards(&job.boards, false)?;
        let variants = self.optional_assembly_variants(&job.assembly_variants, false)?;
        let project = &*self.project;
        let project_dir = project.path().ok_or(OutputJobError::NoProjectDirectory)?;
        for board in &boards {
            let b = board.map(|id| required_board(project, id)).transpose()?;
            for av in &variants {
                let variant = av.map(|id| required_variant(project, id)).transpose()?;
                let lookup = match b {
                    Some(b) => ProjectAttributeLookup::for_board(project, b, variant),
                    None => ProjectAttributeLookup::for_project(project, variant),
                };
                let input_path = output_path(&lookup, &job.input_path);
                let output_fp = self
                    .writer
                    .begin_writing_file(&uuid, &output_path(&lookup, &job.output_path))?;

                // The input file must be located within the project to keep
                // the project self-contained, thus we can load it from the
                // transactional file system. This also ensures that the job
                // works for *.lppz projects. For compatibility, we need to
                // normalize the specified file path.
                let input_fp = project_dir.path_to(&input_path);
                let is_relative = std::path::Path::new(&input_path).is_relative();
                if !is_relative || !input_fp.is_located_in_dir(&project_dir) {
                    return Err(OutputJobError::CopyInputOutsideProject);
                }
                let input_path = input_fp.to_relative(&project_dir);

                // Copy file.
                let mut content = project.directory().read(&input_path)?;
                if job.substitute_variables {
                    content = lookup
                        .substitute(&String::from_utf8_lossy(&content))
                        .into_bytes();
                }
                file_utils::write_file(&output_fp, &content)?;
            }
        }
        Ok(())
    }

    fn run_archive(&mut self, uuid: Uuid, job: &ArchiveOutputJob) -> OutputJobResult<()> {
        let fp = {
            let lookup = ProjectAttributeLookup::for_project(self.project, None);
            self.writer
                .begin_writing_file(&uuid, &output_path(&lookup, &job.output_path))?
        };

        // Collect input files.
        let dir = TransactionalDirectory::new_temporary()?;
        let fs = dir.file_system();
        for (input_job, dest_dir) in &job.input_jobs {
            let Some(files) = self.writer.written_files().get(input_job) else {
                return Err(OutputJobError::ArchiveDependencyNotRun);
            };
            // Upstream iterates `QMultiHash::values()`, i.e. from the most
            // recently to the least recently written file. This matters if
            // several files have the same name: the first written one wins.
            for input_fp in files.iter().rev() {
                let content = file_utils::read_file(input_fp)?;
                fs.write(&format!("{dest_dir}/{}", input_fp.file_name()), &content)?;
            }
        }
        if job.input_jobs.is_empty() {
            self.warning(&tr!(
                TR_CONTEXT,
                "No input jobs selected, thus the resulting archive will be empty."
            ));
        }

        // Export depending on file extension.
        if fp.suffix().to_lowercase() == "zip" {
            fs.export_to_zip_file(&fp, None)?;
            Ok(())
        } else {
            Err(OutputJobError::UnsupportedArchiveFormat(
                fp.suffix().to_owned(),
            ))
        }
    }

    /// Upstream `getBoards(ObjectSet<Uuid>)`.
    fn boards(&self, set: &ObjectSet<BoardId>) -> OutputJobResult<Vec<BoardId>> {
        let all = self.project.boards().iter().map(|b| b.id());
        Ok(match set {
            ObjectSet::All => all.collect(),
            ObjectSet::Default => all.take(1).collect(),
            ObjectSet::Custom(uuids) => {
                let result: Vec<BoardId> = all.filter(|b| uuids.contains(b)).collect();
                if let Some(missing) = uuids.iter().find(|u| !result.contains(u)) {
                    return Err(OutputJobError::BoardNotFound(missing.0));
                }
                result
            }
        })
    }

    /// Upstream `getBoards(ObjectSet<std::optional<Uuid>>, bool)`; `None`
    /// means "no board" (e.g. a BOM of the whole project).
    fn optional_boards(
        &self,
        set: &ObjectSet<Option<BoardId>>,
        include_none_in_all: bool,
    ) -> OutputJobResult<Vec<Option<BoardId>>> {
        select_optional(
            set,
            self.project.boards().iter().map(|b| Some(b.id())),
            include_none_in_all,
            |id| OutputJobError::BoardNotFound(id.0),
        )
    }

    /// Upstream `getAssemblyVariants(ObjectSet<Uuid>)`.
    fn assembly_variants(
        &self,
        set: &ObjectSet<AssemblyVariantId>,
    ) -> OutputJobResult<Vec<AssemblyVariantId>> {
        let all = variant_ids(self.project);
        Ok(match set {
            ObjectSet::All => all.collect(),
            ObjectSet::Default => all.take(1).collect(),
            ObjectSet::Custom(uuids) => {
                let result: Vec<AssemblyVariantId> = all.filter(|v| uuids.contains(v)).collect();
                if let Some(missing) = uuids.iter().find(|u| !result.contains(u)) {
                    return Err(OutputJobError::AssemblyVariantNotFound(missing.0));
                }
                result
            }
        })
    }

    /// Upstream `getAssemblyVariants(ObjectSet<std::optional<Uuid>>,
    /// bool)`; `None` means "no assembly variant".
    fn optional_assembly_variants(
        &self,
        set: &ObjectSet<Option<AssemblyVariantId>>,
        include_none_in_all: bool,
    ) -> OutputJobResult<Vec<Option<AssemblyVariantId>>> {
        select_optional(
            set,
            variant_ids(self.project).map(Some),
            include_none_in_all,
            |id| OutputJobError::AssemblyVariantNotFound(id.0),
        )
    }

    /// Upstream `rebuildOutdatedPlanes()`: rebuilds the planes on copper
    /// layers which are scheduled for rebuild.
    fn rebuild_outdated_planes(&mut self, board: BoardId) -> OutputJobResult<()> {
        let layers: BTreeSet<_> = required_board(self.project, board)?.copper_layers();
        self.project.rebuild_planes(board, Some(&layers))?;
        Ok(())
    }
}

/// Resolve graphics-job boards in project order, with the primary board as default.
pub fn select_job_boards(
    project: &Project,
    set: &ObjectSet<Option<BoardId>>,
) -> OutputJobResult<Vec<Option<BoardId>>> {
    select_optional(
        set,
        project.boards().iter().map(|b| Some(b.id())),
        false,
        |id| OutputJobError::BoardNotFound(id.0),
    )
}
/// Resolve graphics-job variants in project order, with the first variant as default.
pub fn select_job_variants(
    project: &Project,
    set: &ObjectSet<Option<AssemblyVariantId>>,
) -> OutputJobResult<Vec<Option<AssemblyVariantId>>> {
    select_optional(set, variant_ids(project).map(Some), false, |id| {
        OutputJobError::AssemblyVariantNotFound(id.0)
    })
}
fn select_optional<T: Ord + Copy>(
    set: &ObjectSet<Option<T>>,
    all: impl Iterator<Item = Option<T>>,
    include_none_in_all: bool,
    not_found: impl Fn(T) -> OutputJobError,
) -> OutputJobResult<Vec<Option<T>>> {
    match set {
        ObjectSet::All => Ok(include_none_in_all
            .then_some(None)
            .into_iter()
            .chain(all)
            .collect()),
        ObjectSet::Default => Ok(vec![all.take(1).next().flatten()]),
        ObjectSet::Custom(uuids) => custom_selection(uuids, all, not_found),
    }
}

/// Resolves a custom set of optional object IDs in the order of `all`
/// (`None` first), failing for IDs not contained in `all`.
fn custom_selection<T: Ord + Copy>(
    uuids: &BTreeSet<Option<T>>,
    all: impl Iterator<Item = Option<T>>,
    not_found: impl Fn(T) -> OutputJobError,
) -> OutputJobResult<Vec<Option<T>>> {
    let mut result = Vec::new();
    if uuids.contains(&None) {
        result.push(None);
    }
    result.extend(all.filter(|id| id.is_some() && uuids.contains(id)));
    if let Some(Some(missing)) = uuids.iter().find(|u| !result.contains(u)) {
        return Err(not_found(*missing));
    }
    Ok(result)
}

fn variant_ids(project: &Project) -> impl Iterator<Item = AssemblyVariantId> + '_ {
    project
        .circuit()
        .assembly_variants()
        .iter()
        .map(|av| AssemblyVariantId(av.uuid()))
}

fn required_board(project: &Project, id: BoardId) -> OutputJobResult<&super::board::Board> {
    project.board(id).ok_or(OutputJobError::BoardNotFound(id.0))
}

fn required_variant(project: &Project, id: AssemblyVariantId) -> OutputJobResult<&AssemblyVariant> {
    project
        .circuit()
        .assembly_variant(id)
        .ok_or(OutputJobError::AssemblyVariantNotFound(id.0))
}
