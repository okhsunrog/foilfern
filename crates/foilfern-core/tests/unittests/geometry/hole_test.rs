//! Port of tests/unittests/core/geometry/holetest.cpp.

use foilfern_core::geometry::{Hole, NonEmptyPath, Path, Vertex};
use foilfern_core::serialization::DeserializeObject;
use foilfern_core::types::{Angle, Length, MaskConfig, Point, PositiveLength, Uuid};

use super::{assert_roundtrip, parse};

#[test]
fn test_construct_from_sexpression() {
    let obj = Hole::deserialize(&parse(
        "(hole b9445237-8982-4a9f-af06-bfc6c507e010 \
         (diameter 0.5) (stop_mask auto) \
         (vertex (position 1.234 2.345) (angle 45.0)))",
    ))
    .unwrap();
    assert_eq!(
        obj.uuid(),
        "b9445237-8982-4a9f-af06-bfc6c507e010"
            .parse::<Uuid>()
            .unwrap()
    );
    assert_eq!(obj.diameter(), Length::new(500_000));
    assert_eq!(obj.path().vertices().len(), 1);
    assert_eq!(obj.path().first().pos, Point::from_nm(1_234_000, 2_345_000));
    assert_eq!(obj.path().first().angle, Angle::new(45_000_000));
    assert_eq!(obj.stop_mask_config(), MaskConfig::Automatic);
    assert!(!obj.is_slot());
}

#[test]
fn test_construct_from_sexpression_without_vertices() {
    let result = Hole::deserialize(&parse(
        "(hole b9445237-8982-4a9f-af06-bfc6c507e010 (diameter 0.5) (stop_mask auto))",
    ));
    assert!(result.is_err());
}

#[test]
fn test_serialize_and_deserialize() {
    let path = NonEmptyPath::new(Path::new(vec![
        Vertex::new(Point::from_nm(123, 456), Angle::DEG45),
        Vertex::new(Point::from_nm(789, 321), Angle::DEG0),
    ]))
    .unwrap();
    let obj = Hole::new(
        Uuid::new_random(),
        PositiveLength::new(Length::new(123)).unwrap(),
        path,
        MaskConfig::Manual(Length::new(123_456)),
    );
    assert!(obj.is_slot());
    assert!(obj.is_curved_slot());
    assert!(!obj.is_multi_segment_slot());
    assert_roundtrip(&obj);
}
