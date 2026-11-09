//! Builds a small project with the editor commands (components from the
//! upstream "Populated Library", wires, devices, traces, a via, a plane and
//! a board outline), saves it, re-opens it with our loader, checks it with
//! the official `librepcb-cli` and undoes/redoes everything.

use librepcb_core::geometry::NetLineAnchor;
use librepcb_core::project::{BoardId, LibraryElementKind, NetSignalId, Project, SchematicId};
use librepcb_core::types::{Angle, CircuitIdentifier, ElementName, Layer, Length, PositiveLength};
use librepcb_editor::ProjectEditor;
use librepcb_editor::commands::*;

use crate::helpers::{create_editor, diff, lib, mm, open_project, run_cli_erc, snapshot};

struct Demo {
    schematic: SchematicId,
    board: BoardId,
}

fn net(p: &Project, name: &str) -> NetSignalId {
    p.circuit()
        .net_signal_by_name(name)
        .unwrap_or_else(|| panic!("net {name} not found"))
        .0
}

fn pin_net(p: &Project, component: &str, signal: &str) -> Option<NetSignalId> {
    let (_, c) = p.circuit().component_instance_by_name(component).unwrap();
    let lib = p.library().component(&c.lib_component()).unwrap();
    let signal = lib.signals().by_name(signal, true).unwrap().uuid();
    c.signal(&signal).unwrap().net()
}

fn name(s: &str) -> ElementName {
    ElementName::new(s).unwrap()
}

fn ident(s: &str) -> CircuitIdentifier {
    CircuitIdentifier::new(s).unwrap()
}

/// Builds the demo project: R1 and R2 in series between VCC and GND with
/// the middle net "MID", C1 between VCC and GND, all placed and routed.
fn build_demo(editor: &mut ProjectEditor) -> Demo {
    let schematic = editor
        .execute(AddSchematic {
            name: name("Main"),
            index: None,
        })
        .unwrap();
    let board = editor.execute(AddBoard::new(name("default"))).unwrap();
    assert!(board.outline.is_some());
    let board = board.board;

    // Components.
    let place = |x, y, rotation| SymbolPlacement {
        schematic: Some(schematic),
        position: mm(x, y),
        rotation,
        mirrored: false,
        gate_offset: None,
    };
    let r1 = editor
        .execute(AddComponent {
            device: Some(lib::r0805()),
            place: Some(place(20.32, 50.8, Angle::DEG0)),
            ..AddComponent::new(lib::resistor())
        })
        .unwrap();
    assert_eq!(r1.name, "R1");
    assert_eq!(r1.symbols.len(), 1);
    // Component + symbol of the resistor.
    assert_eq!(r1.library_elements.len(), 2);
    let r2 = editor
        .execute(AddComponent {
            device: Some(lib::r0805()),
            value: Some("10k".into()),
            place: Some(place(40.64, 50.8, Angle::DEG0)),
            ..AddComponent::new(lib::resistor())
        })
        .unwrap();
    assert_eq!(r2.name, "R2");
    assert!(r2.library_elements.is_empty());
    let c1 = editor
        .execute(AddComponent {
            device: Some(lib::c0805()),
            place: Some(place(60.96, 40.64, Angle::DEG90)),
            ..AddComponent::new(lib::capacitor())
        })
        .unwrap();
    assert_eq!(c1.name, "C1");

    // Wires.
    let mid = editor
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
        let result = editor
            .execute(ConnectPinToNet {
                pin: PinRef::new(component, pin),
                net: ident(net),
                label: true,
                stub_length: None,
            })
            .unwrap();
        assert!(result.label.is_some());
        assert!(result.wire.is_some());
    }
    // A T-junction on the MID wire, ending in a labeled test point.
    let tap = editor
        .execute(DrawWire {
            schematic: None,
            start: WireAnchor::Line {
                segment: mid.segment,
                line: mid.lines[0],
                position: mm(30.0, 50.8),
            },
            end: WireAnchor::Point(mm(30.48, 60.96)),
            points: vec![],
            net: None,
        })
        .unwrap();
    assert_eq!(tap.segment, mid.segment);
    editor
        .execute(AddNetLabel {
            segment: mid.segment,
            position: mm(30.48, 60.96),
            rotation: Angle::DEG0,
            mirrored: false,
        })
        .unwrap();

    let p = editor.project();
    assert_eq!(pin_net(p, "R1", "2"), Some(net(p, "MID")));
    assert_eq!(pin_net(p, "R2", "1"), Some(net(p, "MID")));
    assert_eq!(pin_net(p, "R1", "1"), Some(net(p, "VCC")));
    assert_eq!(pin_net(p, "C1", "1"), Some(net(p, "VCC")));
    assert_eq!(pin_net(p, "R2", "2"), Some(net(p, "GND")));
    assert_eq!(pin_net(p, "C1", "2"), Some(net(p, "GND")));
    assert_eq!(p.circuit().net_signals().len(), 3);
    assert!(p.is_ref_index_consistent());

    // Board.
    for (component, x, rotation) in [("R1", 10.0, Angle::DEG0), ("R2", 20.0, Angle::DEG0)] {
        editor
            .execute(AddDevice {
                component: component.into(),
                board: Some(board),
                device: None,
                footprint: None,
                position: mm(x, 10.0),
                rotation,
                mirrored: false,
            })
            .unwrap();
    }
    let c1_device = editor
        .execute(AddDevice {
            component: "C1".into(),
            board: None,
            device: None,
            footprint: None,
            position: mm(30.0, 10.0),
            rotation: Angle::DEG0,
            mirrored: false,
        })
        .unwrap();
    assert_eq!(c1_device.device, lib::c0805());
    assert_eq!(c1_device.library_elements.len(), 2); // device + package
    editor
        .execute(SetBoardOutline::rect(
            Some(board),
            mm(0.0, 0.0),
            mm(40.0, 20.0),
        ))
        .unwrap();

    let trace = editor
        .execute(AddTrace {
            board: Some(board),
            start: TraceEndpoint::Pad(PadRef::new("R1", "2")),
            end: TraceEndpoint::Pad(PadRef::new("R2", "1")),
            points: vec![],
            layer: None,
            width: None,
            net: None,
        })
        .unwrap();
    assert_eq!(trace.layer, Layer::TOP_COPPER);
    assert_eq!(trace.net, Some(net(editor.project(), "MID")));
    editor
        .execute(AddTrace {
            board: Some(board),
            start: TraceEndpoint::Pad(PadRef::new("R1", "1")),
            end: TraceEndpoint::Pad(PadRef::new("C1", "1")),
            points: vec![mm(10.0, 15.0), mm(30.0, 15.0)],
            layer: None,
            width: Some(PositiveLength::new(Length::new(500_000)).unwrap()),
            net: None,
        })
        .unwrap();
    let via = editor
        .execute(AddVia {
            board: Some(board),
            position: mm(35.0, 5.0),
            net: Some("GND".into()),
            start_layer: None,
            end_layer: None,
            drill_diameter: None,
            size: None,
            exposure: None,
        })
        .unwrap();
    editor
        .execute(AddTrace {
            board: Some(board),
            start: TraceEndpoint::Pad(PadRef::new("C1", "2")),
            end: TraceEndpoint::Via(via.via),
            points: vec![],
            layer: None,
            width: None,
            net: None,
        })
        .unwrap();
    // A dangling trace from R2 pad 2, then connected to the via: the two
    // segments are combined.
    let dangling = editor
        .execute(AddTrace {
            board: Some(board),
            start: TraceEndpoint::Pad(PadRef::new("R2", "2")),
            end: TraceEndpoint::Point(mm(25.0, 4.0)),
            points: vec![],
            layer: None,
            width: None,
            net: None,
        })
        .unwrap();
    assert_eq!(dangling.net, Some(net(editor.project(), "GND")));
    assert_eq!(
        editor.project().board(board).unwrap().net_segments().len(),
        4
    );
    let combined = editor
        .execute(AddTrace {
            board: Some(board),
            start: TraceEndpoint::Junction(dangling.junctions[0]),
            end: TraceEndpoint::Via(via.via),
            points: vec![],
            layer: None,
            width: None,
            net: None,
        })
        .unwrap();
    assert_eq!(combined.segment, via.segment);
    // Connecting different nets is rejected.
    assert!(
        editor
            .execute(AddTrace {
                board: Some(board),
                start: TraceEndpoint::Pad(PadRef::new("R1", "1")),
                end: TraceEndpoint::Pad(PadRef::new("R2", "2")),
                points: vec![],
                layer: None,
                width: None,
                net: None,
            })
            .is_err()
    );
    editor
        .execute(AddPlane {
            board: Some(board),
            net: Some("GND".into()),
            layer: Some(Layer::BOT_COPPER),
            outline: None,
            settings: PlaneSettings::default(),
        })
        .unwrap();
    editor
        .execute(AddHole {
            board: Some(board),
            position: mm(37.0, 17.0),
            diameter: PositiveLength::new(Length::new(3_200_000)).unwrap(),
            stop_mask: None,
        })
        .unwrap();
    editor
        .execute(AddStrokeText {
            board: Some(board),
            layer: Layer::TOP_LEGEND,
            text: "{{PROJECT}}".into(),
            position: mm(2.0, 17.0),
            rotation: Angle::DEG0,
            height: None,
            stroke_width: None,
            align: None,
        })
        .unwrap();
    editor
        .execute(EditProjectMetadata {
            name: Some(name("Editor Demo")),
            author: Some("librepcb-editor tests".into()),
            ..EditProjectMetadata::default()
        })
        .unwrap();
    let p = editor.project();
    assert!(p.is_ref_index_consistent());
    let b = p.board(board).unwrap();
    assert_eq!(b.devices().len(), 3);
    assert_eq!(b.planes().len(), 1);
    // MID, VCC and GND (the via segment combined with both GND traces).
    assert_eq!(b.net_segments().len(), 3);
    Demo { schematic, board }
}

fn undo_all(editor: &mut ProjectEditor) {
    while editor.undo().unwrap() {}
}

fn redo_all(editor: &mut ProjectEditor) {
    while editor.redo().unwrap() {}
}

#[test]
fn test_build_save_reopen_undo_redo() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("project");
    let mut editor = create_editor(&dir);
    editor.save().unwrap();
    let empty = snapshot(&dir);

    build_demo(&mut editor);
    editor.save().unwrap();
    assert!(editor.is_clean());
    let full = snapshot(&dir);
    assert!(full.keys().any(|f| f.starts_with("library/cmp/")));
    assert!(full.contains_key("schematics/main/schematic.lp"));
    assert!(full.contains_key("boards/default/board.lp"));

    // Re-open with our loader: same content, consistent index, and saving
    // writes identical files.
    let mut reopened = open_project(&dir);
    assert!(reopened.is_ref_index_consistent());
    assert_eq!(reopened.circuit(), editor.project().circuit());
    assert_eq!(reopened.schematics(), editor.project().schematics());
    assert_eq!(reopened.library().devices().len(), 2);
    reopened.save().unwrap();
    let modified = reopened
        .directory()
        .file_system()
        .check_for_modifications()
        .unwrap();
    assert!(modified.is_empty(), "modified on re-save: {modified:?}");

    // For manual inspection: LIBREPCB_EDITOR_TEST_OUTPUT=<dir>.
    if let Some(out) = std::env::var_os("LIBREPCB_EDITOR_TEST_OUTPUT") {
        crate::helpers::copy_dir(&dir, std::path::Path::new(&out));
    }

    // The official CLI opens the project without ERC messages.
    match run_cli_erc(&dir) {
        Some((ok, output)) => assert!(ok, "librepcb-cli failed:\n{output}"),
        None => eprintln!("librepcb-cli not found, skipping the upstream check"),
    }

    // Undo everything: identical to the empty project.
    undo_all(&mut editor);
    assert!(editor.project().is_ref_index_consistent());
    editor.save().unwrap();
    let after_undo = snapshot(&dir);
    assert!(after_undo == empty, "{}", diff(&empty, &after_undo));

    // Redo everything: identical to the full project.
    redo_all(&mut editor);
    assert!(editor.project().is_ref_index_consistent());
    editor.save().unwrap();
    let after_redo = snapshot(&dir);
    assert!(after_redo == full, "{}", diff(&full, &after_redo));
}

/// Copying a board (upstream `Board::copyFrom()`) copies its settings and
/// all items; the project stays valid for the official CLI.
#[test]
fn test_copy_board() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("project");
    let mut editor = create_editor(&dir);
    let demo = build_demo(&mut editor);
    let copy = editor
        .execute(AddBoard {
            name: name("copy"),
            copy_from: Some(demo.board),
            default_outline: true,
        })
        .unwrap()
        .board;
    let p = editor.project();
    assert!(p.is_ref_index_consistent());
    let (a, b) = (p.board(demo.board).unwrap(), p.board(copy).unwrap());
    assert_eq!(a.settings(), b.settings());
    assert_eq!(a.devices(), b.devices());
    assert_eq!(a.net_segments().len(), b.net_segments().len());
    let count = |board: &librepcb_core::project::board::Board| {
        board
            .net_segments()
            .values()
            .map(|s| {
                (
                    s.net(),
                    s.vias().len(),
                    s.junctions().len(),
                    s.traces().len(),
                )
            })
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(count(a), count(b));
    assert_eq!(a.planes().len(), b.planes().len());
    assert_eq!(a.polygons().len(), b.polygons().len());
    assert_eq!(a.stroke_texts().len(), b.stroke_texts().len());
    assert_eq!(a.holes().len(), b.holes().len());
    // New UUIDs for everything but the devices.
    assert!(
        a.polygons()
            .keys()
            .all(|uuid| !b.polygons().contains_key(uuid))
    );
    editor.save().unwrap();
    match run_cli_erc(&dir) {
        Some((ok, output)) => assert!(ok, "librepcb-cli failed:\n{output}"),
        None => eprintln!("librepcb-cli not found, skipping the upstream check"),
    }
    // Undone in one step.
    editor.undo().unwrap();
    assert!(editor.project().board(copy).is_none());
}

#[test]
fn test_remove_and_modify() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("project");
    let mut editor = create_editor(&dir);
    let demo = build_demo(&mut editor);
    editor.save().unwrap();
    let full = snapshot(&dir);
    let history_len = editor.undo_stack().history().len();

    // Move a symbol and a device (texts follow, wires/traces stay
    // attached).
    let (_, r1) = editor
        .project()
        .circuit()
        .component_instance_by_name("R1")
        .unwrap();
    let r1_id = r1.id();
    let symbol = editor
        .project()
        .component_uses(r1_id)
        .unwrap()
        .symbols
        .values()
        .next()
        .unwrap()
        .1;
    editor
        .execute(MoveSymbol {
            symbol,
            position: Some(mm(15.24, 55.88)),
            rotation: Some(Angle::DEG90),
            mirrored: Some(true),
        })
        .unwrap();
    editor
        .execute(MoveDevice {
            component: "R1".into(),
            board: Some(demo.board),
            position: Some(mm(12.0, 8.0)),
            rotation: Some(Angle::DEG180),
            mirrored: None,
            locked: Some(true),
        })
        .unwrap();
    assert!(editor.project().is_ref_index_consistent());

    // Rename a net, merge nets.
    editor
        .execute(EditNet {
            net: "MID".into(),
            name: Some(ident("DIVIDER")),
            net_class: None,
            merge: false,
        })
        .unwrap();
    assert!(
        editor
            .project()
            .circuit()
            .net_signal_by_name("DIVIDER")
            .is_some()
    );

    // Replace the device of R1 (its traces are removed).
    let replaced = editor
        .execute(ReplaceDevice {
            component: "R1".into(),
            board: Some(demo.board),
            device: lib::r0603(),
            footprint: None,
        })
        .unwrap();
    assert_eq!(replaced.device, lib::r0603());
    let p = editor.project();
    let b = p.board(demo.board).unwrap();
    assert_eq!(b.device(r1_id).unwrap().lib_device(), lib::r0603());
    assert!(!b.is_device_used(r1_id));
    // The unused assembly option of R-0805 was removed.
    let r1 = p.circuit().component_instance(r1_id).unwrap();
    assert_eq!(
        r1.compatible_devices().into_iter().collect::<Vec<_>>(),
        vec![lib::r0603()]
    );
    assert!(p.is_ref_index_consistent());

    // Remove board items: the via (traces keep a junction) and the plane.
    let p = editor.project();
    let b = p.board(demo.board).unwrap();
    let (segment, via) = b
        .net_segments()
        .values()
        .find_map(|s| s.vias().keys().next().map(|v| (s.id(), *v)))
        .unwrap();
    let plane = *b.planes().keys().next().unwrap();
    editor
        .execute(RemoveBoardItems {
            board: Some(demo.board),
            selection: BoardSelection {
                vias: vec![(segment, via)],
                planes: vec![plane],
                ..BoardSelection::default()
            },
        })
        .unwrap();
    assert!(editor.project().is_ref_index_consistent());

    // Remove a wire: the pin loses its net.
    let p = editor.project();
    let segment_of_r2_pin = {
        let s = p.schematic(demo.schematic).unwrap();
        let (_, r2) = p.circuit().component_instance_by_name("R2").unwrap();
        let (_, sym) = *p
            .component_uses(r2.id())
            .unwrap()
            .symbols
            .values()
            .next()
            .unwrap();
        let pins = s.symbols()[&sym].pins(p.view()).unwrap();
        let pin = pins
            .iter()
            .find(|v| v.lib_pin().name().as_str() == "2")
            .unwrap();
        let segment = s.pin_net_segment(sym, pin.uuid()).unwrap();
        let line = s.net_segments()[&segment]
            .lines_at(NetLineAnchor::Pin {
                symbol: sym.0,
                pin: pin.uuid(),
            })
            .next()
            .unwrap()
            .uuid();
        (segment, line)
    };
    editor
        .execute(RemoveSchematicItems {
            schematic: demo.schematic,
            selection: SchematicSelection {
                net_lines: vec![segment_of_r2_pin],
                ..SchematicSelection::default()
            },
        })
        .unwrap();
    let p = editor.project();
    assert_eq!(pin_net(p, "R2", "2"), None);
    // The board trace at R2 pad 2 was removed with the connection.
    assert!(p.is_ref_index_consistent());

    // Remove a component with its symbol, device and wires.
    editor
        .execute(RemoveComponent {
            component: "C1".into(),
        })
        .unwrap();
    let p = editor.project();
    assert!(p.circuit().component_instance_by_name("C1").is_none());
    assert!(p.board(demo.board).unwrap().devices().len() == 2);
    assert!(
        !p.library()
            .contains(LibraryElementKind::Component, &lib::capacitor())
    );
    assert!(p.is_ref_index_consistent());

    // Remove the rest.
    for component in ["R1", "R2"] {
        editor
            .execute(RemoveComponent {
                component: component.into(),
            })
            .unwrap();
    }
    let p = editor.project();
    assert!(p.circuit().component_instances().is_empty());
    // Only the labeled dangling tap wire (and its net) remains, like
    // upstream.
    let nets: Vec<_> = p
        .circuit()
        .net_signals()
        .values()
        .map(|n| n.name().to_string())
        .collect();
    assert_eq!(nets, vec!["DIVIDER".to_owned()]);
    assert!(p.library().components().is_empty());
    assert!(p.library().symbols().is_empty());
    assert!(p.is_ref_index_consistent());

    // Undo all of that: identical to the built project.
    while editor
        .undo_stack()
        .history()
        .iter()
        .filter(|h| h.done)
        .count()
        > history_len
    {
        editor.undo().unwrap();
    }
    editor.save().unwrap();
    let restored = snapshot(&dir);
    assert!(restored == full, "{}", diff(&full, &restored));
}

#[test]
fn test_combine_segments_and_nets() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("project");
    let mut editor = create_editor(&dir);
    let schematic = editor
        .execute(AddSchematic {
            name: name("Main"),
            index: None,
        })
        .unwrap();
    for x in [0.0, 20.32, 40.64] {
        editor
            .execute(AddComponent {
                place: Some(SymbolPlacement {
                    schematic: Some(schematic),
                    position: mm(x, 0.0),
                    rotation: Angle::DEG0,
                    mirrored: false,
                    gate_offset: None,
                }),
                ..AddComponent::new(lib::resistor())
            })
            .unwrap();
    }
    let n1 = editor
        .execute(ConnectPins {
            a: PinRef::new("R1", "2"),
            b: PinRef::new("R2", "1"),
            mode: WireMode::default(),
            net: None,
        })
        .unwrap();
    let p = editor.project();
    assert_eq!(
        p.circuit().net_signal(n1.net).unwrap().name().as_str(),
        "N1"
    );
    assert!(p.circuit().net_signal(n1.net).unwrap().has_auto_name());
    let vcc = editor
        .execute(ConnectPinToNet {
            pin: PinRef::new("R3", "1"),
            net: ident("VCC"),
            label: true,
            stub_length: None,
        })
        .unwrap();
    let stub_end = vcc.wire.unwrap().junctions[0];
    let state_before = editor.undo_stack().state_id();

    // Connect both segments: the named net wins, the auto net is removed.
    let wire = editor
        .execute(DrawWire {
            schematic: None,
            start: WireAnchor::Line {
                segment: n1.segment,
                line: n1.lines[0],
                position: mm(12.0, 0.0),
            },
            end: WireAnchor::Junction {
                segment: vcc.segment,
                junction: stub_end,
            },
            points: vec![mm(12.0, 5.08)],
            net: None,
        })
        .unwrap();
    let p = editor.project();
    assert_eq!(wire.segment, vcc.segment);
    assert_eq!(wire.net, net(p, "VCC"));
    assert!(p.circuit().net_signal_by_name("N1").is_none());
    for (component, pin) in [("R1", "2"), ("R2", "1"), ("R3", "1")] {
        assert_eq!(pin_net(p, component, pin), Some(net(p, "VCC")));
    }
    assert_eq!(p.schematic(schematic).unwrap().net_segments().len(), 1);
    assert!(p.is_ref_index_consistent());

    // Merge into another net by renaming.
    editor
        .execute(ConnectPinToNet {
            pin: PinRef::new("R3", "2"),
            net: ident("GND"),
            label: true,
            stub_length: None,
        })
        .unwrap();
    let merged = editor
        .execute(EditNet {
            net: "VCC".into(),
            name: Some(ident("GND")),
            net_class: None,
            merge: true,
        })
        .unwrap();
    let p = editor.project();
    assert_eq!(merged, net(p, "GND"));
    assert!(p.circuit().net_signal_by_name("VCC").is_none());
    assert_eq!(pin_net(p, "R1", "2"), Some(merged));
    assert!(p.is_ref_index_consistent());

    // Undo back to before the combination.
    for _ in 0..3 {
        editor.undo().unwrap();
    }
    assert_eq!(editor.undo_stack().state_id(), state_before);
    let p = editor.project();
    assert_eq!(pin_net(p, "R1", "2"), Some(net(p, "N1")));
    assert_eq!(pin_net(p, "R3", "1"), Some(net(p, "VCC")));
    assert!(p.is_ref_index_consistent());
}

/// The automatic plane outline encloses the bounding rectangle of the board
/// outline including its arcs (upstream `Board::calculateBoundingRect()`).
#[test]
fn test_auto_plane_outline_includes_arcs() {
    let tmp = tempfile::tempdir().unwrap();
    let mut editor = create_editor(&tmp.path().join("p"));
    let board = editor
        .execute(AddBoard {
            name: name("default"),
            copy_from: None,
            default_outline: false,
        })
        .unwrap()
        .board;
    let diameter = PositiveLength::new(Length::new(20_000_000)).unwrap();
    editor
        .execute(AddBoardPolygon {
            board: Some(board),
            layer: Layer::BOARD_OUTLINES,
            path: librepcb_core::geometry::Path::circle(diameter).translated(mm(50.0, 50.0)),
            line_width: None,
            filled: false,
            grab_area: false,
        })
        .unwrap();
    let plane = editor
        .execute(AddPlane {
            board: Some(board),
            net: None,
            layer: None,
            outline: None,
            settings: PlaneSettings::default(),
        })
        .unwrap();
    let outline = editor
        .project()
        .board(board)
        .unwrap()
        .planes()
        .get(&plane)
        .unwrap()
        .outline()
        .clone();
    let ys: Vec<Length> = outline.vertices().iter().map(|v| v.pos.y).collect();
    let xs: Vec<Length> = outline.vertices().iter().map(|v| v.pos.x).collect();
    // The circle spans 40..60 mm in both directions.
    assert!(*ys.iter().min().unwrap() < Length::new(40_000_000));
    assert!(*ys.iter().max().unwrap() > Length::new(60_000_000));
    assert!(*xs.iter().min().unwrap() < Length::new(40_000_000));
    assert!(*xs.iter().max().unwrap() > Length::new(60_000_000));

    // Without board outline, the automatic outline is refused.
    let empty = editor
        .execute(AddBoard {
            name: name("empty"),
            copy_from: None,
            default_outline: false,
        })
        .unwrap()
        .board;
    assert!(
        editor
            .execute(AddPlane {
                board: Some(empty),
                net: None,
                layer: None,
                outline: None,
                settings: PlaneSettings::default(),
            })
            .is_err()
    );
}
