//! Port of tests/unittests/core/export/interactivehtmlbomtest.cpp.

use librepcb_core::export::{
    InteractiveHtmlBom, InteractiveHtmlBomDrawingKind, InteractiveHtmlBomDrawingLayer,
    InteractiveHtmlBomHighlightPin1Mode, InteractiveHtmlBomLayer, InteractiveHtmlBomPad,
    InteractiveHtmlBomSides, InteractiveHtmlBomViewMode,
};
use librepcb_core::geometry::Path;
use librepcb_core::serialization::{FromSExpression, ToSExpression};
use librepcb_core::types::{Angle, Length, Point, PositiveLength, UnsignedLength};

fn pos(nm: i64) -> PositiveLength {
    PositiveLength::new(Length::new(nm)).unwrap()
}

fn unsigned(nm: i64) -> UnsignedLength {
    UnsignedLength::new(Length::new(nm)).unwrap()
}

fn rect() -> Path {
    Path::centered_rect(pos(100000), pos(100000), UnsignedLength::ZERO)
}

#[test]
fn view_mode_serialization() {
    for x in [
        InteractiveHtmlBomViewMode::BomOnly,
        InteractiveHtmlBomViewMode::LeftRight,
        InteractiveHtmlBomViewMode::TopBottom,
    ] {
        let node = x.to_sexpression();
        assert_eq!(
            InteractiveHtmlBomViewMode::from_sexpression(&node).unwrap(),
            x
        );
    }
}

#[test]
fn highlight_pin1_mode_serialization() {
    for x in [
        InteractiveHtmlBomHighlightPin1Mode::None,
        InteractiveHtmlBomHighlightPin1Mode::Selected,
        InteractiveHtmlBomHighlightPin1Mode::All,
    ] {
        let node = x.to_sexpression();
        assert_eq!(
            InteractiveHtmlBomHighlightPin1Mode::from_sexpression(&node).unwrap(),
            x
        );
    }
}

#[test]
fn generate_html() {
    let mut ibom = InteractiveHtmlBom::new(
        "Title",
        "Company",
        "Revision",
        "Date",
        Point::from_nm(0, 0),
        Point::from_nm(100000000, 100000000),
    );
    ibom.set_fields(vec!["Field 1".into(), "Field 2".into()]);

    ibom.add_drawing(
        InteractiveHtmlBomDrawingKind::Polygon,
        InteractiveHtmlBomDrawingLayer::Edge,
        &rect(),
        unsigned(0),
        false,
    );
    ibom.add_drawing(
        InteractiveHtmlBomDrawingKind::Polygon,
        InteractiveHtmlBomDrawingLayer::SilkscreenFront,
        &rect(),
        unsigned(0),
        false,
    );
    ibom.add_drawing(
        InteractiveHtmlBomDrawingKind::ReferenceText,
        InteractiveHtmlBomDrawingLayer::SilkscreenBack,
        &rect(),
        unsigned(100000),
        true,
    );
    ibom.add_drawing(
        InteractiveHtmlBomDrawingKind::Polygon,
        InteractiveHtmlBomDrawingLayer::FabricationFront,
        &rect(),
        unsigned(100000),
        false,
    );
    ibom.add_drawing(
        InteractiveHtmlBomDrawingKind::ValueText,
        InteractiveHtmlBomDrawingLayer::FabricationBack,
        &rect(),
        unsigned(0),
        true,
    );

    ibom.add_track(
        InteractiveHtmlBomLayer::Top,
        Point::from_nm(0, 0),
        Point::from_nm(100000, 100000),
        pos(100000),
        None,
    );
    ibom.add_track(
        InteractiveHtmlBomLayer::Bottom,
        Point::from_nm(0, 0),
        Point::from_nm(100000, 100000),
        pos(100000),
        Some("net"),
    );

    ibom.add_via(
        &[InteractiveHtmlBomLayer::Top],
        Point::from_nm(0, 0),
        pos(2000000),
        pos(1000000),
        None,
    );
    ibom.add_via(
        &[
            InteractiveHtmlBomLayer::Top,
            InteractiveHtmlBomLayer::Bottom,
        ],
        Point::from_nm(100, 200),
        pos(2000000),
        pos(1000000),
        Some("net"),
    );

    ibom.add_plane_fragment(InteractiveHtmlBomLayer::Top, &rect(), None);
    ibom.add_plane_fragment(InteractiveHtmlBomLayer::Bottom, &rect(), Some("net"));

    let fields = ["Value 2".to_owned(), "Value 2".to_owned()];
    let id0 = ibom
        .add_footprint(
            InteractiveHtmlBomLayer::Top,
            Point::from_nm(0, 0),
            Angle::DEG45,
            Point::from_nm(-5, 5),
            Point::from_nm(5, -5),
            true,
            &fields,
            &[InteractiveHtmlBomPad {
                on_top: true,
                on_bottom: true,
                position: Point::ORIGIN,
                rotation: Angle::DEG0,
                mirror_geometry: false,
                geometries: Vec::new(),
                holes: Vec::new(),
                net_name: None,
                pin1: false,
            }],
        )
        .unwrap();
    let id1 = ibom
        .add_footprint(
            InteractiveHtmlBomLayer::Bottom,
            Point::from_nm(0, 0),
            Angle::DEG45,
            Point::from_nm(-5, 5),
            Point::from_nm(5, -5),
            false,
            &fields,
            &[],
        )
        .unwrap();

    ibom.add_bom_row(InteractiveHtmlBomSides::Top, &[("R1".to_owned(), id0)]);
    ibom.add_bom_row(
        InteractiveHtmlBomSides::Both,
        &[("R1".to_owned(), id0), ("R2".to_owned(), id1)],
    );

    let html = ibom.generate_html().unwrap();
    assert!(html.contains("<html"));
}

#[test]
fn generate_html_fails_for_invalid_data() {
    let mut ibom = InteractiveHtmlBom::new(
        "Title",
        "Company",
        "Revision",
        "Date",
        Point::ORIGIN,
        Point::ORIGIN,
    );
    ibom.add_bom_row(InteractiveHtmlBomSides::Top, &[("R1".to_owned(), 0)]);
    assert!(ibom.generate_html().is_err());
}
