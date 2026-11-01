//! ERC connectivity and highlighting on the schematic mutation fixtures.

use super::*;
use librepcb_core::project::erc::{
    ElectricalRuleCheck, ElectricalRuleCheckMessage, ErcMessageKind as Kind,
};
use librepcb_core::rule_check::Severity;
use librepcb_core::serialization::Mode;

fn check(p: &Project) -> Vec<ElectricalRuleCheckMessage> {
    ElectricalRuleCheck::new(p).run_checks().unwrap()
}

fn count(p: &Project, predicate: impl Fn(&Kind) -> bool) -> usize {
    check(p).iter().filter(|m| predicate(m.kind())).count()
}

#[test]
fn unplaced_gates_required_optional_and_fallback_locations() {
    let mut f = Fixture::new(2);
    let mut lib = f.p.remove_library_component(&f.lib.uuid).unwrap();
    lib.symbol_variants_mut()
        .by_uuid_mut(&f.lib.variant)
        .unwrap()
        .symbol_items_mut()
        .by_uuid_mut(&f.lib.gates[1])
        .unwrap()
        .set_is_required(false);
    f.p.add_library_component(lib).unwrap();
    let messages = check(&f.p);
    let required = messages
        .iter()
        .find(|m| matches!(m.kind(), Kind::UnplacedRequiredGate { .. }))
        .unwrap();
    assert_eq!(
        required.rule_message().message(),
        "Unplaced required gate: 'U1:A'"
    );
    assert_eq!(required.rule_message().severity(), Severity::Error);
    assert!(required.rule_message().locations().is_empty());
    let optional = messages
        .iter()
        .find(|m| matches!(m.kind(), Kind::UnplacedOptionalGate { .. }))
        .unwrap();
    assert_eq!(optional.rule_message().message(), "Unplaced gate: 'U1:B'");
    assert_eq!(optional.rule_message().severity(), Severity::Warning);
    assert_eq!(
        optional
            .rule_message()
            .approval()
            .to_string_with_mode(Mode::LibrePcb)
            .unwrap(),
        format!(
            "(approved unplaced_optional_gate\n (component {})\n (gate {})\n)\n",
            f.component.0, f.lib.gates[1]
        )
    );
    f.add_symbol();
    assert_eq!(
        count(&f.p, |k| matches!(k, Kind::UnplacedRequiredGate { .. })),
        0
    );
    let messages = check(&f.p);
    let optional = messages
        .iter()
        .find(|m| matches!(m.kind(), Kind::UnplacedOptionalGate { .. }))
        .unwrap();
    assert_eq!(optional.schematic(), Some(f.schematic));
    assert_eq!(
        optional.rule_message().locations(),
        [Path::circle(pos_len(2_000_000))]
    );
}

fn pos_len(nm: i64) -> PositiveLength {
    PositiveLength::new(Length::new(nm)).unwrap()
}

#[test]
fn connected_pins_without_wires_use_display_names_and_pin_locations() {
    let mut f = Fixture::new(1);
    let symbol = f.add_symbol();
    let messages = check(&f.p);
    let pins: Vec<_> = messages
        .iter()
        .filter(|m| matches!(m.kind(), Kind::ConnectedPinWithoutWire { .. }))
        .collect();
    assert_eq!(pins.len(), 2);
    let pin = pins.iter().find(|m| matches!(m.kind(), Kind::ConnectedPinWithoutWire { pin, .. } if *pin == f.lib.pins[0])).unwrap();
    assert_eq!(
        pin.rule_message().message(),
        "Connected pin without wire: 'U1:S1'"
    );
    assert_eq!(pin.schematic(), Some(f.schematic));
    assert_eq!(
        pin.rule_message().locations(),
        [Path::circle(pos_len(1_100_000))]
    );
    assert_eq!(
        pin.rule_message()
            .approval()
            .to_string_with_mode(Mode::LibrePcb)
            .unwrap(),
        format!(
            "(approved connected_pin_without_wire\n (schematic {})\n (symbol {})\n (pin {})\n)\n",
            f.schematic.0, symbol.symbol.0, f.lib.pins[0]
        )
    );
    let mut segment = SchematicNetSegment::new(Uuid::new_random(), f.net);
    segment.insert_line(NetLine::new(
        Uuid::new_random(),
        width(),
        f.pin(symbol, 0),
        f.pin(symbol, 1),
    ));
    f.p.add_net_segment(f.schematic, segment).unwrap();
    assert_eq!(
        count(&f.p, |k| matches!(k, Kind::ConnectedPinWithoutWire { .. })),
        0
    );
}

#[test]
fn open_wire_suppression_and_exact_segment_approval() {
    let mut f = Fixture::new(1);
    let symbol = f.add_symbol();
    let junction = Uuid::new_random();
    let mut segment = SchematicNetSegment::new(Uuid::new_random(), f.net);
    segment.insert_junction(Junction::new(junction, pt(20.0, 0.0)));
    let line = NetLine::new(
        Uuid::new_random(),
        width(),
        f.pin(symbol, 0),
        NetLineAnchor::Junction(junction),
    );
    segment.insert_line(line.clone());
    let segment_id = segment.id();
    f.p.add_net_segment(f.schematic, segment.clone()).unwrap();
    let messages = check(&f.p);
    let finding = messages
        .iter()
        .find(|m| matches!(m.kind(), Kind::OpenWire { .. }))
        .unwrap();
    assert_eq!(finding.rule_message().message(), "Open wire in net: 'N1'");
    assert_eq!(
        finding
            .rule_message()
            .approval()
            .to_string_with_mode(Mode::LibrePcb)
            .unwrap(),
        format!("(approved open_wire (segment {}))\n", segment_id.0)
    );
    assert_eq!(
        finding.rule_message().locations(),
        [Path::obround_line(
            Point::ORIGIN,
            pt(20.0, 0.0),
            pos_len(width().to_nm() + 1)
        )]
    );
    // Label anywhere on the segment suppresses open-wire warnings.
    f.p.remove_net_segment(NetSegmentRef {
        schematic: f.schematic,
        segment: segment_id,
    })
    .unwrap();
    segment.insert_label(NetLabel::new(
        Uuid::new_random(),
        Point::ORIGIN,
        Angle::DEG0,
        false,
    ));
    f.p.add_net_segment(f.schematic, segment).unwrap();
    assert_eq!(count(&f.p, |k| matches!(k, Kind::OpenWire { .. })), 0);
    // Open-net warnings also suppress open-wire warnings.
    let mut f = Fixture::new(1);
    let symbol = f.add_symbol();
    f.p.apply(Mutation::SetComponentSignalNet {
        signal: ComponentSignalRef {
            component: f.component,
            signal: f.lib.signals[1],
        },
        net: None,
    })
    .unwrap();
    let mut segment = SchematicNetSegment::new(Uuid::new_random(), f.net);
    segment.insert_junction(Junction::new(junction, pt(20.0, 0.0)));
    segment.insert_line(NetLine::new(
        line.uuid(),
        width(),
        f.pin(symbol, 0),
        NetLineAnchor::Junction(junction),
    ));
    f.p.add_net_segment(f.schematic, segment).unwrap();
    assert_eq!(count(&f.p, |k| matches!(k, Kind::OpenWire { .. })), 0);
    assert_eq!(
        count(&f.p, |k| matches!(k, Kind::OpenNet(id) if *id == f.net)),
        1
    );
}

#[test]
fn isolated_net_and_bus_junctions_have_distinct_approval_fields() {
    let mut f = Fixture::new(1);
    let mut segment = SchematicNetSegment::new(Uuid::new_random(), f.net);
    let junction = Uuid::new_random();
    segment.insert_junction(Junction::new(junction, Point::ORIGIN));
    let id = segment.id();
    f.p.add_net_segment(f.schematic, segment).unwrap();
    let bus =
        f.p.add_bus(Bus::new(
            Uuid::new_random(),
            "BUS".parse().unwrap(),
            false,
            false,
            None,
        ))
        .unwrap();
    let mut bus_segment = SchematicBusSegment::new(Uuid::new_random(), bus);
    let bus_junction = Uuid::new_random();
    bus_segment.insert_junction(Junction::new(bus_junction, pt(10.0, 0.0)));
    let bus_id = bus_segment.id();
    f.p.add_bus_segment(f.schematic, bus_segment).unwrap();
    let messages = check(&f.p);
    let net_finding = messages
        .iter()
        .find(|m| matches!(m.kind(), Kind::UnconnectedNetJunction { .. }))
        .unwrap();
    let bus_finding = messages
        .iter()
        .find(|m| matches!(m.kind(), Kind::UnconnectedBusJunction { .. }))
        .unwrap();
    assert_eq!(
        net_finding
            .rule_message()
            .approval()
            .to_string_with_mode(Mode::LibrePcb)
            .unwrap(),
        format!(
            "(approved unconnected_junction\n (schematic {})\n (netsegment {})\n (junction {})\n)\n",
            f.schematic.0, id.0, junction
        )
    );
    assert_eq!(
        bus_finding
            .rule_message()
            .approval()
            .to_string_with_mode(Mode::LibrePcb)
            .unwrap(),
        format!(
            "(approved unconnected_junction\n (schematic {})\n (bussegment {})\n (junction {})\n)\n",
            f.schematic.0, bus_id.0, bus_junction
        )
    );
    assert_eq!(
        bus_finding.rule_message().locations(),
        [Path::circle(pos_len(2_000_000)).translated(pt(10.0, 0.0))]
    );
}

#[test]
fn bus_rules_group_by_net_and_prefer_unnamed_over_open() {
    let mut f = Fixture::new(1);
    let symbol = f.add_symbol();
    let bus =
        f.p.add_bus(Bus::new(
            Uuid::new_random(),
            "BUS".parse().unwrap(),
            false,
            false,
            None,
        ))
        .unwrap();
    let mut segments = Vec::new();
    for pin in 0..2 {
        let mut bus_segment = SchematicBusSegment::new(Uuid::new_random(), bus);
        let junction = Uuid::new_random();
        bus_segment.insert_junction(Junction::new(junction, pt(20.0 + pin as f64, 0.0)));
        let bus_id = bus_segment.id();
        f.p.add_bus_segment(f.schematic, bus_segment).unwrap();
        let mut segment = SchematicNetSegment::new(Uuid::new_random(), f.net);
        segment.insert_line(NetLine::new(
            Uuid::new_random(),
            width(),
            f.pin(symbol, pin),
            NetLineAnchor::BusJunction {
                segment: bus_id.0,
                junction,
            },
        ));
        segments.push(segment);
    }
    // One labeled connection is a net which never leaves the bus.
    segments[0].insert_label(NetLabel::new(
        Uuid::new_random(),
        Point::ORIGIN,
        Angle::DEG0,
        false,
    ));
    let first = segments[0].id();
    f.p.add_net_segment(f.schematic, segments[0].clone())
        .unwrap();
    let messages = check(&f.p);
    let finding = messages
        .iter()
        .find(|m| matches!(m.kind(), Kind::OpenNetInBus { .. }))
        .unwrap();
    assert_eq!(
        finding.rule_message().message(),
        "Bus contains unused net: 'BUS:N1'"
    );
    assert_eq!(finding.rule_message().severity(), Severity::Hint);
    assert_eq!(
        finding
            .rule_message()
            .approval()
            .to_string_with_mode(Mode::LibrePcb)
            .unwrap(),
        format!(
            "(approved open_net_in_bus\n (bus {})\n (net {})\n)\n",
            bus.0, f.net.0
        )
    );
    // An unlabeled second segment produces one unnamed-net warning,
    // suppressing the open-net-in-bus message for the same net.
    let second = segments[1].id();
    f.p.add_net_segment(f.schematic, segments[1].clone())
        .unwrap();
    assert_eq!(
        count(&f.p, |k| matches!(k, Kind::UnnamedNetInBus { .. })),
        1
    );
    assert_eq!(count(&f.p, |k| matches!(k, Kind::OpenNetInBus { .. })), 0);
    let messages = check(&f.p);
    let finding = messages
        .iter()
        .find(|m| matches!(m.kind(), Kind::UnnamedNetInBus { .. }))
        .unwrap();
    assert_eq!(
        finding.rule_message().message(),
        "Bus contains unnamed net: 'BUS:N1'"
    );
    assert_eq!(
        finding
            .rule_message()
            .approval()
            .to_string_with_mode(Mode::LibrePcb)
            .unwrap(),
        format!(
            "(approved unnamed_net_in_bus\n (bus {})\n (net {})\n)\n",
            bus.0, f.net.0
        )
    );
    f.p.remove_net_segment(NetSegmentRef {
        schematic: f.schematic,
        segment: second,
    })
    .unwrap();
    segments[1].insert_label(NetLabel::new(
        Uuid::new_random(),
        Point::ORIGIN,
        Angle::DEG0,
        false,
    ));
    f.p.add_net_segment(f.schematic, segments[1].clone())
        .unwrap();
    assert_eq!(
        count(&f.p, |k| matches!(
            k,
            Kind::UnnamedNetInBus { .. } | Kind::OpenNetInBus { .. }
        )),
        0
    );
    assert!(f.schematic().net_segments().contains_key(&first));
}

#[test]
fn bus_anchor_is_closed_when_multiple_net_wires_touch_it() {
    let mut f = Fixture::new(1);
    let symbol = f.add_symbol();
    let bus =
        f.p.add_bus(Bus::new(
            Uuid::new_random(),
            "BUS".parse().unwrap(),
            false,
            false,
            None,
        ))
        .unwrap();
    let mut bus_segment = SchematicBusSegment::new(Uuid::new_random(), bus);
    let junction = Uuid::new_random();
    bus_segment.insert_junction(Junction::new(junction, pt(20.0, 0.0)));
    let id = bus_segment.id();
    f.p.add_bus_segment(f.schematic, bus_segment).unwrap();
    for pin in 0..2 {
        let mut segment = SchematicNetSegment::new(Uuid::new_random(), f.net);
        segment.insert_line(NetLine::new(
            Uuid::new_random(),
            width(),
            f.pin(symbol, pin),
            NetLineAnchor::BusJunction {
                segment: id.0,
                junction,
            },
        ));
        f.p.add_net_segment(f.schematic, segment).unwrap();
        assert_eq!(
            count(&f.p, |k| matches!(k, Kind::OpenWire { .. })),
            if pin == 0 { 1 } else { 0 }
        );
    }
    // A missing bus line is still reported independently.
    assert_eq!(
        count(&f.p, |k| matches!(k, Kind::UnconnectedBusJunction { .. })),
        1
    );
}
