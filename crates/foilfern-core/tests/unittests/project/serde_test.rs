//! Serde (JSON) representation of mutations, changes and the value types
//! they contain (see `docs/project-model-design.md`, "Serde
//! representation").

use std::collections::BTreeSet;
use std::str::FromStr;

use foilfern_core::attribute::{Attribute, AttributeKey, AttributeList, AttributeType};
use foilfern_core::library::dev::{Part, PartList};
use foilfern_core::project::circuit::{ComponentAssemblyOption, ComponentAssemblyOptionList};
use foilfern_core::project::{
    AssemblyVariantId, Change, ComponentSignalRef, Mutation, NetSignalId,
};
use foilfern_core::types::{
    Angle, CircuitIdentifier, ElementName, Layer, Length, LengthUnit, Point, PositiveLength,
    SimpleString, UnsignedLength, Uuid, Version,
};

use super::*;

#[test]
fn value_types() {
    let uuid: Uuid = "d2c30518-5cd1-4ce9-a569-44f783a3f66a".parse().unwrap();
    assert_eq!(
        serde_json::to_string(&uuid).unwrap(),
        "\"d2c30518-5cd1-4ce9-a569-44f783a3f66a\""
    );
    assert_eq!(
        serde_json::to_string(&NetSignalId(uuid)).unwrap(),
        "\"d2c30518-5cd1-4ce9-a569-44f783a3f66a\""
    );
    assert_eq!(
        serde_json::to_string(&Length::new(2_540_000)).unwrap(),
        "2540000"
    );
    assert_eq!(
        serde_json::to_string(&Angle::new(90_000_000)).unwrap(),
        "90000000"
    );
    assert_eq!(
        serde_json::to_string(&Point {
            x: Length::new(1),
            y: Length::new(-2)
        })
        .unwrap(),
        "{\"x\":1,\"y\":-2}"
    );
    assert_eq!(
        serde_json::to_string(&Version::from_str("1.2").unwrap()).unwrap(),
        "\"1.2\""
    );
    assert_eq!(
        serde_json::to_string(&LengthUnit::Millimeters).unwrap(),
        "\"millimeters\""
    );
    assert_eq!(
        serde_json::to_string(&Layer::from_id("top_cu").unwrap()).unwrap(),
        "\"top_cu\""
    );
    assert_eq!(
        serde_json::to_string(&ElementName::new("Foo").unwrap()).unwrap(),
        "\"Foo\""
    );

    // Validation on deserialization.
    assert!(serde_json::from_str::<Uuid>("\"not-a-uuid\"").is_err());
    assert!(serde_json::from_str::<PositiveLength>("0").is_err());
    assert!(serde_json::from_str::<UnsignedLength>("-1").is_err());
    assert_eq!(
        serde_json::from_str::<UnsignedLength>("5").unwrap(),
        UnsignedLength::new(Length::new(5)).unwrap()
    );
    assert!(serde_json::from_str::<ElementName>("\"\"").is_err());
    assert!(serde_json::from_str::<CircuitIdentifier>("\"a b\"").is_err());
    assert!(serde_json::from_str::<Layer>("\"nope\"").is_err());

    // Attributes: unit by name, validated.
    let attribute = Attribute::new(
        AttributeKey::new("VOLTAGE").unwrap(),
        AttributeType::Voltage,
        "4.2",
        AttributeType::Voltage.unit_from_string("volt").unwrap(),
    )
    .unwrap();
    let json = serde_json::to_string(&attribute).unwrap();
    assert_eq!(
        json,
        "{\"key\":\"VOLTAGE\",\"type\":\"voltage\",\"unit\":\"volt\",\"value\":\"4.2\"}"
    );
    assert_eq!(serde_json::from_str::<Attribute>(&json).unwrap(), attribute);
    assert!(
        serde_json::from_str::<Attribute>(
            "{\"key\":\"VOLTAGE\",\"type\":\"voltage\",\"unit\":\"ohm\",\"value\":\"4.2\"}"
        )
        .is_err()
    );
}

#[test]
fn mutations_roundtrip() {
    let mut p = new_project();
    let (lib, variant, signals) = add_library_component(&mut p);
    let mut cmp = component_instance(&p, lib, variant, "R1");
    cmp.set_signal_net(&signals[0], Some(NetSignalId::new_random()));
    cmp.set_attributes(AttributeList::from(vec![
        Attribute::new(
            AttributeKey::new("MPN").unwrap(),
            AttributeType::String,
            "ABC",
            None,
        )
        .unwrap(),
    ]));
    cmp.set_assembly_options(ComponentAssemblyOptionList::from(vec![
        ComponentAssemblyOption::new(
            Uuid::new_random(),
            AttributeList::new(),
            BTreeSet::from([AssemblyVariantId::new_random()]),
            PartList::from(vec![Part::new(
                SimpleString::new("ABC-123").unwrap(),
                SimpleString::new("ACME").unwrap(),
                AttributeList::new(),
            )]),
        ),
    ]));
    let mutations = vec![
        Mutation::AddComponentInstance(cmp),
        Mutation::SetComponentSignalNet {
            signal: ComponentSignalRef {
                component: ComponentInstanceId::new_random(),
                signal: signals[1],
            },
            net: None,
        },
        Mutation::AddNetSignal(NetSignal::new(
            Uuid::new_random(),
            default_net_class(&p),
            CircuitIdentifier::new("GND").unwrap(),
            false,
        )),
        Mutation::RemoveSchematic(foilfern_core::project::SchematicId::new_random()),
        Mutation::SetProjectSettings(p.settings().clone()),
        Mutation::SetErcApproval {
            approval: SExpression::list("approved"),
            approved: true,
        },
    ];
    let batch = Mutation::Batch(mutations);
    let json = serde_json::to_string_pretty(&batch).unwrap();
    let parsed: Mutation = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, batch);

    let change = Change::ComponentSignalNetChanged {
        signal: ComponentSignalRef {
            component: ComponentInstanceId::new_random(),
            signal: Uuid::new_random(),
        },
        from: Some(NetSignalId::new_random()),
        to: None,
    };
    let json = serde_json::to_string(&change).unwrap();
    assert_eq!(serde_json::from_str::<Change>(&json).unwrap(), change);
}
