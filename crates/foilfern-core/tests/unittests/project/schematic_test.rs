//! Schematic items: loading real projects, mutations with their inverses
//! and error cases, symbol pin views, the net segment splitter and the
//! serde representation.

use std::sync::Arc;

use chrono::Utc;
use foilfern_core::fileio::{FileSystem, TransactionalDirectory, TransactionalFileSystem};
use foilfern_core::geometry::{
    Image, Junction, NetLabel, NetLine, NetLineAnchor, Path, Polygon, Text, Vertex,
};
use foilfern_core::library::cmp::{
    CmpSigPinDisplayType, ComponentPinSignalMapItem, ComponentSymbolVariantItem,
    ComponentSymbolVariantItemSuffix,
};
use foilfern_core::library::sym::{Symbol, SymbolPin};
use foilfern_core::library::{BaseMetadata, LibraryBaseElement};
use foilfern_core::project::circuit::Bus;
use foilfern_core::project::schematic::{
    Schematic, SchematicBusSegment, SchematicChange, SchematicNetSegment,
    SchematicNetSegmentSplitter, SchematicSymbol,
};
use foilfern_core::project::{
    BusId, BusSegmentRef, Change, ChangesSince, ComponentSignalRef, EntityKind, Error,
    NetSegmentRef, SchematicId, SchematicMutation, SymbolArtwork, SymbolId, SymbolRef,
};
use foilfern_core::types::{
    Alignment, Angle, BusName, FileProofName, HAlign, Layer, Length, Point, PositiveLength,
    UnsignedLength, VAlign,
};

use super::*;
use crate::helpers::{TempDir, test_data_dir};

type M = SchematicMutation;

/// An invalid item and a check of the expected error.
type ErrorCase<T> = (T, fn(&Error) -> bool);

#[test]
fn library_symbol_polygons_are_snapshot_safe_and_reversible() {
    let mut project = new_project();
    add_gated_library_component(&mut project, 1);
    let (uuid, symbol) = project.library().symbols().first_key_value().unwrap();
    let uuid = *uuid;
    let pins = symbol.pins().clone();
    let path = format!("library/sym/{uuid}/symbol.lp");
    let before = project.directory().read(&path).unwrap();
    let revision = project.revision();
    let replacement = Mutation::SetLibrarySymbolArtwork(SymbolArtwork {
        symbol: uuid,
        polygons: vec![Polygon::new(
            Uuid::new_random(),
            Layer::SYMBOL_OUTLINES,
            width(),
            false,
            false,
            Path::line(pt(0.0, 0.0), pt(10.0, 0.0), Angle::DEG0),
        )]
        .into(),
        pin_name_positions: [(pins.first().unwrap().uuid(), pt(1.0, 1.0))].into(),
    });
    assert!(
        project
            .apply(Mutation::Batch(vec![
                replacement.clone(),
                Mutation::RemoveSchematic(SchematicId(Uuid::new_random())),
            ]))
            .is_err()
    );
    assert_eq!(project.revision(), revision);
    assert_eq!(project.directory().read(&path).unwrap(), before);
    assert!(
        project
            .library()
            .symbol(&uuid)
            .unwrap()
            .polygons()
            .is_empty()
    );

    let inverse = project.apply(replacement).unwrap();
    let edited = project.directory().read(&path).unwrap();
    assert_ne!(edited, before);
    let snapshot = project.snapshot().unwrap();
    let symbol = snapshot.library().symbol(&uuid).unwrap();
    let mut expected_pins = pins;
    expected_pins
        .get_mut(0)
        .unwrap()
        .set_name_position(pt(1.0, 1.0));
    assert_eq!(symbol.pins(), &expected_pins);
    assert_eq!(
        symbol.polygons(),
        project.library().symbol(&uuid).unwrap().polygons()
    );

    let redo = project.apply(inverse).unwrap();
    assert_eq!(project.directory().read(&path).unwrap(), before);
    project.apply(redo).unwrap();
    assert_eq!(project.directory().read(&path).unwrap(), edited);
}

// --- Fixture ---

fn mm(v: f64) -> Length {
    Length::from_mm(v).unwrap()
}

fn pt(x: f64, y: f64) -> Point {
    Point::from_mm(x, y).unwrap()
}

fn width() -> UnsignedLength {
    SchematicNetSegment::default_line_width()
}

fn text(uuid: Uuid, value: &str, position: Point) -> Text {
    Text::new(
        uuid,
        Layer::from_id("sch_comments").unwrap(),
        value,
        position,
        Angle::DEG0,
        PositiveLength::new(mm(2.54)).unwrap(),
        Alignment::new(HAlign::Left, VAlign::Bottom),
        false,
    )
}

/// A library component with gates using one symbol with two pins at
/// (0, 0) and (10, 0), each gate mapping its pins to its own two signals.
struct LibComponent {
    uuid: Uuid,
    variant: Uuid,
    gates: Vec<Uuid>,
    pins: [Uuid; 2],
    signals: Vec<Uuid>,
}

fn add_gated_library_component(p: &mut Project, gate_count: usize) -> LibComponent {
    let symbol_uuid = Uuid::new_random();
    let pins = [Uuid::new_random(), Uuid::new_random()];
    let mut symbol = Symbol::new(BaseMetadata::new(
        symbol_uuid,
        "0.1".parse().unwrap(),
        "test",
        Utc::now(),
        ElementName::new("Test Symbol").unwrap(),
        "",
        "",
    ))
    .unwrap();
    for (i, pin) in pins.iter().enumerate() {
        symbol.pins_mut().push(SymbolPin::new(
            *pin,
            CircuitIdentifier::new(format!("P{}", i + 1)).unwrap(),
            pt(10.0 * i as f64, 0.0),
            UnsignedLength::new(mm(2.54)).unwrap(),
            Angle::DEG0,
            Point::ORIGIN,
            Angle::DEG0,
            SymbolPin::default_name_height(),
            SymbolPin::default_name_alignment(),
        ));
    }
    symbol
        .texts_mut()
        .push(text(Uuid::new_random(), "{{NAME}}", pt(0.0, 5.0)));
    p.add_library_symbol(symbol).unwrap();

    let (mut cmp, variant) =
        library_component(Uuid::new_random(), 2 * gate_count, Uuid::new_random());
    let signals = cmp.signals().uuids();
    let mut gates = Vec::new();
    for g in 0..gate_count {
        let gate = Uuid::new_random();
        let suffix = if gate_count > 1 {
            ["A", "B", "C"][g]
        } else {
            ""
        };
        let mut item = ComponentSymbolVariantItem::new(
            gate,
            symbol_uuid,
            Point::ORIGIN,
            Angle::DEG0,
            true,
            ComponentSymbolVariantItemSuffix::new(suffix).unwrap(),
        );
        for (i, pin) in pins.iter().enumerate() {
            item.pin_signal_map_mut()
                .push(ComponentPinSignalMapItem::new(
                    *pin,
                    Some(signals[2 * g + i]),
                    CmpSigPinDisplayType::ComponentSignal,
                ));
        }
        cmp.symbol_variants_mut()
            .by_uuid_mut(&variant)
            .unwrap()
            .symbol_items_mut()
            .push(item);
        gates.push(gate);
    }
    let uuid = cmp.metadata().uuid();
    p.add_library_component(cmp).unwrap();
    LibComponent {
        uuid,
        variant,
        gates,
        pins,
        signals,
    }
}

struct Fixture {
    p: Project,
    schematic: SchematicId,
    /// "N1", connected to all signals of the component.
    net: NetSignalId,
    /// "N2", unused.
    other_net: NetSignalId,
    component: ComponentInstanceId,
    lib: LibComponent,
}

impl Fixture {
    fn new(gate_count: usize) -> Self {
        Self::in_project(new_project(), gate_count)
    }

    fn in_project(mut p: Project, gate_count: usize) -> Self {
        let net = add_net(&mut p, "N1");
        let other_net = add_net(&mut p, "N2");
        let lib = add_gated_library_component(&mut p, gate_count);
        let mut cmp = component_instance(&p, lib.uuid, lib.variant, "U1");
        for signal in &lib.signals {
            cmp.set_signal_net(signal, Some(net)).unwrap();
        }
        let component = p.add_component_instance(cmp).unwrap();
        let schematic = p
            .add_schematic(
                Schematic::new(
                    Uuid::new_random(),
                    ElementName::new("Main").unwrap(),
                    "main",
                ),
                None,
            )
            .unwrap();
        Self {
            p,
            schematic,
            net,
            other_net,
            component,
            lib,
        }
    }

    fn new_symbol(&self, gate: usize) -> SchematicSymbol {
        SchematicSymbol::new(
            Uuid::new_random(),
            self.component,
            self.lib.gates[gate],
            Point::ORIGIN,
            Angle::DEG0,
            false,
        )
    }

    /// Places gate 0 at the origin.
    fn add_symbol(&mut self) -> SymbolRef {
        let symbol = self.new_symbol(0);
        let id = symbol.id();
        self.p.add_symbol(self.schematic, symbol).unwrap();
        SymbolRef {
            schematic: self.schematic,
            symbol: id,
        }
    }

    fn pin(&self, symbol: SymbolRef, i: usize) -> NetLineAnchor {
        NetLineAnchor::Pin {
            symbol: symbol.symbol.0,
            pin: self.lib.pins[i],
        }
    }

    /// A segment of N1: pin 1 - junction (5, 5) - pin 2.
    fn wire(&self, symbol: SymbolRef) -> (SchematicNetSegment, Uuid) {
        let mut segment = SchematicNetSegment::new(Uuid::new_random(), self.net);
        let junction = Uuid::new_random();
        segment.insert_junction(Junction::new(junction, pt(5.0, 5.0)));
        for i in 0..2 {
            segment.insert_line(NetLine::new(
                Uuid::new_random(),
                width(),
                self.pin(symbol, i),
                NetLineAnchor::Junction(junction),
            ));
        }
        (segment, junction)
    }

    fn segment_ref(&self, segment: &SchematicNetSegment) -> NetSegmentRef {
        NetSegmentRef {
            schematic: self.schematic,
            segment: segment.id(),
        }
    }

    fn schematic(&self) -> &Schematic {
        self.p.schematic(self.schematic).unwrap()
    }
}

fn sch(m: SchematicMutation) -> Mutation {
    Mutation::Schematic(m)
}

// --- Real projects ---

/// The expected output of the EAGLE import test (not in `tests/data/projects`,
/// so not covered by `tests/project_roundtrip.rs`) has bus segments with
/// net lines attached: its schematic files must save byte-identically.
#[test]
fn eagle_import_project_roundtrip() {
    let dir = test_data_dir().join("unittests/eagleimport/testproject/expected");
    let fs = TransactionalFileSystem::open_ro(&foilfern_core::fileio::FilePath::new(&dir).unwrap())
        .unwrap();
    let mut p = Project::open(TransactionalDirectory::new(Arc::new(fs), ""), "test.lpp").unwrap();
    assert!(p.is_ref_index_consistent());
    let s = &p.schematics()[0];
    assert!(!s.bus_segments().is_empty());
    assert!(
        s.bus_segments()
            .keys()
            .any(|b| !s.attached_net_segments(*b).is_empty())
    );
    p.save().unwrap();
    let modified: Vec<String> = p
        .directory()
        .file_system()
        .check_for_modifications()
        .unwrap()
        .into_iter()
        .filter(|f| f.starts_with("schematics/") && !f.ends_with("settings.user.lp"))
        .collect();
    assert_eq!(modified, Vec::<String>::new());
}

#[test]
fn open_test_projects() {
    for (name, symbols, net_segments, bus_segments) in [
        ("Gerber Test", 5, 13, 2),
        ("DRC", 16, 6, 0),
        ("Nested Planes", 9, 7, 0),
    ] {
        let dir = test_data_dir().join("projects").join(name);
        let fs =
            TransactionalFileSystem::open_ro(&foilfern_core::fileio::FilePath::new(&dir).unwrap())
                .unwrap();
        let directory = TransactionalDirectory::new(Arc::new(fs), "");
        let lpp = directory
            .files("")
            .into_iter()
            .find(|f| f.ends_with(".lpp"))
            .unwrap();
        let p = Project::open(directory, &lpp).unwrap();
        assert!(p.is_ref_index_consistent(), "{name}");
        let s = &p.schematics()[0];
        assert_eq!(s.symbols().len(), symbols, "{name}");
        assert_eq!(s.net_segments().len(), net_segments, "{name}");
        assert_eq!(s.bus_segments().len(), bus_segments, "{name}");
        for symbol in s.symbols().values() {
            for pin in symbol.pins(p.view()).unwrap() {
                assert_eq!(pin.symbol(), symbol.id());
            }
            let uses = p.component_uses(symbol.component()).unwrap();
            assert_eq!(uses.symbols[&symbol.lib_gate()], (s.id(), symbol.id()));
        }
        for (id, segment) in s.net_segments() {
            assert!(segment.is_cohesive());
            assert!(
                p.net_signal_uses(segment.net())
                    .any(|u| *u == foilfern_core::project::NetUse::SchematicSegment(s.id(), *id))
            );
            for line in segment.lines().values() {
                for anchor in [line.p1(), line.p2()] {
                    assert!(s.net_line_anchor_position(*id, anchor, p.view()).is_some());
                    if let NetLineAnchor::Pin { symbol, pin } = anchor {
                        assert_eq!(s.pin_net_segment(SymbolId(symbol), pin), Some(*id));
                    }
                }
            }
            for label in segment.labels().keys() {
                assert!(s.net_label_anchor(*id, *label, p.view()).is_some());
            }
        }
        if name == "Gerber Test" {
            assert_eq!(s.images().len(), 1);
            let attached: usize = s
                .bus_segments()
                .keys()
                .map(|b| s.attached_net_segments(*b).len())
                .sum();
            assert!(attached > 0);
        }
    }
}

// --- Symbols ---

#[test]
fn symbols() {
    let mut f = Fixture::new(2);
    let lib_symbol = f.p.library().symbols().values().next().unwrap();
    let mut symbol = f.new_symbol(0);
    symbol.set_position(pt(10.0, 20.0));
    for text in symbol.default_texts(lib_symbol) {
        symbol.insert_text(text);
    }
    assert_eq!(
        symbol.texts().values().next().unwrap().position(),
        pt(10.0, 25.0)
    );
    assert_inverse(
        &mut f.p,
        sch(M::AddSymbol {
            schematic: f.schematic,
            symbol: symbol.clone(),
        }),
    );
    f.p.add_symbol(f.schematic, symbol.clone()).unwrap();
    let r = SymbolRef {
        schematic: f.schematic,
        symbol: symbol.id(),
    };
    assert_eq!(symbol.name(f.p.view()).unwrap(), "U1-A");

    // Pins.
    let pins = symbol.pins(f.p.view()).unwrap();
    assert_eq!(pins.len(), 2);
    assert_eq!(pins[0].position(), pt(10.0, 20.0));
    assert_eq!(pins[1].position(), pt(20.0, 20.0));
    assert_eq!(pins[0].name(), "S1");
    assert_eq!(pins[0].net(), Some(f.net));
    assert_eq!(pins[0].numbers_text(&["1"]), "1");
    assert_eq!(pins[0].numbers_text(&["S1"]), "");
    assert_eq!(pins[0].numbers_text(&["1", "2", "3", "4", "5"]), "1,2,3,4…");

    // Placement.
    assert_inverse(
        &mut f.p,
        sch(M::SetSymbolPlacement {
            symbol: r,
            position: pt(1.0, 2.0),
            rotation: Angle::DEG90,
            mirrored: true,
        }),
    );
    f.p.apply(sch(M::SetSymbolPlacement {
        symbol: r,
        position: pt(1.0, 2.0),
        rotation: Angle::DEG90,
        mirrored: true,
    }))
    .unwrap();
    let placed = f.schematic().symbols()[&r.symbol].clone();
    let pins = placed.pins(f.p.view()).unwrap();
    assert_eq!(pins[0].position(), pt(1.0, 2.0));
    assert_eq!(pins[1].position(), pt(1.0, -8.0));

    // Texts.
    let text_uuid = Uuid::new_random();
    let t = text(text_uuid, "foo", Point::ORIGIN);
    assert_inverse(
        &mut f.p,
        sch(M::AddSymbolText {
            symbol: r,
            text: t.clone(),
        }),
    );
    let existing = placed.texts().values().next().unwrap().clone();
    let mut edited = existing.clone();
    edited.set_text("bar".to_owned());
    assert_inverse(
        &mut f.p,
        sch(M::UpdateSymbolText {
            symbol: r,
            text: edited,
        }),
    );
    assert_inverse(
        &mut f.p,
        sch(M::RemoveSymbolText {
            symbol: r,
            text: existing.uuid(),
        }),
    );
    let e = assert_fails(
        &mut f.p,
        sch(M::AddSymbolText {
            symbol: r,
            text: existing.clone(),
        }),
    );
    assert!(matches!(
        e,
        Error::DuplicateUuid {
            kind: EntityKind::SchematicText,
            ..
        }
    ));
    let e = assert_fails(
        &mut f.p,
        sch(M::RemoveSymbolText {
            symbol: r,
            text: text_uuid,
        }),
    );
    assert!(matches!(e, Error::NotFound { .. }));

    // Errors.
    let e = assert_fails(
        &mut f.p,
        sch(M::AddSymbol {
            schematic: f.schematic,
            symbol: symbol.clone(),
        }),
    );
    assert!(matches!(
        e,
        Error::DuplicateUuid {
            kind: EntityKind::Symbol,
            ..
        }
    ));
    let same_gate = f.new_symbol(0);
    let e = assert_fails(
        &mut f.p,
        sch(M::AddSymbol {
            schematic: f.schematic,
            symbol: same_gate,
        }),
    );
    assert!(matches!(e, Error::SymbolItemAlreadyPlaced(g) if g == f.lib.gates[0]));
    let bad = SchematicSymbol::new(
        Uuid::new_random(),
        f.component,
        Uuid::new_random(),
        Point::ORIGIN,
        Angle::DEG0,
        false,
    );
    let e = assert_fails(
        &mut f.p,
        sch(M::AddSymbol {
            schematic: f.schematic,
            symbol: bad,
        }),
    );
    assert!(matches!(e, Error::InvalidSymbolItem(_)));
    let orphan = SchematicSymbol::new(
        Uuid::new_random(),
        ComponentInstanceId::new_random(),
        f.lib.gates[0],
        Point::ORIGIN,
        Angle::DEG0,
        false,
    );
    let e = assert_fails(
        &mut f.p,
        sch(M::AddSymbol {
            schematic: f.schematic,
            symbol: orphan,
        }),
    );
    assert!(matches!(e, Error::InexistentComponent(_)));
    assert!(e.to_string().contains("does not exist in the circuit"));

    // All symbols of a component in one schematic.
    let other =
        f.p.add_schematic(
            Schematic::new(
                Uuid::new_random(),
                ElementName::new("Other").unwrap(),
                "other",
            ),
            None,
        )
        .unwrap();
    let gate_b = f.new_symbol(1);
    let e = assert_fails(
        &mut f.p,
        sch(M::AddSymbol {
            schematic: other,
            symbol: gate_b.clone(),
        }),
    );
    assert!(matches!(e, Error::SymbolsInDifferentSchematics));
    assert_inverse(
        &mut f.p,
        sch(M::AddSymbol {
            schematic: f.schematic,
            symbol: gate_b,
        }),
    );

    // Removal.
    let e = assert_fails(&mut f.p, Mutation::RemoveComponentInstance(f.component));
    assert!(matches!(e, Error::ComponentInUse(_)));
    assert_inverse(&mut f.p, sch(M::RemoveSymbol(r)));
    let e = assert_fails(&mut f.p, Mutation::RemoveSchematic(f.schematic));
    assert!(matches!(e, Error::SchematicNotEmpty(_)));
    f.p.remove_symbol(r).unwrap();
    assert!(!f.p.is_component_used(f.component));
    assert!(f.p.is_ref_index_consistent());
}

#[test]
fn missing_library_symbol() {
    let mut p = new_project();
    // A component whose gate references a symbol which is not in the
    // project library.
    let lib = Uuid::new_random();
    let (mut cmp, variant) = library_component(lib, 2, Uuid::new_random());
    let signals = cmp.signals().uuids();
    let gate = Uuid::new_random();
    let missing = Uuid::new_random();
    let mut item = ComponentSymbolVariantItem::new(
        gate,
        missing,
        Point::ORIGIN,
        Angle::DEG0,
        true,
        ComponentSymbolVariantItemSuffix::default(),
    );
    item.pin_signal_map_mut()
        .push(ComponentPinSignalMapItem::new(
            Uuid::new_random(),
            Some(signals[0]),
            CmpSigPinDisplayType::None,
        ));
    cmp.symbol_variants_mut()
        .by_uuid_mut(&variant)
        .unwrap()
        .symbol_items_mut()
        .push(item);
    p.add_library_component(cmp).unwrap();
    let component = p
        .add_component_instance(component_instance(&p, lib, variant, "U1"))
        .unwrap();
    let schematic = p
        .add_schematic(
            Schematic::new(
                Uuid::new_random(),
                ElementName::new("Main").unwrap(),
                "main",
            ),
            None,
        )
        .unwrap();
    let symbol = SchematicSymbol::new(
        Uuid::new_random(),
        component,
        gate,
        Point::ORIGIN,
        Angle::DEG0,
        false,
    );
    let e = assert_fails(&mut p, sch(M::AddSymbol { schematic, symbol }));
    assert!(matches!(e, Error::MissingLibrarySymbol(u) if u == missing));
    assert_eq!(
        e.to_string(),
        format!("No symbol with the UUID \"{missing}\" found in the project's library.")
    );
}

// --- Net segments ---

#[test]
fn net_segments() {
    let mut f = Fixture::new(1);
    let symbol = f.add_symbol();
    let (segment, junction) = f.wire(symbol);
    let r = f.segment_ref(&segment);
    assert_inverse(
        &mut f.p,
        sch(M::AddNetSegment {
            schematic: f.schematic,
            segment: segment.clone(),
        }),
    );
    f.p.add_net_segment(f.schematic, segment.clone()).unwrap();
    let s = f.schematic();
    assert_eq!(
        s.pin_net_segment(symbol.symbol, f.lib.pins[0]),
        Some(r.segment)
    );
    assert!(!s.net_segments()[&r.segment].is_visible_junction(NetLineAnchor::Junction(junction)));
    assert_eq!(
        s.net_line_anchor_position(r.segment, f.pin(symbol, 1), f.p.view()),
        Some(pt(10.0, 0.0))
    );

    // The wired pins lock the component signal, the net and the symbol.
    let e = assert_fails(
        &mut f.p,
        Mutation::SetComponentSignalNet {
            signal: ComponentSignalRef {
                component: f.component,
                signal: f.lib.signals[0],
            },
            net: None,
        },
    );
    assert!(matches!(e, Error::ComponentSignalInUse { .. }));
    let e = assert_fails(&mut f.p, Mutation::RemoveNetSignal(f.net));
    assert!(matches!(e, Error::NetSignalInUse(_)));
    let e = assert_fails(&mut f.p, sch(M::RemoveSymbol(symbol)));
    assert!(matches!(
        e,
        Error::ItemInUse {
            kind: EntityKind::Symbol,
            ..
        }
    ));

    // Pin of another segment / of another net.
    let (second, _) = f.wire(symbol);
    let e = assert_fails(
        &mut f.p,
        sch(M::AddNetSegment {
            schematic: f.schematic,
            segment: second.clone(),
        }),
    );
    assert!(matches!(e, Error::AnchorInMultipleSegments { .. }));
    // The first line (in UUID order) is checked first.
    let pin1 = f.pin(symbol, 0);
    let first_signal = |segment: &SchematicNetSegment| {
        let line = segment.lines().values().next().unwrap();
        if line.p1() == pin1 || line.p2() == pin1 {
            "S1"
        } else {
            "S2"
        }
    };
    assert_eq!(
        e.to_string(),
        format!(
            "There are lines from multiple net segments connected to the pin \"{}\" of \
             component \"U1\" (Test Component).",
            first_signal(&second)
        )
    );
    let mut wrong_net = SchematicNetSegment::new(Uuid::new_random(), f.other_net);
    for (uuid, junction) in second.junctions() {
        wrong_net.insert_junction(Junction::new(*uuid, junction.position()));
    }
    for line in second.lines().values() {
        wrong_net.insert_line(line.clone());
    }
    let e = assert_fails(
        &mut f.p,
        sch(M::AddNetSegment {
            schematic: f.schematic,
            segment: wrong_net,
        }),
    );
    assert_eq!(
        e.to_string(),
        format!(
            "Line of net \"N2\" is not allowed to be connected to pin \"{}\" of component \
             \"U1\" (Test Component) since it is connected to the net \"N1\".",
            first_signal(&second)
        )
    );

    // Invalid segments.
    let j1 = Uuid::new_random();
    let j2 = Uuid::new_random();
    let segment_with = |lines: Vec<(NetLineAnchor, NetLineAnchor)>, net: NetSignalId| {
        let mut s = SchematicNetSegment::new(Uuid::new_random(), net);
        s.insert_junction(Junction::new(j1, pt(0.0, 10.0)));
        s.insert_junction(Junction::new(j2, pt(10.0, 10.0)));
        for (a, b) in lines {
            s.insert_line(NetLine::new(Uuid::new_random(), width(), a, b));
        }
        s
    };
    let junction1 = NetLineAnchor::Junction(j1);
    let junction2 = NetLineAnchor::Junction(j2);
    let cases: Vec<ErrorCase<SchematicNetSegment>> = vec![
        (segment_with(vec![(junction1, junction1)], f.net), |e| {
            matches!(
                e,
                Error::DegenerateLine {
                    kind: EntityKind::NetLine,
                    ..
                }
            )
        }),
        (segment_with(vec![], f.net), |e| {
            matches!(
                e,
                Error::NetSegmentNotCohesive {
                    after_removal: false,
                    ..
                }
            )
        }),
        (
            segment_with(
                vec![(junction1, NetLineAnchor::Junction(Uuid::new_random()))],
                f.net,
            ),
            |e| matches!(e, Error::InexistentNetPoint(_)),
        ),
        (
            segment_with(
                vec![(
                    junction1,
                    NetLineAnchor::Pin {
                        symbol: Uuid::new_random(),
                        pin: f.lib.pins[0],
                    },
                )],
                f.net,
            ),
            |e| matches!(e, Error::InexistentSymbol(_)),
        ),
        (
            segment_with(
                vec![(
                    junction1,
                    NetLineAnchor::Pin {
                        symbol: symbol.symbol.0,
                        pin: Uuid::new_random(),
                    },
                )],
                f.net,
            ),
            |e| matches!(e, Error::InexistentSymbolPin { .. }),
        ),
        (
            segment_with(
                vec![(
                    junction1,
                    NetLineAnchor::BusJunction {
                        segment: Uuid::new_random(),
                        junction: j2,
                    },
                )],
                f.net,
            ),
            |e| matches!(e, Error::InexistentBusSegment(_)),
        ),
        (
            segment_with(vec![(junction1, junction2)], NetSignalId::new_random()),
            |e| matches!(e, Error::InexistentNetSignal(_)),
        ),
    ];
    for (segment, check) in cases {
        let e = assert_fails(
            &mut f.p,
            sch(M::AddNetSegment {
                schematic: f.schematic,
                segment,
            }),
        );
        assert!(check(&e), "unexpected error: {e:?}");
    }

    // Adding elements: junction (5, 10) connected to the existing one.
    let j3 = Junction::new(Uuid::new_random(), pt(5.0, 10.0));
    let line = NetLine::new(
        Uuid::new_random(),
        width(),
        NetLineAnchor::Junction(junction),
        NetLineAnchor::Junction(j3.uuid()),
    );
    let add = sch(M::AddNetSegmentElements {
        segment: r,
        junctions: vec![j3.clone()],
        lines: vec![line.clone()],
    });
    assert_inverse(&mut f.p, add.clone());
    let e = assert_fails(
        &mut f.p,
        sch(M::AddNetSegmentElements {
            segment: r,
            junctions: vec![Junction::new(Uuid::new_random(), pt(9.0, 9.0))],
            lines: vec![],
        }),
    );
    assert!(matches!(
        e,
        Error::NetSegmentNotCohesive {
            after_removal: false,
            ..
        }
    ));
    f.p.apply(add).unwrap();
    assert!(
        f.schematic().net_segments()[&r.segment]
            .is_visible_junction(NetLineAnchor::Junction(junction))
    );
    let e = assert_fails(
        &mut f.p,
        sch(M::AddNetSegmentElements {
            segment: r,
            junctions: vec![],
            lines: vec![line.clone()],
        }),
    );
    assert!(matches!(
        e,
        Error::DuplicateUuid {
            kind: EntityKind::NetLine,
            ..
        }
    ));

    // Removing elements.
    assert_inverse(
        &mut f.p,
        sch(M::RemoveNetSegmentElements {
            segment: r,
            junctions: vec![j3.uuid()],
            lines: vec![line.uuid()],
        }),
    );
    let e = assert_fails(
        &mut f.p,
        sch(M::RemoveNetSegmentElements {
            segment: r,
            junctions: vec![j3.uuid()],
            lines: vec![],
        }),
    );
    assert!(matches!(
        e,
        Error::ItemInUse {
            kind: EntityKind::NetPoint,
            ..
        }
    ));
    let pin_line = segment
        .lines()
        .values()
        .find(|l| l.p1() == f.pin(symbol, 0))
        .unwrap()
        .uuid();
    let e = assert_fails(
        &mut f.p,
        sch(M::RemoveNetSegmentElements {
            segment: r,
            junctions: vec![],
            lines: vec![line.uuid(), pin_line],
        }),
    );
    // Junction (5, 10) is left without lines.
    assert!(matches!(
        e,
        Error::NetSegmentNotCohesive {
            after_removal: true,
            ..
        }
    ));
    let e = assert_fails(
        &mut f.p,
        sch(M::RemoveNetSegmentElements {
            segment: r,
            junctions: vec![],
            lines: vec![Uuid::new_random()],
        }),
    );
    assert!(matches!(
        e,
        Error::NotFound {
            kind: EntityKind::NetLine,
            ..
        }
    ));
    // Removing the line at pin 1 frees the pin (and its component signal).
    assert_inverse(
        &mut f.p,
        sch(M::RemoveNetSegmentElements {
            segment: r,
            junctions: vec![],
            lines: vec![pin_line],
        }),
    );

    // Properties.
    assert_inverse(
        &mut f.p,
        sch(M::SetNetPointPosition {
            segment: r,
            junction,
            position: pt(3.0, 3.0),
        }),
    );
    assert_inverse(
        &mut f.p,
        sch(M::SetNetLineWidth {
            segment: r,
            line: line.uuid(),
            width: UnsignedLength::new(mm(0.5)).unwrap(),
        }),
    );

    // Labels.
    let label = NetLabel::new(Uuid::new_random(), pt(5.0, 12.0), Angle::DEG0, false);
    assert_inverse(
        &mut f.p,
        sch(M::AddNetLabel {
            segment: r,
            label: label.clone(),
        }),
    );
    f.p.apply(sch(M::AddNetLabel {
        segment: r,
        label: label.clone(),
    }))
    .unwrap();
    assert_eq!(
        f.schematic()
            .net_label_anchor(r.segment, label.uuid(), f.p.view()),
        Some(pt(5.0, 10.0))
    );
    let mut moved = label.clone();
    moved.set_rotation(Angle::DEG90);
    assert_inverse(
        &mut f.p,
        sch(M::UpdateNetLabel {
            segment: r,
            label: moved,
        }),
    );
    assert_inverse(
        &mut f.p,
        sch(M::RemoveNetLabel {
            segment: r,
            label: label.uuid(),
        }),
    );

    // Net of a segment: only while empty.
    let e = assert_fails(
        &mut f.p,
        sch(M::SetNetSegmentNet {
            segment: r,
            net: f.other_net,
        }),
    );
    assert!(matches!(
        e,
        Error::ItemInUse {
            kind: EntityKind::NetSegment,
            ..
        }
    ));
    let empty = SchematicNetSegment::new(Uuid::new_random(), f.net);
    let empty_ref = f.segment_ref(&empty);
    f.p.add_net_segment(f.schematic, empty).unwrap();
    assert_inverse(
        &mut f.p,
        sch(M::SetNetSegmentNet {
            segment: empty_ref,
            net: f.other_net,
        }),
    );

    // Removal of the segment.
    assert_inverse(&mut f.p, sch(M::RemoveNetSegment(r)));
    f.p.remove_net_segment(r).unwrap();
    f.p.remove_net_segment(empty_ref).unwrap();
    assert!(!f.p.is_net_signal_used(f.other_net));
    f.p.remove_symbol(symbol).unwrap();
    assert!(f.p.is_ref_index_consistent());
}

// --- Bus segments ---

#[test]
fn bus_segments() {
    let mut f = Fixture::new(1);
    let symbol = f.add_symbol();
    let bus =
        f.p.add_bus(Bus::new(
            Uuid::new_random(),
            BusName::new("D[0..7]").unwrap(),
            false,
            false,
            None,
        ))
        .unwrap();
    let (b1, b2) = (Uuid::new_random(), Uuid::new_random());
    let mut segment = SchematicBusSegment::new(Uuid::new_random(), bus);
    segment.insert_junction(Junction::new(b1, pt(20.0, 0.0)));
    segment.insert_junction(Junction::new(b2, pt(20.0, 10.0)));
    let bus_line = Uuid::new_random();
    segment.insert_line(NetLine::new(
        bus_line,
        SchematicBusSegment::default_line_width(),
        NetLineAnchor::Junction(b1),
        NetLineAnchor::Junction(b2),
    ));
    let r = BusSegmentRef {
        schematic: f.schematic,
        segment: segment.id(),
    };
    assert_inverse(
        &mut f.p,
        sch(M::AddBusSegment {
            schematic: f.schematic,
            segment: segment.clone(),
        }),
    );

    // Invalid segments.
    let with_bus = |bus: BusId| {
        let mut s = SchematicBusSegment::new(segment.uuid(), bus);
        for junction in segment.junctions().values() {
            s.insert_junction(junction.clone());
        }
        for line in segment.lines().values() {
            s.insert_line(line.clone());
        }
        s
    };
    let unknown_bus = with_bus(BusId::new_random());
    let mut not_cohesive = segment.clone();
    not_cohesive.insert_junction(Junction::new(Uuid::new_random(), pt(0.0, 0.0)));
    let mut missing_junction = segment.clone();
    missing_junction.insert_line(NetLine::new(
        Uuid::new_random(),
        width(),
        NetLineAnchor::Junction(b1),
        NetLineAnchor::Junction(Uuid::new_random()),
    ));
    let mut degenerate = segment.clone();
    degenerate.insert_line(NetLine::new(
        Uuid::new_random(),
        width(),
        NetLineAnchor::Junction(b1),
        NetLineAnchor::Junction(b1),
    ));
    let cases: Vec<ErrorCase<SchematicBusSegment>> = vec![
        (unknown_bus, |e| matches!(e, Error::InexistentBus(_))),
        (not_cohesive, |e| {
            matches!(e, Error::BusSegmentNotCohesive(_))
        }),
        (missing_junction, |e| {
            matches!(e, Error::InexistentBusJunction { .. })
        }),
        (degenerate, |e| {
            matches!(
                e,
                Error::DegenerateLine {
                    kind: EntityKind::BusLine,
                    ..
                }
            )
        }),
    ];
    for (segment, check) in cases {
        let e = assert_fails(
            &mut f.p,
            sch(M::AddBusSegment {
                schematic: f.schematic,
                segment,
            }),
        );
        assert!(check(&e), "unexpected error: {e:?}");
    }
    f.p.add_bus_segment(f.schematic, segment.clone()).unwrap();

    // A net segment from pin 2 to bus junction 1.
    let mut net_segment = SchematicNetSegment::new(Uuid::new_random(), f.net);
    let bus_anchor = NetLineAnchor::BusJunction {
        segment: r.segment.0,
        junction: b1,
    };
    net_segment.insert_line(NetLine::new(
        Uuid::new_random(),
        width(),
        f.pin(symbol, 1),
        bus_anchor,
    ));
    let mut missing = net_segment.clone();
    missing.insert_line(NetLine::new(
        Uuid::new_random(),
        width(),
        f.pin(symbol, 1),
        NetLineAnchor::BusJunction {
            segment: r.segment.0,
            junction: Uuid::new_random(),
        },
    ));
    let e = assert_fails(
        &mut f.p,
        sch(M::AddNetSegment {
            schematic: f.schematic,
            segment: missing,
        }),
    );
    assert!(matches!(e, Error::InexistentBusJunction { .. }));
    assert_inverse(
        &mut f.p,
        sch(M::AddNetSegment {
            schematic: f.schematic,
            segment: net_segment.clone(),
        }),
    );
    f.p.add_net_segment(f.schematic, net_segment.clone())
        .unwrap();
    let s = f.schematic();
    assert_eq!(s.attached_net_segments(r.segment), vec![net_segment.id()]);
    assert_eq!(
        s.net_line_anchor_position(net_segment.id(), bus_anchor, f.p.view()),
        Some(pt(20.0, 0.0))
    );

    // Attached net lines lock the bus junction and the segment.
    let e = assert_fails(&mut f.p, sch(M::RemoveBusSegment(r)));
    assert!(matches!(
        e,
        Error::ItemInUse {
            kind: EntityKind::BusSegment,
            ..
        }
    ));
    let e = assert_fails(
        &mut f.p,
        sch(M::RemoveBusSegmentElements {
            segment: r,
            junctions: vec![b1],
            lines: vec![bus_line],
        }),
    );
    assert!(matches!(
        e,
        Error::ItemInUse {
            kind: EntityKind::BusJunction,
            ..
        }
    ));
    let e = assert_fails(&mut f.p, Mutation::RemoveBus(bus));
    assert!(matches!(e, Error::BusInUse(_)));
    let e = assert_fails(
        &mut f.p,
        sch(M::SetBusSegmentBus {
            segment: r,
            bus: BusId::new_random(),
        }),
    );
    assert!(matches!(e, Error::InexistentBus(_)));

    // Elements and properties.
    let b3 = Junction::new(Uuid::new_random(), pt(30.0, 10.0));
    let line = NetLine::new(
        Uuid::new_random(),
        width(),
        NetLineAnchor::Junction(b2),
        NetLineAnchor::Junction(b3.uuid()),
    );
    assert_inverse(
        &mut f.p,
        sch(M::AddBusSegmentElements {
            segment: r,
            junctions: vec![b3.clone()],
            lines: vec![line.clone()],
        }),
    );
    f.p.apply(sch(M::AddBusSegmentElements {
        segment: r,
        junctions: vec![b3.clone()],
        lines: vec![line.clone()],
    }))
    .unwrap();
    let e = assert_fails(
        &mut f.p,
        sch(M::RemoveBusSegmentElements {
            segment: r,
            junctions: vec![],
            lines: vec![bus_line],
        }),
    );
    assert!(matches!(e, Error::BusSegmentNotCohesive(_)));
    assert_inverse(
        &mut f.p,
        sch(M::RemoveBusSegmentElements {
            segment: r,
            junctions: vec![b3.uuid()],
            lines: vec![line.uuid()],
        }),
    );
    assert_inverse(
        &mut f.p,
        sch(M::SetBusJunctionPosition {
            segment: r,
            junction: b1,
            position: pt(25.0, 0.0),
        }),
    );
    assert_inverse(
        &mut f.p,
        sch(M::SetBusLineWidth {
            segment: r,
            line: bus_line,
            width: UnsignedLength::new(mm(1.0)).unwrap(),
        }),
    );
    let label = NetLabel::new(Uuid::new_random(), pt(22.0, 5.0), Angle::DEG0, false);
    assert_inverse(
        &mut f.p,
        sch(M::AddBusLabel {
            segment: r,
            label: label.clone(),
        }),
    );
    f.p.apply(sch(M::AddBusLabel {
        segment: r,
        label: label.clone(),
    }))
    .unwrap();
    assert_eq!(
        f.schematic().bus_label_anchor(r.segment, label.uuid()),
        Some(pt(20.0, 5.0))
    );
    let mut mirrored = label.clone();
    mirrored.set_mirrored(true);
    assert_inverse(
        &mut f.p,
        sch(M::UpdateBusLabel {
            segment: r,
            label: mirrored,
        }),
    );
    assert_inverse(
        &mut f.p,
        sch(M::RemoveBusLabel {
            segment: r,
            label: label.uuid(),
        }),
    );

    // Once the net segment is gone, the bus segment can be removed.
    f.p.remove_net_segment(f.segment_ref(&net_segment)).unwrap();
    assert_inverse(&mut f.p, sch(M::RemoveBusSegment(r)));

    // The bus of a segment changes only while it is empty.
    let other_bus =
        f.p.add_bus(Bus::new(
            Uuid::new_random(),
            BusName::new("A[0..7]").unwrap(),
            false,
            false,
            None,
        ))
        .unwrap();
    let e = assert_fails(
        &mut f.p,
        sch(M::SetBusSegmentBus {
            segment: r,
            bus: other_bus,
        }),
    );
    assert!(matches!(
        e,
        Error::ItemInUse {
            kind: EntityKind::BusSegment,
            ..
        }
    ));
    let empty = SchematicBusSegment::new(Uuid::new_random(), bus);
    let empty_ref = BusSegmentRef {
        schematic: f.schematic,
        segment: empty.id(),
    };
    f.p.add_bus_segment(f.schematic, empty).unwrap();
    assert_inverse(
        &mut f.p,
        sch(M::SetBusSegmentBus {
            segment: empty_ref,
            bus: other_bus,
        }),
    );
    f.p.remove_bus_segment(empty_ref).unwrap();
    f.p.remove_bus_segment(r).unwrap();
    f.p.remove_bus(bus).unwrap();
    assert!(f.p.is_ref_index_consistent());
}

// --- Polygons, texts, images ---

#[test]
fn polygons_texts_images() {
    let mut f = Fixture::new(1);
    let schematic = f.schematic;
    let polygon = Polygon::new(
        Uuid::new_random(),
        Layer::from_id("sch_comments").unwrap(),
        UnsignedLength::new(mm(0.2)).unwrap(),
        false,
        false,
        Path::new(vec![Vertex::at(pt(0.0, 0.0)), Vertex::at(pt(10.0, 0.0))]),
    );
    let t = text(Uuid::new_random(), "Hello", pt(1.0, 1.0));
    let image = Image::new(
        Uuid::new_random(),
        FileProofName::new("logo.png").unwrap(),
        Point::ORIGIN,
        Angle::DEG0,
        PositiveLength::new(mm(5.0)).unwrap(),
        PositiveLength::new(mm(5.0)).unwrap(),
        None,
    );
    for (add, remove, update, duplicate) in [
        (
            M::AddPolygon {
                schematic,
                polygon: polygon.clone(),
            },
            M::RemovePolygon {
                schematic,
                polygon: polygon.uuid(),
            },
            M::UpdatePolygon {
                schematic,
                polygon: {
                    let mut p = polygon.clone();
                    p.set_is_filled(true);
                    p
                },
            },
            EntityKind::SchematicPolygon,
        ),
        (
            M::AddText {
                schematic,
                text: t.clone(),
            },
            M::RemoveText {
                schematic,
                text: t.uuid(),
            },
            M::UpdateText {
                schematic,
                text: {
                    let mut t = t.clone();
                    t.set_locked(true);
                    t
                },
            },
            EntityKind::SchematicText,
        ),
        (
            M::AddImage {
                schematic,
                image: image.clone(),
            },
            M::RemoveImage {
                schematic,
                image: image.uuid(),
            },
            M::UpdateImage {
                schematic,
                image: {
                    let mut i = image.clone();
                    i.set_rotation(Angle::DEG90);
                    i
                },
            },
            EntityKind::SchematicImage,
        ),
    ] {
        assert_inverse(&mut f.p, sch(add.clone()));
        let e = assert_fails(&mut f.p, sch(remove.clone()));
        assert!(matches!(e, Error::NotFound { .. }));
        let e = assert_fails(&mut f.p, sch(update.clone()));
        assert!(matches!(e, Error::NotFound { .. }));
        f.p.apply(sch(add.clone())).unwrap();
        let e = assert_fails(&mut f.p, sch(add));
        assert!(matches!(e, Error::DuplicateUuid { kind, .. } if kind == duplicate));
        assert_inverse(&mut f.p, sch(update));
        assert_inverse(&mut f.p, sch(remove));
    }
    let s = f.schematic();
    assert_eq!(
        (s.polygons().len(), s.texts().len(), s.images().len()),
        (1, 1, 1)
    );
    let e = assert_fails(&mut f.p, Mutation::RemoveSchematic(schematic));
    assert!(matches!(e, Error::SchematicNotEmpty(_)));
}

// --- Whole schematics, batches, journal ---

/// A schematic value with a symbol, a wired net segment with a label, a
/// bus segment, a polygon, a text and an image.
fn full_schematic(f: &Fixture, name: &str, directory: &str) -> Schematic {
    let mut s = Schematic::new(
        Uuid::new_random(),
        ElementName::new(name).unwrap(),
        directory,
    );
    let symbol = f.new_symbol(0);
    let symbol_ref = SymbolRef {
        schematic: s.id(),
        symbol: symbol.id(),
    };
    let mut symbol = symbol;
    symbol.insert_text(text(Uuid::new_random(), "{{NAME}}", pt(0.0, 3.0)));
    s.insert_symbol(symbol);
    let (mut segment, _) = f.wire(symbol_ref);
    segment.insert_label(NetLabel::new(
        Uuid::new_random(),
        pt(5.0, 6.0),
        Angle::DEG0,
        false,
    ));
    s.insert_net_segment(segment);
    s.insert_text(text(Uuid::new_random(), "Title", pt(50.0, 50.0)));
    s.insert_polygon(Polygon::new(
        Uuid::new_random(),
        Layer::from_id("sch_frames").unwrap(),
        UnsignedLength::new(mm(0.2)).unwrap(),
        false,
        true,
        Path::new(vec![Vertex::at(pt(0.0, 0.0)), Vertex::at(pt(0.0, 10.0))]),
    ));
    s.insert_image(Image::new(
        Uuid::new_random(),
        FileProofName::new("logo.png").unwrap(),
        Point::ORIGIN,
        Angle::DEG0,
        PositiveLength::new(mm(5.0)).unwrap(),
        PositiveLength::new(mm(5.0)).unwrap(),
        Some(UnsignedLength::new(mm(0.1)).unwrap()),
    ));
    s
}

#[test]
fn add_whole_schematic() {
    let mut f = Fixture::new(1);
    // The fixture's page is empty; place the component on a new page.
    let full = full_schematic(&f, "Page 2", "page2");
    assert_inverse(
        &mut f.p,
        Mutation::AddSchematic {
            schematic: full.clone(),
            index: None,
        },
    );

    // The same gate twice in one page.
    let mut twice = full.clone();
    let mut symbol = f.new_symbol(0);
    symbol.set_position(pt(30.0, 0.0));
    twice.insert_symbol(symbol);
    let e = assert_fails(
        &mut f.p,
        Mutation::AddSchematic {
            schematic: twice,
            index: None,
        },
    );
    assert!(matches!(e, Error::SymbolItemAlreadyPlaced(_)));

    // A page whose net segment is invalid is rejected as a whole.
    let mut invalid = full.clone();
    let segment = invalid.net_segments().values().next().unwrap().clone();
    let mut segment_other_net = SchematicNetSegment::new(segment.uuid(), f.other_net);
    for junction in segment.junctions().values() {
        segment_other_net.insert_junction(junction.clone());
    }
    for line in segment.lines().values() {
        segment_other_net.insert_line(line.clone());
    }
    invalid.insert_net_segment(segment_other_net);
    let e = assert_fails(
        &mut f.p,
        Mutation::AddSchematic {
            schematic: invalid,
            index: None,
        },
    );
    assert!(matches!(e, Error::AnchorNetMismatch { .. }));

    // A failed batch leaves the published revision intact.
    f.p.add_schematic(full.clone(), None).unwrap();
    let before = snapshot(&f.p);
    let before_revision = f.p.revision();
    let e =
        f.p.apply(Mutation::Batch(vec![
            sch(M::AddText {
                schematic: full.id(),
                text: text(Uuid::new_random(), "ok", Point::ORIGIN),
            }),
            sch(M::RemoveSymbol(SymbolRef {
                schematic: full.id(),
                symbol: *full.symbols().keys().next().unwrap(),
            })),
        ]))
        .unwrap_err();
    assert!(matches!(e, Error::ItemInUse { .. }));
    assert_eq!(snapshot(&f.p), before);
    assert_eq!(f.p.revision(), before_revision);
    assert!(f.p.is_ref_index_consistent());

    // Journal.
    let revision = f.p.revision();
    let label = NetLabel::new(Uuid::new_random(), Point::ORIGIN, Angle::DEG0, false);
    let segment = *full.net_segments().keys().next().unwrap();
    f.p.apply(sch(M::AddNetLabel {
        segment: NetSegmentRef {
            schematic: full.id(),
            segment,
        },
        label: label.clone(),
    }))
    .unwrap();
    assert_eq!(
        f.p.changes_since(revision),
        ChangesSince::Changes(&[Change::Schematic {
            id: full.id(),
            change: SchematicChange::NetLabelAdded {
                segment,
                label: label.uuid(),
            },
        }])
    );
}

#[test]
fn save_and_reopen() {
    let temp = TempDir::new();
    let fs = Arc::new(TransactionalFileSystem::open_rw(temp.path()).unwrap());
    let p = Project::create(
        TransactionalDirectory::new(Arc::clone(&fs), ""),
        "test.lpp",
        Uuid::new_random,
    )
    .unwrap();
    let mut f = Fixture::in_project(p, 1);
    let full = full_schematic(&f, "Page 2", "page2");
    f.p.add_schematic(full.clone(), None).unwrap();
    let before = f.p.schematics().to_vec();
    let mut p = f.p;
    p.save().unwrap();
    fs.save().unwrap();
    drop(p);
    fs.release_lock().unwrap();

    let fs = Arc::new(TransactionalFileSystem::open_ro(temp.path()).unwrap());
    let reopened =
        Project::open(TransactionalDirectory::new(Arc::clone(&fs), ""), "test.lpp").unwrap();
    assert!(reopened.is_ref_index_consistent());
    assert_eq!(reopened.schematics(), before.as_slice());
    let mut reopened = reopened;
    reopened.save().unwrap();
    assert!(
        reopened
            .directory()
            .file_system()
            .check_for_modifications()
            .unwrap()
            .is_empty()
    );
}

// --- Splitter ---

#[test]
fn net_segment_splitter() {
    let j: Vec<Junction> = (0..5)
        .map(|i| Junction::new(Uuid::new_random(), pt(10.0 * i as f64, 0.0)))
        .collect();
    let a = |i: usize| NetLineAnchor::Junction(j[i].uuid());
    let pin = NetLineAnchor::Pin {
        symbol: Uuid::new_random(),
        pin: Uuid::new_random(),
    };
    let lines = [
        NetLine::new(Uuid::new_random(), width(), a(0), a(1)),
        NetLine::new(Uuid::new_random(), width(), a(2), a(3)),
        NetLine::new(Uuid::new_random(), width(), a(1), pin),
    ];
    let mut splitter = SchematicNetSegmentSplitter::new();
    splitter.add_fixed_anchor(pin, pt(10.0, 10.0), false);
    for junction in &j {
        splitter.add_junction(junction.clone());
    }
    for line in &lines {
        splitter.add_net_line(line.clone());
    }
    let label = NetLabel::new(Uuid::new_random(), pt(12.0, 8.0), Angle::DEG0, false);
    splitter.add_net_label(label.clone());
    let segments = splitter.split();
    let sorted = |junctions: &[Junction]| {
        let mut uuids: Vec<Uuid> = junctions.iter().map(Junction::uuid).collect();
        uuids.sort();
        uuids
    };
    assert_eq!(segments.len(), 2);
    assert_eq!(
        sorted(&segments[0].junctions),
        sorted(&[j[0].clone(), j[1].clone()]),
        "junction 4 has no lines and is dropped"
    );
    assert_eq!(segments[0].lines, vec![lines[0].clone(), lines[2].clone()]);
    assert_eq!(segments[0].labels, vec![label]);
    assert_eq!(
        sorted(&segments[1].junctions),
        sorted(&[j[2].clone(), j[3].clone()])
    );
    assert_eq!(segments[1].lines, vec![lines[1].clone()]);

    // Replacing the fixed anchor by a new junction.
    let mut splitter = SchematicNetSegmentSplitter::new();
    splitter.add_fixed_anchor(pin, pt(10.0, 10.0), true);
    splitter.add_junction(j[1].clone());
    splitter.add_net_line(lines[2].clone());
    let segments = splitter.split();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].junctions.len(), 2);
    let new_junction = segments[0]
        .junctions
        .iter()
        .find(|x| x.uuid() != j[1].uuid())
        .unwrap();
    assert_eq!(new_junction.position(), pt(10.0, 10.0));
    assert_eq!(
        segments[0].lines[0].p1(),
        NetLineAnchor::Junction(new_junction.uuid()).min(a(1))
    );
}

// --- Serde ---

#[test]
fn serde_roundtrip() {
    let f = Fixture::new(1);
    let full = full_schematic(&f, "Main", "main");
    let symbol = full.symbols().values().next().unwrap().clone();
    let segment = full.net_segments().values().next().unwrap().clone();
    let r = NetSegmentRef {
        schematic: full.id(),
        segment: segment.id(),
    };
    let mutations = Mutation::Batch(vec![
        Mutation::AddSchematic {
            schematic: full.clone(),
            index: Some(0),
        },
        sch(M::AddSymbol {
            schematic: full.id(),
            symbol,
        }),
        sch(M::AddNetSegment {
            schematic: full.id(),
            segment: segment.clone(),
        }),
        sch(M::AddNetSegmentElements {
            segment: r,
            junctions: segment.junctions().values().cloned().collect(),
            lines: segment.lines().values().cloned().collect(),
        }),
        sch(M::SetNetLineWidth {
            segment: r,
            line: Uuid::new_random(),
            width: width(),
        }),
        sch(M::AddBusSegment {
            schematic: full.id(),
            segment: SchematicBusSegment::new(Uuid::new_random(), BusId::new_random()),
        }),
        sch(M::UpdateText {
            schematic: full.id(),
            text: full.texts().values().next().unwrap().clone(),
        }),
    ]);
    let json = serde_json::to_string_pretty(&mutations).unwrap();
    assert_eq!(serde_json::from_str::<Mutation>(&json).unwrap(), mutations);

    let change = Change::Schematic {
        id: full.id(),
        change: SchematicChange::NetSegmentElementsAdded {
            segment: segment.id(),
            junctions: vec![Uuid::new_random()],
            lines: vec![],
        },
    };
    let json = serde_json::to_string(&change).unwrap();
    assert_eq!(serde_json::from_str::<Change>(&json).unwrap(), change);

    // Wire forms.
    let anchor = NetLineAnchor::Junction("d2c30518-5cd1-4ce9-a569-44f783a3f66a".parse().unwrap());
    assert_eq!(
        serde_json::to_string(&anchor).unwrap(),
        "{\"Junction\":\"d2c30518-5cd1-4ce9-a569-44f783a3f66a\"}"
    );
    assert_eq!(
        serde_json::to_string(&Alignment::new(HAlign::Left, VAlign::Bottom)).unwrap(),
        "{\"h\":\"left\",\"v\":\"bottom\"}"
    );
    // Net line anchors are normalized when deserializing.
    let line = segment.lines().values().next().unwrap();
    let swapped = serde_json::json!({
        "uuid": line.uuid(),
        "width": line.width(),
        "p1": line.p2(),
        "p2": line.p1(),
    });
    assert_eq!(&serde_json::from_value::<NetLine>(swapped).unwrap(), line);
    // Map keys must match the UUIDs of their values.
    let mut value = serde_json::to_value(&segment).unwrap();
    let junctions = value["junctions"].as_object_mut().unwrap();
    let (key, junction) = junctions
        .iter()
        .next()
        .map(|(k, v)| (k.clone(), v.clone()))
        .unwrap();
    junctions.remove(&key);
    junctions.insert(Uuid::new_random().to_string(), junction);
    assert!(serde_json::from_value::<SchematicNetSegment>(value).is_err());
}

#[test]
fn symbol_attribute_lookup() {
    use foilfern_core::attribute::{Attribute, AttributeList, AttributeType};
    use foilfern_core::project::ProjectAttributeLookup;

    let f = Fixture::new(2);
    let symbol = f.new_symbol(0);
    let schematic = f.p.schematic(f.schematic).unwrap();
    let variant = f.p.circuit().assembly_variants().first().unwrap();
    let lookup =
        ProjectAttributeLookup::for_symbol(&f.p, schematic, &symbol, None, None, Some(variant));
    assert_eq!(lookup.value("NAME").as_deref(), Some("U1-A"));
    assert_eq!(lookup.value("SHEET").as_deref(), Some("Main"));
    assert_eq!(lookup.value("VARIANT").as_deref(), Some("Std"));
    assert_eq!(
        lookup.substitute("{{NAME}}: {{PAGE_X_OF_Y}}"),
        "U1-A: Page 1 of 1"
    );
    let part = foilfern_core::library::dev::Part::new(
        "MPN".parse().unwrap(),
        "Maker".parse().unwrap(),
        AttributeList::from(vec![
            Attribute::new(
                "NAME".parse().unwrap(),
                AttributeType::String,
                "part name",
                None,
            )
            .unwrap(),
        ]),
    );
    let lookup =
        ProjectAttributeLookup::for_symbol(&f.p, schematic, &symbol, None, Some(&part), None);
    assert_eq!(lookup.value("NAME").as_deref(), Some("part name"));
    assert_eq!(lookup.value("VARIANT"), None);
}
