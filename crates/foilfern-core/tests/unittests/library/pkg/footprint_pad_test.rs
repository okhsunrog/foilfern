//! Port of tests/unittests/core/library/pkg/footprintpadtest.cpp.

use foilfern_core::geometry::{
    ComponentSide, NonEmptyPath, Pad, PadFunction, PadHole, PadHoleList, PadShape, Path, Vertex,
};
use foilfern_core::library::pkg::FootprintPad;
use foilfern_core::serialization::{
    DeserializeObject, FromSExpression, SExpression, ToSExpression,
};
use foilfern_core::types::{
    Angle, Length, MaskConfig, Point, PositiveLength, Ratio, UnsignedLength, UnsignedLimitedRatio,
    Uuid,
};

use crate::geometry::{assert_roundtrip, parse};

fn pos(nm: i64) -> PositiveLength {
    PositiveLength::new(Length::new(nm)).unwrap()
}

fn uuid(s: &str) -> Uuid {
    s.parse().unwrap()
}

#[test]
fn test_functions_serialization() {
    let items = [
        (PadFunction::Unspecified, "unspecified"),
        (PadFunction::StandardPad, "standard"),
        (PadFunction::PressFitPad, "pressfit"),
        (PadFunction::ThermalPad, "thermal"),
        (PadFunction::BgaPad, "bga"),
        (PadFunction::EdgeConnectorPad, "edge_connector"),
        (PadFunction::TestPad, "test"),
        (PadFunction::LocalFiducial, "local_fiducial"),
        (PadFunction::GlobalFiducial, "global_fiducial"),
    ];
    for (function, token) in items {
        // Serialize
        assert_eq!(function.to_sexpression().value().unwrap(), token);

        // Deserialize
        assert_eq!(
            PadFunction::from_sexpression(&SExpression::token(token)).unwrap(),
            function
        );
    }

    // "press_fit" was only accepted in LibrePCB 1.x (file format "1"); the
    // current file format is "2".
    assert!(PadFunction::from_sexpression(&SExpression::token("press_fit")).is_err());
}

#[test]
fn test_construct_from_sexpression_connected() {
    let obj = FootprintPad::deserialize(&parse(
        "(pad 7040952d-7016-49cd-8c3e-6078ecca98b9 (side top) (shape roundrect)\n \
         (position 1.234 2.345) (rotation 45.0) (size 1.1 2.2) (radius 0.5)\n \
         (stop_mask auto) (solder_paste 0.25) (clearance 0.33) (function unspecified)\n \
         (package_pad d48b8bd2-a46c-4495-87a5-662747034098)\n)",
    ))
    .unwrap();
    let pad = obj.pad();
    assert_eq!(obj.uuid(), uuid("7040952d-7016-49cd-8c3e-6078ecca98b9"));
    assert_eq!(
        obj.package_pad_uuid(),
        Some(uuid("d48b8bd2-a46c-4495-87a5-662747034098"))
    );
    assert_eq!(pad.position(), Point::from_nm(1_234_000, 2_345_000));
    assert_eq!(pad.rotation(), Angle::DEG45);
    assert_eq!(pad.shape(), PadShape::RoundedRect);
    assert_eq!(pad.width(), pos(1_100_000));
    assert_eq!(*pad.height(), Length::new(2_200_000));
    assert_eq!(
        pad.radius(),
        UnsignedLimitedRatio::new(Ratio::from_percent(50)).unwrap()
    );
    assert_eq!(pad.stop_mask_config(), MaskConfig::Automatic);
    assert_eq!(
        pad.solder_paste_config(),
        MaskConfig::Manual(Length::new(250_000))
    );
    assert_eq!(*pad.copper_clearance(), Length::new(330_000));
    assert_eq!(pad.component_side(), ComponentSide::Top);
    assert_eq!(pad.function(), PadFunction::Unspecified);
    assert_eq!(pad.holes().len(), 0);
}

#[test]
fn test_construct_from_sexpression_unconnected() {
    let obj = FootprintPad::deserialize(&parse(
        "(pad 7040952d-7016-49cd-8c3e-6078ecca98b9 (side bottom) (shape custom)\n \
         (position 1.234 2.345) (rotation 45.0) (size 1.1 2.2) (radius 0.5)\n \
         (stop_mask off) (solder_paste auto) (clearance 0.33) (function standard)\n \
         (package_pad none)\n \
         (vertex (position -1.1 -2.2) (angle 45.0))\n \
         (vertex (position 1.1 -2.2) (angle 90.0))\n \
         (vertex (position 0.0 2.2) (angle 0.0))\n \
         (hole 7040952d-7016-49cd-8c3e-6078ecca98b9 (diameter 1.0)\n  \
         (vertex (position 1.1 2.2) (angle 45.0))\n )\n \
         (hole d48b8bd2-a46c-4495-87a5-662747034098 (diameter 2.0)\n  \
         (vertex (position 3.3 4.4) (angle 0.0))\n )\n)",
    ))
    .unwrap();
    let pad = obj.pad();
    assert_eq!(obj.uuid(), uuid("7040952d-7016-49cd-8c3e-6078ecca98b9"));
    assert_eq!(obj.package_pad_uuid(), None);
    assert_eq!(pad.position(), Point::from_nm(1_234_000, 2_345_000));
    assert_eq!(pad.rotation(), Angle::DEG45);
    assert_eq!(pad.shape(), PadShape::Custom);
    assert_eq!(pad.width(), pos(1_100_000));
    assert_eq!(*pad.height(), Length::new(2_200_000));
    assert_eq!(
        pad.radius(),
        UnsignedLimitedRatio::new(Ratio::from_percent(50)).unwrap()
    );
    assert_eq!(pad.stop_mask_config(), MaskConfig::Off);
    assert_eq!(pad.solder_paste_config(), MaskConfig::Automatic);
    assert_eq!(*pad.copper_clearance(), Length::new(330_000));
    assert_eq!(pad.component_side(), ComponentSide::Bottom);
    assert_eq!(pad.function(), PadFunction::StandardPad);
    assert_eq!(pad.custom_shape_outline().vertices().len(), 3);
    assert_eq!(pad.holes().len(), 2);
}

#[test]
fn test_serialize_and_deserialize() {
    let pad = Pad::new(
        Uuid::new_random(),
        Point::from_nm(123, 567),
        Angle::new(789),
        PadShape::RoundedOctagon,
        pos(123),
        pos(456),
        UnsignedLimitedRatio::new(Ratio::from_percent(50)).unwrap(),
        Path::new(vec![
            Vertex::new(Point::from_nm(1, 2), Angle::new(3)),
            Vertex::new(Point::from_nm(4, 5), Angle::new(6)),
        ]),
        MaskConfig::Automatic,
        MaskConfig::Manual(Length::new(123_456)),
        UnsignedLength::new(Length::new(98_765)).unwrap(),
        ComponentSide::Top,
        PadFunction::Unspecified,
        PadHoleList::from(vec![
            PadHole::new(
                Uuid::new_random(),
                pos(100_000),
                NonEmptyPath::from_point(Point::from_nm(100, 200)),
            ),
            PadHole::new(
                Uuid::new_random(),
                pos(200_000),
                NonEmptyPath::from_point(Point::from_nm(300, 400)),
            ),
        ]),
    );
    let obj = FootprintPad::new(pad, Some(Uuid::new_random()));
    assert_roundtrip(&obj);
}
