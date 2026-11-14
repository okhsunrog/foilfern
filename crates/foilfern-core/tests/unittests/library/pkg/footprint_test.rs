//! Tests of libs/librepcb/core/library/pkg/footprint.{h,cpp} (no upstream
//! test exists).

use std::collections::BTreeSet;

use foilfern_core::geometry::{Path, Polygon};
use foilfern_core::library::pkg::Footprint;
use foilfern_core::serialization::DeserializeObject;
use foilfern_core::types::{Angle, ElementName, Layer, Length, Point, Tag, UnsignedLength, Uuid};

use crate::geometry::{assert_roundtrip, parse, serialize_str};

const FOOTPRINT: &str = "(footprint 7040952d-7016-49cd-8c3e-6078ecca98b9\n \
    (name \"default\")\n \
    (name (locale \"de_DE\") \"Standard\")\n \
    (description \"\")\n \
    (tag \"b\")\n (tag \"a\")\n \
    (3d_position 0.1 0.2 0.3) (3d_rotation 10.0 20.0 30.0)\n \
    (3d_model d48b8bd2-a46c-4495-87a5-662747034098)\n \
    (3d_model 1ce5ab2f-f1d0-4e23-9b3c-c5a5b8c6b4d6)\n \
    (pad 1ce5ab2f-f1d0-4e23-9b3c-c5a5b8c6b4d6 (side top) (shape roundrect)\n  \
    (position 0.0 0.0) (rotation 0.0) (size 1.0 1.0) (radius 0.0)\n  \
    (stop_mask auto) (solder_paste auto) (clearance 0.0) (function unspecified)\n  \
    (package_pad none)\n )\n \
    (polygon 5b8f4c6e-6e1a-4a3b-bf2c-5e5f1f1f1f1f (layer top_courtyard)\n  \
    (width 0.0) (fill false) (grab_area false)\n  \
    (vertex (position -1.0 -1.0) (angle 0.0))\n  \
    (vertex (position 1.0 1.0) (angle 0.0))\n )\n)";

fn uuid(s: &str) -> Uuid {
    s.parse().unwrap()
}

fn unsigned(nm: i64) -> UnsignedLength {
    UnsignedLength::new(Length::new(nm)).unwrap()
}

fn rect_polygon(layer: Layer, width: i64, half_size: i64) -> Polygon {
    let p1 = Point::from_nm(-half_size, -half_size);
    let p2 = Point::from_nm(half_size, half_size);
    Polygon::new(
        Uuid::new_random(),
        layer,
        unsigned(width),
        false,
        false,
        Path::rect(p1, p2),
    )
}

#[test]
fn test_construct_from_sexpression() {
    let obj = Footprint::deserialize(&parse(FOOTPRINT)).unwrap();
    assert_eq!(obj.uuid(), uuid("7040952d-7016-49cd-8c3e-6078ecca98b9"));
    assert_eq!(obj.names().default_value().as_str(), "default");
    assert_eq!(obj.names().get("de_DE").unwrap().as_str(), "Standard");
    assert_eq!(obj.descriptions().default_value(), "");
    assert_eq!(
        obj.tags().iter().map(|t| t.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(
        obj.model_position(),
        (
            Length::new(100_000),
            Length::new(200_000),
            Length::new(300_000)
        )
    );
    assert_eq!(
        obj.model_rotation(),
        (
            Angle::new(10_000_000),
            Angle::new(20_000_000),
            Angle::new(30_000_000)
        )
    );
    assert_eq!(
        obj.models(),
        &BTreeSet::from([
            uuid("1ce5ab2f-f1d0-4e23-9b3c-c5a5b8c6b4d6"),
            uuid("d48b8bd2-a46c-4495-87a5-662747034098"),
        ])
    );
    assert_eq!(obj.pads().len(), 1);
    assert_eq!(obj.polygons().len(), 1);
    assert!(obj.circles().is_empty());
    assert!(obj.stroke_texts().is_empty());
    assert!(obj.zones().is_empty());
    assert!(obj.holes().is_empty());
}

#[test]
fn test_construct_from_sexpression_without_3d_position() {
    let result = Footprint::deserialize(&parse(
        "(footprint 7040952d-7016-49cd-8c3e-6078ecca98b9 (name \"default\") \
         (description \"\") (3d_rotation 0.0 0.0 0.0))",
    ));
    assert!(result.is_err());
}

#[test]
fn test_serialize_sorts_tags_and_models() {
    let obj = Footprint::deserialize(&parse(FOOTPRINT)).unwrap();
    let s = serialize_str(&obj, "footprint");
    assert!(s.contains(" (tag \"a\")\n (tag \"b\")\n"), "{s}");
    assert!(
        s.contains(
            " (3d_model 1ce5ab2f-f1d0-4e23-9b3c-c5a5b8c6b4d6)\n \
             (3d_model d48b8bd2-a46c-4495-87a5-662747034098)\n"
        ),
        "{s}"
    );
}

#[test]
fn test_serialize_and_deserialize() {
    let mut obj = Footprint::new(
        Uuid::new_random(),
        ElementName::new("foo").unwrap(),
        "bar".into(),
    );
    obj.names_mut()
        .insert("de_DE", ElementName::new("Foo").unwrap());
    assert!(obj.set_tags(BTreeSet::from([Tag::new("tag").unwrap()])));
    assert!(obj.set_model_position((Length::new(1), Length::new(2), Length::new(3))));
    assert!(obj.set_model_rotation((Angle::new(4), Angle::new(5), Angle::new(6))));
    assert!(obj.set_models(BTreeSet::from([Uuid::new_random()])));
    obj.polygons_mut()
        .push(rect_polygon(Layer::TOP_COURTYARD, 0, 1_000_000));
    assert_roundtrip(&obj);

    let copy = Footprint::deserialize(&parse(&serialize_str(&obj, "footprint"))).unwrap();
    assert_eq!(copy, obj);
}

#[test]
fn test_calculate_bounding_rect_empty() {
    let obj = Footprint::new(
        Uuid::new_random(),
        ElementName::new("foo").unwrap(),
        String::new(),
    );
    assert_eq!(
        obj.calculate_bounding_rect(true),
        (Point::default(), Point::default())
    );
}

#[test]
fn test_calculate_bounding_rect() {
    let mut obj = Footprint::new(
        Uuid::new_random(),
        ElementName::new("foo").unwrap(),
        String::new(),
    );
    obj.polygons_mut()
        .push(rect_polygon(Layer::TOP_LEGEND, 0, 5_000_000));
    assert_eq!(
        obj.calculate_bounding_rect(true),
        (
            Point::from_nm(-5_000_000, -5_000_000),
            Point::from_nm(5_000_000, 5_000_000)
        )
    );

    // Documentation takes precedence over legend.
    obj.polygons_mut()
        .push(rect_polygon(Layer::BOT_DOCUMENTATION, 0, 4_000_000));
    assert_eq!(
        obj.calculate_bounding_rect(true),
        (
            Point::from_nm(-4_000_000, -4_000_000),
            Point::from_nm(4_000_000, 4_000_000)
        )
    );

    // Package outlines take precedence over documentation. Outlines with a
    // width are taken into account as strokes.
    obj.polygons_mut().push(rect_polygon(
        Layer::TOP_PACKAGE_OUTLINES,
        200_000,
        2_000_000,
    ));
    let expected = (
        Point::from_nm(-2_100_000, -2_100_000),
        Point::from_nm(2_100_000, 2_100_000),
    );
    assert_eq!(obj.calculate_bounding_rect(true), expected);

    // Courtyard takes precedence over package outlines, if requested.
    obj.polygons_mut()
        .push(rect_polygon(Layer::BOT_COURTYARD, 0, 3_000_000));
    assert_eq!(
        obj.calculate_bounding_rect(true),
        (
            Point::from_nm(-3_000_000, -3_000_000),
            Point::from_nm(3_000_000, 3_000_000)
        )
    );
    assert_eq!(obj.calculate_bounding_rect(false), expected);
}

#[test]
fn test_bounding_rect_ignores_zero_length_outlines() {
    // A polygon without extent draws nothing, so the next layer group is
    // used (upstream: `QPainterPath::isEmpty()`).
    let mut obj = Footprint::new(
        Uuid::new_random(),
        ElementName::new("foo").unwrap(),
        String::new(),
    );
    obj.polygons_mut().push(Polygon::new(
        Uuid::new_random(),
        Layer::TOP_PACKAGE_OUTLINES,
        unsigned(0),
        false,
        false,
        Path::line(
            Point::from_nm(1_000, 1_000),
            Point::from_nm(1_000, 1_000),
            Angle::DEG0,
        ),
    ));
    obj.polygons_mut()
        .push(rect_polygon(Layer::TOP_LEGEND, 0, 5_000_000));
    assert_eq!(
        obj.calculate_bounding_rect(false),
        (
            Point::from_nm(-5_000_000, -5_000_000),
            Point::from_nm(5_000_000, 5_000_000)
        )
    );
}
