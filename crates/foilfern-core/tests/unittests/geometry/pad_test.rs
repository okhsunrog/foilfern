//! Tests of libs/librepcb/core/geometry/pad.{h,cpp} (no upstream test
//! exists; see also library/pkg/footprint_pad_test.rs).

use std::collections::BTreeMap;

use foilfern_core::geometry::{
    ComponentSide, NonEmptyPath, Pad, PadFunction, PadGeometry, PadHole, PadHoleList, PadShape,
    Path, Vertex,
};
use foilfern_core::serialization::{DeserializeObject, List, Mode, SExpression};
use foilfern_core::types::{
    Angle, Layer, Length, MaskConfig, Point, PositiveLength, Ratio, UnsignedLength,
    UnsignedLimitedRatio, Uuid,
};

use super::parse;

fn pos(nm: i64) -> PositiveLength {
    PositiveLength::new(Length::new(nm)).unwrap()
}

fn ratio(percent: i32) -> UnsignedLimitedRatio {
    UnsignedLimitedRatio::new(Ratio::from_percent(percent)).unwrap()
}

fn tht_holes() -> PadHoleList {
    PadHoleList::from(vec![PadHole::new(
        Uuid::new_random(),
        pos(500_000),
        NonEmptyPath::from_point(Point::default()),
    )])
}

fn make_pad(
    shape: PadShape,
    width: i64,
    height: i64,
    side: ComponentSide,
    stop_mask: MaskConfig,
    solder_paste: MaskConfig,
    holes: PadHoleList,
) -> Pad {
    Pad::new(
        Uuid::new_random(),
        Point::from_nm(1_000, 2_000),
        Angle::DEG90,
        shape,
        pos(width),
        pos(height),
        ratio(50),
        Path::new(vec![
            Vertex::at(Point::from_nm(0, 0)),
            Vertex::at(Point::from_nm(width, 0)),
            Vertex::at(Point::from_nm(0, height)),
        ]),
        stop_mask,
        solder_paste,
        UnsignedLength::ZERO,
        side,
        PadFunction::StandardPad,
        holes,
    )
}

fn smt_pad(side: ComponentSide) -> Pad {
    make_pad(
        PadShape::RoundedRect,
        1_000_000,
        2_000_000,
        side,
        MaskConfig::Automatic,
        MaskConfig::Automatic,
        PadHoleList::new(),
    )
}

#[test]
fn test_recommended_radius() {
    let cases = [
        (1_000_000, 2_000_000, 50),
        (2_000_000, 3_000_000, 25),
        (3_000_000, 3_000_000, 16), // 16.67% rounded down
        (200_000, 5_000_000, 50),   // limited to 50%
        (5_000_000, 5_000_000, 10),
        (100_000_000, 100_000_000, 0),
    ];
    for (w, h, percent) in cases {
        assert_eq!(
            Pad::recommended_radius(pos(w), pos(h)),
            ratio(percent),
            "{w} x {h}"
        );
    }
}

#[test]
fn test_function_properties() {
    for &function in PadFunction::ALL {
        let fiducial = matches!(
            function,
            PadFunction::LocalFiducial | PadFunction::GlobalFiducial
        );
        let soldered = matches!(
            function,
            PadFunction::Unspecified
                | PadFunction::StandardPad
                | PadFunction::PressFitPad
                | PadFunction::ThermalPad
                | PadFunction::BgaPad
        );
        assert_eq!(function.is_fiducial(), fiducial, "{function}");
        assert_eq!(function.needs_soldering(), soldered, "{function}");
        assert!(!function.description_tr().is_empty());
    }
}

#[test]
fn test_geometry() {
    let pad = smt_pad(ComponentSide::Top);
    assert_eq!(
        pad.geometry(),
        PadGeometry::rounded_rect(
            pos(1_000_000),
            pos(2_000_000),
            ratio(50),
            PadHoleList::new()
        )
    );
    let holes = tht_holes();
    let pad = make_pad(
        PadShape::RoundedOctagon,
        1_000_000,
        2_000_000,
        ComponentSide::Top,
        MaskConfig::Off,
        MaskConfig::Off,
        holes.clone(),
    );
    assert_eq!(
        pad.geometry(),
        PadGeometry::rounded_octagon(pos(1_000_000), pos(2_000_000), ratio(50), holes.clone())
    );
    let pad = make_pad(
        PadShape::Custom,
        1_000_000,
        2_000_000,
        ComponentSide::Top,
        MaskConfig::Off,
        MaskConfig::Off,
        holes.clone(),
    );
    assert_eq!(
        pad.geometry(),
        PadGeometry::custom(pad.custom_shape_outline().clone(), holes)
    );
}

#[test]
fn test_layers_smt() {
    let top = smt_pad(ComponentSide::Top);
    assert!(!top.is_tht());
    assert_eq!(top.smt_layer(), Layer::TOP_COPPER);
    assert!(top.is_on_layer(Layer::TOP_COPPER));
    assert!(!top.is_on_layer(Layer::BOT_COPPER));
    assert!(!top.is_on_layer(Layer::inner_copper(1).unwrap()));
    assert!(top.has_top_copper());
    assert!(!top.has_bottom_copper());
    assert!(top.has_auto_top_stop_mask());
    assert!(!top.has_auto_bottom_stop_mask());
    assert!(top.has_auto_top_solder_paste());
    assert!(!top.has_auto_bottom_solder_paste());

    let bot = smt_pad(ComponentSide::Bottom);
    assert_eq!(bot.smt_layer(), Layer::BOT_COPPER);
    assert!(bot.is_on_layer(Layer::BOT_COPPER));
    assert!(!bot.is_on_layer(Layer::TOP_COPPER));
    assert!(!bot.has_top_copper());
    assert!(bot.has_bottom_copper());
    assert!(!bot.has_auto_top_stop_mask());
    assert!(bot.has_auto_bottom_stop_mask());
    assert!(!bot.has_auto_top_solder_paste());
    assert!(bot.has_auto_bottom_solder_paste());
}

#[test]
fn test_layers_tht() {
    let pad = make_pad(
        PadShape::RoundedRect,
        1_000_000,
        1_000_000,
        ComponentSide::Top,
        MaskConfig::Automatic,
        MaskConfig::Automatic,
        tht_holes(),
    );
    assert!(pad.is_tht());
    assert!(pad.is_on_layer(Layer::TOP_COPPER));
    assert!(pad.is_on_layer(Layer::inner_copper(1).unwrap()));
    assert!(pad.is_on_layer(Layer::BOT_COPPER));
    assert!(!pad.is_on_layer(Layer::TOP_STOP_MASK));
    assert!(pad.has_top_copper());
    assert!(pad.has_bottom_copper());
    assert!(pad.has_auto_top_stop_mask());
    assert!(pad.has_auto_bottom_stop_mask());
    // Solder paste only on the side opposite to the component.
    assert!(!pad.has_auto_top_solder_paste());
    assert!(pad.has_auto_bottom_solder_paste());

    let mut pad = pad;
    assert!(pad.set_stop_mask_config(MaskConfig::Off));
    assert!(pad.set_solder_paste_config(MaskConfig::Off));
    assert!(!pad.has_auto_top_stop_mask());
    assert!(!pad.has_auto_bottom_stop_mask());
    assert!(!pad.has_auto_bottom_solder_paste());
}

#[test]
fn test_build_preview_geometries_smt() {
    // Automatic offsets: 100µm stop mask, solder paste 10% of the smaller
    // pad side.
    let pad = smt_pad(ComponentSide::Top);
    let g = pad.geometry();
    assert_eq!(
        pad.build_preview_geometries(),
        BTreeMap::from([
            (Layer::TOP_COPPER, vec![g.clone()]),
            (
                Layer::TOP_STOP_MASK,
                vec![g.with_offset(Length::new(100_000))]
            ),
            (
                Layer::TOP_SOLDER_PASTE,
                vec![g.with_offset(Length::new(-100_000))]
            ),
        ])
    );

    // Manual offsets, bottom side.
    let pad = make_pad(
        PadShape::RoundedRect,
        1_000_000,
        2_000_000,
        ComponentSide::Bottom,
        MaskConfig::Manual(Length::new(50_000)),
        MaskConfig::Manual(Length::new(30_000)),
        PadHoleList::new(),
    );
    let g = pad.geometry();
    assert_eq!(
        pad.build_preview_geometries(),
        BTreeMap::from([
            (Layer::BOT_COPPER, vec![g.clone()]),
            (
                Layer::BOT_STOP_MASK,
                vec![g.with_offset(Length::new(50_000))]
            ),
            (
                Layer::BOT_SOLDER_PASTE,
                vec![g.with_offset(Length::new(-30_000))]
            ),
        ])
    );

    // Automatic solder paste offset is limited to 1mm, and zero for custom
    // shapes.
    let pad = make_pad(
        PadShape::RoundedRect,
        20_000_000,
        30_000_000,
        ComponentSide::Top,
        MaskConfig::Off,
        MaskConfig::Automatic,
        PadHoleList::new(),
    );
    let g = pad.geometry();
    assert_eq!(
        pad.build_preview_geometries()[&Layer::TOP_SOLDER_PASTE],
        [g.with_offset(Length::new(-1_000_000))]
    );
    assert!(
        !pad.build_preview_geometries()
            .contains_key(&Layer::TOP_STOP_MASK)
    );
    let pad = make_pad(
        PadShape::Custom,
        1_000_000,
        2_000_000,
        ComponentSide::Top,
        MaskConfig::Off,
        MaskConfig::Automatic,
        PadHoleList::new(),
    );
    let g = pad.geometry();
    assert_eq!(
        pad.build_preview_geometries()[&Layer::TOP_SOLDER_PASTE],
        [g.with_offset(Length::ZERO)]
    );
}

#[test]
fn test_build_preview_geometries_tht() {
    let pad = make_pad(
        PadShape::RoundedOctagon,
        1_500_000,
        1_500_000,
        ComponentSide::Top,
        MaskConfig::Automatic,
        MaskConfig::Off,
        tht_holes(),
    );
    let g = pad.geometry();
    let mask = g.with_offset(Length::new(100_000));
    assert_eq!(
        pad.build_preview_geometries(),
        BTreeMap::from([
            (Layer::TOP_COPPER, vec![g.clone()]),
            (Layer::TOP_STOP_MASK, vec![mask.clone()]),
            (Layer::BOT_COPPER, vec![g]),
            (Layer::BOT_STOP_MASK, vec![mask]),
        ])
    );
}

#[test]
fn test_serialize_with() {
    let node = parse(
        "(pad 7040952d-7016-49cd-8c3e-6078ecca98b9 (side bottom) (shape custom)\n \
         (position 1.234 2.345) (rotation 45.0) (size 1.1 2.2) (radius 0.5)\n \
         (stop_mask off) (solder_paste auto) (clearance 0.33) (function standard)\n \
         (vertex (position -1.1 -2.2) (angle 45.0))\n \
         (hole 7040952d-7016-49cd-8c3e-6078ecca98b9 (diameter 1.0)\n  \
         (vertex (position 1.1 2.2) (angle 45.0))\n )\n)",
    );
    let pad = Pad::deserialize(&node).unwrap();
    let mut root = List::new("pad");
    pad.serialize_with(&mut root, |root| {
        root.append_child("lock", &true);
    });
    let s = SExpression::List(root)
        .to_string_with_mode(Mode::LibrePcb)
        .unwrap();
    assert_eq!(
        s,
        "(pad 7040952d-7016-49cd-8c3e-6078ecca98b9 (side bottom) (shape custom)\n \
         (position 1.234 2.345) (rotation 45.0) (size 1.1 2.2) (radius 0.5)\n \
         (stop_mask off) (solder_paste auto) (clearance 0.33) (function standard)\n \
         (lock true)\n \
         (vertex (position -1.1 -2.2) (angle 45.0))\n \
         (hole 7040952d-7016-49cd-8c3e-6078ecca98b9 (diameter 1.0)\n  \
         (vertex (position 1.1 2.2) (angle 45.0))\n )\n)\n"
    );
}
