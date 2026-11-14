//! Autorouting of boards built with the editor commands, checked by our
//! DRC, the air wires and the official `librepcb-cli`.

use foilfern_core::geometry::Via;
use foilfern_core::project::board::{BoardAirWiresBuilder, ViaView};
use foilfern_core::project::{BoardId, Project};
use foilfern_core::rule_check::Severity;
use foilfern_core::types::{
    Angle, CircuitIdentifier, ElementName, Layer, Length, MaskConfig, Point, PositiveLength,
    UnsignedLength, Uuid,
};
use foilfern_editor::ProjectEditor;
use foilfern_editor::commands::*;

use crate::helpers::{create_editor, lib, mm, run_cli};

fn name(s: &str) -> ElementName {
    ElementName::new(s).unwrap()
}

fn ident(s: &str) -> CircuitIdentifier {
    CircuitIdentifier::new(s).unwrap()
}

fn positive(mm: f64) -> PositiveLength {
    PositiveLength::new(Length::from_mm(mm).unwrap()).unwrap()
}

fn set_via_rules(editor: &mut ProjectEditor, board: BoardId, clamped: bool) {
    let b = editor.project().board(board).unwrap();
    let mut rules = b.design_rules().clone();
    rules.set_default_via_drill_diameter(positive(0.3));
    let mut drc = b.settings().drc_settings.clone();
    drc.set_min_pth_drill_diameter(
        UnsignedLength::new(*positive(if clamped { 0.5 } else { 0.3 })).unwrap(),
    );
    drc.set_min_pth_annular_ring(
        UnsignedLength::new(*positive(if clamped { 0.2 } else { 0.15 })).unwrap(),
    );
    editor
        .execute(EditBoardSettings {
            board: Some(board),
            settings: None,
            design_rules: Some(rules),
            drc_settings: Some(drc),
            inner_layer_count: None,
        })
        .unwrap();
}

#[test]
fn test_autoroute_via_dimensions_match_applied_model() {
    let tmp = tempfile::tempdir().unwrap();
    let mut editor = create_editor(&tmp.path().join("project"));
    let board = build_unrouted(&mut editor);
    let class = editor
        .project()
        .circuit()
        .net_signals()
        .values()
        .next()
        .unwrap()
        .net_class();
    for class_drill in [None, Some(positive(0.45))] {
        editor
            .execute(EditNetClass {
                net_class: class,
                name: None,
                default_trace_width: None,
                default_via_drill: class_drill,
                inherit_defaults: true,
            })
            .unwrap();
        for clamped in [false, true] {
            set_via_rules(&mut editor, board, clamped);
            for (drill, size) in [
                (None, None),
                (Some(0.3), Some(0.6)),
                (None, Some(0.6)),
                (Some(0.4), None),
                (Some(0.3), None),
                (Some(0.4), Some(0.9)),
                (Some(0.1), Some(0.2)),
            ] {
                let options = Autoroute {
                    via_drill: drill.map(positive),
                    via_size: size.map(positive),
                    ..Default::default()
                };
                let p = editor.project();
                let extracted = autoroute::extract_problem(p, board, &options, None).unwrap();
                let b = p.board(board).unwrap();
                for (net, (drill, size)) in &extracted.via_sizes {
                    let via = Via::new(
                        Uuid::new_random(),
                        Layer::TOP_COPPER,
                        Layer::BOT_COPPER,
                        Point::ORIGIN,
                        *drill,
                        *size,
                        MaskConfig::Off,
                    )
                    .unwrap_or_else(|e| {
                        panic!("{options:?}, class {class_drill:?}, clamped {clamped}: {e}")
                    });
                    let view = ViaView::new(&via, b, Some(*net), p.circuit()).unwrap();
                    assert_eq!(view.drill_diameter(), extracted.problem.rules.via_drill);
                    assert_eq!(view.size(), extracted.problem.rules.via_diameter);
                    let auto_drill =
                        class_drill.unwrap_or(b.design_rules().default_via_drill_diameter());
                    let auto_size =
                        Via::calc_size_from_rules(auto_drill, &b.design_rules().via_annular_ring());
                    if view.drill_diameter() == auto_drill && view.size() == auto_size {
                        assert!(drill.is_none() && size.is_none());
                    }
                }
            }
        }
    }
}

#[test]
fn test_autoroute_explicit_via_size_with_default_drill() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("project");
    let mut editor = create_editor(&dir);
    let board = build_unrouted(&mut editor);
    set_via_rules(&mut editor, board, false);
    editor
        .execute(MoveDevice {
            component: "R2".into(),
            board: Some(board),
            position: None,
            rotation: None,
            mirrored: Some(true),
            locked: None,
        })
        .unwrap();
    let result = editor
        .execute(Autoroute {
            via_drill: Some(positive(0.3)),
            via_size: Some(positive(0.6)),
            ..Default::default()
        })
        .unwrap();
    assert!(result.vias > 0, "{result:?}");
    assert_eq!(result.unrouted, 0, "{result:?}");
    let p = editor.project();
    let b = p.board(board).unwrap();
    for segment in b.net_segments().values() {
        for via in segment.vias().values() {
            let view = ViaView::new(via, b, segment.net(), p.circuit()).unwrap();
            assert_eq!(view.drill_diameter(), positive(0.3));
            assert_eq!(view.size(), positive(0.6));
        }
    }
    editor.undo().unwrap();
    assert!(
        editor
            .project()
            .board(board)
            .unwrap()
            .net_segments()
            .is_empty()
    );
    editor.redo().unwrap();
    editor.save().unwrap();
    let messages = run_drc(&mut editor.into_project(), board);
    assert!(
        messages
            .iter()
            .all(|(severity, _)| *severity != Severity::Error),
        "{messages:#?}"
    );
    check_cli_without_errors(&dir, &["--drc"]);
}

#[test]
fn test_routing_validation_does_not_pass_failed_checks_or_netlist_changes() {
    use foilfern_editor::routing_validation::{RoutingValidation, capture_routing_state};

    let tmp = tempfile::tempdir().unwrap();
    let mut editor = create_editor(&tmp.path().join("project"));
    let board = build_unrouted(&mut editor);
    let before = capture_routing_state(&mut editor, board).unwrap();
    let failed = capture_routing_state(&mut editor, BoardId(Uuid::new_random())).unwrap();
    let validation = RoutingValidation::compare(before.clone(), failed);
    assert!(!validation.after.ran && !validation.after.complete);
    assert!(!validation.after.check_failures.is_empty());
    assert!(!validation.acceptable && !validation.drc_clean);
    assert!(validation.summary().contains("INCOMPLETE"));

    editor
        .execute(RemoveComponent {
            component: "R1".into(),
        })
        .unwrap();
    let after = capture_routing_state(&mut editor, board).unwrap();
    let validation = RoutingValidation::compare(before, after);
    assert!(!validation.netlist_preserved && !validation.acceptable);
}

/// Returns the number of air wires of a board (computed from the model).
pub fn air_wire_count(p: &Project, board: BoardId) -> usize {
    let b = p.board(board).unwrap();
    let builder = BoardAirWiresBuilder::new(b, p.library(), p.circuit());
    p.circuit()
        .net_signals()
        .keys()
        .map(|n| builder.build_air_wires(*n).len())
        .sum()
}

/// Runs our full DRC (after rebuilding the planes) and returns the
/// messages as (severity, message) pairs.
pub fn run_drc(p: &mut Project, board: BoardId) -> Vec<(Severity, String)> {
    p.rebuild_planes(board, None).unwrap();
    let result = p.run_drc(board, None, false, &|_| {}).unwrap();
    assert!(result.errors.is_empty(), "DRC failed: {:?}", result.errors);
    result
        .messages
        .iter()
        .map(|m| (m.message().severity(), m.message().message().to_owned()))
        .collect()
}

/// Runs the official `librepcb-cli open-project <args>` (if available)
/// and checks that it reports no errors (only warnings) and that it
/// opened the project. Returns the output.
pub fn check_cli_without_errors(dir: &std::path::Path, args: &[&str]) -> Option<String> {
    let Some((ok, output)) = run_cli(dir, args) else {
        eprintln!("librepcb-cli not found, skipping the upstream check");
        return None;
    };
    assert!(
        ok || output.contains("Finished with errors!") && !output.contains("[ERROR]"),
        "librepcb-cli failed:\n{output}"
    );
    assert!(
        !output.contains("[ERROR]"),
        "librepcb-cli errors:\n{output}"
    );
    Some(output)
}

/// Builds R1, R2, C1 (upstream "Populated Library") with the nets VCC,
/// MID and GND, placed on a 40x20 mm board but not routed.
fn build_unrouted(editor: &mut ProjectEditor) -> BoardId {
    let schematic = editor
        .execute(AddSchematic {
            name: name("Main"),
            index: None,
        })
        .unwrap();
    let board = editor
        .execute(AddBoard::new(name("default")))
        .unwrap()
        .board;
    let place = |x, y| SymbolPlacement {
        schematic: Some(schematic),
        position: mm(x, y),
        rotation: Angle::DEG0,
        mirrored: false,
        gate_offset: None,
    };
    for (component, device, x) in [
        (lib::resistor(), lib::r0805(), 20.32),
        (lib::resistor(), lib::r0805(), 40.64),
        (lib::capacitor(), lib::c0805(), 60.96),
    ] {
        editor
            .execute(AddComponent {
                device: Some(device),
                place: Some(place(x, 50.8)),
                ..AddComponent::new(component)
            })
            .unwrap();
    }
    editor
        .execute(ConnectPins {
            a: PinRef::new("R1", "2"),
            b: PinRef::new("R2", "1"),
            mode: WireMode::HV,
            net: Some(ident("MID")),
        })
        .unwrap();
    for (component, pin, net) in [
        ("R1", "1", "VCC"),
        ("C1", "1", "VCC"),
        ("R2", "2", "GND"),
        ("C1", "2", "GND"),
    ] {
        editor
            .execute(ConnectPinToNet {
                pin: PinRef::new(component, pin),
                net: ident(net),
                label: true,
                stub_length: None,
            })
            .unwrap();
    }
    // Deliberately placed so that the direct connections cross.
    for (component, x, y, rotation) in [
        ("R1", 10.0, 6.0, Angle::DEG0),
        ("R2", 30.0, 14.0, Angle::DEG180),
        ("C1", 20.0, 10.0, Angle::DEG90),
    ] {
        editor
            .execute(AddDevice {
                component: component.into(),
                board: Some(board),
                device: None,
                footprint: None,
                position: mm(x, y),
                rotation,
                mirrored: false,
            })
            .unwrap();
    }
    editor
        .execute(SetBoardOutline::rect(
            Some(board),
            mm(0.0, 0.0),
            mm(40.0, 20.0),
        ))
        .unwrap();
    board
}

#[test]
fn test_autoroute_populated_library() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("project");
    let mut editor = create_editor(&dir);
    let board = build_unrouted(&mut editor);
    assert!(air_wire_count(editor.project(), board) > 0);

    let history_len = editor.undo_stack().history().len();
    let result = editor.execute(Autoroute::default()).unwrap();
    assert_eq!(editor.undo_stack().history().len(), history_len + 1);
    assert!(result.routed > 0, "{result:?}");
    assert_eq!(result.unrouted, 0, "{result:?}");
    assert!(result.remaining_air_wires.is_empty(), "{result:?}");
    assert!(result.traces > 0);
    let p = editor.project();
    assert!(p.is_ref_index_consistent());
    assert_eq!(air_wire_count(p, board), 0);
    // One segment per net.
    assert_eq!(p.board(board).unwrap().net_segments().len(), 3);

    // Undo removes all routes, redo restores them.
    editor.undo().unwrap();
    assert!(air_wire_count(editor.project(), board) > 0);
    editor.redo().unwrap();
    assert_eq!(air_wire_count(editor.project(), board), 0);

    // Routing again finds nothing to do.
    let again = editor.execute(Autoroute::default()).unwrap();
    assert_eq!(again.routed, 0);
    assert_eq!(again.traces, 0);

    editor.save().unwrap();
    let mut project = editor.into_project();
    let messages = run_drc(&mut project, board);
    let errors: Vec<_> = messages
        .iter()
        .filter(|(s, _)| *s == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "DRC errors: {messages:#?}");

    // The official CLI finds no DRC errors either (warnings about the
    // silkscreen of the library footprints are possible).
    check_cli_without_errors(&dir, &["--drc"]);
}

#[test]
fn test_autoroute_nets_and_layers_filter() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("project");
    let mut editor = create_editor(&dir);
    let board = build_unrouted(&mut editor);
    let result = editor
        .execute(Autoroute {
            nets: Some(vec!["MID".into()]),
            layers: Some(vec![Layer::TOP_COPPER]),
            ..Autoroute::default()
        })
        .unwrap();
    assert_eq!(result.routed, 1, "{result:?}");
    let p = editor.project();
    let b = p.board(board).unwrap();
    let mid = p.circuit().net_signal_by_name("MID").unwrap().0;
    for segment in b.net_segments().values() {
        assert_eq!(segment.net(), Some(mid));
        assert!(segment.vias().is_empty());
        assert!(
            segment
                .traces()
                .values()
                .all(|t| t.layer() == Layer::TOP_COPPER)
        );
    }
    assert!(air_wire_count(p, board) > 0);
}
