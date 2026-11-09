//! The properties dialogs (M3b): opened from the requests of the schematic
//! and board tabs (or directly for items the test adds with editor
//! commands), fields edited like the UI does, applied as one undo group and
//! undone again.
//!
//! No Slint platform is needed: the dialogs' forms are plain models.

use std::collections::BTreeSet;
use std::rc::Rc;

use crate::common::*;
use librepcb_app::dialogs::{
    self, Applied, DialogContext, FieldEvent, FormDialog, ListAction, TabDialogResult,
};
use librepcb_app::project::AppProject;
use librepcb_app::tabs::{Board2dTab, PropertiesTarget, SchematicTab, TabRequest};
use librepcb_app::ui;
use librepcb_core::geometry::{Path, ZoneRules};
use librepcb_core::project::{BoardId, Mutation, SchematicMutation};
use librepcb_core::types::{
    Angle, GridStyle, Layer, Length, LengthUnit, MaskConfig, PositiveLength, UnsignedLength,
};
use librepcb_editor::commands::{AddHole, AddPlane, AddStrokeText, AddVia, AddZone};
use librepcb_editor::fsm::board::BoardItemRef;
use slint::{Model, SharedString};

#[allow(unused_imports)]
use librepcb_app::dialogs::Form;

fn len(mm: f64) -> Length {
    Length::from_mm(mm).unwrap()
}

/// Edits a field like the UI and reports it to the dialog.
fn edit(
    dialog: &mut dyn FormDialog,
    project: &Rc<AppProject>,
    id: &str,
    f: impl FnOnce(&mut ui::FormField),
) {
    let (id, event) = dialog.form_mut().edit(id, f).expect("field");
    dialog.field_event(&DialogContext::new(project), &id, event);
}

fn apply(dialog: &mut dyn FormDialog, project: &Rc<AppProject>) -> Result<Applied, String> {
    dialog.apply(&DialogContext::new(project))
}

fn properties_request(requests: &[TabRequest]) -> PropertiesTarget {
    requests
        .iter()
        .find_map(|r| match r {
            TabRequest::Properties(t) => Some(t.clone()),
            _ => None,
        })
        .expect("properties request")
}

fn undo(project: &AppProject) {
    project.shared().lock().editor.undo().unwrap();
}

#[test]
fn symbol_properties() {
    let dir = tempfile::tempdir().unwrap();
    let (project, sch, _) = create_project(dir.path());
    let mut tab = SchematicTab::new(Rc::clone(&project), sch, GridStyle::Lines);
    let _ = tab.render_scene(1200.0, 800.0, 1.0);
    let (r1, pos) = symbol(&project, sch, "R1");
    tab.click(pos);
    let update = tab.trigger(ui::TabAction::EditProperties);
    let target = properties_request(&update.requests);
    assert_eq!(target, PropertiesTarget::Symbol(r1));
    let dialog = dialogs::open_properties(&project, &target, LengthUnit::Millimeters).unwrap();
    assert_eq!(dialog.title(), "Properties of R1");
    assert_eq!(dialog.form().get_text("name"), "R1");
    let mut concrete =
        dialogs::schematic::SymbolPropertiesDialog::new(&project, r1, LengthUnit::Millimeters)
            .unwrap();
    let attributes = Rc::clone(concrete.attributes());
    let dialog: &mut dyn FormDialog = &mut concrete;
    let index = undo_index(&project);

    // Value, position, rotation, mirror and a new attribute.
    edit(dialog, &project, "value", |f| f.text = "4k7".into());
    edit(dialog, &project, "pos_x", |f| {
        f.length = ui::LengthEditData {
            value: librepcb_app::helpers::length_to_ui(len(10.16)),
            ..f.length.clone()
        }
    });
    edit(dialog, &project, "rotation", |f| {
        f.action = ui::FormFieldAction::Increase;
    });
    assert_eq!(dialog.form().get_angle("rotation"), Angle::DEG90);
    edit(dialog, &project, "mirror", |f| f.checked = true);
    {
        // A new attribute typed into the last row of the attribute list
        // (the application calls `row_written()` for UI writes).
        let rows = attributes.borrow().rows();
        let last = rows.len() - 1;
        let mut row = rows[last].clone();
        row.key = "MPN".into();
        attributes.borrow_mut().row_written(last, &row);
        let mut row = attributes.borrow().rows()[last].clone();
        row.value = "RC0805".into();
        attributes.borrow_mut().row_written(last, &row);
        assert_eq!(
            dialog
                .form()
                .field("attributes")
                .unwrap()
                .attributes
                .row_count(),
            rows.len() + 1
        );
    }
    assert_eq!(apply(dialog, &project), Ok(Applied::Project));
    assert_eq!(undo_index(&project), index + 1);
    {
        let p = project.shared().lock();
        let prj = p.project();
        let (_, c) = prj.circuit().component_instance_by_name("R1").unwrap();
        assert_eq!(c.value(), "4k7");
        assert!(c.attributes().contains_name("MPN"));
        let s = &prj.schematic(sch).unwrap().symbols()[&r1];
        assert_eq!(s.position().x, len(10.16));
        assert_eq!(s.rotation(), Angle::DEG90);
        assert!(s.mirrored());
    }
    undo(&project);
    let (_, restored) = symbol(&project, sch, "R1");
    assert_eq!(restored, pos);

    // Renaming to an existing name asks to swap (second OK swaps).
    let mut dialog = dialogs::open_properties(&project, &target, LengthUnit::Millimeters).unwrap();
    edit(dialog.as_mut(), &project, "name", |f| f.text = "R2".into());
    let question = apply(dialog.as_mut(), &project).unwrap_err();
    assert!(question.contains("swap"), "{question}");
    assert_eq!(apply(dialog.as_mut(), &project), Ok(Applied::Project));
    let r1_component = symbol_component(&project, sch, r1);
    {
        let p = project.shared().lock();
        let circuit = p.project().circuit();
        assert_eq!(
            circuit.component_instance_by_name("R2").unwrap().0,
            r1_component
        );
        assert!(circuit.component_instance_by_name("R1").is_some());
    }
    undo(&project);
    let p = project.shared().lock();
    assert_eq!(
        p.project()
            .circuit()
            .component_instance_by_name("R1")
            .unwrap()
            .0,
        r1_component
    );
}

fn symbol_component(
    project: &AppProject,
    sch: librepcb_core::project::SchematicId,
    symbol: librepcb_core::project::SymbolId,
) -> librepcb_core::project::ComponentInstanceId {
    project
        .shared()
        .lock()
        .project()
        .schematic(sch)
        .unwrap()
        .symbols()[&symbol]
        .component()
}

#[test]
fn net_label_rename() {
    let dir = tempfile::tempdir().unwrap();
    let (project, sch, _) = create_project(dir.path());
    let segment = {
        let p = project.shared().lock();
        let (net, _) = p.project().circuit().net_signal_by_name("GND").unwrap();
        p.project()
            .schematic(sch)
            .unwrap()
            .net_segments()
            .values()
            .find(|s| s.net() == net)
            .unwrap()
            .uuid()
    };
    let target = PropertiesTarget::NetSegment(sch, segment);
    let mut dialog = dialogs::open_properties(&project, &target, LengthUnit::Millimeters).unwrap();
    assert_eq!(dialog.form().get_text("name"), "GND");
    // Rename the whole net (upstream default: only this segment).
    edit(dialog.as_mut(), &project, "scope", |f| f.index = 1);
    edit(dialog.as_mut(), &project, "name", |f| {
        f.text = "gnd 2".into()
    });
    // Cleaned like upstream (spaces become underscores, case is kept).
    assert!(dialog.form().get_text("description").contains("gnd_2"));
    assert_eq!(apply(dialog.as_mut(), &project), Ok(Applied::Project));
    {
        let p = project.shared().lock();
        assert!(p.project().circuit().net_signal_by_name("gnd_2").is_some());
        assert!(p.project().circuit().net_signal_by_name("GND").is_none());
    }
    undo(&project);
    let p = project.shared().lock();
    assert!(p.project().circuit().net_signal_by_name("GND").is_some());
}

#[test]
fn schematic_polygon_and_text() {
    let dir = tempfile::tempdir().unwrap();
    let (project, sch, _) = create_project(dir.path());
    let polygon = librepcb_core::geometry::Polygon::new(
        librepcb_core::types::Uuid::new_random(),
        Layer::SCHEMATIC_GUIDE,
        UnsignedLength::new(len(0.2)).unwrap(),
        false,
        false,
        Path::rect(mm(0.0, 0.0), mm(10.0, 10.0)),
    );
    let text = librepcb_core::geometry::Text::new(
        librepcb_core::types::Uuid::new_random(),
        Layer::SCHEMATIC_COMMENTS,
        "Hello",
        mm(5.0, 5.0),
        Angle::DEG0,
        PositiveLength::new(len(2.5)).unwrap(),
        librepcb_core::types::Alignment::default(),
        false,
    );
    let (pu, tu) = (polygon.uuid(), text.uuid());
    project
        .shared()
        .lock()
        .editor
        .apply_mutations(
            "add",
            vec![
                Mutation::Schematic(SchematicMutation::AddPolygon {
                    schematic: sch,
                    polygon,
                }),
                Mutation::Schematic(SchematicMutation::AddText {
                    schematic: sch,
                    text,
                }),
            ],
        )
        .unwrap();

    let mut dialog = dialogs::open_properties(
        &project,
        &PropertiesTarget::SchematicPolygon(sch, pu),
        LengthUnit::Millimeters,
    )
    .unwrap();
    edit(dialog.as_mut(), &project, "fill", |f| f.checked = true);
    edit(dialog.as_mut(), &project, "vertex_1_x", |f| {
        f.length.value = librepcb_app::helpers::length_to_ui(len(20.0));
    });
    assert_eq!(apply(dialog.as_mut(), &project), Ok(Applied::Project));
    {
        let p = project.shared().lock();
        let polygon = &p.project().schematic(sch).unwrap().polygons()[&pu];
        assert!(polygon.is_filled());
        assert_eq!(polygon.path().vertices()[1].pos.x, len(20.0));
    }

    let mut dialog = dialogs::open_properties(
        &project,
        &PropertiesTarget::SchematicText(sch, tu),
        LengthUnit::Millimeters,
    )
    .unwrap();
    edit(dialog.as_mut(), &project, "text", |f| {
        f.text = "World".into()
    });
    edit(dialog.as_mut(), &project, "halign", |f| f.index = 2);
    assert_eq!(apply(dialog.as_mut(), &project), Ok(Applied::Project));
    {
        let p = project.shared().lock();
        let text = &p.project().schematic(sch).unwrap().texts()[&tu];
        assert_eq!(text.text(), "World");
        assert_eq!(text.align().h, librepcb_core::types::HAlign::Right);
    }
    undo(&project);
    let p = project.shared().lock();
    assert_eq!(
        p.project().schematic(sch).unwrap().texts()[&tu].text(),
        "Hello"
    );
}

#[test]
fn device_properties() {
    let dir = tempfile::tempdir().unwrap();
    let (project, _, board) = create_project(dir.path());
    let mut tab = Board2dTab::new(Rc::clone(&project), board, GridStyle::Lines);
    let _ = tab.render_scene(1200.0, 800.0, 1.0);
    let (component, pad) = {
        let p = project.shared().lock();
        let prj = p.project();
        let (c, _) = prj.circuit().component_instance_by_name("R2").unwrap();
        let d = prj.board(board).unwrap().device(c).unwrap();
        (
            c,
            d.pads(prj.library(), prj.circuit()).unwrap()[0].position(),
        )
    };
    tab.click(pad);
    let update = tab.trigger(ui::TabAction::EditProperties);
    let target = properties_request(&update.requests);
    assert_eq!(
        target,
        PropertiesTarget::Board(board, BoardItemRef::Device(component))
    );
    let mut dialog = dialogs::open_properties(&project, &target, LengthUnit::Millimeters).unwrap();
    let index = undo_index(&project);
    edit(dialog.as_mut(), &project, "rotation", |f| {
        f.angle.value = Angle::DEG180.to_micro_deg();
    });
    edit(dialog.as_mut(), &project, "lock", |f| f.checked = true);
    edit(dialog.as_mut(), &project, "value", |f| f.text = "1k".into());
    assert_eq!(apply(dialog.as_mut(), &project), Ok(Applied::Project));
    assert_eq!(undo_index(&project), index + 1);
    {
        let p = project.shared().lock();
        let d = p.project().board(board).unwrap().device(component).unwrap();
        assert_eq!(d.rotation(), Angle::DEG180);
        assert!(d.locked());
        assert!(d.stroke_texts().values().all(|t| t.locked()));
        let c = p.project().circuit().component_instance(component).unwrap();
        assert_eq!(c.value(), "1k");
    }
    undo(&project);
    let p = project.shared().lock();
    let d = p.project().board(board).unwrap().device(component).unwrap();
    assert_eq!(d.rotation(), Angle::DEG0);
    assert!(!d.locked());
}

fn board_item(project: &Rc<AppProject>, board: BoardId, item: BoardItemRef) -> Box<dyn FormDialog> {
    dialogs::open_properties(
        project,
        &PropertiesTarget::Board(board, item),
        LengthUnit::Millimeters,
    )
    .expect("dialog")
}

#[test]
fn via_properties() {
    let dir = tempfile::tempdir().unwrap();
    let (project, _, board) = create_project(dir.path());
    let via = project
        .shared()
        .lock()
        .editor
        .execute(AddVia {
            board: Some(board),
            position: mm(30.0, 15.0),
            net: Some("GND".into()),
            start_layer: None,
            end_layer: None,
            drill_diameter: None,
            size: None,
            exposure: None,
        })
        .unwrap();
    let mut dialog = board_item(&project, board, BoardItemRef::Via(via.segment, via.via));
    assert!(dialog.form().get_auto("drill"));
    assert!(dialog.form().get_auto("size"));
    // Manual size unchecks the automatic drill (upstream behavior).
    edit(dialog.as_mut(), &project, "size", |f| {
        f.auto_checked = false
    });
    edit(dialog.as_mut(), &project, "drill", |f| {
        f.auto_checked = false
    });
    edit(dialog.as_mut(), &project, "drill", |f| {
        f.length.value = librepcb_app::helpers::length_to_ui(len(0.4));
    });
    edit(dialog.as_mut(), &project, "size", |f| {
        f.length.value = librepcb_app::helpers::length_to_ui(len(0.8));
    });
    edit(dialog.as_mut(), &project, "exposure", |f| f.index = 2);
    assert!(dialog.form().field("exposure_offset").unwrap().enabled);
    assert_eq!(apply(dialog.as_mut(), &project), Ok(Applied::Project));
    {
        let p = project.shared().lock();
        let v = &p
            .project()
            .board(board)
            .unwrap()
            .net_segment(via.segment)
            .unwrap()
            .vias()[&via.via];
        assert_eq!(v.drill_diameter().map(|d| *d), Some(len(0.4)));
        assert_eq!(v.size().map(|d| *d), Some(len(0.8)));
        assert!(matches!(v.exposure_config(), MaskConfig::Manual(_)));
    }
    // Drill larger than size is rejected.
    edit(dialog.as_mut(), &project, "drill", |f| {
        f.length.value = librepcb_app::helpers::length_to_ui(len(1.0));
    });
    assert!(apply(dialog.as_mut(), &project).is_err());
    undo(&project);
    let p = project.shared().lock();
    let v = &p
        .project()
        .board(board)
        .unwrap()
        .net_segment(via.segment)
        .unwrap()
        .vias()[&via.via];
    assert_eq!(v.drill_diameter(), None);
}

#[test]
fn plane_properties() {
    let dir = tempfile::tempdir().unwrap();
    let (project, _, board) = create_project(dir.path());
    let plane = project
        .shared()
        .lock()
        .editor
        .execute(AddPlane {
            board: Some(board),
            net: Some("GND".into()),
            layer: None,
            outline: None,
            settings: Default::default(),
        })
        .unwrap();
    let mut dialog = board_item(&project, board, BoardItemRef::Plane(plane));
    assert!(dialog.form().field("thermal_gap").unwrap().enabled);
    edit(dialog.as_mut(), &project, "connect_style", |f| f.index = 2);
    assert!(!dialog.form().field("thermal_gap").unwrap().enabled);
    edit(dialog.as_mut(), &project, "priority", |f| {
        f.text = "x".into()
    });
    assert!(!dialog.form().field("priority").unwrap().error.is_empty());
    assert!(apply(dialog.as_mut(), &project).is_err());
    edit(dialog.as_mut(), &project, "priority", |f| {
        f.text = "3".into()
    });
    edit(dialog.as_mut(), &project, "net", |f| f.index = 0);
    edit(dialog.as_mut(), &project, "keep_islands", |f| {
        f.checked = true
    });
    assert_eq!(apply(dialog.as_mut(), &project), Ok(Applied::Project));
    {
        let p = project.shared().lock();
        let pl = p.project().board(board).unwrap().plane(plane).unwrap();
        assert_eq!(pl.priority(), 3);
        assert_eq!(pl.net(), None);
        assert!(pl.keep_islands());
        assert_eq!(
            pl.connect_style(),
            librepcb_core::project::board::PlaneConnectStyle::Solid
        );
    }
    undo(&project);
    let p = project.shared().lock();
    assert_eq!(
        p.project()
            .board(board)
            .unwrap()
            .plane(plane)
            .unwrap()
            .priority(),
        0
    );
}

#[test]
fn board_geometry_items() {
    let dir = tempfile::tempdir().unwrap();
    let (project, _, board) = create_project(dir.path());
    let (text, hole, zone, outline) = {
        let mut p = project.shared().lock();
        let text = p
            .editor
            .execute(AddStrokeText {
                board: Some(board),
                layer: Layer::TOP_LEGEND,
                text: "LOGO".into(),
                position: mm(5.0, 5.0),
                rotation: Angle::DEG0,
                height: None,
                stroke_width: None,
                align: None,
            })
            .unwrap();
        let hole = p
            .editor
            .execute(AddHole {
                board: Some(board),
                position: mm(2.0, 2.0),
                diameter: PositiveLength::new(len(1.0)).unwrap(),
                stop_mask: None,
            })
            .unwrap();
        let zone = p
            .editor
            .execute(AddZone {
                board: Some(board),
                layers: BTreeSet::from([Layer::TOP_COPPER]),
                rules: ZoneRules::NO_COPPER,
                outline: Path::rect(mm(20.0, 2.0), mm(25.0, 6.0)),
            })
            .unwrap();
        let outline = *p
            .project()
            .board(board)
            .unwrap()
            .polygons()
            .keys()
            .next()
            .unwrap();
        (text, hole, zone, outline)
    };

    // Stroke text.
    let mut dialog = board_item(&project, board, BoardItemRef::StrokeText(text));
    edit(dialog.as_mut(), &project, "text", |f| {
        f.text = "REV A".into()
    });
    edit(dialog.as_mut(), &project, "letter_spacing_auto", |f| {
        f.checked = false;
    });
    assert!(dialog.form().field("letter_spacing").unwrap().enabled);
    edit(dialog.as_mut(), &project, "mirror", |f| f.checked = true);
    assert_eq!(apply(dialog.as_mut(), &project), Ok(Applied::Project));
    {
        let p = project.shared().lock();
        let t = &p.project().board(board).unwrap().stroke_texts()[&text];
        assert_eq!(t.text(), "REV A");
        assert!(t.mirrored());
        assert!(matches!(
            t.letter_spacing(),
            librepcb_core::types::StrokeTextSpacing::Manual(_)
        ));
    }

    // Hole.
    let mut dialog = board_item(&project, board, BoardItemRef::Hole(hole));
    assert_eq!(dialog.form().pages().len(), 2);
    edit(dialog.as_mut(), &project, "diameter", |f| {
        f.length.value = librepcb_app::helpers::length_to_ui(len(2.0));
    });
    edit(dialog.as_mut(), &project, "stop_mask", |f| f.index = 0);
    assert_eq!(apply(dialog.as_mut(), &project), Ok(Applied::Project));
    {
        let p = project.shared().lock();
        let h = &p.project().board(board).unwrap().holes()[&hole];
        assert_eq!(*h.diameter(), len(2.0));
        assert_eq!(h.stop_mask_config(), MaskConfig::Off);
    }

    // Zone: layers are a check list.
    let mut dialog = board_item(&project, board, BoardItemRef::Zone(zone));
    edit(dialog.as_mut(), &project, "layers", |f| {
        f.action = ui::FormFieldAction::Toggle;
        f.action_row = 1; // Bottom copper (sorted: top, bottom).
    });
    edit(dialog.as_mut(), &project, "rule_3", |f| f.checked = true);
    assert_eq!(apply(dialog.as_mut(), &project), Ok(Applied::Project));
    {
        let p = project.shared().lock();
        let z = &p.project().board(board).unwrap().zones()[&zone];
        assert_eq!(z.layers().len(), 2);
        assert!(z.rules().contains(ZoneRules::NO_DEVICES));
    }

    // Board outline polygon.
    let mut dialog = board_item(&project, board, BoardItemRef::Polygon(outline));
    edit(dialog.as_mut(), &project, "line_width", |f| {
        f.action = ui::FormFieldAction::Increase;
    });
    edit(dialog.as_mut(), &project, "lock", |f| f.checked = true);
    assert_eq!(apply(dialog.as_mut(), &project), Ok(Applied::Project));
    {
        let p = project.shared().lock();
        assert!(p.project().board(board).unwrap().polygons()[&outline].locked());
    }
    undo(&project);
    undo(&project);
    undo(&project);
    undo(&project);
    let p = project.shared().lock();
    let b = p.project().board(board).unwrap();
    assert_eq!(b.stroke_texts()[&text].text(), "LOGO");
    assert_eq!(*b.holes()[&hole].diameter(), len(1.0));
    assert_eq!(b.zones()[&zone].layers().len(), 1);
    assert!(!b.polygons()[&outline].locked());
}

#[test]
fn line_width_dialog() {
    let dir = tempfile::tempdir().unwrap();
    let (project, _, _) = create_project(dir.path());
    let mut dialog = dialogs::board::LineWidthDialog::new(
        UnsignedLength::new(len(0.3)).unwrap(),
        LengthUnit::Millimeters,
    );
    edit(&mut dialog, &project, "width", |f| {
        f.length.value = librepcb_app::helpers::length_to_ui(len(0.5));
    });
    assert_eq!(
        apply(&mut dialog, &project),
        Ok(Applied::Tab(TabDialogResult::LineWidth(
            UnsignedLength::new(len(0.5)).unwrap()
        )))
    );
    // Lists report their actions.
    let _ = (
        FieldEvent::Edited,
        ListAction::Select(0),
        SharedString::new(),
    );
}
