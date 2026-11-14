//! Port of tests/unittests/core/geometry/stroketexttest.cpp.

use foilfern_core::geometry::StrokeText;
use foilfern_core::serialization::DeserializeObject;
use foilfern_core::types::{
    Alignment, Angle, HAlign, Layer, Length, Point, PositiveLength, Ratio, StrokeTextSpacing,
    UnsignedLength, Uuid, VAlign,
};

use super::{assert_roundtrip, parse};

#[test]
fn test_construct_from_sexpression() {
    let obj = StrokeText::deserialize(&parse(
        "(stroke_text 0a8d7180-68e1-4749-bf8c-538b0d88f08c \
         (layer bot_names) (height 1.0) (stroke_width 0.2) \
         (letter_spacing auto) (line_spacing auto) (align left bottom) \
         (position 1.234 2.345) (rotation 45.0) (auto_rotate true) \
         (mirror true) (lock true) (value \"Foo Bar\"))",
    ))
    .unwrap();
    assert_eq!(
        obj.uuid(),
        "0a8d7180-68e1-4749-bf8c-538b0d88f08c"
            .parse::<Uuid>()
            .unwrap()
    );
    assert_eq!(obj.layer().id(), "bot_names");
    assert_eq!(obj.height(), Length::new(1_000_000));
    assert_eq!(obj.stroke_width(), Length::new(200_000));
    assert_eq!(obj.letter_spacing().ratio(), None);
    assert_eq!(obj.line_spacing().ratio(), None);
    assert_eq!(obj.align(), Alignment::new(HAlign::Left, VAlign::Bottom));
    assert_eq!(obj.position(), Point::from_nm(1_234_000, 2_345_000));
    assert_eq!(obj.rotation(), Angle::DEG45);
    assert!(obj.auto_rotate());
    assert!(obj.mirrored());
    assert!(obj.locked());
    assert_eq!(obj.text(), "Foo Bar");
}

#[test]
fn test_serialize_and_deserialize() {
    let obj = StrokeText::new(
        Uuid::new_random(),
        Layer::BOT_COPPER,
        "hello world",
        Point::from_nm(12, 34),
        Angle::new(56),
        PositiveLength::new(Length::new(123)).unwrap(),
        UnsignedLength::new(Length::new(456)).unwrap(),
        StrokeTextSpacing::Auto,
        StrokeTextSpacing::Manual(Ratio::new(1234)),
        Alignment::new(HAlign::Right, VAlign::Center),
        true,
        false,
        true,
    );
    assert_roundtrip(&obj);
}
