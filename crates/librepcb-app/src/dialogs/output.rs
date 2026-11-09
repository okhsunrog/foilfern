//! The graphics export dialog and the output jobs dialog.
//!
//! Ports of libs/librepcb/editor/dialogs/graphicsexportdialog.{ui,cpp} and
//! libs/librepcb/editor/project/outputjobsdialog/*.{ui,cpp} (the dialog
//! and the widgets of all job types).
//!
//! Differences to upstream:
//! - The graphics export dialog edits a graphics output job (like the
//!   output jobs dialog does) and writes the file into the project's
//!   output directory (`output_path` relative to it, with `{{PROJECT}}`
//!   etc. substituted) instead of asking for a file with a file dialog;
//!   the format follows the file extension (`.pdf`, `.svg`, `.png`, ...).
//!   Printing, copying to the clipboard and the live page preview are not
//!   supported; all schematic pages are exported (no page range).
//! - Colors are listed by their color role ID (e.g. "board copper top")
//!   where no layer name exists; they can be enabled and disabled, but
//!   their color cannot be changed.
//! - Output jobs are added from a combo box instead of a menu; presets of
//!   organizations in the workspace libraries and "Import Old Settings"
//!   are not offered; running jobs reports in notifications instead of a
//!   log panel; unknown files in the output directory are not listed.
//! - The interactive HTML BOM job edits name, output, attributes, boards,
//!   assembly variants and the display options (not the check boxes and
//!   component order lists).

use std::collections::BTreeSet;

use librepcb_core::export::{GraphicsExportSettings, PageOrientation, PageSize};
use librepcb_core::job::{
    ArchiveOutputJob, Board3DOutputJob, BomOutputJob, CopyOutputJob, GerberExcellonOutputJob,
    GerberX3OutputJob, GraphicsContentType, GraphicsOutputJob, InteractiveHtmlBomOutputJob,
    LppzOutputJob, NetlistOutputJob, ObjectSet, OutputJob, OutputJobKind, OutputJobList,
    PickPlaceOutputJob, ProjectJsonOutputJob,
};
use librepcb_core::project::{AssemblyVariantId, BoardId, Mutation};
use librepcb_core::types::{
    Color, ElementName, Layer, Length, LengthUnit, UnsignedLength, UnsignedRatio, Uuid,
};
use librepcb_i18n::tr;

use super::{
    Applied, ButtonResult, DialogContext, DialogOptions, FieldEvent, Form, FormDialog, ListAction,
    ListButtons, ListItem, transaction,
};
use crate::project::AppProject;

const GED: &str = "librepcb::editor::GraphicsExportDialog";
const GOJ: &str = "librepcb::editor::GraphicsOutputJobWidget";
const OJD: &str = "librepcb::editor::OutputJobsDialog";

/// Boards and assembly variants of the project (for object set fields).
#[derive(Debug, Clone, Default)]
pub struct ProjectObjects {
    boards: Vec<(BoardId, String)>,
    variants: Vec<(AssemblyVariantId, String)>,
    inner_layers: usize,
    custom_bom_attributes: Vec<String>,
}

impl ProjectObjects {
    fn new(project: &AppProject) -> Self {
        let p = project.shared().lock();
        let prj = p.project();
        Self {
            boards: prj
                .boards()
                .iter()
                .map(|b| (b.id(), b.name().to_string()))
                .collect(),
            variants: prj
                .circuit()
                .assembly_variants()
                .iter()
                .map(|v| (AssemblyVariantId(v.uuid()), v.display_text()))
                .collect(),
            inner_layers: prj
                .boards()
                .iter()
                .map(|b| b.settings().inner_layer_count as usize)
                .max()
                .unwrap_or(0),
            custom_bom_attributes: prj.settings().custom_bom_attributes.clone(),
        }
    }
}

// --- Object sets -------------------------------------------------------------

/// Adds the radio buttons "All", "Default", "Custom:" and a checkable list
/// of the objects (upstream `rbtnBoardsAll` etc.).
fn object_set_fields<T: Ord + Clone>(
    form: &mut Form,
    id: &str,
    label: &str,
    context: &str,
    set: &ObjectSet<T>,
    items: &[(T, String)],
) {
    let index = match set {
        ObjectSet::All => 0,
        ObjectSet::Default => 1,
        ObjectSet::Custom(_) => 2,
    };
    form.radio(
        id,
        label,
        &[
            tr!(context, "All"),
            tr!(context, "Default"),
            tr!(context, "Custom:"),
        ],
        index,
    );
    let checked: Vec<ListItem> = items
        .iter()
        .map(|(v, name)| {
            let on = set.custom_set().is_some_and(|s| s.contains(v));
            ListItem::check(name, on)
        })
        .collect();
    let list = format!("{id}_custom");
    form.list(&list, "", &[], &checked, 3, ListButtons::default());
    form.set_enabled(&list, index == 2);
}

fn chosen_object_set<T: Ord + Clone>(form: &Form, id: &str, items: &[(T, String)]) -> ObjectSet<T> {
    match form.get_index(id) {
        Some(0) => ObjectSet::All,
        Some(1) => ObjectSet::Default,
        _ => {
            let checked = form.get_list_checked(&format!("{id}_custom"));
            ObjectSet::custom(
                items
                    .iter()
                    .zip(checked)
                    .filter(|(_, c)| *c)
                    .map(|((v, _), _)| v.clone()),
            )
        }
    }
}

/// Handles the events of object set fields; returns whether it was one.
fn object_set_event(form: &Form, id: &str, event: &FieldEvent) -> bool {
    if let Some(base) = id.strip_suffix("_custom") {
        if let FieldEvent::List(ListAction::Toggle(row)) = event {
            let mut items: Vec<bool> = form.get_list_checked(id);
            if let Some(v) = items.get_mut(*row) {
                *v = !*v;
            }
            form.update(id, |f| {
                let model = f.items.clone();
                for (i, c) in items.iter().enumerate() {
                    if let Some(mut item) = slint::Model::row_data(&model, i) {
                        item.checked = *c;
                        slint::Model::set_row_data(&model, i, item);
                    }
                }
            });
            let _ = base;
            return true;
        }
        return false;
    }
    if form.contains(&format!("{id}_custom")) {
        form.set_enabled(&format!("{id}_custom"), form.get_index(id) == Some(2));
        return true;
    }
    false
}

fn opt_items<T: Clone>(items: &[(T, String)], none: String) -> Vec<(Option<T>, String)> {
    std::iter::once((None, none))
        .chain(items.iter().map(|(v, n)| (Some(v.clone()), n.clone())))
        .collect()
}

// --- Graphics ------------------------------------------------------------------

/// The page sizes of the combo box (`None` = adjust to content).
fn page_sizes() -> Vec<Option<&'static str>> {
    std::iter::once(None)
        .chain(PageSize::keys().map(Some))
        .collect()
}

/// Name of a color role for the colors list.
fn role_name(role: &str) -> String {
    if let Some(layer) = Layer::all().iter().find(|l| l.color_role() == role) {
        return layer.name_tr();
    }
    let name = role
        .trim_start_matches("schematic_")
        .trim_start_matches("board_")
        .replace('_', " ");
    let mut chars = name.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

/// Editor of a graphics output job: the job settings and the settings of
/// one page (content) at a time.
#[derive(Debug, Clone)]
pub struct GraphicsJobEditor {
    job: GraphicsOutputJob,
    page: usize,
    /// All color roles of the current page with their (default) colors.
    roles: Vec<(String, Color)>,
}

impl GraphicsJobEditor {
    fn new(job: GraphicsOutputJob) -> Self {
        Self {
            job,
            page: 0,
            roles: Vec::new(),
        }
    }

    /// Adds the fields of the job and of the selected page.
    fn build(&mut self, form: &mut Form, objects: &ProjectObjects, with_title: bool) {
        if with_title {
            form.text(
                "g_document_title",
                tr!(GOJ, "Document Title:"),
                self.job.document_title.as_str(),
            );
            form.text("g_output", tr!(GOJ, "Output:"), &self.job.output_path);
        }
        let pages: Vec<ListItem> = self
            .job
            .content
            .iter()
            .map(|c| ListItem::text(&c.title))
            .collect();
        form.list(
            "g_pages",
            tr!("librepcb::editor::GraphicsExportDialog", "Pages"),
            &[],
            &pages,
            3,
            ListButtons {
                remove: true,
                duplicate: true,
                ..ListButtons::default()
            },
        );
        form.set_index("g_pages", Some(self.page).filter(|p| *p < pages.len()));
        self.build_page(form, objects);
    }

    fn build_page(&mut self, form: &mut Form, objects: &ProjectObjects) {
        form.truncate_after("g_pages");
        let Some(c) = self.job.content.get(self.page) else {
            return;
        };
        let c = c.clone();
        form.text(
            "g_title",
            tr!(
                "librepcb::editor::GraphicsOutputJobWidget",
                "Document Title:"
            ),
            &c.title,
        );
        let sizes = page_sizes();
        let names: Vec<String> = sizes
            .iter()
            .map(|s| s.map_or_else(|| tr!(GED, "Custom (adjust to content)"), str::to_owned))
            .collect();
        form.choice(
            "g_page_size",
            tr!(GOJ, "Page size:"),
            &names,
            sizes
                .iter()
                .position(|s| s.map(str::to_owned) == c.page_size),
        );
        form.radio(
            "g_orientation",
            tr!(GOJ, "Orientation:"),
            &[
                tr!(GOJ, "Auto"),
                tr!(GOJ, "Landscape"),
                tr!(GOJ, "Portrait"),
            ],
            match c.orientation {
                PageOrientation::Auto => 0,
                PageOrientation::Landscape => 1,
                PageOrientation::Portrait => 2,
            },
        );
        form.text("g_dpi", tr!(GOJ, "Resolution:"), c.pixmap_dpi.to_string());
        form.set_hint("g_dpi", "dpi");
        form.checkbox(
            "g_scale_auto",
            tr!(GOJ, "Scale factor:"),
            tr!(GOJ, "Fit to page"),
            c.scale.is_none(),
        );
        form.ratio(
            "g_scale",
            "",
            c.scale
                .map_or(librepcb_core::types::Ratio::from_percent(100), |s| *s),
            1,
            i32::MAX,
        );
        form.set_enabled("g_scale", c.scale.is_some());
        form.radio(
            "g_background",
            tr!(GOJ, "Background:"),
            &[tr!(GOJ, "None"), tr!(GOJ, "White"), tr!(GOJ, "Black")],
            if c.background_color == Color::WHITE {
                1
            } else if c.background_color == Color::BLACK {
                2
            } else {
                0
            },
        );
        form.length(
            "g_margin_left",
            tr!(GOJ, "Margins:"),
            *c.margin_left,
            Length::new(0),
        );
        form.length("g_margin_right", "", *c.margin_right, Length::new(0));
        form.length("g_margin_top", "", *c.margin_top, Length::new(0));
        form.length("g_margin_bottom", "", *c.margin_bottom, Length::new(0));
        form.checkbox(
            "g_rotate",
            tr!(GOJ, "Transformation:"),
            tr!(GOJ, "Rotate"),
            c.rotate,
        );
        form.checkbox("g_mirror", "", tr!(GOJ, "Mirror"), c.mirror);
        form.length(
            "g_min_line_width",
            tr!(GOJ, "Minimum line width:"),
            *c.min_line_width,
            Length::new(0),
        );
        form.checkbox(
            "g_monochrome",
            tr!(GOJ, "Colors:"),
            tr!(GOJ, "Monochrome"),
            c.monochrome,
        );
        // Colors: all roles of the content type, enabled if in the page.
        let mut defaults = GraphicsExportSettings::default();
        let prefix = match c.content_type {
            GraphicsContentType::Schematic => "schematic_",
            GraphicsContentType::BoardRendering => {
                defaults.load_board_rendering_colors(objects.inner_layers);
                ""
            }
            _ => {
                defaults.load_default_colors(false, true, objects.inner_layers);
                "board_"
            }
        };
        let mut roles: Vec<(String, Color)> = defaults
            .colors
            .into_iter()
            .filter(|(r, _)| r.starts_with(prefix))
            .collect();
        for (role, color) in &c.layers {
            match roles.iter_mut().find(|(r, _)| r == role) {
                Some(r) => r.1 = *color,
                None => roles.push((role.clone(), *color)),
            }
        }
        let items: Vec<ListItem> = roles
            .iter()
            .map(|(role, color)| ListItem {
                color: Some(slint::Color::from_argb_u8(
                    color.a, color.r, color.g, color.b,
                )),
                ..ListItem::check(role_name(role), c.layers.contains_key(role))
            })
            .collect();
        self.roles = roles;
        form.list("g_layers", "", &[], &items, 8, ListButtons::default());
        if c.content_type != GraphicsContentType::Schematic {
            let boards = opt_items(&objects.boards, tr!(GOJ, "None"));
            object_set_fields(
                form,
                "g_boards",
                &tr!("librepcb::editor::BomOutputJobWidget", "Boards:"),
                "librepcb::editor::BomOutputJobWidget",
                &c.boards,
                &boards,
            );
            let variants = opt_items(&objects.variants, tr!(GOJ, "None"));
            object_set_fields(
                form,
                "g_variants",
                &tr!("librepcb::editor::BomOutputJobWidget", "Assembly Variants:"),
                "librepcb::editor::BomOutputJobWidget",
                &c.assembly_variants,
                &variants,
            );
        }
    }

    /// Reads the fields into the job.
    fn sync(&mut self, form: &Form, objects: &ProjectObjects) -> Result<(), String> {
        if form.contains("g_document_title") {
            self.job.document_title =
                librepcb_core::types::SimpleString::new(form.get_text("g_document_title").trim())
                    .map_err(|e| e.to_string())?;
            self.job.output_path = form.get_text("g_output").trim().to_owned();
        }
        let roles = self.roles.clone();
        let Some(c) = self.job.content.get_mut(self.page) else {
            return Ok(());
        };
        if !form.contains("g_title") {
            return Ok(());
        }
        c.title = form.get_text("g_title");
        c.page_size = form
            .get_index("g_page_size")
            .and_then(|i| page_sizes().get(i).copied())
            .flatten()
            .map(str::to_owned);
        c.orientation = match form.get_index("g_orientation") {
            Some(1) => PageOrientation::Landscape,
            Some(2) => PageOrientation::Portrait,
            _ => PageOrientation::Auto,
        };
        c.pixmap_dpi = form
            .get_text("g_dpi")
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|d| (1..=10_000).contains(d))
            .ok_or_else(|| {
                tr!(GOJ, "Resolution:").replace(':', "") + ": " + &tr!("SlintHelpers", "Invalid")
            })?;
        c.scale = if form.get_checked("g_scale_auto") {
            None
        } else {
            Some(UnsignedRatio::new(form.get_ratio("g_scale")).map_err(|e| e.to_string())?)
        };
        c.background_color = match form.get_index("g_background") {
            Some(1) => Color::WHITE,
            Some(2) => Color::BLACK,
            _ => Color::TRANSPARENT,
        };
        let unsigned =
            |id: &str| UnsignedLength::new(form.get_length(id)).map_err(|e| e.to_string());
        c.margin_left = unsigned("g_margin_left")?;
        c.margin_right = unsigned("g_margin_right")?;
        c.margin_top = unsigned("g_margin_top")?;
        c.margin_bottom = unsigned("g_margin_bottom")?;
        c.rotate = form.get_checked("g_rotate");
        c.mirror = form.get_checked("g_mirror");
        c.min_line_width = unsigned("g_min_line_width")?;
        c.monochrome = form.get_checked("g_monochrome");
        let checked = form.get_list_checked("g_layers");
        c.layers = roles
            .iter()
            .zip(checked)
            .filter(|(_, on)| *on)
            .map(|((r, color), _)| (r.clone(), *color))
            .collect();
        if form.contains("g_boards") {
            c.boards =
                chosen_object_set(form, "g_boards", &opt_items(&objects.boards, String::new()));
            c.assembly_variants = chosen_object_set(
                form,
                "g_variants",
                &opt_items(&objects.variants, String::new()),
            );
        }
        Ok(())
    }

    /// Handles an event; returns `true` if it was a field of this editor.
    fn event(
        &mut self,
        form: &mut Form,
        objects: &ProjectObjects,
        id: &str,
        event: &FieldEvent,
    ) -> bool {
        if !id.starts_with("g_") {
            return false;
        }
        match (id, event) {
            ("g_pages", FieldEvent::List(action)) => {
                let _ = self.sync(form, objects);
                match action {
                    ListAction::Select(i) => self.page = *i,
                    ListAction::Remove(i)
                        if *i < self.job.content.len() && self.job.content.len() > 1 =>
                    {
                        self.job.content.remove(*i);
                        self.page = 0;
                    }
                    ListAction::Duplicate(i) => {
                        if let Some(c) = self.job.content.get(*i).cloned() {
                            self.job.content.insert(*i + 1, c);
                            self.page = *i + 1;
                        }
                    }
                    _ => {}
                }
                let pages: Vec<ListItem> = self
                    .job
                    .content
                    .iter()
                    .map(|c| ListItem::text(&c.title))
                    .collect();
                form.set_items("g_pages", &pages, Some(self.page));
                self.build_page(form, objects);
            }
            ("g_scale_auto", _) => {
                form.set_enabled("g_scale", !form.get_checked("g_scale_auto"));
            }
            ("g_layers", FieldEvent::List(ListAction::Toggle(row))) => {
                let mut checked = form.get_list_checked("g_layers");
                if let Some(c) = checked.get_mut(*row) {
                    *c = !*c;
                }
                let items: Vec<ListItem> = self
                    .roles
                    .iter()
                    .zip(&checked)
                    .map(|((role, color), on)| ListItem {
                        color: Some(slint::Color::from_argb_u8(
                            color.a, color.r, color.g, color.b,
                        )),
                        ..ListItem::check(role_name(role), *on)
                    })
                    .collect();
                let index = form.get_index("g_layers");
                form.set_items("g_layers", &items, index);
            }
            _ => {
                object_set_event(form, id, event);
            }
        }
        true
    }
}

/// The graphics export dialog (upstream `GraphicsExportDialog` for PDF and
/// image exports).
pub struct GraphicsExportDialog {
    form: Form,
    title: String,
    name: ElementName,
    editor: GraphicsJobEditor,
    objects: ProjectObjects,
}

/// What the graphics export dialog exports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsExportKind {
    /// Schematics as PDF.
    SchematicPdf,
    /// Schematics as image (PNG, SVG).
    SchematicImage,
    /// A board's assembly drawings as PDF.
    BoardPdf(BoardId),
    /// A board as image.
    BoardImage(BoardId),
}

impl GraphicsExportDialog {
    /// Opens the dialog.
    pub fn new(project: &AppProject, kind: GraphicsExportKind, unit: LengthUnit) -> Self {
        let objects = ProjectObjects::new(project);
        let (mut job, title, image) = match kind {
            GraphicsExportKind::SchematicPdf => (
                GraphicsOutputJob::schematic_pdf(),
                tr!(GED, "Export PDF"),
                false,
            ),
            GraphicsExportKind::SchematicImage => (
                GraphicsOutputJob::schematic_pdf(),
                tr!(GED, "Export Image"),
                true,
            ),
            GraphicsExportKind::BoardPdf(_) => (
                GraphicsOutputJob::board_assembly_pdf(),
                tr!(GED, "Export PDF"),
                false,
            ),
            GraphicsExportKind::BoardImage(_) => (
                GraphicsOutputJob::board_assembly_pdf(),
                tr!(GED, "Export Image"),
                true,
            ),
        };
        let name = job.name().clone();
        let OutputJobKind::Graphics(mut g) = job.kind_mut().clone() else {
            unreachable!("graphics preset");
        };
        if let GraphicsExportKind::BoardPdf(b) | GraphicsExportKind::BoardImage(b) = kind {
            for c in &mut g.content {
                c.boards = ObjectSet::custom([Some(b)]);
            }
        }
        if image {
            g.output_path = g.output_path.replace(".pdf", ".png");
            // One image per page.
            g.content.truncate(1);
        }
        let mut form = Form::new(unit);
        let mut editor = GraphicsJobEditor::new(g);
        editor.build(&mut form, &objects, true);
        if image {
            form.set_hint(
                "g_output",
                tr!(GED, "The page number will be appended to the filename."),
            );
        } else {
            form.set_hint("g_dpi", "");
        }
        Self {
            form,
            title,
            name,
            editor,
            objects,
        }
    }

    /// The job to run (tests).
    pub fn job(&mut self) -> Result<OutputJob, String> {
        self.editor.sync(&self.form, &self.objects)?;
        Ok(OutputJob::new(
            Uuid::new_random(),
            self.name.clone(),
            OutputJobKind::Graphics(self.editor.job.clone()),
        ))
    }
}

impl FormDialog for GraphicsExportDialog {
    fn title(&self) -> String {
        self.title.clone()
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        DialogOptions {
            apply: false,
            ok_text: Some(tr!(
                "librepcb::editor::OutputJobListWidgetItem",
                "Run this job"
            )),
            width: 600.0,
            label_width: 150.0,
            ..DialogOptions::default()
        }
    }

    fn field_event(&mut self, _ctx: &DialogContext<'_>, id: &str, event: FieldEvent) {
        self.editor.event(&mut self.form, &self.objects, id, &event);
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let job = self.job()?;
        Ok(Applied::RunJobs {
            title: self.title.clone(),
            jobs: vec![job],
        })
    }
}

// --- Output jobs -----------------------------------------------------------------

/// The job types offered by "Add" (upstream `addClicked()` menu).
type JobPreset = (String, fn(&ProjectObjects) -> OutputJob);

fn job_presets() -> Vec<JobPreset> {
    vec![
        (tr!(OJD, "Schematic PDF/Image"), |_| {
            GraphicsOutputJob::schematic_pdf()
        }),
        (tr!(OJD, "Board Assembly PDF/Image"), |_| {
            GraphicsOutputJob::board_assembly_pdf()
        }),
        (tr!(OJD, "Board Rendering PDF/Image"), |_| {
            GraphicsOutputJob::board_rendering_pdf()
        }),
        (
            format!(
                "{}: {}",
                OutputJob::new_default::<GerberExcellonOutputJob>().type_tr(),
                tr!(OJD, "Generic Default Settings (*.gbr)")
            ),
            |_| GerberExcellonOutputJob::default_style(),
        ),
        (
            format!(
                "{}: {}",
                OutputJob::new_default::<GerberExcellonOutputJob>().type_tr(),
                tr!(OJD, "Generic Default Settings (Protel Style)")
            ),
            |_| GerberExcellonOutputJob::protel_style(),
        ),
        (
            OutputJob::new_default::<PickPlaceOutputJob>().type_tr(),
            |_| OutputJob::new_default::<PickPlaceOutputJob>(),
        ),
        (
            OutputJob::new_default::<GerberX3OutputJob>().type_tr(),
            |_| OutputJob::new_default::<GerberX3OutputJob>(),
        ),
        (
            OutputJob::new_default::<NetlistOutputJob>().type_tr(),
            |_| OutputJob::new_default::<NetlistOutputJob>(),
        ),
        (OutputJob::new_default::<BomOutputJob>().type_tr(), |o| {
            let mut job = OutputJob::new_default::<BomOutputJob>();
            if let OutputJobKind::Bom(b) = job.kind_mut() {
                // Upstream: migrate the legacy custom BOM attributes.
                b.custom_attributes = o.custom_bom_attributes.clone();
            }
            job
        }),
        (
            OutputJob::new_default::<InteractiveHtmlBomOutputJob>().type_tr(),
            |_| OutputJob::new_default::<InteractiveHtmlBomOutputJob>(),
        ),
        (
            OutputJob::new_default::<Board3DOutputJob>().type_tr(),
            |_| OutputJob::new_default::<Board3DOutputJob>(),
        ),
        (OutputJob::new_default::<CopyOutputJob>().type_tr(), |_| {
            OutputJob::new_default::<CopyOutputJob>()
        }),
        (
            OutputJob::new_default::<ArchiveOutputJob>().type_tr(),
            |_| OutputJob::new_default::<ArchiveOutputJob>(),
        ),
        (
            OutputJob::new_default::<ProjectJsonOutputJob>().type_tr(),
            |_| OutputJob::new_default::<ProjectJsonOutputJob>(),
        ),
        (OutputJob::new_default::<LppzOutputJob>().type_tr(), |_| {
            OutputJob::new_default::<LppzOutputJob>()
        }),
    ]
}

fn csv(v: &[String]) -> String {
    v.join(", ")
}

fn from_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

/// The output jobs dialog (upstream `OutputJobsDialog`).
pub struct OutputJobsDialog {
    form: Form,
    jobs: Vec<OutputJob>,
    current: Option<usize>,
    graphics: Option<GraphicsJobEditor>,
    objects: ProjectObjects,
}

impl OutputJobsDialog {
    /// Opens the dialog with the jobs of the project.
    pub fn new(project: &AppProject, unit: LengthUnit) -> Self {
        let objects = ProjectObjects::new(project);
        let jobs: Vec<OutputJob> = project
            .shared()
            .lock()
            .project()
            .output_jobs()
            .iter()
            .cloned()
            .collect();
        let mut form = Form::new(unit);
        form.side_column(true);
        form.list(
            "jobs",
            "",
            &[],
            &[],
            14,
            ListButtons {
                remove: true,
                move_: true,
                duplicate: true,
                ..ListButtons::default()
            },
        );
        let presets: Vec<String> = job_presets().into_iter().map(|(n, _)| n).collect();
        form.choice("add_type", "", &presets, None);
        form.button("add", "", tr!(OJD, "Add a new job"));
        form.side_column(false);
        form.note("job_anchor", "");
        let mut dialog = Self {
            form,
            jobs,
            current: None,
            graphics: None,
            objects,
        };
        dialog.refresh_jobs();
        let first = (!dialog.jobs.is_empty()).then_some(0);
        dialog.select(first);
        dialog
    }

    /// The edited jobs (tests).
    pub fn jobs(&mut self) -> Result<&[OutputJob], String> {
        self.sync()?;
        Ok(&self.jobs)
    }

    fn refresh_jobs(&self) {
        let items: Vec<ListItem> = self
            .jobs
            .iter()
            .map(|j| ListItem::row(vec![j.name().to_string(), j.type_tr()]))
            .collect();
        self.form.set_items("jobs", &items, self.current);
    }

    /// Shows the fields of a job.
    fn select(&mut self, index: Option<usize>) {
        self.current = index.filter(|i| *i < self.jobs.len());
        self.form.truncate_after("job_anchor");
        self.graphics = None;
        self.form.set_index("jobs", self.current);
        let Some(i) = self.current else {
            self.form.set_text(
                "job_anchor",
                tr!(
                    OJD,
                    "Click on the {0} button below to add output jobs. Or just close this dialog to not generate any output files.",
                    "+"
                ),
            );
            return;
        };
        self.form.set_text("job_anchor", "");
        let job = self.jobs[i].clone();
        let f = &mut self.form;
        let o = &self.objects;
        f.header(job.type_tr());
        f.text("name", tr!(GOJ, "Name:"), job.name().as_str());
        let bctx = "librepcb::editor::BomOutputJobWidget";
        let boards = |f: &mut Form, set: &ObjectSet<BoardId>| {
            object_set_fields(f, "boards", &tr!(bctx, "Boards:"), bctx, set, &o.boards);
        };
        let variants = |f: &mut Form, set: &ObjectSet<AssemblyVariantId>| {
            object_set_fields(
                f,
                "variants",
                &tr!(bctx, "Assembly Variants:"),
                bctx,
                set,
                &o.variants,
            );
        };
        match job.kind() {
            OutputJobKind::Graphics(g) => {
                f.note(
                    "description",
                    tr!(
                        GOJ,
                        "Customizable PDF/image export for schematics and boards."
                    ),
                );
                let mut editor = GraphicsJobEditor::new(g.clone());
                editor.build(f, o, true);
                self.graphics = Some(editor);
            }
            OutputJobKind::GerberExcellon(g) => {
                let c = "librepcb::editor::GerberExcellonOutputJobWidget";
                f.note(
                    "description",
                    tr!(
                        c,
                        "Gerber (RS-274X) / Excellon (XNC) PCB production data export for boards."
                    ),
                );
                f.text("output", tr!(c, "Base Path:"), &g.output_path);
                boards(f, &g.boards);
                for (id, label, value) in [
                    ("suffix_outlines", "Outlines:", &g.suffix_outlines),
                    ("suffix_copper_top", "Top Copper:", &g.suffix_copper_top),
                    (
                        "suffix_copper_inner",
                        "Inner Copper:",
                        &g.suffix_copper_inner,
                    ),
                    ("suffix_copper_bot", "Bottom Copper:", &g.suffix_copper_bot),
                    (
                        "suffix_solder_mask_top",
                        "Top Stopmask:",
                        &g.suffix_solder_mask_top,
                    ),
                    (
                        "suffix_solder_mask_bot",
                        "Bottom Stopmask:",
                        &g.suffix_solder_mask_bot,
                    ),
                    (
                        "suffix_silkscreen_top",
                        "Top Silkscreen:",
                        &g.suffix_silkscreen_top,
                    ),
                    (
                        "suffix_silkscreen_bot",
                        "Bottom Silkscreen:",
                        &g.suffix_silkscreen_bot,
                    ),
                    ("suffix_drills_npth", "Drills NPTH:", &g.suffix_drills_npth),
                    ("suffix_drills_pth", "Drills PTH:", &g.suffix_drills_pth),
                    (
                        "suffix_drills_blind_buried",
                        "Drills Blind/Buried:",
                        &g.suffix_drills_blind_buried,
                    ),
                ] {
                    f.text(id, tr!(c, label), value);
                }
                f.checkbox(
                    "merge_drills",
                    "",
                    tr!(c, "Merge PTH and NPTH drills into one file:"),
                    g.merge_drill_files,
                );
                f.text("suffix_drills", "", &g.suffix_drills);
                f.checkbox(
                    "g85",
                    "",
                    tr!(c, "Use drilled slot command in Excellon files (G85)"),
                    g.use_g85_slot_command,
                );
                f.checkbox(
                    "paste_top",
                    "",
                    tr!(c, "Top Solder Paste\n(Top Stencil):").replace('\n', " "),
                    g.enable_solder_paste_top,
                );
                f.text("suffix_solder_paste_top", "", &g.suffix_solder_paste_top);
                f.checkbox(
                    "paste_bot",
                    "",
                    tr!(c, "Bottom Solder Paste\n(Bottom Stencil):").replace('\n', " "),
                    g.enable_solder_paste_bot,
                );
                f.text("suffix_solder_paste_bot", "", &g.suffix_solder_paste_bot);
            }
            OutputJobKind::PickPlace(j) => {
                let c = "librepcb::editor::PickPlaceOutputJobWidget";
                f.note(
                    "description",
                    tr!(c, "CSV pick&place position file export for boards."),
                );
                boards(f, &j.boards);
                variants(f, &j.assembly_variants);
                f.checkbox(
                    "tech_tht",
                    tr!(c, "Technologies:"),
                    tr!(c, "THT"),
                    j.technologies.tht,
                );
                f.checkbox("tech_smt", "", tr!(c, "SMT"), j.technologies.smt);
                f.checkbox("tech_mixed", "", tr!(c, "Mixed"), j.technologies.mixed);
                f.checkbox(
                    "tech_fiducial",
                    "",
                    tr!(c, "Fiducial"),
                    j.technologies.fiducial,
                );
                f.checkbox("tech_other", "", tr!(c, "Other"), j.technologies.other);
                f.checkbox(
                    "include_comment",
                    tr!(c, "Options:"),
                    tr!(c, "Include metadata as comments"),
                    j.include_comment,
                );
                f.checkbox("create_top", "", tr!(c, "Output Top:"), j.create_top);
                f.text("output_top", "", &j.output_path_top);
                f.checkbox(
                    "create_bottom",
                    "",
                    tr!(c, "Output Bottom:"),
                    j.create_bottom,
                );
                f.text("output_bottom", "", &j.output_path_bottom);
                f.checkbox("create_both", "", tr!(c, "Output Combined:"), j.create_both);
                f.text("output_both", "", &j.output_path_both);
            }
            OutputJobKind::GerberX3(j) => {
                let c = "librepcb::editor::GerberX3OutputJobWidget";
                f.note(
                    "description",
                    tr!(
                        c,
                        "Gerber X3 pick&place position file & glue mask (RS-274X) export for boards."
                    ),
                );
                boards(f, &j.boards);
                variants(f, &j.assembly_variants);
                f.checkbox(
                    "cmp_top",
                    "",
                    tr!(c, "Top Components:"),
                    j.enable_components_top,
                );
                f.text("output_cmp_top", "", &j.output_path_components_top);
                f.checkbox(
                    "cmp_bot",
                    "",
                    tr!(c, "Bottom Components:"),
                    j.enable_components_bot,
                );
                f.text("output_cmp_bot", "", &j.output_path_components_bot);
                f.checkbox("glue_top", "", tr!(c, "Top Glue Mask:"), j.enable_glue_top);
                f.text("output_glue_top", "", &j.output_path_glue_top);
                f.checkbox(
                    "glue_bot",
                    "",
                    tr!(c, "Bottom Glue Mask:"),
                    j.enable_glue_bot,
                );
                f.text("output_glue_bot", "", &j.output_path_glue_bot);
            }
            OutputJobKind::Netlist(j) => {
                let c = "librepcb::editor::NetlistOutputJobWidget";
                f.note(
                    "description",
                    tr!(c, "IPC D-356A netlist export for boards."),
                );
                f.text("output", tr!(c, "Output:"), &j.output_path);
                boards(f, &j.boards);
            }
            OutputJobKind::Bom(j) => {
                f.note(
                    "description",
                    tr!(bctx, "Bill of materials (BOM) export to CSV files."),
                );
                f.text("output", tr!(bctx, "Output:"), &j.output_path);
                f.text(
                    "attributes",
                    tr!(bctx, "Custom Attributes:"),
                    csv(&j.custom_attributes),
                );
                f.update("attributes", |x| {
                    x.placeholder = tr!(bctx, "Comma-separated attributes (optional)").into();
                });
                let b = opt_items(&o.boards, tr!(GOJ, "None"));
                object_set_fields(f, "opt_boards", &tr!(bctx, "Boards:"), bctx, &j.boards, &b);
                variants(f, &j.assembly_variants);
            }
            OutputJobKind::InteractiveHtmlBom(j) => {
                let c = "librepcb::editor::InteractiveHtmlBomOutputJobWidget";
                f.note(
                    "description",
                    tr!(c, "Interactive HTML bill of materials (BOM) export."),
                );
                f.text("output", tr!(c, "Output:"), &j.output_path);
                f.text(
                    "attributes",
                    tr!(c, "Custom Attributes:"),
                    csv(&j.custom_attributes),
                );
                boards(f, &j.boards);
                variants(f, &j.assembly_variants);
                f.checkbox("dark_mode", "", "Dark Mode", j.dark_mode);
                f.checkbox("show_silkscreen", "", "Silkscreen", j.show_silkscreen);
                f.checkbox("show_fabrication", "", "Fabrication", j.show_fabrication);
                f.checkbox("show_pads", "", "Pads", j.show_pads);
                f.checkbox("show_tracks", "", "Tracks", j.show_tracks);
                f.checkbox("show_zones", "", "Zones", j.show_zones);
            }
            OutputJobKind::Board3D(j) => {
                let c = "librepcb::editor::Board3DOutputJobWidget";
                f.note("description", tr!(c, "3D Model export for boards."));
                f.text("output", tr!(c, "Output:"), &j.output_path);
                boards(f, &j.boards);
                let v = opt_items(&o.variants, tr!(GOJ, "None"));
                object_set_fields(
                    f,
                    "opt_variants",
                    &tr!(c, "Assembly Variants:"),
                    c,
                    &j.assembly_variants,
                    &v,
                );
            }
            OutputJobKind::ProjectJson(j) => {
                let c = "librepcb::editor::ProjectJsonOutputJobWidget";
                f.note(
                    "description",
                    tr!(
                        c,
                        "Export general project data to a machine-readable JSON file."
                    ),
                );
                f.text("output", tr!(c, "Output:"), &j.output_path);
            }
            OutputJobKind::Lppz(j) => {
                let c = "librepcb::editor::LppzOutputJobWidget";
                f.note(
                    "description",
                    tr!(
                        c,
                        "Store a snapshot of the whole project as a *.lppz archive."
                    ),
                );
                f.text("output", tr!(c, "Output:"), &j.output_path);
            }
            OutputJobKind::Copy(j) => {
                let c = "librepcb::editor::CopyOutputJobWidget";
                f.note(
                    "description",
                    tr!(
                        c,
                        "Copy an arbitrary file into the output folder, optionally with variable substitution."
                    ),
                );
                f.text("input", tr!(c, "Input File:"), &j.input_path);
                f.text("output", tr!(c, "Output File:"), &j.output_path);
                f.checkbox(
                    "substitute",
                    tr!(c, "Options:"),
                    tr!(c, "Substitute Variables"),
                    j.substitute_variables,
                );
                let b = opt_items(&o.boards, tr!(GOJ, "None"));
                object_set_fields(f, "opt_boards", &tr!(c, "Boards:"), c, &j.boards, &b);
                let v = opt_items(&o.variants, tr!(GOJ, "None"));
                object_set_fields(
                    f,
                    "opt_variants",
                    &tr!(c, "Assembly Variants:"),
                    c,
                    &j.assembly_variants,
                    &v,
                );
            }
            OutputJobKind::Archive(j) => {
                let c = "librepcb::editor::ArchiveOutputJobWidget";
                f.note(
                    "description",
                    tr!(
                        c,
                        "Bundle the output of other jobs in a single archive file."
                    ),
                );
                f.text("output", tr!(c, "Output:"), &j.output_path);
                let items: Vec<ListItem> = self
                    .jobs
                    .iter()
                    .filter(|x| x.uuid() != job.uuid())
                    .map(|x| {
                        ListItem::check(x.name().as_str(), j.input_jobs.contains_key(&x.uuid()))
                    })
                    .collect();
                f.list(
                    "inputs",
                    tr!(c, "Content:"),
                    &[],
                    &items,
                    6,
                    ListButtons::default(),
                );
            }
            _ => {
                f.note(
                    "description",
                    tr!(
                        "librepcb::editor::OutputJobHomeWidget",
                        "This output job type is not supported by this version of LibrePCB."
                    ),
                );
            }
        }
    }

    /// Reads the fields of the current job.
    fn sync(&mut self) -> Result<(), String> {
        let Some(i) = self.current else {
            return Ok(());
        };
        let f = &self.form;
        let o = &self.objects;
        let other_uuids: Vec<Uuid> = self
            .jobs
            .iter()
            .filter(|x| x.uuid() != self.jobs[i].uuid())
            .map(OutputJob::uuid)
            .collect();
        let job = &mut self.jobs[i];
        let name =
            ElementName::new(ElementName::clean(&f.get_text("name"))).map_err(|e| e.to_string())?;
        job.set_name(name);
        let text = |id: &str| f.get_text(id).trim().to_owned();
        match job.kind_mut() {
            OutputJobKind::Graphics(g) => {
                if let Some(editor) = &mut self.graphics {
                    editor.sync(f, o)?;
                    *g = editor.job.clone();
                }
            }
            OutputJobKind::GerberExcellon(g) => {
                g.output_path = text("output");
                g.boards = chosen_object_set(f, "boards", &o.boards);
                g.suffix_outlines = text("suffix_outlines");
                g.suffix_copper_top = text("suffix_copper_top");
                g.suffix_copper_inner = text("suffix_copper_inner");
                g.suffix_copper_bot = text("suffix_copper_bot");
                g.suffix_solder_mask_top = text("suffix_solder_mask_top");
                g.suffix_solder_mask_bot = text("suffix_solder_mask_bot");
                g.suffix_silkscreen_top = text("suffix_silkscreen_top");
                g.suffix_silkscreen_bot = text("suffix_silkscreen_bot");
                g.suffix_drills_npth = text("suffix_drills_npth");
                g.suffix_drills_pth = text("suffix_drills_pth");
                g.suffix_drills_blind_buried = text("suffix_drills_blind_buried");
                g.merge_drill_files = f.get_checked("merge_drills");
                g.suffix_drills = text("suffix_drills");
                g.use_g85_slot_command = f.get_checked("g85");
                g.enable_solder_paste_top = f.get_checked("paste_top");
                g.suffix_solder_paste_top = text("suffix_solder_paste_top");
                g.enable_solder_paste_bot = f.get_checked("paste_bot");
                g.suffix_solder_paste_bot = text("suffix_solder_paste_bot");
            }
            OutputJobKind::PickPlace(j) => {
                j.boards = chosen_object_set(f, "boards", &o.boards);
                j.assembly_variants = chosen_object_set(f, "variants", &o.variants);
                j.technologies.tht = f.get_checked("tech_tht");
                j.technologies.smt = f.get_checked("tech_smt");
                j.technologies.mixed = f.get_checked("tech_mixed");
                j.technologies.fiducial = f.get_checked("tech_fiducial");
                j.technologies.other = f.get_checked("tech_other");
                j.include_comment = f.get_checked("include_comment");
                j.create_top = f.get_checked("create_top");
                j.output_path_top = text("output_top");
                j.create_bottom = f.get_checked("create_bottom");
                j.output_path_bottom = text("output_bottom");
                j.create_both = f.get_checked("create_both");
                j.output_path_both = text("output_both");
            }
            OutputJobKind::GerberX3(j) => {
                j.boards = chosen_object_set(f, "boards", &o.boards);
                j.assembly_variants = chosen_object_set(f, "variants", &o.variants);
                j.enable_components_top = f.get_checked("cmp_top");
                j.output_path_components_top = text("output_cmp_top");
                j.enable_components_bot = f.get_checked("cmp_bot");
                j.output_path_components_bot = text("output_cmp_bot");
                j.enable_glue_top = f.get_checked("glue_top");
                j.output_path_glue_top = text("output_glue_top");
                j.enable_glue_bot = f.get_checked("glue_bot");
                j.output_path_glue_bot = text("output_glue_bot");
            }
            OutputJobKind::Netlist(j) => {
                j.output_path = text("output");
                j.boards = chosen_object_set(f, "boards", &o.boards);
            }
            OutputJobKind::Bom(j) => {
                j.output_path = text("output");
                j.custom_attributes = from_csv(&f.get_text("attributes"));
                j.boards = chosen_object_set(f, "opt_boards", &opt_items(&o.boards, String::new()));
                j.assembly_variants = chosen_object_set(f, "variants", &o.variants);
            }
            OutputJobKind::InteractiveHtmlBom(j) => {
                j.output_path = text("output");
                j.custom_attributes = from_csv(&f.get_text("attributes"));
                j.boards = chosen_object_set(f, "boards", &o.boards);
                j.assembly_variants = chosen_object_set(f, "variants", &o.variants);
                j.dark_mode = f.get_checked("dark_mode");
                j.show_silkscreen = f.get_checked("show_silkscreen");
                j.show_fabrication = f.get_checked("show_fabrication");
                j.show_pads = f.get_checked("show_pads");
                j.show_tracks = f.get_checked("show_tracks");
                j.show_zones = f.get_checked("show_zones");
            }
            OutputJobKind::Board3D(j) => {
                j.output_path = text("output");
                j.boards = chosen_object_set(f, "boards", &o.boards);
                j.assembly_variants =
                    chosen_object_set(f, "opt_variants", &opt_items(&o.variants, String::new()));
            }
            OutputJobKind::ProjectJson(j) => j.output_path = text("output"),
            OutputJobKind::Lppz(j) => j.output_path = text("output"),
            OutputJobKind::Copy(j) => {
                j.input_path = text("input");
                j.output_path = text("output");
                j.substitute_variables = f.get_checked("substitute");
                j.boards = chosen_object_set(f, "opt_boards", &opt_items(&o.boards, String::new()));
                j.assembly_variants =
                    chosen_object_set(f, "opt_variants", &opt_items(&o.variants, String::new()));
            }
            OutputJobKind::Archive(j) => {
                j.output_path = text("output");
                let checked = f.get_list_checked("inputs");
                let old = std::mem::take(&mut j.input_jobs);
                for (uuid, on) in other_uuids.iter().zip(checked) {
                    if on {
                        j.input_jobs
                            .insert(*uuid, old.get(uuid).cloned().unwrap_or_default());
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The jobs to run for "Run" on the selected job: the job and the jobs
    /// it depends on (upstream `runJob()`).
    fn selected_jobs(&self) -> Vec<OutputJob> {
        let Some(job) = self.current.and_then(|i| self.jobs.get(i)) else {
            return Vec::new();
        };
        let deps: BTreeSet<Uuid> = job.dependencies();
        self.jobs
            .iter()
            .filter(|j| deps.contains(&j.uuid()))
            .cloned()
            .chain(std::iter::once(job.clone()))
            .collect()
    }
}

impl FormDialog for OutputJobsDialog {
    fn title(&self) -> String {
        tr!(OJD, "Output Jobs")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        DialogOptions {
            extra_buttons: vec![
                tr!("librepcb::editor::OutputJobListWidgetItem", "Run this job"),
                tr!("librepcb::editor::OutputJobListWidgetItem", "Run all jobs"),
            ],
            width: 620.0,
            label_width: 150.0,
            side_width: 240.0,
            ..DialogOptions::default()
        }
    }

    fn field_event(&mut self, _ctx: &DialogContext<'_>, id: &str, event: FieldEvent) {
        if let Some(editor) = &mut self.graphics
            && editor.event(&mut self.form, &self.objects, id, &event)
        {
            return;
        }
        match (id, event) {
            ("jobs", FieldEvent::List(action)) => {
                if let Err(e) = self.sync() {
                    log::warn!("Invalid output job settings: {e}");
                }
                let sel = match action {
                    ListAction::Select(i) => Some(i),
                    ListAction::Remove(i) if i < self.jobs.len() => {
                        let removed = self.jobs.remove(i);
                        for j in &mut self.jobs {
                            j.remove_dependency(&removed.uuid());
                        }
                        (!self.jobs.is_empty()).then(|| i.min(self.jobs.len() - 1))
                    }
                    ListAction::MoveUp(i) if i > 0 && i < self.jobs.len() => {
                        self.jobs.swap(i, i - 1);
                        Some(i - 1)
                    }
                    ListAction::MoveDown(i) if i + 1 < self.jobs.len() => {
                        self.jobs.swap(i, i + 1);
                        Some(i + 1)
                    }
                    ListAction::Duplicate(i) => self.jobs.get(i).cloned().map(|mut job| {
                        job.set_uuid(Uuid::new_random());
                        let name = format!("{} {}", job.name(), tr!(OJD, "(copy)"));
                        if let Ok(name) = ElementName::new(ElementName::clean(&name)) {
                            job.set_name(name);
                        }
                        self.jobs.insert(i + 1, job);
                        i + 1
                    }),
                    _ => self.current,
                };
                self.current = sel;
                self.refresh_jobs();
                self.select(sel);
            }
            ("add", FieldEvent::Clicked) => {
                let Some(preset) = self
                    .form
                    .get_index("add_type")
                    .and_then(|i| job_presets().into_iter().nth(i))
                else {
                    return;
                };
                if let Err(e) = self.sync() {
                    log::warn!("Invalid output job settings: {e}");
                }
                let index = self.current.map_or(self.jobs.len(), |i| i + 1);
                self.jobs.insert(index, (preset.1)(&self.objects));
                self.current = Some(index);
                self.refresh_jobs();
                self.select(Some(index));
            }
            ("name", FieldEvent::Edited) => {
                let _ = self.sync();
                self.refresh_jobs();
            }
            ("inputs", FieldEvent::List(ListAction::Toggle(row))) => {
                let mut checked = self.form.get_list_checked("inputs");
                if let Some(c) = checked.get_mut(row) {
                    *c = !*c;
                }
                self.form.update("inputs", |f| {
                    for (i, c) in checked.iter().enumerate() {
                        if let Some(mut item) = slint::Model::row_data(&f.items, i) {
                            item.checked = *c;
                            slint::Model::set_row_data(&f.items, i, item);
                        }
                    }
                });
            }
            (id, event) => {
                object_set_event(&self.form, id, &event);
            }
        }
    }

    /// Stores the jobs in the project (one undo step).
    fn apply(&mut self, ctx: &DialogContext<'_>) -> Result<Applied, String> {
        self.sync()?;
        let mut list = OutputJobList::new();
        for job in &self.jobs {
            list.push(job.clone());
        }
        let unchanged = ctx.project()?.shared().lock().project().output_jobs() == &list;
        if unchanged {
            return Ok(Applied::Nothing);
        }
        transaction(ctx.project()?, tr!(OJD, "Output Jobs"), |e| {
            e.apply_mutations(tr!(OJD, "Output Jobs"), vec![Mutation::SetOutputJobs(list)])
        })?;
        Ok(Applied::Project)
    }

    /// "Run" (the selected job and its dependencies) and "Run All".
    fn button(&mut self, _ctx: &DialogContext<'_>, index: usize) -> Result<ButtonResult, String> {
        self.sync()?;
        let jobs = if index == 0 {
            self.selected_jobs()
        } else {
            self.jobs.clone()
        };
        if jobs.is_empty() {
            return Ok(ButtonResult::Keep);
        }
        Ok(ButtonResult::RunJobs {
            title: tr!(OJD, "Output Jobs"),
            jobs,
        })
    }
}
