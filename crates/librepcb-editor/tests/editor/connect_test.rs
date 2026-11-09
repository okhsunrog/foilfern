//! Forced net names of supply symbols, vias connecting to traces and the
//! automatic trace width.

use librepcb_core::project::{BoardId, NetSignalId, Project, SchematicId};
use librepcb_core::types::{Angle, CircuitIdentifier, ElementName, Length, PositiveLength};
use librepcb_editor::ProjectEditor;
use librepcb_editor::commands::*;

use crate::helpers::{create_editor, lib, mm};

fn net_name(p: &Project, net: NetSignalId) -> String {
    p.circuit().net_signal(net).unwrap().name().to_string()
}

fn add(
    editor: &mut ProjectEditor,
    schematic: SchematicId,
    cmp: librepcb_core::types::Uuid,
    x: f64,
) -> String {
    editor
        .execute(AddComponent {
            place: Some(SymbolPlacement {
                schematic: Some(schematic),
                position: mm(x, 50.8),
                rotation: Angle::DEG0,
                mirrored: false,
                gate_offset: None,
            }),
            ..AddComponent::new(cmp)
        })
        .unwrap()
        .name
}

fn connect(editor: &mut ProjectEditor, a: (&str, &str), b: (&str, &str)) -> WireResult {
    editor
        .execute(ConnectPins {
            a: PinRef::new(a.0, a.1),
            b: PinRef::new(b.0, b.1),
            mode: WireMode::HV,
            net: None,
        })
        .unwrap()
}

#[test]
fn test_forced_net_names() {
    let tmp = tempfile::tempdir().unwrap();
    let mut editor = create_editor(&tmp.path().join("project"));
    let schematic = editor
        .execute(AddSchematic {
            name: ElementName::new("Main").unwrap(),
            index: None,
        })
        .unwrap();
    let r1 = add(&mut editor, schematic, lib::resistor(), 20.32);
    let c1 = add(&mut editor, schematic, lib::capacitor(), 40.64);
    let vcc = add(&mut editor, schematic, lib::supply_vcc(), 20.32);
    let gnd1 = add(&mut editor, schematic, lib::supply_gnd(), 30.48);
    let gnd2 = add(&mut editor, schematic, lib::supply_gnd(), 50.8);

    // Supply pin at the end, then at the start of the wire.
    let w = connect(&mut editor, (&r1, "1"), (&vcc, "Net"));
    assert_eq!(net_name(editor.project(), w.net), "VCC");
    let gnd = connect(&mut editor, (&gnd1, "Net"), (&r1, "2"));
    assert_eq!(net_name(editor.project(), gnd.net), "GND");
    // A second supply symbol joins the existing net.
    let w = connect(&mut editor, (&c1, "2"), (&gnd2, "Net"));
    assert_eq!(w.net, gnd.net);
    // A named net connected to a segment with a supply symbol: the forced
    // net wins regardless of the wire direction.
    let foo = editor
        .execute(ConnectPinToNet {
            pin: PinRef::new(c1.as_str(), "1"),
            net: CircuitIdentifier::new("FOO").unwrap(),
            label: false,
            stub_length: None,
        })
        .unwrap();
    let stub_end = foo.wire.unwrap().junctions[0];
    let w = editor
        .execute(DrawWire {
            schematic: Some(schematic),
            start: WireAnchor::Line {
                segment: gnd.segment,
                line: gnd.lines[0],
                position: mm(25.0, 50.8),
            },
            end: WireAnchor::Junction {
                segment: foo.segment,
                junction: stub_end,
            },
            points: vec![],
            net: None,
        })
        .unwrap();
    let p = editor.project();
    assert_eq!(net_name(p, w.net), "GND");
    assert!(p.circuit().net_signal_by_name("FOO").is_none());
    assert_eq!(p.circuit().net_signals().len(), 2);
    assert!(p.is_ref_index_consistent());

    // Removing the wire again: the part with the supply symbol keeps its
    // forced net, the other part gets a new net.
    let segment = w.segment;
    editor
        .execute(RemoveSchematicItems {
            schematic,
            selection: SchematicSelection {
                net_lines: w.lines.iter().map(|l| (segment, *l)).collect(),
                ..SchematicSelection::default()
            },
        })
        .unwrap();
    let p = editor.project();
    let pin_net = |component: &str, signal: &str| {
        let (_, c) = p.circuit().component_instance_by_name(component).unwrap();
        let lib = p.library().component(&c.lib_component()).unwrap();
        let signal = lib.signals().by_name(signal, true).unwrap().uuid();
        net_name(p, c.signal(&signal).unwrap().net().unwrap())
    };
    assert_eq!(pin_net(&r1, "2"), "GND");
    assert_eq!(pin_net(&c1, "2"), "GND");
    assert_ne!(pin_net(&c1, "1"), "GND");
    assert!(p.is_ref_index_consistent());
    assert!(
        librepcb_core::project::erc::run_erc(p)
            .iter()
            .all(|m| !matches!(
                m.kind(),
                librepcb_core::project::erc::ErcMessageKind::ForcedNetSignalNameConflict(_)
            ))
    );
}

/// R1 with VCC on pad 1 and GND on pad 2, placed on a board.
fn board_with_resistor(editor: &mut ProjectEditor) -> BoardId {
    let schematic = editor
        .execute(AddSchematic {
            name: ElementName::new("Main").unwrap(),
            index: None,
        })
        .unwrap();
    editor
        .execute(AddComponent {
            device: Some(lib::r0805()),
            place: Some(SymbolPlacement {
                schematic: Some(schematic),
                position: mm(20.32, 50.8),
                rotation: Angle::DEG0,
                mirrored: false,
                gate_offset: None,
            }),
            ..AddComponent::new(lib::resistor())
        })
        .unwrap();
    for (pin, net) in [("1", "VCC"), ("2", "GND")] {
        editor
            .execute(ConnectPinToNet {
                pin: PinRef::new("R1", pin),
                net: CircuitIdentifier::new(net).unwrap(),
                label: true,
                stub_length: None,
            })
            .unwrap();
    }
    let board = editor
        .execute(AddBoard::new(ElementName::new("default").unwrap()))
        .unwrap()
        .board;
    editor
        .execute(AddDevice {
            component: "R1".into(),
            board: Some(board),
            device: None,
            footprint: None,
            position: mm(10.0, 10.0),
            rotation: Angle::DEG0,
            mirrored: false,
        })
        .unwrap();
    board
}

fn width(mm: f64) -> PositiveLength {
    PositiveLength::new(Length::from_mm(mm).unwrap()).unwrap()
}

#[test]
fn test_via_connects_to_traces() {
    let tmp = tempfile::tempdir().unwrap();
    let mut editor = create_editor(&tmp.path().join("project"));
    let board = board_with_resistor(&mut editor);
    let trace = editor
        .execute(AddTrace {
            board: Some(board),
            start: TraceEndpoint::Pad(PadRef::new("R1", "1")),
            end: TraceEndpoint::Point(mm(10.0, 20.0)),
            points: vec![mm(5.0, 10.0), mm(5.0, 20.0)],
            layer: None,
            width: Some(width(0.3)),
            net: None,
        })
        .unwrap();
    let count = |editor: &ProjectEditor| {
        let b = editor.project().board(board).unwrap();
        let s = b.net_segment(trace.segment);
        (
            b.net_segments().len(),
            s.map(|s| (s.vias().len(), s.junctions().len(), s.traces().len())),
        )
    };
    assert_eq!(count(&editor), (1, Some((0, 3, 3))));

    // A via of another net at the trace is not connected.
    let other = editor
        .execute(AddVia {
            board: Some(board),
            position: mm(5.0, 15.0),
            net: Some("GND".into()),
            start_layer: None,
            end_layer: None,
            drill_diameter: None,
            size: None,
            exposure: None,
        })
        .unwrap();
    assert_ne!(other.segment, trace.segment);
    assert_eq!(count(&editor), (2, Some((0, 3, 3))));
    editor.undo().unwrap();

    // A via in the middle of a trace splits it.
    let via = editor
        .execute(AddVia {
            board: Some(board),
            position: mm(5.0, 15.0),
            net: Some("VCC".into()),
            start_layer: None,
            end_layer: None,
            drill_diameter: None,
            size: None,
            exposure: None,
        })
        .unwrap();
    let b = editor.project().board(board).unwrap();
    assert_eq!(b.net_segments().len(), 1);
    let seg = b.net_segment(via.segment).unwrap();
    assert_eq!((seg.vias().len(), seg.junctions().len()), (1, 3));
    assert_eq!(seg.traces().len(), 4);
    assert!(editor.project().is_ref_index_consistent());

    // A via at the end of a trace replaces the junction.
    let end = editor
        .execute(AddVia {
            board: Some(board),
            position: mm(10.0, 20.0),
            net: Some("VCC".into()),
            start_layer: None,
            end_layer: None,
            drill_diameter: None,
            size: None,
            exposure: None,
        })
        .unwrap();
    let b = editor.project().board(board).unwrap();
    assert_eq!(b.net_segments().len(), 1);
    let seg = b.net_segment(end.segment).unwrap();
    assert_eq!((seg.vias().len(), seg.junctions().len()), (2, 2));
    assert_eq!(seg.traces().len(), 4);
    assert!(editor.project().is_ref_index_consistent());
    // Undo restores the previous state exactly.
    editor.undo().unwrap();
    editor.undo().unwrap();
    assert_eq!(count(&editor), (1, Some((0, 3, 3))));
}

#[test]
fn test_trace_auto_width() {
    let tmp = tempfile::tempdir().unwrap();
    let mut editor = create_editor(&tmp.path().join("project"));
    let board = board_with_resistor(&mut editor);
    let first = editor
        .execute(AddTrace {
            board: Some(board),
            start: TraceEndpoint::Pad(PadRef::new("R1", "1")),
            end: TraceEndpoint::Point(mm(5.0, 10.0)),
            points: vec![],
            layer: None,
            width: Some(width(0.3)),
            net: None,
        })
        .unwrap();
    // Continuing at the end takes over the width of the existing trace.
    let next = editor
        .execute(AddTrace {
            board: Some(board),
            start: TraceEndpoint::Junction(first.junctions[0]),
            end: TraceEndpoint::Point(mm(5.0, 20.0)),
            points: vec![],
            layer: None,
            width: None,
            net: None,
        })
        .unwrap();
    let b = editor.project().board(board).unwrap();
    let trace = &b.net_segment(next.segment).unwrap().traces()[&next.traces[0]];
    assert_eq!(trace.width(), width(0.3));
    // Starting at a pad without traces uses the default width.
    let other = editor
        .execute(AddTrace {
            board: Some(board),
            start: TraceEndpoint::Pad(PadRef::new("R1", "2")),
            end: TraceEndpoint::Point(mm(15.0, 10.0)),
            points: vec![],
            layer: None,
            width: None,
            net: None,
        })
        .unwrap();
    let b = editor.project().board(board).unwrap();
    let trace = &b.net_segment(other.segment).unwrap().traces()[&other.traces[0]];
    assert_eq!(trace.width(), b.design_rules().default_trace_width());
}
