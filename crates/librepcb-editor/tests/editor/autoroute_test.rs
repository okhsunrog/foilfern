//! Autorouting of boards built with the editor commands, checked by our
//! DRC, the air wires and the official `librepcb-cli`.

use librepcb_core::project::board::BoardAirWiresBuilder;
use librepcb_core::project::{BoardId, Project};
use librepcb_core::rule_check::Severity;
use librepcb_core::types::{Angle, CircuitIdentifier, ElementName, Layer};
use librepcb_editor::ProjectEditor;
use librepcb_editor::commands::*;

use crate::helpers::{create_editor, lib, mm, run_cli};

fn name(s: &str) -> ElementName {
    ElementName::new(s).unwrap()
}

fn ident(s: &str) -> CircuitIdentifier {
    CircuitIdentifier::new(s).unwrap()
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
