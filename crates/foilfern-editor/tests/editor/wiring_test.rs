//! Tests of the agent oriented commands: net-level wiring
//! (`ConnectNet`, `DisconnectSignals`) and automatic placement
//! (`AutoPlaceSymbols`, `AutoPlaceDevices`).

use foilfern_core::project::erc::run_erc;
use foilfern_core::project::{ComponentSignalRef, Project};
use foilfern_core::rule_check::Severity;
use foilfern_core::types::{CircuitIdentifier, ElementName, Point, Uuid};
use foilfern_editor::ProjectEditor;
use foilfern_editor::commands::*;

use crate::helpers::{create_editor, lib, mm};

fn signal(p: &Project, component: &str, name: &str) -> ComponentSignalRef {
    let (id, c) = p.circuit().component_instance_by_name(component).unwrap();
    let lib = p.library().component(&c.lib_component()).unwrap();
    ComponentSignalRef {
        component: id,
        signal: lib.signals().by_name(name, true).unwrap().uuid(),
    }
}

fn net_of(p: &Project, s: ComponentSignalRef) -> Option<String> {
    let net = p
        .circuit()
        .component_instance(s.component)?
        .signal(&s.signal)?
        .net()?;
    Some(p.circuit().net_signal(net)?.name().to_string())
}

fn add(editor: &mut ProjectEditor, component: Uuid, device: Option<Uuid>) -> String {
    let added = editor
        .execute(AddComponent {
            device,
            place: Some(SymbolPlacement {
                schematic: None,
                position: Point::ORIGIN,
                rotation: Default::default(),
                mirrored: false,
                gate_offset: None,
            }),
            ..AddComponent::new(component)
        })
        .unwrap();
    editor
        .execute(AutoPlaceSymbols {
            symbols: added.symbols,
            margin: None,
        })
        .unwrap();
    added.name
}

#[test]
fn test_connect_net_disconnect_and_auto_place() {
    let tmp = tempfile::tempdir().unwrap();
    let mut editor = create_editor(tmp.path());
    editor
        .execute(AddSchematic {
            name: ElementName::new("Main").unwrap(),
            index: None,
        })
        .unwrap();
    let r1 = add(&mut editor, lib::resistor(), Some(lib::r0805()));
    let c1 = add(&mut editor, lib::capacitor(), Some(lib::c0805()));
    let c2 = add(&mut editor, lib::capacitor(), Some(lib::c0805()));
    let vcc = add(&mut editor, lib::supply_vcc(), None);
    let gnd = add(&mut editor, lib::supply_gnd(), None);

    // Automatic placement: no two symbols at the same place.
    let positions: Vec<Point> = editor.project().schematics()[0]
        .symbols()
        .values()
        .map(|s| s.position())
        .collect();
    for (i, a) in positions.iter().enumerate() {
        for b in &positions[i + 1..] {
            assert_ne!(a, b);
        }
    }

    let p = editor.project();
    let connect = |editor: &mut ProjectEditor, net: Option<&str>, pins: Vec<ComponentSignalRef>| {
        editor
            .execute(ConnectNet {
                net: net.map(|n| CircuitIdentifier::new(n).unwrap()),
                signals: pins,
                max_wire_length: None,
            })
            .unwrap()
    };
    let (r1_1, r1_2) = (signal(p, &r1, "1"), signal(p, &r1, "2"));
    let (c1_1, c1_2) = (signal(p, &c1, "1"), signal(p, &c1, "2"));
    let (c2_1, c2_2) = (signal(p, &c2, "1"), signal(p, &c2, "2"));
    let (vcc_net, gnd_net) = (signal(p, &vcc, "Net"), signal(p, &gnd, "Net"));

    // The forced net name wins over the requested one.
    let r = connect(&mut editor, Some("POWER"), vec![r1_1, c2_1, vcc_net]);
    assert_eq!(r.name, "VCC");
    assert_eq!(r.warnings.len(), 1);
    let r = connect(&mut editor, Some("OUT"), vec![r1_2, c1_1]);
    assert_eq!(r.name, "OUT");
    let r = connect(&mut editor, None, vec![c1_2, c2_2, gnd_net]);
    assert_eq!(r.name, "GND");
    let p = editor.project();
    assert_eq!(net_of(p, c1_2).as_deref(), Some("GND"));
    assert_eq!(p.circuit().net_signals().len(), 3);
    let erc = run_erc(p);
    assert!(
        erc.iter().all(|m| m.message().severity() == Severity::Hint),
        "{:?}",
        erc.iter()
            .map(|m| m.message().message())
            .collect::<Vec<_>>()
    );

    // Connecting pins of different nets merges them.
    let r = connect(&mut editor, Some("OUT"), vec![c1_1, c2_1]);
    assert_eq!(r.merged_nets, ["VCC"]);
    assert_eq!(net_of(editor.project(), vcc_net).as_deref(), Some("OUT"));
    editor.undo().unwrap();
    assert_eq!(net_of(editor.project(), vcc_net).as_deref(), Some("VCC"));

    // Disconnecting one end of a direct wire keeps the other end on the
    // net (with a label), and is undoable.
    editor
        .execute(DisconnectSignals {
            signals: vec![r1_2],
        })
        .unwrap();
    let p = editor.project();
    assert_eq!(net_of(p, r1_2), None);
    assert_eq!(net_of(p, c1_1).as_deref(), Some("OUT"));
    editor.undo().unwrap();
    assert_eq!(net_of(editor.project(), r1_2).as_deref(), Some("OUT"));

    // Devices: packed inside the outline without overlap, supply symbols
    // (schematic-only) skipped.
    let board = editor
        .execute(AddBoard::new(ElementName::new("default").unwrap()))
        .unwrap()
        .board;
    editor
        .execute(SetBoardOutline::rect(
            Some(board),
            mm(0.0, 0.0),
            mm(20.0, 15.0),
        ))
        .unwrap();
    let placed = editor.execute(AutoPlaceDevices::default()).unwrap();
    assert_eq!(placed.placed.len(), 3, "{placed:?}");
    assert!(placed.outside.is_empty());
    assert_eq!(placed.skipped.len(), 2);
    let p = editor.project();
    let b = p.board(board).unwrap();
    let rects: Vec<Rect> = b
        .devices()
        .values()
        .map(|d| device_rect(p, d).unwrap())
        .collect();
    let outline = Rect {
        min: mm(0.0, 0.0),
        max: mm(20.0, 15.0),
    };
    for (i, r) in rects.iter().enumerate() {
        assert!(outline.contains(r), "{r:?}");
        for other in &rects[i + 1..] {
            assert!(!r.intersects(other), "{r:?} {other:?}");
        }
    }
    // Too small: devices which do not fit are reported.
    editor.undo().unwrap();
    editor
        .execute(SetBoardOutline::rect(
            Some(board),
            mm(0.0, 0.0),
            mm(4.0, 4.0),
        ))
        .unwrap();
    let placed = editor.execute(AutoPlaceDevices::default()).unwrap();
    assert!(!placed.outside.is_empty());
}

/// The bounding rectangles of the pins of all symbols of the first page.
fn schematic_rects(p: &Project) -> Vec<(String, Rect)> {
    let s = &p.schematics()[0];
    let mut rects = Vec::new();
    for sym in s.symbols().values() {
        let pins: Vec<Point> = sym
            .pins(p.view())
            .unwrap()
            .iter()
            .map(|pin| pin.position())
            .collect();
        let name = p
            .circuit()
            .component_instance(sym.component())
            .unwrap()
            .name()
            .to_string();
        rects.push((name, Rect::bounding(pins).unwrap()));
    }
    rects
}

/// A small circuit with supply symbols: R1 and C1 in series between VCC
/// and GND, C2 parallel to C1, plus a second resistor R2 to OUT.
fn build_rc(editor: &mut ProjectEditor) -> Vec<ComponentSignalRef> {
    editor
        .execute(AddSchematic {
            name: ElementName::new("Main").unwrap(),
            index: None,
        })
        .unwrap();
    let r1 = add(editor, lib::resistor(), Some(lib::r0805()));
    let r2 = add(editor, lib::resistor(), Some(lib::r0805()));
    let c1 = add(editor, lib::capacitor(), Some(lib::c0805()));
    let c2 = add(editor, lib::capacitor(), Some(lib::c0805()));
    add(editor, lib::supply_vcc(), None);
    add(editor, lib::supply_gnd(), None);
    let p = editor.project();
    let pins = vec![
        signal(p, &r1, "1"),
        signal(p, &r1, "2"),
        signal(p, &c1, "1"),
        signal(p, &c1, "2"),
        signal(p, &c2, "1"),
        signal(p, &c2, "2"),
        signal(p, &r2, "1"),
        signal(p, &r2, "2"),
    ];
    for (net, signals) in [
        ("VCC", vec![pins[0]]),
        ("MID", vec![pins[1], pins[2], pins[4], pins[6]]),
        ("GND", vec![pins[3], pins[5]]),
        ("OUT", vec![pins[7]]),
    ] {
        editor
            .execute(ConnectNet {
                net: Some(CircuitIdentifier::new(net).unwrap()),
                signals,
                max_wire_length: None,
            })
            .unwrap();
    }
    pins
}

#[test]
fn test_connect_attaches_supply_symbols() {
    let tmp = tempfile::tempdir().unwrap();
    let mut editor = create_editor(tmp.path());
    editor
        .execute(AddSchematic {
            name: ElementName::new("Main").unwrap(),
            index: None,
        })
        .unwrap();
    let r1 = add(&mut editor, lib::resistor(), Some(lib::r0805()));
    let gnd = add(&mut editor, lib::supply_gnd(), None);
    let gnd2 = add(&mut editor, lib::supply_gnd(), None);
    let vcc = add(&mut editor, lib::supply_vcc(), None);
    let p = editor.project();
    let (r1_1, r1_2) = (signal(p, &r1, "1"), signal(p, &r1, "2"));
    let (gnd_net, gnd2_net, vcc_net) = (
        signal(p, &gnd, "Net"),
        signal(p, &gnd2, "Net"),
        signal(p, &vcc, "Net"),
    );
    // Connecting net "GND" attaches both unconnected GND symbols, next to
    // the resistor pin; the VCC symbol stays unconnected.
    let r = editor
        .execute(ConnectNet {
            net: Some(CircuitIdentifier::new("GND").unwrap()),
            signals: vec![r1_1],
            max_wire_length: None,
        })
        .unwrap();
    assert_eq!(r.attached_supplies, [gnd.clone(), gnd2.clone()]);
    let p = editor.project();
    assert_eq!(net_of(p, gnd_net).as_deref(), Some("GND"));
    assert_eq!(net_of(p, gnd2_net).as_deref(), Some("GND"));
    assert_eq!(net_of(p, vcc_net), None);
    // Supply symbols already on the net are not attached again.
    let r = editor
        .execute(ConnectNet {
            net: Some(CircuitIdentifier::new("VCC").unwrap()),
            signals: vec![r1_2],
            max_wire_length: None,
        })
        .unwrap();
    assert_eq!(r.attached_supplies, [vcc.as_str()]);
    // No unconnected pins (single-pin nets are warnings of this test).
    let erc = run_erc(editor.project());
    assert!(
        erc.iter().all(|m| m.message().severity() != Severity::Error
            && !m.message().message().contains("Unconnected")),
        "{:?}",
        erc.iter()
            .map(|m| m.message().message())
            .collect::<Vec<_>>()
    );
    // The supply symbol went next to the resistor pin (5.08 mm in front).
    let p = editor.project();
    let s = &p.schematics()[0];
    let pin_pos = |component: &str| {
        let (id, _) = p.circuit().component_instance_by_name(component).unwrap();
        s.symbols()
            .values()
            .find(|sym| sym.component() == id)
            .unwrap()
            .pins(p.view())
            .unwrap()[0]
            .position()
    };
    let r1_pin = s
        .symbols()
        .values()
        .find(|sym| sym.component() == r1_1.component)
        .unwrap()
        .pins(p.view())
        .unwrap()
        .iter()
        .find(|pin| pin.signal() == r1_2)
        .unwrap()
        .position();
    let vcc_pin = pin_pos(&vcc);
    let d = (vcc_pin.x - r1_pin.x).abs() + (vcc_pin.y - r1_pin.y).abs();
    assert!(d.to_mm() <= 5.08 + 1e-6, "{d:?}");
}

#[test]
fn test_labels_do_not_overlap_and_tidy() {
    let tmp = tempfile::tempdir().unwrap();
    let mut editor = create_editor(tmp.path());
    let pins = build_rc(&mut editor);

    // Net labels do not overlap each other or symbols of other components.
    let check_labels = |p: &Project| {
        let s = &p.schematics()[0];
        let mut labels: Vec<(Point, String)> = Vec::new();
        for seg in s.net_segments().values() {
            let name = p
                .circuit()
                .net_signal(seg.net())
                .unwrap()
                .name()
                .to_string();
            for l in seg.labels().values() {
                labels.push((l.position(), name.clone()));
            }
        }
        for (i, a) in labels.iter().enumerate() {
            for b in &labels[i + 1..] {
                assert_ne!(a.0, b.0, "labels {a:?} and {b:?} at the same place");
            }
        }
        labels.len()
    };
    let before = check_labels(editor.project());
    assert!(before > 0);
    let nets_before: Vec<Option<String>> =
        pins.iter().map(|s| net_of(editor.project(), *s)).collect();

    // Tidy: nets unchanged, supply symbols connected, ERC clean, symbols
    // apart, undoable.
    let r = editor.execute(TidySchematic::default()).unwrap();
    assert_eq!(r.placed, 4);
    assert_eq!(r.supplies, 2);
    assert_eq!(r.nets, 4);
    let p = editor.project();
    let nets_after: Vec<Option<String>> = pins.iter().map(|s| net_of(p, *s)).collect();
    assert_eq!(nets_before, nets_after);
    check_labels(p);
    let erc = run_erc(p);
    assert!(
        erc.iter().all(|m| m.message().severity() != Severity::Error
            && !m.message().message().contains("Unconnected")),
        "{:?}",
        erc.iter()
            .map(|m| m.message().message())
            .collect::<Vec<_>>()
    );
    let rects = schematic_rects(p);
    for (i, (na, a)) in rects.iter().enumerate() {
        for (nb, b) in &rects[i + 1..] {
            assert!(!a.intersects(b), "{na} {a:?} overlaps {nb} {b:?}");
        }
    }
    // Deterministic.
    let positions = |p: &Project| -> Vec<Point> {
        p.schematics()[0]
            .symbols()
            .values()
            .map(|s| s.position())
            .collect()
    };
    let first = positions(editor.project());
    editor.undo().unwrap();
    assert_eq!(
        nets_before,
        pins.iter()
            .map(|s| net_of(editor.project(), *s))
            .collect::<Vec<_>>()
    );
    editor.execute(TidySchematic::default()).unwrap();
    assert_eq!(first, positions(editor.project()));
}

#[test]
fn test_auto_place_devices_by_connectivity() {
    let tmp = tempfile::tempdir().unwrap();
    let mut editor = create_editor(tmp.path());
    build_rc(&mut editor);
    let board = editor
        .execute(AddBoard::new(ElementName::new("default").unwrap()))
        .unwrap()
        .board;
    editor
        .execute(SetBoardOutline::rect(
            Some(board),
            mm(0.0, 0.0),
            mm(60.0, 40.0),
        ))
        .unwrap();
    let placed = editor.execute(AutoPlaceDevices::default()).unwrap();
    assert_eq!(placed.placed.len(), 4, "{placed:?}");
    let p = editor.project();
    let b = p.board(board).unwrap();
    let rects: Vec<(String, Rect)> = b
        .devices()
        .values()
        .map(|d| {
            let name = p
                .circuit()
                .component_instance(d.component())
                .unwrap()
                .name()
                .to_string();
            (name, device_rect(p, d).unwrap())
        })
        .collect();
    let outline = Rect {
        min: mm(0.0, 0.0),
        max: mm(60.0, 40.0),
    };
    for (i, (_, r)) in rects.iter().enumerate() {
        assert!(outline.contains(r), "{r:?}");
        for (_, other) in &rects[i + 1..] {
            assert!(!r.intersects(other), "{r:?} {other:?}");
        }
    }
    // Connected devices (all share MID) are close together: the group is
    // much smaller than the board.
    let group = Rect::bounding(rects.iter().flat_map(|(_, r)| [r.min, r.max])).unwrap();
    assert!(group.width().to_mm() < 45.0, "{group:?}");
    assert!(group.height().to_mm() < 30.0, "{group:?}");
    // Deterministic.
    let first: Vec<Point> = placed.placed.iter().map(|(_, p)| *p).collect();
    editor.undo().unwrap();
    let again = editor.execute(AutoPlaceDevices::default()).unwrap();
    let second: Vec<Point> = again.placed.iter().map(|(_, p)| *p).collect();
    assert_eq!(first, second);
}

#[test]
fn test_automatic_wire_group_joins_a_three_pin_branch() {
    use foilfern_core::types::Angle;

    let tmp = tempfile::tempdir().unwrap();
    let mut editor = create_editor(tmp.path());
    editor
        .execute(AddSchematic {
            name: ElementName::new("Main").unwrap(),
            index: None,
        })
        .unwrap();
    for (position, rotation) in [
        (mm(0.0, 0.0), Angle::DEG0),
        (mm(30.48, 0.0), Angle::DEG0),
        (mm(15.24, -20.32), Angle::DEG270),
    ] {
        editor
            .execute(AddComponent {
                value: Some("1k".into()),
                device: Some(lib::r0805()),
                place: Some(SymbolPlacement {
                    schematic: None,
                    position,
                    rotation,
                    mirrored: false,
                    gate_offset: None,
                }),
                ..AddComponent::new(lib::resistor())
            })
            .unwrap();
    }
    // A field beside the lower branch extends above the pin. Its bounding
    // union with the symbol must not block the clear space above that pin.
    let schematic = editor.project().schematics()[0].id();
    let component = editor
        .project()
        .circuit()
        .component_instance_by_name("R3")
        .unwrap()
        .0;
    let symbol = editor.project().schematics()[0]
        .symbols()
        .values()
        .find(|symbol| symbol.component() == component)
        .unwrap();
    let symbol_id = symbol.id();
    let mut text = symbol.texts().values().next().unwrap().clone();
    text.set_position(mm(30.48, -12.7));
    text.set_rotation(Angle::DEG0);
    editor
        .apply_mutations(
            "Place field beside branch",
            vec![foilfern_core::project::Mutation::Schematic(
                foilfern_core::project::SchematicMutation::UpdateSymbolText {
                    symbol: foilfern_core::project::SymbolRef {
                        schematic,
                        symbol: symbol_id,
                    },
                    text,
                },
            )],
        )
        .unwrap();
    let pins = vec![
        signal(editor.project(), "R1", "2"),
        signal(editor.project(), "R2", "1"),
        signal(editor.project(), "R3", "1"),
    ];
    let net = editor
        .execute(AssignNet(ConnectNet {
            net: Some(CircuitIdentifier::new("BRANCH").unwrap()),
            signals: pins.clone(),
            max_wire_length: None,
        }))
        .unwrap()
        .net
        .unwrap();
    let schematic = editor.project().schematics()[0].id();
    editor
        .execute(WireGroup {
            schematic,
            net,
            pins: pins.clone(),
        })
        .unwrap();
    let sheet = &editor.project().schematics()[0];
    assert_eq!(sheet.net_segments().len(), 1);
    let segment = sheet.net_segments().values().next().unwrap();
    assert_eq!(segment.connected_pins().len(), 3);
    assert!(segment.is_cohesive());
    assert!(segment.junctions().values().any(|junction| {
        segment
            .lines()
            .values()
            .filter(|line| {
                [line.p1(), line.p2()].contains(&foilfern_core::geometry::NetLineAnchor::Junction(
                    junction.uuid(),
                ))
            })
            .count()
            == 3
    }));
    let revision = editor.project().revision();
    editor
        .execute(WireGroup {
            schematic,
            net,
            pins,
        })
        .unwrap();
    assert_eq!(editor.project().revision(), revision);
    editor.undo().unwrap();
    assert!(editor.project().schematics()[0].net_segments().is_empty());
    editor.redo().unwrap();
    assert_eq!(editor.project().schematics()[0].net_segments().len(), 1);
}

#[test]
fn test_tidy_preserves_complete_circuit_and_other_pages() {
    let tmp = tempfile::tempdir().unwrap();
    let mut editor = create_editor(tmp.path());
    build_rc(&mut editor);
    editor
        .execute(AddNet {
            name: Some(CircuitIdentifier::new("UNUSED").unwrap()),
            net_class: None,
        })
        .unwrap();
    let other = editor
        .execute(AddSchematic {
            name: ElementName::new("Other").unwrap(),
            index: None,
        })
        .unwrap();
    editor
        .execute(AddComponent {
            place: Some(SymbolPlacement {
                schematic: Some(other),
                position: mm(20.0, 20.0),
                rotation: Default::default(),
                mirrored: false,
                gate_offset: None,
            }),
            ..AddComponent::new(lib::supply_gnd())
        })
        .unwrap();
    let circuit = editor.project().circuit().clone();
    let other_page = editor.project().schematic(other).unwrap().clone();
    editor.execute(TidySchematic::default()).unwrap();
    assert_eq!(editor.project().circuit(), &circuit);
    assert_eq!(editor.project().schematic(other), Some(&other_page));
    let page = editor.project().schematics()[0].id();
    let scene = foilfern_scene::SchematicScene::build(
        editor.project(),
        page,
        &foilfern_scene::ColorScheme::SCHEMATIC_LIGHT,
    )
    .unwrap();
    assert!(foilfern_scene::annotation_overlaps(&scene, None).is_empty());
    editor.undo().unwrap();
    assert_eq!(editor.project().circuit(), &circuit);
    editor.redo().unwrap();
    assert_eq!(editor.project().circuit(), &circuit);
    assert_eq!(editor.project().schematic(other), Some(&other_page));
}

/// Rails joining a row of parallel pins, their junctions and the group
/// label stay on the page grid when the pins are on it.
#[test]
fn test_wire_group_rails_stay_on_the_page_grid() {
    let tmp = tempfile::tempdir().unwrap();
    let mut editor = create_editor(tmp.path());
    editor
        .execute(AddSchematic {
            name: ElementName::new("Main").unwrap(),
            index: None,
        })
        .unwrap();
    let mut names = Vec::new();
    for index in 0..4 {
        let added = editor
            .execute(AddComponent {
                value: Some("100nF".into()),
                device: Some(lib::c0805()),
                place: Some(SymbolPlacement {
                    schematic: None,
                    position: mm(12.7 * f64::from(index), 0.0),
                    rotation: Default::default(),
                    mirrored: false,
                    gate_offset: None,
                }),
                ..AddComponent::new(lib::capacitor())
            })
            .unwrap();
        names.push(added.name);
    }
    let schematic = editor.project().schematics()[0].id();
    for (net, pin) in [("VCC", "1"), ("GND", "2")] {
        let pins: Vec<_> = names
            .iter()
            .map(|name| signal(editor.project(), name, pin))
            .collect();
        let net = editor
            .execute(AssignNet(ConnectNet {
                net: Some(CircuitIdentifier::new(net).unwrap()),
                signals: pins.clone(),
                max_wire_length: None,
            }))
            .unwrap()
            .net
            .unwrap();
        editor
            .execute(WireGroup {
                schematic,
                net,
                pins,
            })
            .unwrap();
    }
    let sheet = &editor.project().schematics()[0];
    let grid = sheet.grid_interval();
    assert_eq!(sheet.net_segments().len(), 2);
    for segment in sheet.net_segments().values() {
        assert_eq!(segment.connected_pins().len(), 4);
        assert!(segment.is_cohesive());
        for junction in segment.junctions().values() {
            assert!(junction.position().is_on_grid(grid), "{junction:?}");
        }
        for label in segment.labels().values() {
            assert!(label.position().is_on_grid(grid), "{label:?}");
        }
    }
}

/// A field drawn over the plates of its own capacitor overlaps the symbol
/// artwork; the same field beside the symbol does not.
#[test]
fn test_review_reports_text_on_symbol_artwork() {
    use foilfern_scene::{Annotation, ColorScheme, SchematicScene, annotation_overlaps};

    let tmp = tempfile::tempdir().unwrap();
    let mut editor = create_editor(tmp.path());
    editor
        .execute(AddSchematic {
            name: ElementName::new("Main").unwrap(),
            index: None,
        })
        .unwrap();
    let added = editor
        .execute(AddComponent {
            value: Some("100nF".into()),
            device: Some(lib::c0805()),
            place: Some(SymbolPlacement {
                schematic: None,
                position: mm(0.0, 0.0),
                rotation: Default::default(),
                mirrored: false,
                gate_offset: None,
            }),
            ..AddComponent::new(lib::capacitor())
        })
        .unwrap();
    let schematic = editor.project().schematics()[0].id();
    let symbol = added.symbols[0];
    let move_text = |editor: &mut foilfern_editor::ProjectEditor, position| {
        let mut text = editor.project().schematics()[0].symbols()[&symbol]
            .texts()
            .values()
            .find(|text| text.layer().id() == "sym_values")
            .unwrap()
            .clone();
        text.set_position(position);
        editor
            .apply_mutations(
                "Move value",
                vec![foilfern_core::project::Mutation::Schematic(
                    foilfern_core::project::SchematicMutation::UpdateSymbolText {
                        symbol: foilfern_core::project::SymbolRef { schematic, symbol },
                        text,
                    },
                )],
            )
            .unwrap();
    };
    let overlaps = |editor: &foilfern_editor::ProjectEditor| {
        let scene =
            SchematicScene::build(editor.project(), schematic, &ColorScheme::SCHEMATIC_LIGHT)
                .unwrap();
        annotation_overlaps(&scene, None)
            .into_iter()
            .filter(|o| o.second == Annotation::SymbolArtwork(symbol))
            .count()
    };
    move_text(&mut editor, mm(10.16, 0.0));
    assert_eq!(overlaps(&editor), 0);
    move_text(&mut editor, mm(-1.27, 0.0));
    assert_eq!(overlaps(&editor), 1);
}
