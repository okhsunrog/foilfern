//! Port of tests/unittests/core/geometry/tracetest.cpp.

use foilfern_core::geometry::{Trace, TraceAnchor};
use foilfern_core::serialization::DeserializeObject;
use foilfern_core::types::{Layer, Length, PositiveLength, Uuid};

use super::{assert_roundtrip, parse};

fn uuid(s: &str) -> Uuid {
    s.parse().unwrap()
}

fn fp(device: &str, pad: &str) -> TraceAnchor {
    TraceAnchor::FootprintPad {
        device: uuid(device),
        pad: uuid(pad),
    }
}

#[test]
fn test_anchor_less_than() {
    // The comparison operator is relevant for the file format.
    let input = vec![
        TraceAnchor::Junction(uuid("5bed2074-1b02-4db5-9b0e-293c42d8728f")),
        fp(
            "d14141ff-651f-40f0-87be-b4f86831375a",
            "65ab6c75-b264-4fed-b445-d3d98c956008",
        ),
        TraceAnchor::Pad(uuid("e706fdf8-4ced-4cc4-a49f-757ca395272b")),
        TraceAnchor::Via(uuid("c893f5a0-3fec-498b-99d6-467d5d69825d")),
        fp(
            "94ca7c55-bf86-43e0-8399-d713ce1f1929",
            "65ab6c75-b264-4fed-b445-d3d98c956008",
        ),
        TraceAnchor::Junction(uuid("0d8f2ef9-34f4-4400-a313-f17cdcdfe924")),
        TraceAnchor::Via(uuid("1e80206f-158b-48e6-9cb4-6e368af7b7d7")),
        TraceAnchor::Pad(uuid("70c4ec26-2d47-441d-beeb-43aa968b4d2e")),
        fp(
            "94ca7c55-bf86-43e0-8399-d713ce1f1929",
            "04bb6ac3-34d7-4fb3-b274-44f845f8d3b5",
        ),
    ];
    let expected = vec![
        fp(
            "94ca7c55-bf86-43e0-8399-d713ce1f1929",
            "04bb6ac3-34d7-4fb3-b274-44f845f8d3b5",
        ),
        fp(
            "94ca7c55-bf86-43e0-8399-d713ce1f1929",
            "65ab6c75-b264-4fed-b445-d3d98c956008",
        ),
        fp(
            "d14141ff-651f-40f0-87be-b4f86831375a",
            "65ab6c75-b264-4fed-b445-d3d98c956008",
        ),
        TraceAnchor::Pad(uuid("70c4ec26-2d47-441d-beeb-43aa968b4d2e")),
        TraceAnchor::Pad(uuid("e706fdf8-4ced-4cc4-a49f-757ca395272b")),
        TraceAnchor::Via(uuid("1e80206f-158b-48e6-9cb4-6e368af7b7d7")),
        TraceAnchor::Via(uuid("c893f5a0-3fec-498b-99d6-467d5d69825d")),
        TraceAnchor::Junction(uuid("0d8f2ef9-34f4-4400-a313-f17cdcdfe924")),
        TraceAnchor::Junction(uuid("5bed2074-1b02-4db5-9b0e-293c42d8728f")),
    ];
    let mut actual = input;
    actual.sort();
    assert_eq!(actual, expected);
}

#[test]
fn test_construct_from_sexpression() {
    let obj = Trace::deserialize(&parse(
        "(trace c893f5a0-3fec-498b-99d6-467d5d69825d (layer bot_cu) (width 0.5) \
         (from (via 1e80206f-158b-48e6-9cb4-6e368af7b7d7)) \
         (to (device 0d8f2ef9-34f4-4400-a313-f17cdcdfe924) \
         (pad 65ab6c75-b264-4fed-b445-d3d98c956008)))",
    ))
    .unwrap();
    assert_eq!(obj.uuid(), uuid("c893f5a0-3fec-498b-99d6-467d5d69825d"));
    assert_eq!(obj.layer().id(), "bot_cu");
    assert_eq!(obj.width(), Length::new(500_000));
    assert_eq!(
        obj.p1(),
        fp(
            "0d8f2ef9-34f4-4400-a313-f17cdcdfe924",
            "65ab6c75-b264-4fed-b445-d3d98c956008"
        )
    );
    assert_eq!(
        obj.p2(),
        TraceAnchor::Via(uuid("1e80206f-158b-48e6-9cb4-6e368af7b7d7"))
    );
}

#[test]
fn test_serialize_and_deserialize() {
    let obj = Trace::new(
        Uuid::new_random(),
        Layer::TOP_COPPER,
        PositiveLength::new(Length::new(123)).unwrap(),
        TraceAnchor::Junction(Uuid::new_random()),
        TraceAnchor::Pad(Uuid::new_random()),
    );
    assert_roundtrip(&obj);
}
