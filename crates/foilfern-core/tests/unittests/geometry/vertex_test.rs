//! Port of tests/unittests/core/geometry/vertextest.cpp.

use foilfern_core::geometry::Vertex;
use foilfern_core::serialization::DeserializeObject;
use foilfern_core::types::{Angle, Point};

use super::{assert_roundtrip, parse};

#[test]
fn test_construct_from_sexpression() {
    let obj = Vertex::deserialize(&parse("(vertex (position 1.2 3.4) (angle 45.0))")).unwrap();
    assert_eq!(obj.pos, Point::from_nm(1_200_000, 3_400_000));
    assert_eq!(obj.angle, Angle::DEG45);
}

#[test]
fn test_serialize_and_deserialize() {
    assert_roundtrip(&Vertex::new(Point::from_nm(123, 567), Angle::new(789)));
}

#[test]
fn test_ordering() {
    let v = |x, y, a| Vertex::new(Point::from_nm(x, y), Angle::new(a));
    assert!(v(1, 2, 3) < v(1, 2, 4));
    assert!(v(1, 2, 9) < v(1, 3, 0));
    assert!(v(0, 9, 9) < v(1, 0, 0));
    assert_eq!(v(1, 2, 3), v(1, 2, 3));
}
