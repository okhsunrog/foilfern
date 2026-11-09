//! More dialogs of M3b without a Slint platform: the bus segment rename
//! dialog, the symbol text properties dialog, the board setup and the
//! project setup dialogs (fields edited like the UI does, applied as one
//! undo group and undone again).

use std::rc::Rc;

use crate::common::*;
use librepcb_app::dialogs::setup::{BoardSetupDialog, ProjectSetupDialog};
use librepcb_app::dialogs::{self, Applied, DialogContext, FieldEvent, FormDialog, ListAction};
use librepcb_app::helpers::length_to_ui;
use librepcb_app::project::AppProject;
use librepcb_app::tabs::PropertiesTarget;
use librepcb_app::ui;
use librepcb_core::types::{Angle, Layer, Length, LengthUnit};
use librepcb_editor::commands::{BusAnchor, DrawBus};

fn len(mm: f64) -> Length {
    Length::from_mm(mm).unwrap()
}

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

fn set_length(dialog: &mut dyn FormDialog, project: &Rc<AppProject>, id: &str, value: Length) {
    edit(dialog, project, id, |f| {
        f.length.value = length_to_ui(value)
    });
}

fn apply(dialog: &mut dyn FormDialog, project: &Rc<AppProject>) -> Result<Applied, String> {
    dialog.apply(&DialogContext::new(project))
}

fn undo(project: &AppProject) {
    project.shared().lock().editor.undo().unwrap();
}

#[test]
fn bus_segment_rename() {
    let dir = tempfile::tempdir().unwrap();
    let (project, sch, _) = create_project(dir.path());
    // Two segments of one bus.
    let (segment, bus) = {
        let mut p = project.shared().lock();
        let first = p
            .editor
            .execute(DrawBus {
                schematic: Some(sch),
                start: BusAnchor::Point(mm(10.16, 10.16)),
                end: BusAnchor::Point(mm(30.48, 10.16)),
                points: Vec::new(),
                bus: None,
            })
            .unwrap();
        p.editor
            .execute(DrawBus {
                schematic: Some(sch),
                start: BusAnchor::Point(mm(10.16, 30.48)),
                end: BusAnchor::Point(mm(30.48, 30.48)),
                points: Vec::new(),
                bus: Some(first.bus),
            })
            .unwrap();
        (first.segment, first.bus)
    };
    let bus_name = |project: &AppProject| {
        let p = project.shared().lock();
        let seg = &p.project().schematic(sch).unwrap().bus_segments()[&segment];
        p.project()
            .circuit()
            .bus(seg.bus())
            .unwrap()
            .name()
            .to_string()
    };
    let old = bus_name(&project);
    let target = PropertiesTarget::BusSegment(sch, segment.0);
    let mut dialog = dialogs::open_properties(&project, &target, LengthUnit::Millimeters).unwrap();
    assert_eq!(dialog.title(), "Rename Bus Segment");
    assert_eq!(dialog.form().get_text("name"), old);
    // Default: only this segment (the bus has two segments).
    assert_eq!(dialog.form().get_index("scope"), Some(0));
    edit(dialog.as_mut(), &project, "name", |f| {
        f.text = "D[0..7]".into()
    });
    assert!(
        dialog
            .form()
            .get_text("description")
            .contains("new bus 'D[0..7]'")
    );
    assert_eq!(apply(dialog.as_mut(), &project), Ok(Applied::Project));
    assert_eq!(bus_name(&project), "D[0..7]");
    {
        // The other segment keeps the old bus.
        let p = project.shared().lock();
        assert_eq!(p.project().circuit().bus(bus).unwrap().name().as_str(), old);
    }
    undo(&project);
    assert_eq!(bus_name(&project), old);

    // Rename the whole bus.
    let mut dialog = dialogs::open_properties(&project, &target, LengthUnit::Millimeters).unwrap();
    edit(dialog.as_mut(), &project, "scope", |f| f.index = 1);
    edit(dialog.as_mut(), &project, "name", |f| {
        f.text = "ADDR".into()
    });
    assert!(
        dialog
            .form()
            .get_text("description")
            .contains("renamed to 'ADDR'")
    );
    assert_eq!(apply(dialog.as_mut(), &project), Ok(Applied::Project));
    {
        let p = project.shared().lock();
        let bus = p.project().circuit().bus(bus).unwrap();
        assert_eq!(bus.name().as_str(), "ADDR");
        assert!(!bus.has_auto_name());
    }
    undo(&project);
    assert_eq!(bus_name(&project), old);
}

#[test]
fn symbol_text_properties() {
    let dir = tempfile::tempdir().unwrap();
    let (project, sch, _) = create_project(dir.path());
    let (r1, _) = symbol(&project, sch, "R1");
    let (uuid, text) = {
        let p = project.shared().lock();
        let (uuid, text) = p.project().schematic(sch).unwrap().symbols()[&r1]
            .texts()
            .iter()
            .next()
            .map(|(u, t)| (*u, t.clone()))
            .unwrap();
        (uuid, text)
    };
    let target = PropertiesTarget::SymbolText(r1, uuid);
    let mut dialog = dialogs::open_properties(&project, &target, LengthUnit::Millimeters).unwrap();
    assert_eq!(&dialog.form().get_text("text"), text.text());
    edit(dialog.as_mut(), &project, "rotation", |f| {
        f.action = ui::FormFieldAction::Increase;
    });
    set_length(dialog.as_mut(), &project, "height", len(3.0));
    assert_eq!(apply(dialog.as_mut(), &project), Ok(Applied::Project));
    let changed = |project: &AppProject| {
        let p = project.shared().lock();
        p.project().schematic(sch).unwrap().symbols()[&r1].texts()[&uuid].clone()
    };
    let t = changed(&project);
    assert_eq!(t.rotation(), text.rotation() + Angle::DEG90);
    assert_eq!(*t.height(), len(3.0));
    undo(&project);
    assert_eq!(changed(&project), text);
}

#[test]
fn board_setup() {
    let dir = tempfile::tempdir().unwrap();
    let (project, _, board) = create_project(dir.path());
    let mut dialog = BoardSetupDialog::new(&project, board).unwrap();
    assert_eq!(dialog.form().pages().len(), 3);
    assert_eq!(dialog.form().get_text("name"), "default");
    let d: &mut dyn FormDialog = &mut dialog;
    edit(d, &project, "name", |f| f.text = "Main Board".into());
    edit(d, &project, "inner_layers", |f| f.index = 2);
    set_length(d, &project, "pcb_thickness", len(1.0));
    edit(d, &project, "silk_top_names", |f| f.checked = false);
    edit(d, &project, "solder_resist", |f| f.index = 0);
    set_length(d, &project, "default_trace_width", len(0.3));
    edit(d, &project, "cmp_side_pads", |f| f.index = 1);
    set_length(d, &project, "stop_mask_min", len(0.05));
    set_length(d, &project, "drc_copper_copper", len(0.15));
    edit(d, &project, "drc_blind_vias", |f| f.checked = true);
    edit(d, &project, "drc_pth_slots", |f| f.index = 3);
    let index = undo_index(&project);
    assert_eq!(apply(d, &project), Ok(Applied::Project));
    assert_eq!(undo_index(&project), index + 1);
    {
        let p = project.shared().lock();
        let b = p.project().board(board).unwrap();
        let s = b.settings();
        assert_eq!(b.name().as_str(), "Main Board");
        assert_eq!(s.inner_layer_count, 2);
        assert_eq!(*s.pcb_thickness, len(1.0));
        assert_eq!(s.solder_resist, None);
        assert!(!s.silkscreen_layers_top.contains(&Layer::TOP_NAMES));
        assert!(s.silkscreen_layers_top.contains(&Layer::TOP_LEGEND));
        assert_eq!(*s.design_rules.default_trace_width(), len(0.3));
        assert!(s.design_rules.pad_cmp_side_auto_annular_ring());
        assert_eq!(*s.design_rules.stop_mask_clearance().min_value(), len(0.05));
        assert_eq!(*s.drc_settings.min_copper_copper_clearance(), len(0.15));
        assert!(s.drc_settings.blind_vias_allowed());
        assert_eq!(
            s.drc_settings.allowed_pth_slots(),
            librepcb_core::library::org::AllowedSlots::Any
        );
    }
    // Reset the DRC settings to the defaults.
    edit(d, &project, "drc_defaults", |f| {
        f.action = ui::FormFieldAction::Clicked;
    });
    assert_eq!(d.form().get_length("drc_copper_copper"), len(0.2));
    assert!(!d.form().get_checked("drc_blind_vias"));
    // Invalid bounds are reported.
    set_length(d, &project, "stop_mask_max", len(0.01));
    assert!(apply(d, &project).is_err());
    undo(&project);
    let p = project.shared().lock();
    let b = p.project().board(board).unwrap();
    assert_eq!(b.name().as_str(), "default");
    assert_eq!(b.settings().inner_layer_count, 0);
}

#[test]
fn project_setup() {
    let dir = tempfile::tempdir().unwrap();
    let (project, _, _) = create_project(dir.path());
    let mut dialog = ProjectSetupDialog::new(&project);
    assert_eq!(dialog.form().pages().len(), 5);
    let d: &mut dyn FormDialog = &mut dialog;
    edit(d, &project, "name", |f| f.text = "My Project".into());
    edit(d, &project, "author", |f| f.text = " Me ".into());
    edit(d, &project, "version", |f| f.text = "v2".into());
    // Locales and norms.
    list(d, &project, "locales", ListAction::Add("de_CH".into()));
    list(d, &project, "locales", ListAction::Add("en_US".into()));
    list(d, &project, "locales", ListAction::MoveUp(1));
    list(d, &project, "norms", ListAction::Add("IEEE 315".into()));
    // Net classes: add one, rename the default one.
    list(d, &project, "net_classes", ListAction::Add("Power".into()));
    list(d, &project, "net_classes", ListAction::Select(0));
    let old_class = d.form().get_text("net_class_name");
    assert!(!old_class.is_empty());
    // The default net class is in use and cannot be removed.
    list(d, &project, "net_classes", ListAction::Remove(0));
    assert_eq!(
        slint::Model::row_count(&d.form().field("net_classes").unwrap().items),
        2
    );
    edit(d, &project, "net_class_name", |f| f.text = "Signal".into());
    // Assembly variants: add one with a description.
    list(d, &project, "variants", ListAction::Add("Lite".into()));
    edit(d, &project, "variant_description", |f| {
        f.text = "Without extras".into();
    });
    let index = undo_index(&project);
    assert_eq!(apply(d, &project), Ok(Applied::Project));
    assert_eq!(undo_index(&project), index + 1);
    {
        let p = project.shared().lock();
        let prj = p.project();
        assert_eq!(prj.metadata().name.as_str(), "My Project");
        assert_eq!(prj.metadata().author, "Me");
        assert_eq!(prj.metadata().version.as_str(), "v2");
        assert_eq!(prj.settings().locale_order, vec!["en_US", "de_CH"]);
        assert_eq!(prj.settings().norm_order, vec!["IEEE 315"]);
        let circuit = prj.circuit();
        assert!(circuit.net_class_by_name("Power").is_some());
        assert!(circuit.net_class_by_name("Signal").is_some());
        assert!(circuit.net_class_by_name(&old_class).is_none());
        let variants: Vec<String> = circuit
            .assembly_variants()
            .iter()
            .map(|v| v.display_text())
            .collect();
        assert_eq!(variants.last().unwrap(), "Lite (Without extras)");
    }
    // An empty version is rejected.
    edit(d, &project, "version", |f| f.text = "".into());
    assert!(apply(d, &project).is_err());
    undo(&project);
    let p = project.shared().lock();
    assert_ne!(p.project().metadata().name.as_str(), "My Project");
    assert!(p.project().circuit().net_class_by_name("Power").is_none());
}

#[test]
fn move_align() {
    use librepcb_app::dialogs::TabDialogResult;
    use librepcb_app::dialogs::move_align::MoveAlignDialog;
    let dir = tempfile::tempdir().unwrap();
    let (project, _, _) = create_project(dir.path());
    // Three elements in a row (given unordered).
    let positions = vec![mm(5.08, 0.0), mm(0.0, 0.0), mm(2.54, 0.0)];
    let mut dialog = MoveAlignDialog::new(positions, LengthUnit::Millimeters);
    let d: &mut dyn FormDialog = &mut dialog;
    // Absolute mode with the leftmost element as reference, constant pitch.
    assert_eq!(d.form().get_index("mode"), Some(0));
    assert_eq!(d.form().get_length("x"), len(0.0));
    assert!(d.form().get_checked("interval_x_on"));
    assert_eq!(d.form().get_length("interval_x"), len(2.54));
    set_length(d, &project, "x", len(10.0));
    set_length(d, &project, "y", len(5.0));
    set_length(d, &project, "interval_x", len(1.0));
    assert_eq!(
        apply(d, &project),
        Ok(Applied::Tab(TabDialogResult::Positions(vec![
            mm(12.0, 5.0),
            mm(10.0, 5.0),
            mm(11.0, 5.0)
        ])))
    );
    // Centered around the Y axis.
    edit(d, &project, "center_h", |f| f.checked = true);
    assert!(!d.form().field("x").unwrap().enabled);
    assert_eq!(
        apply(d, &project),
        Ok(Applied::Tab(TabDialogResult::Positions(vec![
            mm(1.0, 5.0),
            mm(-1.0, 5.0),
            mm(0.0, 5.0)
        ])))
    );
    edit(d, &project, "center_h", |f| f.checked = false);
    // Relative mode converts the reference position.
    edit(d, &project, "mode", |f| f.index = 1);
    assert_eq!(d.form().get_length("x"), len(10.0));
    edit(d, &project, "mode", |f| f.index = 0);
    assert_eq!(d.form().get_length("x"), len(10.0));
    // Align vertically: ΔX = 0.
    edit(d, &project, "align_vertically", |f| {
        f.action = ui::FormFieldAction::Clicked;
    });
    assert_eq!(d.form().get_length("interval_x"), len(0.0));

    // A single element: relative mode, no pitch.
    let mut dialog = MoveAlignDialog::new(vec![mm(1.0, 2.0)], LengthUnit::Millimeters);
    let d: &mut dyn FormDialog = &mut dialog;
    assert_eq!(d.form().get_index("mode"), Some(1));
    assert!(!d.form().field("interval_x_on").unwrap().enabled);
    set_length(d, &project, "x", len(1.0));
    assert_eq!(
        apply(d, &project),
        Ok(Applied::Tab(TabDialogResult::Positions(vec![mm(2.0, 2.0)])))
    );
}
