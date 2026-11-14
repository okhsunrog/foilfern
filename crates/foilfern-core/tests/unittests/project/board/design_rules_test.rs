//! Port of tests/unittests/core/project/board/boarddesignrulestest.cpp.

use foilfern_core::project::board::BoardDesignRules;
use foilfern_core::serialization::{DeserializeObject, List, Mode, SExpression, SerializeObject};
use foilfern_core::types::{
    BoundedUnsignedRatio, Length, PositiveLength, Ratio, UnsignedLength, UnsignedRatio,
};

fn ul(nm: i64) -> UnsignedLength {
    UnsignedLength::new(Length::new(nm)).unwrap()
}

fn bounded(ppm: i32, min: i64, max: i64) -> BoundedUnsignedRatio {
    BoundedUnsignedRatio::new(
        UnsignedRatio::new(Ratio::new(ppm)).unwrap(),
        ul(min),
        ul(max),
    )
    .unwrap()
}

fn to_bytes(rules: &BoardDesignRules) -> Vec<u8> {
    let mut root = List::new("obj");
    rules.serialize(&mut root);
    SExpression::from(root)
        .to_byte_array(Mode::LibrePcb)
        .unwrap()
}

#[test]
fn test_construct_from_sexpression() {
    let sexpr = SExpression::parse(
        b"(design_rules\n \
          (default_trace_width 0.31)\n \
          (default_via_drill_diameter 0.51)\n \
          (stopmask_max_via_drill_diameter 0.2)\n \
          (stopmask_clearance (ratio 0.1) (min 1.1) (max 2.1))\n \
          (solderpaste_clearance (ratio 0.3) (min 1.3) (max 2.3))\n \
          (pad_annular_ring (outer auto) (inner full) (ratio 0.4) (min 1.4) (max 2.4))\n \
          (via_annular_ring (ratio 0.5) (min 1.5) (max 2.5))\n\
          )",
        None,
        Mode::LibrePcb,
    )
    .unwrap();
    let obj = BoardDesignRules::deserialize(&sexpr).unwrap();
    assert_eq!(
        PositiveLength::new(Length::new(310_000)).unwrap(),
        obj.default_trace_width()
    );
    assert_eq!(
        PositiveLength::new(Length::new(510_000)).unwrap(),
        obj.default_via_drill_diameter()
    );
    assert_eq!(ul(200_000), obj.stop_mask_max_via_drill_diameter());
    assert_eq!(
        bounded(100_000, 1_100_000, 2_100_000),
        obj.stop_mask_clearance()
    );
    assert_eq!(
        bounded(300_000, 1_300_000, 2_300_000),
        obj.solder_paste_clearance()
    );
    assert!(obj.pad_cmp_side_auto_annular_ring());
    assert!(!obj.pad_inner_auto_annular_ring());
    assert_eq!(
        bounded(400_000, 1_400_000, 2_400_000),
        obj.pad_annular_ring()
    );
    assert_eq!(
        bounded(500_000, 1_500_000, 2_500_000),
        obj.via_annular_ring()
    );
}

#[test]
fn test_serialize_and_deserialize() {
    let mut obj1 = BoardDesignRules::default();
    obj1.set_default_trace_width(PositiveLength::new(Length::new(33)).unwrap());
    obj1.set_default_via_drill_diameter(PositiveLength::new(Length::new(22)).unwrap());
    obj1.set_stop_mask_max_via_drill_diameter(ul(44));
    obj1.set_stop_mask_clearance(bounded(11, 22, 33));
    obj1.set_solder_paste_clearance(bounded(55, 66, 77));
    obj1.set_pad_cmp_side_auto_annular_ring(true);
    obj1.set_pad_inner_auto_annular_ring(false);
    obj1.set_pad_annular_ring(bounded(88, 99, 111));
    obj1.set_via_annular_ring(bounded(222, 333, 444));
    let bytes1 = to_bytes(&obj1);
    let sexpr = SExpression::parse(&bytes1, None, Mode::LibrePcb).unwrap();
    let obj2 = BoardDesignRules::deserialize(&sexpr).unwrap();
    assert_eq!(bytes1, to_bytes(&obj2));
    assert_eq!(obj1, obj2);
}

#[test]
fn invalid_pad_annular_shape() {
    let mut bytes = to_bytes(&BoardDesignRules::default());
    let text = String::from_utf8(bytes.clone()).unwrap();
    bytes = text.replace("(outer full)", "(outer foo)").into_bytes();
    let sexpr = SExpression::parse(&bytes, None, Mode::LibrePcb).unwrap();
    let error = BoardDesignRules::deserialize(&sexpr).unwrap_err();
    assert_eq!(error.to_string(), "Invalid pad annular shape: 'foo'");
}
