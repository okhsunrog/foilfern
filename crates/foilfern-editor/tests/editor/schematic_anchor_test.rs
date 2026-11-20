//! Tests of `PlaceAtPin`: placing a part beside a pin of a fixed symbol.

use foilfern_core::geometry::NetLineAnchor;
use foilfern_core::geometry::Text;
use foilfern_core::project::{ComponentSignalRef, Mutation, Project, SchematicMutation};
use foilfern_core::types::{
    Alignment, Angle, CircuitIdentifier, ElementName, Layer, Point, PositiveLength, Uuid,
};
use foilfern_editor::ProjectEditor;
use foilfern_editor::commands::*;

use crate::helpers::{create_editor, lib, mm};

fn add(editor: &mut ProjectEditor, component: Uuid, device: Uuid, at: Point) -> AddedComponent {
    editor
        .execute(AddComponent {
            device: Some(device),
            place: Some(SymbolPlacement {
                schematic: None,
                position: at,
                rotation: Default::default(),
                mirrored: false,
                gate_offset: None,
            }),
            ..AddComponent::new(component)
        })
        .unwrap()
}

/// (position, rotation, library pin UUID) of the placed pin of a component
/// signal.
fn pin(p: &Project, component: &AddedComponent, name: &str) -> (Point, Angle, Uuid) {
    let wanted = signal(p, component, name);
    let sheet = &p.schematics()[0];
    let pin = sheet.symbols()[&component.symbols[0]]
        .pins(p.view())
        .unwrap()
        .into_iter()
        .find(|pin| pin.signal() == wanted)
        .unwrap();
    (pin.position(), pin.rotation(), pin.uuid())
}

fn signal(p: &Project, component: &AddedComponent, name: &str) -> ComponentSignalRef {
    let instance = p.circuit().component_instance(component.component).unwrap();
    let lib = p.library().component(&instance.lib_component()).unwrap();
    ComponentSignalRef {
        component: component.component,
        signal: lib.signals().by_name(name, true).unwrap().uuid(),
    }
}

fn setup() -> (tempfile::TempDir, ProjectEditor) {
    let tmp = tempfile::tempdir().unwrap();
    let mut editor = create_editor(tmp.path());
    editor
        .execute(AddSchematic {
            name: ElementName::new("Main").unwrap(),
            index: None,
        })
        .unwrap();
    (tmp, editor)
}

#[test]
fn test_place_at_pin_aligns_the_facing_pin_on_the_target_axis() {
    let (_tmp, mut editor) = setup();
    let r1 = add(&mut editor, lib::resistor(), lib::r0805(), mm(0.0, 0.0));
    let c1 = add(&mut editor, lib::capacitor(), lib::c0805(), mm(50.8, 50.8));
    let schematic = editor.project().schematics()[0].id();
    let (target, target_rotation, target_pin) = pin(editor.project(), &r1, "2");
    let (_, _, facing) = pin(editor.project(), &c1, "1");
    let placed = editor
        .execute(PlaceAtPin {
            schematic,
            symbol: c1.symbols[0],
            pin: facing,
            target: (r1.symbols[0], target_pin),
            distance: mm(5.08, 0.0).x,
            max_steps: 4,
        })
        .unwrap();
    assert_eq!(placed.target, target);
    let (position, rotation, _) = pin(editor.project(), &c1, "1");
    assert_eq!(position, placed.pin);
    // On the target pin's outward axis at the requested distance.
    let delta = position - target;
    assert!(delta.x.to_nm() == 0 || delta.y.to_nm() == 0, "{delta:?}");
    assert_eq!(delta.x.abs() + delta.y.abs(), mm(5.08, 0.0).x);
    // The facing pin points back at the target.
    assert_eq!(
        (rotation + Angle::DEG180).mapped_to_0_360deg(),
        target_rotation.mapped_to_0_360deg()
    );
    assert_eq!(placed.distance, mm(5.08, 0.0).x);

    // A straight automatic wire connects the two pins.
    let pins = vec![
        signal(editor.project(), &r1, "2"),
        signal(editor.project(), &c1, "1"),
    ];
    let net = editor
        .execute(AssignNet(ConnectNet {
            net: Some(CircuitIdentifier::new("DEC").unwrap()),
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
    let sheet = &editor.project().schematics()[0];
    let segment = sheet.net_segments().values().next().unwrap();
    assert_eq!(segment.lines().len(), 1);
    let line = segment.lines().values().next().unwrap();
    assert!(matches!(line.p1(), NetLineAnchor::Pin { .. }));
    assert!(matches!(line.p2(), NetLineAnchor::Pin { .. }));

    // One undo step restores the original placement.
    editor.undo().unwrap();
    editor.undo().unwrap();
    editor.undo().unwrap();
    let (back, _, _) = pin(editor.project(), &c1, "1");
    assert_ne!(back, placed.pin);
}

#[test]
fn test_place_at_pin_steps_past_obstacles_and_reports_failure() {
    let (_tmp, mut editor) = setup();
    let r1 = add(&mut editor, lib::resistor(), lib::r0805(), mm(0.0, 0.0));
    let c1 = add(&mut editor, lib::capacitor(), lib::c0805(), mm(50.8, 50.8));
    let schematic = editor.project().schematics()[0].id();
    let (target, target_rotation, target_pin) = pin(editor.project(), &r1, "2");
    let (before, _, facing) = pin(editor.project(), &c1, "1");
    let outward = (target_rotation + Angle::DEG180).mapped_to_0_360deg();
    let step = |d: f64| {
        let d = mm(d, 0.0).x;
        if outward == Angle::DEG0 {
            Point::new(target.x + d, target.y)
        } else if outward == Angle::DEG180 {
            Point::new(target.x - d, target.y)
        } else if outward == Angle::DEG90 {
            Point::new(target.x, target.y + d)
        } else {
            Point::new(target.x, target.y - d)
        }
    };
    let request = |max_steps| PlaceAtPin {
        schematic,
        symbol: c1.symbols[0],
        pin: facing,
        target: (r1.symbols[0], target_pin),
        distance: mm(5.08, 0.0).x,
        max_steps,
    };
    // A page note beside the axis blocks the part at the initial distance
    // but not the straight connection, so the part moves further out.
    let side = |p: Point, d: f64| {
        let d = mm(d, 0.0).x;
        if outward == Angle::DEG0 || outward == Angle::DEG180 {
            Point::new(p.x, p.y + d)
        } else {
            Point::new(p.x + d, p.y)
        }
    };
    editor
        .apply_mutations(
            "Add note",
            vec![Mutation::Schematic(SchematicMutation::AddText {
                schematic,
                text: Text::new(
                    Uuid::new_random(),
                    Layer::SCHEMATIC_COMMENTS,
                    "NOTE",
                    side(step(6.35), 2.54),
                    Angle::DEG0,
                    PositiveLength::new(mm(2.54, 0.0).x).unwrap(),
                    Alignment::default(),
                    false,
                ),
            })],
        )
        .unwrap();
    assert!(editor.execute(request(0)).is_err());
    // The failed attempt changed nothing.
    let (unchanged, _, _) = pin(editor.project(), &c1, "1");
    assert_eq!(unchanged, before);
    let placed = editor.execute(request(16)).unwrap();
    assert!(placed.distance > mm(5.08, 0.0).x, "{placed:?}");
    assert_eq!(placed.pin, step(placed.distance.to_mm()));
    editor.undo().unwrap();

    // A resistor on the axis blocks the connection at every distance.
    add(&mut editor, lib::resistor(), lib::r0805(), step(20.32));
    let error = editor.execute(request(16)).unwrap_err();
    assert!(error.to_string().contains("No clear position"), "{error}");
}
