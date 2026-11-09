//! Editing in the schematic and board tabs through the Rust API of the
//! tabs, with simulated pointer events in screen coordinates (like the
//! Slint UI sends them): moving a symbol, drawing a wire, the context menu,
//! copy/paste, drawing a trace and undo.
//!
//! The tabs work without a Slint window (the scene is rendered into an
//! image), so no platform is needed.

use std::rc::Rc;

use crate::common::*;
use librepcb_app::project::AppProject;
use librepcb_app::tabs::{Board2dTab, SchematicTab, TabRequest};
use librepcb_app::ui;
use librepcb_core::types::{Angle, GridStyle};
use librepcb_editor::fsm::board::BoardTool;
use librepcb_editor::fsm::schematic::SchematicTool;

#[test]
fn schematic_move_symbol_draw_wire_undo() {
    let dir = tempfile::tempdir().unwrap();
    let (project, sch, _) = create_project(dir.path());
    let mut tab = SchematicTab::new(Rc::clone(&project), sch, GridStyle::Lines);
    let _ = tab.render_scene(1200.0, 800.0, 1.0);

    // Hovering a symbol shows the pointing hand cursor or info box.
    let (r2, r2_pos) = symbol(&project, sch, "R2");
    let wires_before = wire_count(&project, sch);

    // Drag R2 by its origin.
    let index = undo_index(&project);
    tab.move_to(r2_pos);
    tab.press(r2_pos);
    let target = r2_pos + mm(5.0, 10.0);
    tab.move_to(target);
    assert!(
        project
            .shared()
            .lock()
            .editor
            .undo_stack()
            .is_group_active()
    );
    let update = tab.release(target);
    assert!(update.repaint && update.data_changed);
    assert!(
        !project
            .shared()
            .lock()
            .editor
            .undo_stack()
            .is_group_active()
    );
    assert_eq!(undo_index(&project), index + 1);
    let (_, moved) = symbol(&project, sch, "R2");
    assert_eq!(moved, r2_pos + mm(5.08, 10.16));
    // The moved symbol is selected and highlighted in the scene.
    assert!(
        tab.fsm()
            .selection()
            .contains(&librepcb_editor::fsm::schematic::SchematicItem::Symbol(r2))
    );
    assert!(tab.scene().unwrap().scene().selection().next().is_some());

    // Draw a wire from R1 pin 2 to R2 pin 1.
    let from = pins(&project, sch, "R1")[1];
    let to = pins(&project, sch, "R2")[0];
    tab.trigger(ui::TabAction::ToolWire);
    assert_eq!(tab.fsm().tool(), SchematicTool::Wire);
    assert_eq!(tab.derived_ui_data(&[]).tool, ui::EditorTool::Wire);
    tab.click(from);
    tab.move_to(to);
    // The preview is in the model and the scene.
    assert!(wire_count(&project, sch) > wires_before);
    tab.click(to);
    assert!(
        !project
            .shared()
            .lock()
            .editor
            .undo_stack()
            .is_group_active()
    );
    let wires = wire_count(&project, sch);
    assert!(wires > wires_before);
    // Escape (the abort shortcut) leaves the tool.
    tab.trigger(ui::TabAction::Abort);
    assert_eq!(tab.fsm().tool(), SchematicTool::Select);

    // Undo the wire (and its simplification), then the move.
    while wire_count(&project, sch) > wires_before {
        tab.trigger(ui::TabAction::Undo);
    }
    tab.trigger(ui::TabAction::Undo);
    assert_eq!(symbol(&project, sch, "R2").1, r2_pos);
    assert_eq!(undo_index(&project), index);
    // Redo is available.
    assert_eq!(tab.ui_data().features.redo, ui::FeatureState::Enabled);
    tab.trigger(ui::TabAction::Redo);
    assert_eq!(symbol(&project, sch, "R2").1, moved);
}

#[test]
fn schematic_context_menu_and_clipboard() {
    let dir = tempfile::tempdir().unwrap();
    let (project, sch, _) = create_project(dir.path());
    let mut tab = SchematicTab::new(Rc::clone(&project), sch, GridStyle::Lines);
    let _ = tab.render_scene(1200.0, 800.0, 1.0);
    let (r1, r1_pos) = symbol(&project, sch, "R1");

    // Right click: the context menu of the symbol.
    let update = tab.right_click(r1_pos);
    let entries = update
        .requests
        .iter()
        .find_map(|r| match r {
            TabRequest::ContextMenu { entries, .. } => Some(entries.clone()),
            _ => None,
        })
        .expect("context menu");
    let rotate = entries
        .iter()
        .position(|e| e.text == "Rotate Counterclockwise")
        .expect("rotate entry");
    tab.context_menu_action(rotate);
    {
        let p = project.shared().lock();
        let s = p.project().schematic(sch).unwrap();
        assert_eq!(s.symbols()[&r1].rotation(), Angle::DEG90);
    }
    // Properties: the tab requests the properties dialog.
    let update = tab.trigger(ui::TabAction::EditProperties);
    assert!(
        update
            .requests
            .iter()
            .any(|r| matches!(r, TabRequest::Properties(_)))
    );

    // Copy the selected symbol and paste it somewhere else.
    let symbols_before = project
        .shared()
        .lock()
        .project()
        .schematic(sch)
        .unwrap()
        .symbols()
        .len();
    assert!(tab.fsm().view_state().features.copy);
    tab.trigger(ui::TabAction::Copy);
    let target = r1_pos + mm(0.0, 20.32);
    tab.move_to(target);
    tab.trigger(ui::TabAction::Paste);
    tab.move_to(target);
    tab.click(target);
    let symbols = project
        .shared()
        .lock()
        .project()
        .schematic(sch)
        .unwrap()
        .symbols()
        .len();
    assert_eq!(symbols, symbols_before + 1);

    // Select all and delete.
    tab.trigger(ui::TabAction::SelectAll);
    tab.trigger(ui::TabAction::Delete);
    let symbols = project
        .shared()
        .lock()
        .project()
        .schematic(sch)
        .unwrap()
        .symbols()
        .len();
    assert_eq!(symbols, 0);
    tab.trigger(ui::TabAction::Undo);
    let symbols = project
        .shared()
        .lock()
        .project()
        .schematic(sch)
        .unwrap()
        .symbols()
        .len();
    assert_eq!(symbols, symbols_before + 1);
}

#[test]
fn board_draw_trace_and_undo() {
    let dir = tempfile::tempdir().unwrap();
    let (project, _, board) = create_project(dir.path());
    let mut tab = Board2dTab::new(Rc::clone(&project), board, GridStyle::Lines);
    let _ = tab.render_scene(1200.0, 800.0, 1.0);

    // Pads of the GND net: R1.1 and R2.2.
    let pad = |name: &str, pad: &str| {
        let p = project.shared().lock();
        let proj = p.project();
        let (c, _) = proj.circuit().component_instance_by_name(name).unwrap();
        proj.board(board)
            .unwrap()
            .device(c)
            .unwrap()
            .pads(proj.library(), proj.circuit())
            .unwrap()
            .into_iter()
            .find(|v| v.package_pad().is_some_and(|pp| pp.name().as_str() == pad))
            .unwrap()
            .position()
    };
    let a = pad("R1", "1");
    let b = pad("R2", "2");
    let index = undo_index(&project);

    tab.trigger(ui::TabAction::ToolWire);
    assert_eq!(tab.fsm().tool(), BoardTool::DrawTrace);
    let data = tab.derived_ui_data(&[]);
    assert_eq!(data.tool, ui::EditorTool::Wire);
    assert!(data.tool_layer.current_index >= 0);
    tab.click(a);
    // The net of the trace is highlighted and cross-probed.
    assert!(!tab.fsm().highlighted_nets().is_empty());
    assert!(!tab.cross_probe().nets.is_empty());
    tab.move_to(b);
    tab.click(b);
    assert!(
        !project
            .shared()
            .lock()
            .editor
            .undo_stack()
            .is_group_active()
    );
    assert!(trace_count(&project, board) > 0);
    assert_eq!(undo_index(&project), index + 1);

    // Tool bar: change the trace width through the UI data.
    let mut data = tab.derived_ui_data(&[]);
    data.tool_line_width.increase = true;
    let before = tab.fsm().tool_data().line_width;
    tab.set_derived_ui_data(&data);
    assert_ne!(tab.fsm().tool_data().line_width, before);

    tab.trigger(ui::TabAction::Abort);
    assert_eq!(tab.fsm().tool(), BoardTool::Select);
    tab.trigger(ui::TabAction::Undo);
    assert_eq!(trace_count(&project, board), 0);
    assert_eq!(undo_index(&project), index);
}

#[test]
fn board_move_device_and_rotate() {
    let dir = tempfile::tempdir().unwrap();
    let (project, _, board) = create_project(dir.path());
    let mut tab = Board2dTab::new(Rc::clone(&project), board, GridStyle::Lines);
    let _ = tab.render_scene(1200.0, 800.0, 1.0);
    let device = |project: &AppProject| {
        let p = project.shared().lock();
        let proj = p.project();
        let (c, _) = proj.circuit().component_instance_by_name("R2").unwrap();
        let d = proj.board(board).unwrap().device(c).unwrap();
        (d.transform().position, d.transform().rotation)
    };
    let (pos, _) = device(&project);
    // Grab the device at a pad.
    let grab = {
        let p = project.shared().lock();
        let proj = p.project();
        let (c, _) = proj.circuit().component_instance_by_name("R2").unwrap();
        let d = proj.board(board).unwrap().device(c).unwrap();
        d.pads(proj.library(), proj.circuit()).unwrap()[0].position()
    };
    tab.move_to(grab);
    tab.press(grab);
    let target = grab + mm(2.0, 3.0);
    tab.move_to(target);
    tab.release(target);
    let (moved, _) = device(&project);
    assert_ne!(moved, pos);
    tab.trigger(ui::TabAction::RotateCcw);
    assert_eq!(device(&project).1, Angle::DEG90);
    tab.trigger(ui::TabAction::Undo);
    tab.trigger(ui::TabAction::Undo);
    assert_eq!(device(&project), (pos, Angle::DEG0));
}
