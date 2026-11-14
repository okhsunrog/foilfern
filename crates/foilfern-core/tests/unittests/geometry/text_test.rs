//! Port of tests/unittests/core/geometry/texttest.cpp.

use foilfern_core::geometry::Text;
use foilfern_core::serialization::DeserializeObject;
use foilfern_core::types::{
    Alignment, Angle, HAlign, Layer, Length, Point, PositiveLength, Uuid, VAlign,
};

use super::{assert_roundtrip, parse};

#[test]
fn test_construct_from_sexpression() {
    let obj = Text::deserialize(&parse(
        "(text eabf43fb-496b-4dc8-8ff7-ffac67991390 (layer sym_names) \
         (value \"{{NAME}}\") (align center bottom) (height 2.54) \
         (position 1.234 2.345) (rotation 45.0) (lock true))",
    ))
    .unwrap();
    assert_eq!(
        obj.uuid(),
        "eabf43fb-496b-4dc8-8ff7-ffac67991390"
            .parse::<Uuid>()
            .unwrap()
    );
    assert_eq!(obj.layer().id(), "sym_names");
    assert_eq!(obj.text(), "{{NAME}}");
    assert_eq!(obj.align(), Alignment::new(HAlign::Center, VAlign::Bottom));
    assert_eq!(obj.height(), Length::new(2_540_000));
    assert_eq!(obj.position(), Point::from_nm(1_234_000, 2_345_000));
    assert_eq!(obj.rotation(), Angle::DEG45);
    assert!(obj.locked());
}

#[test]
fn test_serialize_and_deserialize() {
    let obj = Text::new(
        Uuid::new_random(),
        Layer::BOT_COPPER,
        "foo bar",
        Point::from_nm(12, 34),
        Angle::new(56),
        PositiveLength::new(Length::new(78)).unwrap(),
        Alignment::new(HAlign::Right, VAlign::Center),
        false,
    );
    assert_roundtrip(&obj);
}
