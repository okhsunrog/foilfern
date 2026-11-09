//! The graphics export, output jobs, BOM review and pick&place generator
//! dialogs (M3b) without a Slint platform: fields edited like the UI does,
//! the resulting jobs run with the application's output job runner, and
//! project changes stored as one undo step.

use std::rc::Rc;

use crate::common::*;
use librepcb_app::dialogs::output::{GraphicsExportDialog, GraphicsExportKind, OutputJobsDialog};
use librepcb_app::dialogs::{
    Applied, ButtonResult, DialogContext, FieldEvent, FormDialog, ListAction,
};
use librepcb_app::outputs::run_output_jobs;
use librepcb_app::project::AppProject;
use librepcb_app::ui;
use librepcb_core::export::PageOrientation;
use librepcb_core::job::{ObjectSet, OutputJob, OutputJobKind};
use librepcb_core::types::LengthUnit;
use slint::Model;

fn edit(
    dialog: &mut dyn FormDialog,
    project: &Rc<AppProject>,
    id: &str,
    f: impl FnOnce(&mut ui::FormField),
) {
    let (id, event) = dialog.form_mut().edit(id, f).expect("field");
    dialog.field_event(&DialogContext::new(project), &id, event);
}

fn list(dialog: &mut dyn FormDialog, project: &Rc<AppProject>, id: &str, action: ListAction) {
    dialog.field_event(&DialogContext::new(project), id, FieldEvent::List(action));
}

fn click(dialog: &mut dyn FormDialog, project: &Rc<AppProject>, id: &str) {
    edit(dialog, project, id, |f| {
        f.action = ui::FormFieldAction::Clicked
    });
}

fn run(project: &AppProject, jobs: &[OutputJob]) -> Vec<String> {
    let mut open = project.shared().lock();
    let (files, _warnings) = run_output_jobs(&mut open, jobs, false, &|_, _| {}).unwrap();
    files.iter().map(|f| f.file_name().to_owned()).collect()
}

#[test]
fn graphics_export() {
    let dir = tempfile::tempdir().unwrap();
    let (project, _, _) = create_project(dir.path());
    let mut dialog = GraphicsExportDialog::new(
        &project,
        GraphicsExportKind::SchematicPdf,
        LengthUnit::Millimeters,
    );
    assert_eq!(dialog.title(), "Export PDF");
    let d: &mut dyn FormDialog = &mut dialog;
    let a4 = d
        .form()
        .field("g_page_size")
        .unwrap()
        .options
        .iter()
        .position(|o| o == "A4")
        .unwrap();
    edit(d, &project, "g_page_size", |f| f.index = a4 as i32);
    edit(d, &project, "g_orientation", |f| f.index = 1);
    edit(d, &project, "g_background", |f| f.index = 1);
    // Disable the first color (layer).
    let layers = d.form().get_list_checked("g_layers");
    let first_on = layers.iter().position(|c| *c).unwrap();
    list(d, &project, "g_layers", ListAction::Toggle(first_on));
    assert!(!d.form().get_list_checked("g_layers")[first_on]);
    edit(d, &project, "g_output", |f| {
        f.text = "sch/{{PROJECT}}.pdf".into()
    });
    let Ok(Applied::RunJobs { jobs, .. }) = d.apply(&DialogContext::new(&project)) else {
        panic!("jobs expected");
    };
    let OutputJobKind::Graphics(g) = jobs[0].kind() else {
        panic!("graphics job expected");
    };
    let c = &g.content[0];
    assert_eq!(c.page_size.as_deref(), Some("A4"));
    assert_eq!(c.orientation, PageOrientation::Landscape);
    assert_eq!(c.background_color, librepcb_core::types::Color::WHITE);
    assert_eq!(c.layers.len(), layers.iter().filter(|c| **c).count() - 1);
    let files = run(&project, &jobs);
    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with(".pdf"), "{files:?}");

    // Board image: one page, PNG.
    let board = project.shared().lock().project().boards()[0].id();
    let mut dialog = GraphicsExportDialog::new(
        &project,
        GraphicsExportKind::BoardImage(board),
        LengthUnit::Millimeters,
    );
    assert_eq!(dialog.form().field("g_pages").unwrap().items.row_count(), 1);
    edit(&mut dialog, &project, "g_dpi", |f| f.text = "100".into());
    let job = dialog.job().unwrap();
    let OutputJobKind::Graphics(g) = job.kind() else {
        panic!("graphics job expected");
    };
    assert!(g.output_path.ends_with(".png"));
    assert_eq!(g.content[0].boards, ObjectSet::custom([Some(board)]));
    let files = run(&project, &[job]);
    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with(".png"), "{files:?}");
}

#[test]
fn output_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let (project, _, _) = create_project(dir.path());
    let initial = project.shared().lock().project().output_jobs().len();
    let mut dialog = OutputJobsDialog::new(&project, LengthUnit::Millimeters);
    let d: &mut dyn FormDialog = &mut dialog;
    assert_eq!(d.form().field("jobs").unwrap().items.row_count(), initial);

    // Add a netlist job and a BOM job, rename and configure them.
    let types = d.form().field("add_type").unwrap().options;
    let netlist = types.iter().position(|t| t.contains("Netlist")).unwrap();
    edit(d, &project, "add_type", |f| f.index = netlist as i32);
    click(d, &project, "add");
    edit(d, &project, "name", |f| f.text = "My Netlist".into());
    edit(d, &project, "output", |f| {
        f.text = "net/{{PROJECT}}.d356".into()
    });
    edit(d, &project, "boards", |f| f.index = 0);
    let bom = types
        .iter()
        .position(|t| t.to_lowercase().contains("bill of"))
        .unwrap();
    edit(d, &project, "add_type", |f| f.index = bom as i32);
    click(d, &project, "add");
    edit(d, &project, "attributes", |f| {
        f.text = "MPN, MANUFACTURER".into()
    });
    // Back to the netlist job: its settings were kept.
    let rows = d.form().field("jobs").unwrap().items.row_count();
    let netlist_row = (0..rows)
        .find(|i| {
            d.form()
                .field("jobs")
                .unwrap()
                .items
                .row_data(*i)
                .unwrap()
                .cells
                .row_data(0)
                == Some("My Netlist".into())
        })
        .unwrap();
    list(d, &project, "jobs", ListAction::Select(netlist_row));
    assert_eq!(d.form().get_text("output"), "net/{{PROJECT}}.d356");
    assert_eq!(d.form().get_index("boards"), Some(0));

    // Run the selected job.
    let Ok(ButtonResult::RunJobs { jobs, .. }) = d.button(&DialogContext::new(&project), 0) else {
        panic!("jobs expected");
    };
    assert_eq!(jobs.len(), 1);
    let files = run(&project, &jobs);
    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with(".d356"), "{files:?}");

    // Duplicate and remove.
    list(d, &project, "jobs", ListAction::Duplicate(netlist_row));
    assert_eq!(
        d.form().field("jobs").unwrap().items.row_count(),
        initial + 3
    );
    list(d, &project, "jobs", ListAction::Remove(netlist_row + 1));

    // Apply: one undo step.
    let index = undo_index(&project);
    assert_eq!(d.apply(&DialogContext::new(&project)), Ok(Applied::Project));
    assert_eq!(undo_index(&project), index + 1);
    {
        let p = project.shared().lock();
        let jobs = p.project().output_jobs();
        assert_eq!(jobs.len(), initial + 2);
        let bom = jobs
            .iter()
            .find_map(|j| match j.kind() {
                OutputJobKind::Bom(b) => Some(b.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(bom.custom_attributes, vec!["MPN", "MANUFACTURER"]);
        assert!(jobs.iter().any(|j| j.name().as_str() == "My Netlist"));
    }
    // Unchanged: nothing to apply.
    assert_eq!(d.apply(&DialogContext::new(&project)), Ok(Applied::Nothing));
    project.shared().lock().editor.undo().unwrap();
    assert_eq!(
        project.shared().lock().project().output_jobs().len(),
        initial
    );
}

#[test]
fn bom_review() {
    use librepcb_app::dialogs::review::BomReviewDialog;
    let dir = tempfile::tempdir().unwrap();
    let (project, _, board) = create_project(dir.path());
    let mut dialog = BomReviewDialog::new(&project, Some(board));
    assert_eq!(dialog.form().get_index("board"), Some(1));
    let (header, rows) = dialog.table();
    assert!(header.len() >= 3, "{header:?}");
    // R1 and R2 have the same device: one row.
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(
        rows[0].iter().any(|c| c.contains("R1") && c.contains("R2")),
        "{rows:?}"
    );
    edit(&mut dialog, &project, "attributes", |f| {
        f.text = "MPN, Foo".into()
    });
    let (header2, _) = dialog.table();
    assert_eq!(header2.len(), header.len() + 2, "{header2:?}");
    let index = undo_index(&project);
    assert_eq!(
        dialog.apply(&DialogContext::new(&project)),
        Ok(Applied::Project)
    );
    assert_eq!(undo_index(&project), index + 1);
    assert_eq!(
        project
            .shared()
            .lock()
            .project()
            .settings()
            .custom_bom_attributes,
        vec!["MPN", "Foo"]
    );
    assert_eq!(
        dialog.apply(&DialogContext::new(&project)),
        Ok(Applied::Nothing)
    );
}

#[test]
fn pick_place_generator() {
    use librepcb_app::dialogs::review::PickPlaceGeneratorDialog;
    let dir = tempfile::tempdir().unwrap();
    let (project, _, board) = create_project(dir.path());
    let mut dialog = PickPlaceGeneratorDialog::new(&project, board).unwrap();
    assert_eq!(dialog.form().field("data").unwrap().items.row_count(), 2);
    let jobs = match dialog.apply(&DialogContext::new(&project)) {
        Ok(Applied::RunJobs { jobs, .. }) => jobs,
        other => panic!("jobs expected: {other:?}"),
    };
    let mut files = run(&project, &jobs);
    files.sort();
    assert_eq!(files.len(), 2, "{files:?}");
    assert!(files[0].ends_with("_BOT.csv"), "{files:?}");
    // Gerber X3 changes the file extensions.
    edit(&mut dialog, &project, "format", |f| f.index = 2);
    assert!(dialog.form().get_text("top_path").ends_with("_TOP.gbr"));
    edit(&mut dialog, &project, "bottom", |f| f.checked = false);
    let job = dialog.job().unwrap();
    let files = run(&project, &[job]);
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(files[0].ends_with("_TOP.gbr"), "{files:?}");
}
