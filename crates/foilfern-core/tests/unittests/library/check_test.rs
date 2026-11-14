//! Tests of the library element checks (no upstream unit tests exist; the
//! expected messages and approvals are those of upstream LibrePCB 2.x).

use std::collections::BTreeSet;

use foilfern_core::attribute::AttributeList;
use foilfern_core::geometry::{Circle, Path, Polygon, Text, Vertex};
use foilfern_core::library::cat::ComponentCategory;
use foilfern_core::library::cmp::{
    CmpSigPinDisplayType, Component, ComponentPinSignalMapItem, ComponentPrefix, ComponentSignal,
    ComponentSymbolVariant, ComponentSymbolVariantItem, ComponentSymbolVariantItemSuffix,
};
use foilfern_core::library::dev::{Device, DevicePadSignalMapItem, Part};
use foilfern_core::library::sym::{Symbol, SymbolPin};
use foilfern_core::library::{BaseMetadata, LibraryBaseElement, LibraryCheckMessage};
use foilfern_core::rule_check::{RuleCheckMessage, Severity};
use foilfern_core::serialization::{Mode, SExpression};
use foilfern_core::types::{
    Alignment, Angle, CircuitIdentifier, ElementName, Layer, Length, Point, PositiveLength,
    SignalRole, SimpleString, UnsignedLength, Uuid,
};

use super::{data_path, open_dir};

fn uuid(s: &str) -> Uuid {
    s.parse().unwrap()
}

fn metadata(name: &str, author: &str) -> BaseMetadata {
    BaseMetadata::new(
        uuid("11111111-1111-4111-8111-111111111111"),
        "0.1".parse().unwrap(),
        author,
        chrono::Utc::now(),
        ElementName::new(name).unwrap(),
        "",
        "",
    )
}

fn positive(nm: i64) -> PositiveLength {
    PositiveLength::new(Length::new(nm)).unwrap()
}

/// Returns `[severity] message` and the approval of all messages.
fn summary(msgs: &[LibraryCheckMessage]) -> Vec<(String, String)> {
    foilfern_i18n::set_language("en").unwrap();
    msgs.iter()
        .map(RuleCheckMessage::from)
        .map(|m| {
            (
                format!(
                    "[{}] {}",
                    m.severity().name_tr().to_uppercase(),
                    m.message()
                ),
                m.approval().to_string_with_mode(Mode::LibrePcb).unwrap(),
            )
        })
        .collect()
}

fn pin(uuid_str: &str, name: &str, x: i64, y: i64) -> SymbolPin {
    SymbolPin::new(
        uuid(uuid_str),
        CircuitIdentifier::new(name).unwrap(),
        Point::from_nm(x, y),
        UnsignedLength::new(Length::new(2_540_000)).unwrap(),
        Angle::DEG0,
        Point::from_nm(3_810_000, 0),
        Angle::DEG0,
        SymbolPin::default_name_height(),
        SymbolPin::default_name_alignment(),
    )
}

fn text(uuid_str: &str, layer: Layer, value: &str) -> Text {
    Text::new(
        uuid(uuid_str),
        layer,
        value,
        Point::ORIGIN,
        Angle::DEG0,
        positive(2_540_000),
        Alignment::default(),
        false,
    )
}

#[test]
fn test_symbol_check() {
    let mut symbol = Symbol::new(metadata("my symbol", " ")).unwrap();
    let pins = symbol.pins_mut();
    pins.push(pin("00000000-0000-4000-8000-000000000003", "A", 0, 0));
    pins.push(pin("00000000-0000-4000-8000-000000000001", "A10", 0, 0));
    pins.push(pin("00000000-0000-4000-8000-000000000002", "A2", 0, 0));
    pins.push(pin(
        "00000000-0000-4000-8000-000000000004",
        "/RST",
        1,
        2_540_000,
    ));
    pins.push(pin(
        "00000000-0000-4000-8000-000000000005",
        "A",
        7_620_000,
        0,
    ));
    symbol.texts_mut().push(text(
        "00000000-0000-4000-8000-000000000010",
        Layer::SYMBOL_OUTLINES,
        "{{NAME}}",
    ));

    let msgs = symbol.run_checks().unwrap();
    let expected: Vec<(&str, &str)> = vec![
        (
            "[HINT] Name not title case: 'my symbol'",
            "(approved name_not_title_case)\n",
        ),
        ("[WARNING] Author not set", "(approved empty_author)\n"),
        (
            "[ERROR] No categories set",
            "(approved missing_categories)\n",
        ),
        (
            "[ERROR] Duplicate pin name: 'A'",
            "(approved duplicate_pin_name (name \"A\"))\n",
        ),
        (
            "[HINT] Non-functional inversion sign: '/RST'",
            "(approved nonfunctional_inversion_sign\n (pin 00000000-0000-4000-8000-000000000004)\n)\n",
        ),
        (
            "[ERROR] Pin not on 2.54mm grid: '/RST'",
            "(approved pin_not_on_grid\n (pin 00000000-0000-4000-8000-000000000004)\n)\n",
        ),
        (
            "[ERROR] Overlapping pins: 'A', 'A2', 'A10'",
            "(approved overlapping_pins\n (pin 00000000-0000-4000-8000-000000000003)\n \
             (pin 00000000-0000-4000-8000-000000000001)\n \
             (pin 00000000-0000-4000-8000-000000000002)\n)\n",
        ),
        (
            "[WARNING] Missing text: '{{VALUE}}'",
            "(approved missing_value_text)\n",
        ),
        (
            "[WARNING] Layer of '{{NAME}}' is not 'Names'",
            "(approved unusual_text_layer\n (text 00000000-0000-4000-8000-000000000010)\n)\n",
        ),
        // Pins span x=0..7.62mm, y=0..2.54mm -> center (3.81mm, 1.27mm).
        (
            "[HINT] Origin not in center",
            "(approved origin_not_in_center)\n",
        ),
    ];
    let actual = summary(&msgs);
    let actual: Vec<(&str, &str)> = actual
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn test_symbol_check_origin_not_in_center() {
    let mut symbol = Symbol::new(metadata("Symbol", "me")).unwrap();
    symbol.categories_mut_for_test();
    // A grab area rectangle from (0, 0) to (10.16, 5.08) -> center not in
    // the origin.
    let path = Path::new(vec![
        Vertex::new(Point::from_nm(0, 0), Angle::DEG0),
        Vertex::new(Point::from_nm(10_160_000, 0), Angle::DEG0),
        Vertex::new(Point::from_nm(10_160_000, 5_080_000), Angle::DEG0),
        Vertex::new(Point::from_nm(0, 5_080_000), Angle::DEG0),
        Vertex::new(Point::from_nm(0, 0), Angle::DEG0),
    ]);
    symbol.polygons_mut().push(Polygon::new(
        Uuid::new_random(),
        Layer::SYMBOL_OUTLINES,
        UnsignedLength::new(Length::new(200_000)).unwrap(),
        false,
        true,
        path,
    ));
    let msgs = symbol.run_checks().unwrap();
    assert!(summary(&msgs).contains(&(
        "[HINT] Origin not in center".to_owned(),
        "(approved origin_not_in_center)\n".to_owned()
    )));

    // Symbols without pins and grab areas are not checked.
    let mut frame = Symbol::new(metadata("Frame", "me")).unwrap();
    frame.circles_mut().push(Circle::new(
        Uuid::new_random(),
        Layer::SYMBOL_OUTLINES,
        UnsignedLength::new(Length::new(200_000)).unwrap(),
        false,
        false,
        Point::from_nm(50_000_000, 0),
        positive(1_000_000),
    ));
    let msgs = frame.run_checks().unwrap();
    assert!(
        !summary(&msgs)
            .iter()
            .any(|(m, _)| m.contains("Origin not in center"))
    );
}

/// Helper to set a category, so the "missing categories" message does not
/// clutter the tests.
trait SetCategory {
    fn categories_mut_for_test(&mut self);
}

impl<T: foilfern_core::library::LibraryElement> SetCategory for T {
    fn categories_mut_for_test(&mut self) {
        self.element_metadata_mut()
            .set_categories(BTreeSet::from([Uuid::new_random()]));
    }
}

#[test]
fn test_component_check() {
    let mut cmp = Component::new(metadata("Component", "me")).unwrap();
    cmp.categories_mut_for_test();
    let sig1 = "00000000-0000-4000-8000-000000000001";
    let sig2 = "00000000-0000-4000-8000-000000000002";
    for (uuid_str, name, forced) in [(sig1, "nRESET", "GND"), (sig2, "nRESET", "")] {
        cmp.signals_mut().push(ComponentSignal::new(
            uuid(uuid_str),
            CircuitIdentifier::new(name).unwrap(),
            SignalRole::Passive,
            forced,
            false,
            false,
            false,
        ));
    }
    let var_empty = "00000000-0000-4000-8000-000000000011";
    let var_unconnected = "00000000-0000-4000-8000-000000000012";
    let var_ok = "00000000-0000-4000-8000-000000000013";
    for (uuid_str, name) in [
        (var_empty, "Empty"),
        (var_unconnected, "Unconnected"),
        (var_ok, "OK"),
    ] {
        cmp.symbol_variants_mut().push(ComponentSymbolVariant::new(
            uuid(uuid_str),
            "",
            ElementName::new(name).unwrap(),
            "",
        ));
    }
    for (index, signal) in [(1, None), (2, Some(uuid(sig1)))] {
        let mut item = ComponentSymbolVariantItem::new(
            Uuid::new_random(),
            Uuid::new_random(),
            Point::ORIGIN,
            Angle::DEG0,
            true,
            ComponentSymbolVariantItemSuffix::default(),
        );
        item.pin_signal_map_mut()
            .push(ComponentPinSignalMapItem::new(
                Uuid::new_random(),
                signal,
                CmpSigPinDisplayType::ComponentSignal,
            ));
        cmp.symbol_variants_mut()[index]
            .symbol_items_mut()
            .push(item);
    }

    let msgs = cmp.run_checks().unwrap();
    let actual: Vec<String> = summary(&msgs).into_iter().map(|(m, _)| m).collect();
    assert_eq!(
        actual,
        [
            "[WARNING] No component prefix set",
            "[WARNING] No default value set",
            "[ERROR] Duplicate signal name: 'nRESET'",
            "[HINT] Non-functional inversion sign: 'nRESET'",
            "[HINT] Non-functional inversion sign: 'nRESET'",
            "[WARNING] Suspicious use of forced nets",
            "[ERROR] Symbol variant 'Empty' has no items",
            "[ERROR] No pins connected in 'Unconnected'",
        ]
    );
    let approvals: Vec<String> = summary(&msgs).into_iter().map(|(_, a)| a).collect();
    assert_eq!(approvals[0], "(approved empty_prefix)\n");
    assert_eq!(approvals[1], "(approved empty_default_value)\n");
    assert_eq!(
        approvals[2],
        "(approved duplicate_signal_name (name \"nRESET\"))\n"
    );
    assert_eq!(
        approvals[3],
        format!("(approved nonfunctional_inversion_sign\n (signal {sig1})\n)\n")
    );
    assert_eq!(approvals[5], "(approved suspicious_forced_nets)\n");
    assert_eq!(
        approvals[6],
        format!("(approved missing_gates\n (variant {var_empty})\n)\n")
    );
    assert_eq!(
        approvals[7],
        format!("(approved no_pins_connected\n (variant {var_unconnected})\n)\n")
    );

    // Fixed component: no messages.
    cmp.set_prefixes(foilfern_core::library::cmp::NormDependentPrefixMap::new(
        ComponentPrefix::new("U").unwrap(),
    ));
    cmp.set_default_value("{{MPN}}".into());
    cmp.signals_mut().clear();
    cmp.symbol_variants_mut().clear();
    let msgs = cmp.run_checks().unwrap();
    let actual: Vec<String> = summary(&msgs).into_iter().map(|(m, _)| m).collect();
    assert_eq!(actual, ["[ERROR] No symbol variant defined"]);
}

#[test]
fn test_device_check() {
    let mut dev = Device::new(
        metadata("Device", "me"),
        Uuid::new_random(),
        Uuid::new_random(),
    )
    .unwrap();
    dev.categories_mut_for_test();
    // Only "no part numbers added".
    assert_eq!(dev.run_checks().unwrap().len(), 1);
    dev.pad_signal_map_mut()
        .push(DevicePadSignalMapItem::new(Uuid::new_random(), None, false));
    let msgs = dev.run_checks().unwrap();
    assert_eq!(
        summary(&msgs),
        [
            (
                "[WARNING] No pads connected".to_owned(),
                "(approved no_pads_connected)\n".to_owned()
            ),
            (
                "[HINT] No part numbers added".to_owned(),
                "(approved no_parts)\n".to_owned()
            ),
        ]
    );
    dev.pad_signal_map_mut().push(DevicePadSignalMapItem::new(
        Uuid::new_random(),
        Some(Uuid::new_random()),
        false,
    ));
    dev.parts_mut().push(Part::new(
        SimpleString::new("MPN").unwrap(),
        SimpleString::default(),
        AttributeList::new(),
    ));
    assert!(dev.run_checks().unwrap().is_empty());
}

#[test]
fn test_category_check_and_approvals() {
    // This category is its own parent, and the message is approved.
    let path =
        data_path("libraries/Populated Library.lplib/cmpcat/437f96af-8d27-4108-8d0f-6c57b60d7468");
    let cat = ComponentCategory::open(open_dir(&path, false)).unwrap();
    let msgs: Vec<RuleCheckMessage> = cat
        .run_checks()
        .unwrap()
        .iter()
        .map(RuleCheckMessage::from)
        .collect();
    assert!(msgs.iter().any(|m| m.severity() == Severity::Error));
    let approval = SExpression::parse(b"(approved invalid_parent)", None, Mode::LibrePcb).unwrap();
    assert!(msgs.iter().any(|m| *m.approval() == approval));
    assert!(cat.metadata().message_approvals().contains(&approval));
}
