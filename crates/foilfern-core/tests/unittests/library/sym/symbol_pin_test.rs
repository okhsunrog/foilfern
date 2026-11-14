//! Port of tests/unittests/core/library/sym/symbolpintest.cpp.

use foilfern_core::library::sym::SymbolPin;
use foilfern_core::serialization::DeserializeObject;
use foilfern_core::types::{
    Alignment, Angle, CircuitIdentifier, HAlign, Length, Point, PositiveLength, UnsignedLength,
    Uuid, VAlign,
};

use crate::geometry::{assert_roundtrip, parse};

#[test]
fn test_construct_from_sexpression() {
    let sexpr = parse(
        "(pin d48b8bd2-a46c-4495-87a5-662747034098 (name \"1\")\n\
         (position 1.234 2.345) (rotation 45.0) (length 0.5)\n\
         (name_position 0.1 0.2) (name_rotation -90.0) (name_height 1.234)\n\
         (name_align center bottom)\n\
         )",
    );
    let obj = SymbolPin::deserialize(&sexpr).unwrap();
    assert_eq!(
        "d48b8bd2-a46c-4495-87a5-662747034098"
            .parse::<Uuid>()
            .unwrap(),
        obj.uuid()
    );
    assert_eq!("1", obj.name().as_str());
    assert_eq!(Point::from_nm(1_234_000, 2_345_000), obj.position());
    assert_eq!(Angle::DEG45, obj.rotation());
    assert_eq!(
        UnsignedLength::new(Length::new(500_000)).unwrap(),
        obj.length()
    );
    assert_eq!(Point::from_nm(100_000, 200_000), obj.name_position());
    assert_eq!(-Angle::DEG90, obj.name_rotation());
    assert_eq!(
        PositiveLength::new(Length::new(1_234_000)).unwrap(),
        obj.name_height()
    );
    assert_eq!(
        Alignment::new(HAlign::Center, VAlign::Bottom),
        obj.name_alignment()
    );
}

#[test]
fn test_serialize_and_deserialize() {
    let obj = SymbolPin::new(
        Uuid::new_random(),
        CircuitIdentifier::new("foo").unwrap(),
        Point::from_nm(123, 567),
        UnsignedLength::new(Length::new(321)).unwrap(),
        Angle::new(789),
        Point::from_nm(100_000, 200_000),
        Angle::new(321),
        PositiveLength::new(Length::new(123_456)).unwrap(),
        Alignment::new(HAlign::Center, VAlign::Bottom),
    );
    assert_roundtrip(&obj);
}
