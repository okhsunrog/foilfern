//! Port of tests/unittests/core/geometry/viatest.cpp.

use foilfern_core::geometry::Via;
use foilfern_core::serialization::DeserializeObject;
use foilfern_core::types::{
    BoundedUnsignedRatio, Layer, Length, MaskConfig, Point, PositiveLength, Ratio, UnsignedLength,
    UnsignedRatio, Uuid,
};

use super::{assert_roundtrip, parse};

fn pos(nm: i64) -> PositiveLength {
    PositiveLength::new(Length::new(nm)).unwrap()
}

fn unsigned(nm: i64) -> UnsignedLength {
    UnsignedLength::new(Length::new(nm)).unwrap()
}

fn ratio(r: Ratio, min: i64, max: i64) -> BoundedUnsignedRatio {
    BoundedUnsignedRatio::new(UnsignedRatio::new(r).unwrap(), unsigned(min), unsigned(max)).unwrap()
}

#[test]
fn test_construct_from_sexpression() {
    let obj = Via::deserialize(&parse(
        "(via b9445237-8982-4a9f-af06-bfc6c507e010 (from top_cu) (to in2_cu) \
         (position 1.234 2.345) (size 0.9) (drill 0.4) (exposure off))",
    ))
    .unwrap();
    assert_eq!(
        obj.uuid(),
        "b9445237-8982-4a9f-af06-bfc6c507e010"
            .parse::<Uuid>()
            .unwrap()
    );
    assert_eq!(obj.start_layer(), Layer::TOP_COPPER);
    assert_eq!(Some(obj.end_layer()), Layer::inner_copper(2));
    assert_eq!(obj.position(), Point::from_nm(1_234_000, 2_345_000));
    assert_eq!(obj.size(), Some(pos(900_000)));
    assert_eq!(obj.drill_diameter(), Some(pos(400_000)));
    assert_eq!(obj.exposure_config(), MaskConfig::Off);
    assert!(obj.is_blind());
    assert!(!obj.is_through());
    assert!(!obj.is_buried());
    assert!(obj.is_on_layer(Layer::TOP_COPPER));
    assert!(!obj.is_on_layer(Layer::BOT_COPPER));
}

#[test]
fn test_construct_from_sexpression_invalid() {
    // Drill larger than size.
    assert!(
        Via::deserialize(&parse(
            "(via b9445237-8982-4a9f-af06-bfc6c507e010 (from top_cu) (to bot_cu) \
         (position 0.0 0.0) (drill 0.5) (size 0.4) (exposure off))",
        ))
        .is_err()
    );
    // Wrong layer order.
    assert!(
        Via::deserialize(&parse(
            "(via b9445237-8982-4a9f-af06-bfc6c507e010 (from bot_cu) (to top_cu) \
         (position 0.0 0.0) (drill auto) (size auto) (exposure off))",
        ))
        .is_err()
    );
}

#[test]
fn test_serialize_and_deserialize() {
    let obj = Via::new(
        Uuid::new_random(),
        Layer::TOP_COPPER,
        Layer::BOT_COPPER,
        Point::from_nm(123, 456),
        Some(pos(789)),
        None,
        MaskConfig::Off,
    )
    .unwrap();
    assert_roundtrip(&obj);
}

#[test]
fn test_calc_size_from_rules_zero_annular() {
    let r = ratio(Ratio::new(0), 0, 0);
    assert_eq!(
        Via::calc_size_from_rules(pos(1_000_000), &r),
        pos(1_000_000)
    );
}

#[test]
fn test_calc_size_from_rules_min_annular() {
    let r = ratio(Ratio::from_percent(10), 200_000, 1_000_000);
    assert_eq!(
        Via::calc_size_from_rules(pos(1_000_000), &r),
        pos(1_400_000)
    );
}

#[test]
fn test_calc_size_from_rules_scaled_annular() {
    let r = ratio(Ratio::from_percent(40), 200_000, 1_000_000);
    assert_eq!(
        Via::calc_size_from_rules(pos(1_000_000), &r),
        pos(1_800_000)
    );
}

#[test]
fn test_calc_size_from_rules_max_annular() {
    let r = ratio(Ratio::from_percent(40), 200_000, 300_000);
    assert_eq!(
        Via::calc_size_from_rules(pos(1_000_000), &r),
        pos(1_600_000)
    );
}
