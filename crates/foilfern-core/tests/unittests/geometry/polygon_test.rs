//! Port of tests/unittests/core/geometry/polygontest.cpp.

use foilfern_core::geometry::{Path, Polygon, Vertex};
use foilfern_core::serialization::DeserializeObject;
use foilfern_core::types::{Angle, Layer, Length, Point, UnsignedLength, Uuid};

use super::{assert_roundtrip, parse};

#[test]
fn test_construct_from_sexpression() {
    let obj = Polygon::deserialize(&parse(
        "(polygon 2889d60c-1d18-44c3-bf9e-07733b67e480 (layer bot_stop_mask)\n\
         (width 0.1) (fill true) (grab_area false)\n\
         (vertex (position 0.0 0.0) (angle 0.0))\n\
         (vertex (position 120.0 0.0) (angle 0.0))\n\
         (vertex (position 120.0 4.0) (angle 0.0))\n\
         (vertex (position 0.0 4.0) (angle 0.0))\n\
         (vertex (position 0.0 0.0) (angle 0.0))\n\
         )\n",
    ))
    .unwrap();
    assert_eq!(
        obj.uuid(),
        "2889d60c-1d18-44c3-bf9e-07733b67e480"
            .parse::<Uuid>()
            .unwrap()
    );
    assert_eq!(obj.layer().id(), "bot_stop_mask");
    assert_eq!(obj.line_width(), Length::new(100_000));
    assert!(obj.is_filled());
    assert!(!obj.is_grab_area());
    assert_eq!(obj.path().vertices().len(), 5);
}

#[test]
fn test_serialize_and_deserialize() {
    let obj = Polygon::new(
        Uuid::new_random(),
        Layer::BOT_COPPER,
        UnsignedLength::new(Length::new(456)).unwrap(),
        true,
        false,
        Path::new(vec![
            Vertex::new(Point::from_nm(1, 2), Angle::new(3)),
            Vertex::new(Point::from_nm(4, 5), Angle::new(6)),
        ]),
    );
    assert_roundtrip(&obj);
}

#[test]
fn test_path_for_rendering() {
    let open = Path::new(vec![
        Vertex::at(Point::from_nm(0, 0)),
        Vertex::at(Point::from_nm(10, 0)),
        Vertex::at(Point::from_nm(10, 10)),
    ]);
    let mut obj = Polygon::new(
        Uuid::new_random(),
        Layer::TOP_LEGEND,
        UnsignedLength::ZERO,
        false,
        false,
        open.clone(),
    );
    assert_eq!(obj.path_for_rendering(), open);
    assert!(obj.set_layer(Layer::TOP_COURTYARD));
    assert!(!obj.set_layer(Layer::TOP_COURTYARD));
    assert!(obj.path_for_rendering().is_closed());
}
