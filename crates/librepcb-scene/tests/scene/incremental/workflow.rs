//! A project built step by step with the editor commands (like the
//! editor's workflow test), then modified, removed, undone and redone; the
//! scenes are checked after every step.

use librepcb_core::geometry::NetLineAnchor;
use librepcb_core::project::{BoardId, SchematicId};
use librepcb_core::types::{Angle, CircuitIdentifier, ElementName, Layer, Length, PositiveLength};
use librepcb_editor::commands::*;

use super::helpers::{Harness, create_editor, dump_board, dump_schematic, lib, mm};

fn name(s: &str) -> ElementName {
    ElementName::new(s).unwrap()
}

fn ident(s: &str) -> CircuitIdentifier {
    CircuitIdentifier::new(s).unwrap()
}

/// Builds R1 and R2 in series between VCC and GND with the middle net MID
/// and C1 between VCC and GND, placed and routed; every step is checked.
/// Returns the schematic and the board.
fn build_demo(h: &mut Harness) -> (SchematicId, BoardId) {
    let schematic = h.run(AddSchematic {
        name: name("Main"),
        index: None,
    });
    let board = h.run(AddBoard::new(name("default"))).board;

    let place = |x, y, rotation| SymbolPlacement {
        schematic: Some(schematic),
        position: mm(x, y),
        rotation,
        mirrored: false,
        gate_offset: None,
    };
    h.run(AddComponent {
        device: Some(lib::r0805()),
        place: Some(place(20.32, 50.8, Angle::DEG0)),
        ..AddComponent::new(lib::resistor())
    });
    h.run(AddComponent {
        device: Some(lib::r0805()),
        value: Some("10k".into()),
        place: Some(place(40.64, 50.8, Angle::DEG0)),
        ..AddComponent::new(lib::resistor())
    });
    h.run(AddComponent {
        device: Some(lib::c0805()),
        place: Some(place(60.96, 40.64, Angle::DEG90)),
        ..AddComponent::new(lib::capacitor())
    });

    // Wires and labels.
    let mid = h.run(ConnectPins {
        a: PinRef::new("R1", "2"),
        b: PinRef::new("R2", "1"),
        mode: WireMode::HV,
        net: Some(ident("MID")),
    });
    for (component, pin, net) in [
        ("R1", "1", "VCC"),
        ("C1", "1", "VCC"),
        ("R2", "2", "GND"),
        ("C1", "2", "GND"),
    ] {
        h.run(ConnectPinToNet {
            pin: PinRef::new(component, pin),
            net: ident(net),
            label: true,
            stub_length: None,
        });
    }
    h.run(DrawWire {
        schematic: None,
        start: WireAnchor::Line {
            segment: mid.segment,
            line: mid.lines[0],
            position: mm(30.0, 50.8),
        },
        end: WireAnchor::Point(mm(30.48, 60.96)),
        points: vec![],
        net: None,
    });
    h.run(AddNetLabel {
        segment: mid.segment,
        position: mm(30.48, 60.96),
        rotation: Angle::DEG0,
        mirrored: false,
    });

    // Board.
    for (component, x) in [("R1", 10.0), ("R2", 20.0), ("C1", 30.0)] {
        h.run(AddDevice {
            component: component.into(),
            board: Some(board),
            device: None,
            footprint: None,
            position: mm(x, 10.0),
            rotation: Angle::DEG0,
            mirrored: false,
        });
    }
    h.run(SetBoardOutline::rect(
        Some(board),
        mm(0.0, 0.0),
        mm(40.0, 20.0),
    ));
    h.run(AddTrace {
        board: Some(board),
        start: TraceEndpoint::Pad(PadRef::new("R1", "2")),
        end: TraceEndpoint::Pad(PadRef::new("R2", "1")),
        points: vec![],
        layer: None,
        width: None,
        net: None,
    });
    h.run(AddTrace {
        board: Some(board),
        start: TraceEndpoint::Pad(PadRef::new("R1", "1")),
        end: TraceEndpoint::Pad(PadRef::new("C1", "1")),
        points: vec![mm(10.0, 15.0), mm(30.0, 15.0)],
        layer: None,
        width: Some(PositiveLength::new(Length::new(500_000)).unwrap()),
        net: None,
    });
    let via = h.run(AddVia {
        board: Some(board),
        position: mm(35.0, 5.0),
        net: Some("GND".into()),
        start_layer: None,
        end_layer: None,
        drill_diameter: None,
        size: None,
        exposure: None,
    });
    h.run(AddTrace {
        board: Some(board),
        start: TraceEndpoint::Pad(PadRef::new("C1", "2")),
        end: TraceEndpoint::Via(via.via),
        points: vec![],
        layer: None,
        width: None,
        net: None,
    });
    let dangling = h.run(AddTrace {
        board: Some(board),
        start: TraceEndpoint::Pad(PadRef::new("R2", "2")),
        end: TraceEndpoint::Point(mm(25.0, 4.0)),
        points: vec![],
        layer: None,
        width: None,
        net: None,
    });
    // Combines two segments.
    h.run(AddTrace {
        board: Some(board),
        start: TraceEndpoint::Junction(dangling.junctions[0]),
        end: TraceEndpoint::Via(via.via),
        points: vec![mm(30.0, 2.0)],
        layer: None,
        width: None,
        net: None,
    });
    h.run(AddPlane {
        board: Some(board),
        net: Some("GND".into()),
        layer: Some(Layer::BOT_COPPER),
        outline: None,
        settings: PlaneSettings::default(),
    });
    h.run(AddHole {
        board: Some(board),
        position: mm(37.0, 17.0),
        diameter: PositiveLength::new(Length::new(3_200_000)).unwrap(),
        stop_mask: None,
    });
    h.run(AddStrokeText {
        board: Some(board),
        layer: Layer::TOP_LEGEND,
        text: "{{PROJECT}}".into(),
        position: mm(2.0, 17.0),
        rotation: Angle::DEG0,
        height: None,
        stroke_width: None,
        align: None,
    });
    (schematic, board)
}

/// Modifies and removes things; every step is checked and none of them
/// needs a full rebuild.
fn modify_demo(h: &mut Harness, schematic: SchematicId, board: BoardId) {
    let r1 = h
        .project()
        .circuit()
        .component_instance_by_name("R1")
        .unwrap()
        .1
        .id();
    let symbol = h
        .project()
        .component_uses(r1)
        .unwrap()
        .symbols
        .values()
        .next()
        .unwrap()
        .1;
    h.run(MoveSymbol {
        symbol,
        position: Some(mm(15.24, 55.88)),
        rotation: Some(Angle::DEG90),
        mirrored: Some(true),
    });
    h.run(MoveDevice {
        component: "R1".into(),
        board: Some(board),
        position: Some(mm(12.0, 8.0)),
        rotation: Some(Angle::DEG180),
        mirrored: None,
        locked: Some(true),
    });
    h.run(MoveDevice {
        component: "R2".into(),
        board: Some(board),
        position: None,
        rotation: Some(Angle::DEG90),
        mirrored: None,
        locked: None,
    });
    h.run(EditComponent {
        component: "R2".into(),
        name: None,
        value: Some("22k".into()),
        attributes: None,
        assembly_options: None,
        lock_assembly: None,
    });
    h.run(EditNet {
        net: "MID".into(),
        name: Some(ident("DIVIDER")),
        net_class: None,
        merge: false,
    });
    h.run(ReplaceDevice {
        component: "R1".into(),
        board: Some(board),
        device: lib::r0603(),
        footprint: None,
    });

    // Remove board items: the via and the plane.
    let b = h.project().board(board).unwrap();
    let (segment, via) = b
        .net_segments()
        .values()
        .find_map(|s| s.vias().keys().next().map(|v| (s.id(), *v)))
        .unwrap();
    let plane = *b.planes().keys().next().unwrap();
    h.run(RemoveBoardItems {
        board: Some(board),
        selection: BoardSelection {
            vias: vec![(segment, via)],
            planes: vec![plane],
            ..BoardSelection::default()
        },
    });

    // Remove a wire at a pin of R2.
    let p = h.project();
    let s = p.schematic(schematic).unwrap();
    let r2 = p.circuit().component_instance_by_name("R2").unwrap().1.id();
    let sym = p
        .component_uses(r2)
        .unwrap()
        .symbols
        .values()
        .next()
        .unwrap()
        .1;
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
    h.run(RemoveSchematicItems {
        schematic,
        selection: SchematicSelection {
            net_lines: vec![(segment, line)],
            ..SchematicSelection::default()
        },
    });

    // Remove components with their symbols, devices and wires.
    for component in ["C1", "R1"] {
        h.run(RemoveComponent {
            component: component.into(),
        });
    }
}

#[test]
fn test_build_modify_undo_redo() {
    let tmp = tempfile::tempdir().unwrap();
    let mut h = Harness::new(create_editor(&tmp.path().join("project")));
    let (schematic, board) = build_demo(&mut h);
    // Adding pages and boards rebuilds (page numbers, primary devices).
    let built_rebuilds = h.stats.rebuilds;
    h.stats.rebuilds = 0;
    modify_demo(&mut h, schematic, board);
    assert_eq!(h.stats.rebuilds, 0, "modifications rebuilt a scene");

    // Undo and redo everything step by step.
    while h.undo() {}
    while h.redo() {}
    while h.undo() {}
    println!(
        "workflow: {} steps, {} incremental updates, {} + {} full rebuilds",
        h.stats.steps, h.stats.incremental, built_rebuilds, h.stats.rebuilds
    );
}

/// Standalone pads are drawn with their board geometries, which depend on
/// the connected traces: adding and removing a trace at a pad rebuilds it.
#[test]
fn test_standalone_pad_with_traces() {
    use librepcb_core::geometry::{
        ComponentSide, Junction, NonEmptyPath, Pad, PadFunction, PadHole, PadShape, Path, Trace,
        TraceAnchor,
    };
    use librepcb_core::project::board::{
        BoardNetSegment, BoardPadData, BoardSegmentElements, BoardSegmentItems,
    };
    use librepcb_core::project::{BoardMutation, BoardNetSegmentRef, Mutation};
    use librepcb_core::types::{MaskConfig, Ratio, UnsignedLength, UnsignedLimitedRatio, Uuid};
    let tmp = tempfile::tempdir().unwrap();
    let mut h = Harness::new(create_editor(&tmp.path().join("project")));
    let (_, board) = build_demo(&mut h);
    // Automatic annular rings on the component side depend on the traces.
    let mut settings = h.project().board(board).unwrap().settings().clone();
    settings
        .design_rules
        .set_pad_cmp_side_auto_annular_ring(true);
    h.run(ApplyMutations {
        text: Some("Rules".into()),
        mutations: vec![Mutation::Board(BoardMutation::SetSettings {
            board,
            settings: Box::new(settings),
        })],
    });
    let size = PositiveLength::new(Length::new(5_000_000)).unwrap();
    let pad = Pad::new(
        Uuid::new_random(),
        mm(40.0, 20.0),
        Angle::DEG0,
        PadShape::RoundedRect,
        size,
        size,
        UnsignedLimitedRatio::new(Ratio::from_percent(0)).unwrap(),
        Path::default(),
        MaskConfig::Automatic,
        MaskConfig::Off,
        UnsignedLength::default(),
        ComponentSide::Top,
        PadFunction::StandardPad,
        [PadHole::new(
            Uuid::new_random(),
            PositiveLength::new(Length::new(3_000_000)).unwrap(),
            NonEmptyPath::from_point(mm(0.0, 0.0)),
        )]
        .into_iter()
        .collect(),
    );
    let pad_uuid = pad.uuid();
    {
        let b = h.project().board(board).unwrap();
        let data = BoardPadData::new(pad.clone(), false);
        let layers = b.copper_layers();
        let rules = b.design_rules();
        let top = std::collections::BTreeSet::from([Layer::TOP_COPPER]);
        assert_ne!(
            data.geometries(&layers, rules, &top),
            data.geometries(&layers, rules, &Default::default()),
            "the geometries depend on the connected layers"
        );
    }
    let segment = BoardNetSegment::with_elements(
        Uuid::new_random(),
        None,
        vec![BoardPadData::new(pad, false)],
        vec![],
        vec![],
        vec![],
    );
    let seg_ref = BoardNetSegmentRef {
        board,
        segment: segment.id(),
    };
    h.run(ApplyMutations {
        text: Some("Add pad".into()),
        mutations: vec![Mutation::Board(BoardMutation::AddNetSegment {
            board,
            segment,
        })],
    });
    let junction = Junction::new(Uuid::new_random(), mm(45.0, 20.0));
    let trace = Trace::new(
        Uuid::new_random(),
        // The component side: the annular ring depends on connected traces.
        Layer::TOP_COPPER,
        PositiveLength::new(Length::new(500_000)).unwrap(),
        TraceAnchor::Pad(pad_uuid),
        TraceAnchor::Junction(junction.uuid()),
    );
    let (trace_uuid, junction_uuid) = (trace.uuid(), junction.uuid());
    h.run(ApplyMutations {
        text: Some("Add trace".into()),
        mutations: vec![Mutation::Board(BoardMutation::AddNetSegmentElements {
            segment: seg_ref,
            elements: BoardSegmentElements {
                junctions: vec![junction],
                traces: vec![trace],
                ..BoardSegmentElements::default()
            },
        })],
    });
    h.run(ApplyMutations {
        text: Some("Remove trace".into()),
        mutations: vec![Mutation::Board(BoardMutation::RemoveNetSegmentElements {
            segment: seg_ref,
            elements: BoardSegmentItems {
                traces: vec![trace_uuid],
                junctions: vec![junction_uuid],
                ..BoardSegmentItems::default()
            },
        })],
    });
    while h.undo() {}
}

#[test]
fn test_resync_rebuilds() {
    use librepcb_core::project::ChangesSince;
    use librepcb_scene::{BoardScene, BoardSide, ColorScheme, SceneSync, SchematicScene};

    let tmp = tempfile::tempdir().unwrap();
    let mut h = Harness::new(create_editor(&tmp.path().join("project")));
    let (schematic, board) = build_demo(&mut h);
    let p = h.project();
    let mut sch = SchematicScene::build(p, schematic, &ColorScheme::SCHEMATIC_LIGHT).unwrap();
    let mut brd = BoardScene::build(p, board, BoardSide::Top, &ColorScheme::BOARD_DARK).unwrap();
    let (sch_before, brd_before) = (dump_schematic(&sch), dump_board(&brd));
    assert!(sch.apply_changes(p, ChangesSince::Resync).unwrap());
    assert!(brd.apply_changes(p, ChangesSince::Resync).unwrap());
    assert_eq!(dump_schematic(&sch), sch_before);
    assert_eq!(dump_board(&brd), brd_before);
    // No changes: nothing to do.
    assert!(!sch.apply_changes(p, ChangesSince::Changes(&[])).unwrap());
    // A cursor ahead of the journal (e.g. another project) resyncs.
    let mut sync = SceneSync::at(p.revision() + 10);
    assert!(sync.sync(p, &mut brd).unwrap());
    assert_eq!(sync.cursor(), p.revision());
    assert!(!sync.sync(p, &mut brd).unwrap());
}
