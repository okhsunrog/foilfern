//! Output generation from the menus: graphics (PDF) export, Gerber/Excellon,
//! pick&place, IPC-D-356A netlist, bill of materials, `*.lppz` archive and
//! running the project's output jobs.
//!
//! Port of the menu actions of libs/librepcb/editor/project/projecteditor.cpp
//! (`ProjectAction::BillOfMaterials`, `ExportLppz`, `OpenOutputJobs`) and
//! libs/librepcb/editor/project/board/board2dtab.cpp /
//! schematictab.cpp (`TabAction::ExportPdf`, `ExportFabricationData`,
//! `ExportPickPlace`, `ExportD356Netlist`, `BillOfMaterials`).
//!
//! Upstream opens a dialog for each of them (graphics export dialog,
//! fabrication output dialog, BOM generator dialog, output jobs dialog,
//! ...); these dialogs come with M3b. Until then the actions run with the
//! default settings and write into the default output directory
//! `<project>/output/<version>/` (the same directory as upstream's default
//! output jobs), and "Output Jobs" runs all output jobs of the project.
//! Each export runs in a worker thread with a progress notification and
//! reports the written files (or the error) in a notification. The worker
//! holds the project lock while it runs (exports read the whole project and
//! rebuild the planes first), so the UI waits for it when it needs the
//! project, like upstream's modal progress dialogs.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use librepcb_app_ui as ui;
use librepcb_core::export::{BomCsvWriter, PickPlaceSides, Timestamp};
use librepcb_core::fileio::{CleanFileNameOptions, FileNameCase, FilePath};
use librepcb_core::job::{GraphicsOutputJob, LppzOutputJob, ObjectSet, OutputJob, OutputJobKind};
use librepcb_core::project::board::{
    ExportInfo, export_d356_netlist, export_fabrication_data, export_pick_place_csv,
};
use librepcb_core::project::{
    AssemblyVariantId, BoardId, BomGenerator, OutputJobError, OutputJobEvent, OutputJobRunner,
    Project,
};
use librepcb_editor::{OpenProject, SharedProject};
use librepcb_graphics::ProjectGraphicsExporter;
use librepcb_i18n::tr;

use crate::app::{APP_VERSION, State, with_current_state};
use crate::notifications::{Notification, NotificationId};
use crate::project::AppProject;
use crate::tabs::Tab;

/// Reports progress of an export: percent (0..=100) and a status text.
pub type ExportProgress<'a> = &'a (dyn Fn(u32, &str) + Sync);

/// The result of an export: the written files.
pub type ExportResult = Result<Vec<FilePath>, String>;

/// An export running in a worker thread on the locked project.
type ExportJob = Box<dyn FnOnce(&mut OpenProject, ExportProgress<'_>) -> ExportResult + Send>;

/// The default output directory `<project>/output/<version>` (upstream
/// default output jobs: `./output/{{VERSION}}/`).
pub fn output_dir(p: &Project) -> Result<FilePath, String> {
    let dir = p
        .path()
        .ok_or_else(|| "The project has no directory on disk.".to_owned())?;
    let version = FilePath::clean_file_name(
        p.metadata().version.as_str(),
        CleanFileNameOptions::new(true, FileNameCase::Keep),
        120,
    );
    Ok(dir.path_to(&format!("output/{version}")))
}

/// The project name as file name.
fn file_base(p: &Project) -> String {
    FilePath::clean_file_name(
        p.metadata().name.as_str(),
        CleanFileNameOptions::new(true, FileNameCase::Keep),
        120,
    )
}

/// The first (default) assembly variant.
fn default_variant(p: &Project) -> Result<AssemblyVariantId, String> {
    p.circuit()
        .assembly_variants()
        .iter()
        .next()
        .map(|av| AssemblyVariantId(av.uuid()))
        .ok_or_else(|| "The project has no assembly variant.".to_owned())
}

fn export_info() -> ExportInfo {
    ExportInfo::now(APP_VERSION)
}

/// Runs output jobs (upstream `OutputJobRunner` with the graphics
/// exporter of the scene crate); returns the written files. Jobs whose
/// type is not supported yet (3D) are skipped with a warning when
/// `skip_unsupported`.
pub fn run_output_jobs(
    open: &mut OpenProject,
    jobs: &[OutputJob],
    skip_unsupported: bool,
    progress: ExportProgress<'_>,
) -> Result<(Vec<FilePath>, Vec<String>), String> {
    let written = Arc::new(parking_lot::Mutex::new(Vec::<FilePath>::new()));
    let warnings = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let (w, warn) = (Arc::clone(&written), Arc::clone(&warnings));
    let count = jobs.len().max(1);
    let result = open
        .editor
        .update_derived_data(|project| {
            let mut runner = OutputJobRunner::new(project, export_info())?;
            runner.set_step_exporter(Some(librepcb_step::board_exporter()));
            runner.set_graphics_exporter(Some(Arc::new(ProjectGraphicsExporter::new(format!(
                "LibrePCB {APP_VERSION}"
            )))));
            runner.set_observer(Some(Box::new(move |event| match event {
                OutputJobEvent::AboutToWriteFile(fp) => w.lock().push(fp.clone()),
                OutputJobEvent::Warning(msg) => warn.lock().push(msg.to_string()),
                _ => {}
            })));
            let mut skipped = Vec::new();
            for (i, job) in jobs.iter().enumerate() {
                progress(
                    (i * 100 / count) as u32,
                    &tr!(
                        "CommandLineInterface",
                        "Run output job '{0}'...",
                        job.name().as_str()
                    ),
                );
                match runner.run(std::slice::from_ref(job)) {
                    Err(OutputJobError::Unsupported(kind)) if skip_unsupported => {
                        skipped.push(format!(
                            "Skipped the output job \"{}\" ({kind} jobs are not supported yet).",
                            job.name().as_str()
                        ));
                    }
                    Err(OutputJobError::ArchiveDependencyNotRun)
                        if skip_unsupported && !skipped.is_empty() =>
                    {
                        skipped.push(format!(
                            "Skipped the output job \"{}\" (it archives files of a skipped job).",
                            job.name().as_str()
                        ));
                    }
                    other => other?,
                }
            }
            Ok::<_, OutputJobError>(skipped)
        })
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let mut files = written.lock().clone();
    files.sort();
    files.dedup();
    let mut warnings = warnings.lock().clone();
    warnings.extend(result);
    Ok((files, warnings))
}

impl State {
    /// Handles the output related `Backend.trigger-tab()` actions of
    /// schematic and board tabs; returns whether the action was handled.
    pub(crate) fn trigger_tab_output(
        &mut self,
        section: usize,
        tab: usize,
        action: ui::TabAction,
    ) -> bool {
        let Some(t) = self.sections.get(section).and_then(|s| s.tabs().get(tab)) else {
            return false;
        };
        let (project, board) = match t {
            Tab::Schematic(t) => (Rc::clone(t.project()), None),
            Tab::Board2d(t) => (Rc::clone(t.project()), Some(t.board())),
            _ => return false,
        };
        use crate::dialogs::output::{GraphicsExportDialog, GraphicsExportKind as K};
        let unit = t
            .length_unit()
            .unwrap_or(librepcb_core::types::LengthUnit::Millimeters);
        let graphics = match (action, board) {
            (ui::TabAction::ExportPdf, None) => Some(K::SchematicPdf),
            (ui::TabAction::ExportPdf, Some(b)) => Some(K::BoardPdf(b)),
            (ui::TabAction::ExportImage, None) => Some(K::SchematicImage),
            (ui::TabAction::ExportImage, Some(b)) => Some(K::BoardImage(b)),
            _ => None,
        };
        if let Some(kind) = graphics {
            let dialog = GraphicsExportDialog::new(&project, kind, unit);
            self.show_form_dialog(project, Box::new(dialog));
            return true;
        }
        match (action, board) {
            (ui::TabAction::ExportFabricationData, Some(b)) => self.export_fabrication(&project, b),
            (ui::TabAction::ExportPickPlace, Some(b)) => {
                if let Some(dialog) =
                    crate::dialogs::review::PickPlaceGeneratorDialog::new(&project, b)
                {
                    self.show_form_dialog(project, Box::new(dialog));
                }
            }
            (ui::TabAction::ExportD356Netlist, Some(b)) => self.export_netlist(&project, b),
            (ui::TabAction::BillOfMaterials, b) => {
                let dialog = crate::dialogs::review::BomReviewDialog::new(&project, b);
                self.show_form_dialog(project, Box::new(dialog));
            }
            (ui::TabAction::Print | ui::TabAction::ExportSpecctra, _) => {
                self.not_implemented(&format!("{action:?}"));
            }
            _ => return false,
        }
        true
    }

    /// Handles the output related `Backend.trigger-project()` actions;
    /// returns whether the action was handled.
    pub(crate) fn trigger_project_output(
        &mut self,
        project: &Rc<AppProject>,
        action: ui::ProjectAction,
    ) -> bool {
        match action {
            ui::ProjectAction::BillOfMaterials => {
                // Upstream: the board if the project has only one.
                let board = {
                    let p = project.shared().lock();
                    let boards = p.project().boards();
                    (boards.len() == 1).then(|| boards[0].id())
                };
                let dialog = crate::dialogs::review::BomReviewDialog::new(project, board);
                self.show_form_dialog(Rc::clone(project), Box::new(dialog));
            }
            ui::ProjectAction::ExportLppz => self.export_lppz(project),
            ui::ProjectAction::OpenOutputJobs => {
                let unit = crate::dialogs::default_unit(project);
                let dialog = crate::dialogs::output::OutputJobsDialog::new(project, unit);
                self.show_form_dialog(Rc::clone(project), Box::new(dialog));
            }
            _ => return false,
        }
        true
    }

    /// Exports the schematics (`board == None`) or the assembly drawings of
    /// a board as PDF (upstream graphics export dialog with its default
    /// settings).
    pub fn export_graphics(&mut self, project: &Rc<AppProject>, board: Option<BoardId>) {
        let mut job = match board {
            None => GraphicsOutputJob::schematic_pdf(),
            Some(_) => GraphicsOutputJob::board_assembly_pdf(),
        };
        if let (Some(board), OutputJobKind::Graphics(g)) = (board, job.kind_mut()) {
            for content in &mut g.content {
                content.boards = ObjectSet::custom([Some(board)]);
            }
        }
        let title = tr!("librepcb::editor::GraphicsExportDialog", "Export PDF");
        self.run_export(
            project,
            title,
            Box::new(move |open, progress| {
                run_output_jobs(open, std::slice::from_ref(&job), false, progress)
                    .map(|(files, _)| files)
            }),
        );
    }

    /// Generates the Gerber/Excellon files of a board with its fabrication
    /// output settings.
    pub fn export_fabrication(&mut self, project: &Rc<AppProject>, board: BoardId) {
        let title = tr!("EditorCommandSet", "Generate Fabrication Data");
        self.run_export(
            project,
            title,
            Box::new(move |open, _| {
                let info = export_info();
                open.editor
                    .update_derived_data(|p| export_fabrication_data(p, board, None, &info))
                    .map_err(|e| e.to_string())?
                    .map_err(|e| e.to_string())
            }),
        );
    }

    /// Generates the pick&place files (top and bottom) of a board.
    pub fn export_pick_place(&mut self, project: &Rc<AppProject>, board: BoardId) {
        let title = tr!(
            "librepcb::editor::BoardPickPlaceGeneratorDialog",
            "Generate Pick&Place Data"
        );
        self.run_export(
            project,
            title,
            Box::new(move |open, _| {
                let p = open.project();
                let variant = default_variant(p)?;
                let dir = output_dir(p)?;
                let base = file_base(p);
                let mut files = Vec::new();
                for (sides, name) in [
                    (PickPlaceSides::Top, "top"),
                    (PickPlaceSides::Bottom, "bottom"),
                ] {
                    let file = dir.path_to(&format!("assembly/{base}_PnP-{name}.csv"));
                    export_pick_place_csv(
                        p,
                        board,
                        variant,
                        sides,
                        &file,
                        APP_VERSION,
                        Timestamp::now(),
                    )
                    .map_err(|e| e.to_string())?;
                    files.push(file);
                }
                Ok(files)
            }),
        );
    }

    /// Exports the IPC-D-356A netlist of a board.
    pub fn export_netlist(&mut self, project: &Rc<AppProject>, board: BoardId) {
        let title = tr!("librepcb::editor::Board2dTab", "Export IPC D-356A Netlist");
        self.run_export(
            project,
            title,
            Box::new(move |open, _| {
                let p = open.project();
                let file = output_dir(p)?.path_to(&format!("{}_netlist.d356", file_base(p)));
                export_d356_netlist(p, board, &file, &export_info()).map_err(|e| e.to_string())?;
                Ok(vec![file])
            }),
        );
    }

    /// Generates the bill of materials (of all components, or of the
    /// devices of a board) of the default assembly variant as CSV.
    pub fn export_bom(&mut self, project: &Rc<AppProject>, board: Option<BoardId>) {
        let title = tr!("EditorCommandSet", "Bill Of Materials");
        self.run_export(
            project,
            title,
            Box::new(move |open, _| {
                let p = open.project();
                let variant = default_variant(p)?;
                let b = board.and_then(|b| p.board(b));
                let mut generator = BomGenerator::new(p);
                generator.set_additional_attributes(p.settings().custom_bom_attributes.clone());
                let bom = generator.generate(b, variant);
                let csv = BomCsvWriter::new(&bom)
                    .generate_csv()
                    .map_err(|e| e.to_string())?;
                let suffix = b
                    .map(|b| format!("_{}", b.properties().name))
                    .unwrap_or_default();
                let file = output_dir(p)?.path_to(&format!("{}_BOM{suffix}.csv", file_base(p)));
                csv.save_to_file(&file).map_err(|e| e.to_string())?;
                Ok(vec![file])
            }),
        );
    }

    /// Exports the project as `*.lppz` archive.
    pub fn export_lppz(&mut self, project: &Rc<AppProject>) {
        let title = tr!("EditorCommandSet", "Export *.lppz Archive");
        self.run_export(
            project,
            title,
            Box::new(move |open, progress| {
                let job = OutputJob::new_default::<LppzOutputJob>();
                run_output_jobs(open, std::slice::from_ref(&job), false, progress)
                    .map(|(files, _)| files)
            }),
        );
    }

    /// Runs all output jobs of the project (upstream output jobs dialog,
    /// "Run All").
    pub fn run_all_output_jobs(&mut self, project: &Rc<AppProject>) {
        let title = tr!("librepcb::editor::OutputJobsDialog", "Output Jobs");
        self.run_export(
            project,
            title,
            Box::new(move |open, progress| {
                let jobs: Vec<OutputJob> = open.project().output_jobs().iter().cloned().collect();
                if jobs.is_empty() {
                    return Err("The project has no output jobs.".to_owned());
                }
                let (files, warnings) = run_output_jobs(open, &jobs, true, progress)?;
                for w in &warnings {
                    log::warn!("{w}");
                }
                Ok(files)
            }),
        );
    }

    /// Runs output jobs (e.g. of the output jobs or graphics export
    /// dialog) in a worker thread.
    pub fn run_jobs(&mut self, project: &Rc<AppProject>, title: String, jobs: Vec<OutputJob>) {
        self.run_export(
            project,
            title,
            Box::new(move |open, progress| {
                let (files, warnings) = run_output_jobs(open, &jobs, false, progress)?;
                for w in &warnings {
                    log::warn!("{w}");
                }
                Ok(files)
            }),
        );
    }

    /// Runs an export in a worker thread with a progress notification.
    fn run_export(&mut self, project: &Rc<AppProject>, title: String, job: ExportJob) {
        let notification = self
            .notifications
            .borrow_mut()
            .push_with_id(Notification::new(
                ui::NotificationType::Progress,
                format!("{title}..."),
                String::new(),
            ));
        let shared = Arc::clone(project.shared());
        let spawned = std::thread::Builder::new()
            .name("export".into())
            .spawn(move || {
                let last = AtomicUsize::new(usize::MAX);
                let progress = |percent: u32, status: &str| {
                    let Some(id) = notification else { return };
                    if last.swap(percent as usize, Ordering::Relaxed) == percent as usize {
                        return;
                    }
                    let status = slint::SharedString::from(status);
                    let _ = slint::invoke_from_event_loop(move || {
                        with_current_state(|s| {
                            s.notifications.borrow_mut().update(id, |d| {
                                d.progress = percent as i32;
                                d.description = status;
                            });
                        });
                    });
                };
                let result = {
                    let mut open = shared.lock();
                    job(&mut open, &progress)
                };
                let _ = slint::invoke_from_event_loop(move || {
                    with_current_state(|s| {
                        s.export_finished(&shared, &title, notification, result)
                    });
                });
            });
        if let Err(e) = spawned {
            let shared = Arc::clone(project.shared());
            self.export_finished(&shared, "", notification, Err(e.to_string()));
        }
    }

    /// Reports the result of an export.
    fn export_finished(
        &mut self,
        shared: &SharedProject,
        title: &str,
        notification: Option<NotificationId>,
        result: ExportResult,
    ) {
        if let Some(id) = notification {
            self.notifications.borrow_mut().dismiss(id);
        }
        let n = match &result {
            Ok(files) => {
                let list: Vec<String> = files.iter().map(FilePath::to_native).collect();
                log::info!("{title}: wrote {}", list.join(", "));
                let text = tr!(
                    "librepcb::editor::OutputJobsDialog",
                    "Success! Generated files:"
                );
                self.show_status(
                    &tr!(
                        "librepcb::editor::MainWindow",
                        "{0}: {1} file(s) written",
                        title,
                        files.len()
                    ),
                    4000,
                );
                Notification::new(
                    ui::NotificationType::Info,
                    title.to_owned(),
                    format!("{text}\n{}", list.join("\n")),
                )
            }
            Err(e) => {
                log::error!("{title} failed: {e}");
                Notification {
                    auto_popup: true,
                    ..Notification::new(ui::NotificationType::Critical, title.to_owned(), e.clone())
                }
            }
        };
        self.notifications.borrow_mut().push(n);
        // Planes may have been rebuilt.
        if let Some(p) = self
            .projects
            .iter()
            .find(|p| Arc::ptr_eq(p.shared(), shared))
            .cloned()
        {
            self.sync_project_tabs(&p);
        }
    }
}
