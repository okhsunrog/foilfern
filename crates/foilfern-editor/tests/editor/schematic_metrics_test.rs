//! Tests of the schematic layout measurements (`foilfern_scene::schematic_metrics`)
//! on pages built with editor commands.

use foilfern_core::types::{ElementName, Point};
use foilfern_editor::ProjectEditor;
use foilfern_editor::commands::*;
use foilfern_scene::{ColorScheme, SchematicMetrics, SchematicScene, schematic_metrics};

use crate::helpers::{create_editor, lib, mm};

fn metrics(editor: &ProjectEditor) -> SchematicMetrics {
    let project = editor.project();
    let page = project.schematics()[0].id();
    let scene = SchematicScene::build(project, page, &ColorScheme::SCHEMATIC_LIGHT).unwrap();
    schematic_metrics(project, &scene).unwrap()
}

fn wire(editor: &mut ProjectEditor, start: Point, points: &[Point], end: Point) {
    editor
        .execute(DrawWire {
            schematic: None,
            start: WireAnchor::Point(start),
            end: WireAnchor::Point(end),
            points: points.to_vec(),
            net: None,
        })
        .unwrap();
}

fn resistor(editor: &mut ProjectEditor, position: Point) -> AddedComponent {
    editor
        .execute(AddComponent {
            device: Some(lib::r0805()),
            place: Some(SymbolPlacement {
                schematic: None,
                position,
                rotation: Default::default(),
                mirrored: false,
                gate_offset: None,
            }),
            ..AddComponent::new(lib::resistor())
        })
        .unwrap()
}

#[test]
fn test_metrics_report_misleading_geometry() {
    let tmp = tempfile::tempdir().unwrap();
    let mut editor = create_editor(tmp.path());
    editor
        .execute(AddSchematic {
            name: ElementName::new("Main").unwrap(),
            index: None,
        })
        .unwrap();
    let empty = metrics(&editor);
    assert_eq!(empty.symbols, 0);
    assert_eq!(empty.content_bounds, None);
    assert_eq!(empty.defects(), 0);

    let r1 = resistor(&mut editor, mm(0.0, 0.0));
    let r2 = resistor(&mut editor, mm(2.54, 0.0));
    resistor(&mut editor, mm(50.8, 0.0));
    // Two nets drawn along the same stretch.
    wire(&mut editor, mm(101.6, 101.6), &[], mm(121.92, 101.6));
    wire(&mut editor, mm(111.76, 101.6), &[], mm(132.08, 101.6));
    // A wire of another net ending on the interior of a wire.
    wire(&mut editor, mm(101.6, 81.28), &[], mm(121.92, 81.28));
    wire(&mut editor, mm(111.76, 71.12), &[], mm(111.76, 81.28));
    // A proper crossing and a wire with one corner (a bend).
    wire(&mut editor, mm(101.6, 60.96), &[], mm(121.92, 60.96));
    wire(&mut editor, mm(111.76, 50.8), &[], mm(111.76, 66.04));
    wire(
        &mut editor,
        mm(152.4, 50.8),
        &[mm(162.56, 50.8)],
        mm(162.56, 60.96),
    );
    // An off-grid junction (grid 2.54 mm).
    wire(&mut editor, mm(152.4, 101.6), &[], mm(153.67, 101.6));

    let m = metrics(&editor);
    assert_eq!(m.symbols, 3);
    let overlap = &m.symbol_overlaps[..];
    assert_eq!(overlap.len(), 1, "{overlap:?}");
    let pair = [r1.symbols[0], r2.symbols[0]];
    assert_eq!(
        (overlap[0].first, overlap[0].second),
        (*pair.iter().min().unwrap(), *pair.iter().max().unwrap())
    );
    let w = &m.wires;
    assert_eq!(w.different_net_overlaps.len(), 1, "{w:?}");
    let o = &w.different_net_overlaps[0];
    let close = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6;
    assert!(
        close(o.from, (111.76, 101.6)) && close(o.to, (121.92, 101.6)),
        "{o:?}"
    );
    assert_eq!(w.foreign_contacts.len(), 1, "{w:?}");
    assert!(close(w.foreign_contacts[0].position, (111.76, 81.28)));
    assert_eq!(w.crossings, 1);
    assert_eq!(w.bends, 1);
    assert_eq!(w.diagonal, 0);
    assert_eq!(w.lines, 9);
    assert!((w.longest_mm - 20.32).abs() < 1e-6);
    assert_eq!(m.defects(), 3);
    assert_eq!(m.off_grid.junctions, 1);
    assert_eq!(m.off_grid.symbols, 0);
    assert_eq!(m.off_grid.pins, 0);
    // The resistors have no nets, the free wires have no pins.
    assert_eq!(m.nets.unwired_pins, 0);
    assert_eq!(m.nets.pin_span_mm, 0.0);
    assert!(m.symbol_fill > 0.0 && m.symbol_fill < 1.0);

    // Separating the symbols removes the body overlap.
    editor
        .execute(MoveSymbol {
            symbol: r2.symbols[0],
            position: Some(mm(25.4, 0.0)),
            rotation: None,
            mirrored: None,
        })
        .unwrap();
    assert!(metrics(&editor).symbol_overlaps.is_empty());
}

#[test]
fn test_metrics_measure_net_spread_and_split_nets() {
    let tmp = tempfile::tempdir().unwrap();
    let mut editor = create_editor(tmp.path());
    editor
        .execute(AddSchematic {
            name: ElementName::new("Main").unwrap(),
            index: None,
        })
        .unwrap();
    let r1 = resistor(&mut editor, mm(0.0, 0.0));
    let r2 = resistor(&mut editor, mm(25.4, 0.0));
    let pin = |c: &AddedComponent, pin: &str| PinRef {
        component: ComponentRef::Id(c.component),
        pin: pin.to_owned(),
    };
    editor
        .execute(ConnectPins {
            a: pin(&r1, "2"),
            b: pin(&r2, "1"),
            mode: Default::default(),
            net: None,
        })
        .unwrap();
    let m = metrics(&editor);
    assert_eq!(m.nets.nets, 1);
    assert_eq!(m.nets.segments, 1);
    assert_eq!(m.nets.split_nets, 0);
    assert_eq!(m.nets.unwired_pins, 0);
    assert!(m.nets.pin_span_mm > 0.0);
    assert!(m.wires.total_length_mm > 0.0);
    assert_eq!(m.defects(), 0);

    // Moving R2 further away spreads the connected pins.
    let before = m.nets.pin_span_mm;
    editor
        .execute(MoveSymbol {
            symbol: r2.symbols[0],
            position: Some(mm(50.8, 0.0)),
            rotation: None,
            mirrored: None,
        })
        .unwrap();
    let after = metrics(&editor).nets.pin_span_mm;
    assert!((after - before - 25.4).abs() < 1e-6, "{before} -> {after}");
}
