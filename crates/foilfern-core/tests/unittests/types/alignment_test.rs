//! Port of tests/unittests/core/types/alignmenttest.cpp.
//!
//! The `toQtAlign()`/`fromQt()` tests are not ported (Qt specific API).

use foilfern_core::serialization::{DeserializeObject, List, Mode, SExpression, SerializeObject};
use foilfern_core::types::{Alignment, HAlign, VAlign};

struct AlignmentTestData {
    h: HAlign,
    v: VAlign,
    h_mirrored: HAlign,
    v_mirrored: VAlign,
    serialized: &'static str,
    valid_sexpression: bool,
}

const fn d(
    h: HAlign,
    v: VAlign,
    h_mirrored: HAlign,
    v_mirrored: VAlign,
    serialized: &'static str,
    valid_sexpression: bool,
) -> AlignmentTestData {
    AlignmentTestData {
        h,
        v,
        h_mirrored,
        v_mirrored,
        serialized,
        valid_sexpression,
    }
}

use HAlign as H;
use VAlign as V;

#[rustfmt::skip]
const DATA: &[AlignmentTestData] = &[
    // Invalid serialization
    d(H::Center, V::Center, H::Center, V::Center, "(align \"\" \"\")\n", false),
    d(H::Center, V::Center, H::Center, V::Center, "(align center foo)\n", false),
    d(H::Center, V::Center, H::Center, V::Center, "(align center)\n", false),
    d(H::Center, V::Center, H::Center, V::Center, "(align)\n", false),
    d(H::Center, V::Center, H::Center, V::Center, "center\n", false),
    // Valid serialization
    d(H::Left,   V::Bottom, H::Right,  V::Top,    "(align left bottom)\n", true),
    d(H::Right,  V::Top,    H::Left,   V::Bottom, "(align right top)\n", true),
    d(H::Center, V::Center, H::Center, V::Center, "(align center center)\n", true),
];

#[test]
fn test_construct_from_sexpression() {
    for data in DATA {
        let sexpr = SExpression::parse(data.serialized.as_bytes(), None, Mode::LibrePcb).unwrap();
        let result = Alignment::deserialize(&sexpr);
        if data.valid_sexpression {
            assert_eq!(result, Ok(Alignment::new(data.h, data.v)));
        } else {
            assert!(result.is_err(), "{}", data.serialized);
        }
    }
}

#[test]
fn test_serialize() {
    for data in DATA.iter().filter(|d| d.valid_sexpression) {
        let mut root = List::new("align");
        Alignment::new(data.h, data.v).serialize(&mut root);
        let bytes = SExpression::from(root)
            .to_byte_array(Mode::LibrePcb)
            .unwrap();
        assert_eq!(bytes, data.serialized.as_bytes());
    }
}

#[test]
fn test_mirror() {
    for data in DATA {
        let alignment = Alignment::new(data.h, data.v);
        assert_eq!(
            alignment.mirrored(),
            Alignment::new(data.h_mirrored, data.v_mirrored)
        );
        assert_eq!(
            alignment.mirrored_h(),
            Alignment::new(data.h_mirrored, data.v)
        );
        assert_eq!(
            alignment.mirrored_v(),
            Alignment::new(data.h, data.v_mirrored)
        );
        assert_eq!(alignment, Alignment::new(data.h, data.v));
    }
}
