//! Port of tests/unittests/core/geometry/imagetest.cpp.

use foilfern_core::geometry::Image;
use foilfern_core::serialization::DeserializeObject;
use foilfern_core::types::{
    Angle, FileProofName, Length, Point, PositiveLength, UnsignedLength, Uuid,
};

use super::{assert_roundtrip, parse};

#[test]
fn test_construct_from_sexpression() {
    let obj = Image::deserialize(&parse(
        "(image b9445237-8982-4a9f-af06-bfc6c507e010 (file \"foo-bar.png\") \
         (position 17.78 7.62) (rotation 45.0) (width 15.24) (height 5.08) \
         (border none))",
    ))
    .unwrap();
    assert_eq!(
        obj.uuid(),
        "b9445237-8982-4a9f-af06-bfc6c507e010"
            .parse::<Uuid>()
            .unwrap()
    );
    assert_eq!(obj.file_name().as_str(), "foo-bar.png");
    assert_eq!(obj.file_basename(), "foo-bar");
    assert_eq!(obj.file_extension(), "png");
    assert_eq!(obj.position(), Point::from_nm(17_780_000, 7_620_000));
    assert_eq!(obj.rotation(), Angle::DEG45);
    assert_eq!(obj.width(), Length::new(15_240_000));
    assert_eq!(obj.height(), Length::new(5_080_000));
    assert_eq!(obj.border_width(), None);
}

#[test]
fn test_serialize_and_deserialize() {
    let obj = Image::new(
        Uuid::new_random(),
        FileProofName::new("foo.svg").unwrap(),
        Point::from_nm(123, 456),
        Angle::DEG45,
        PositiveLength::new(Length::new(123)).unwrap(),
        PositiveLength::new(Length::new(456)).unwrap(),
        Some(UnsignedLength::new(Length::new(111)).unwrap()),
    );
    assert_roundtrip(&obj);
}
