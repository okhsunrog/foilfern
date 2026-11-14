//! Port of
//! tests/unittests/core/project/board/boardfabricationoutputsettingstest.cpp.

use foilfern_core::project::board::BoardFabricationOutputSettings;
use foilfern_core::serialization::{DeserializeObject, List, Mode, SExpression, SerializeObject};

fn to_bytes(obj: &BoardFabricationOutputSettings) -> Vec<u8> {
    let mut root = List::new("obj");
    obj.serialize(&mut root);
    SExpression::from(root)
        .to_byte_array(Mode::LibrePcb)
        .unwrap()
}

#[test]
fn test_serialize_and_deserialize() {
    let mut obj1 = BoardFabricationOutputSettings::default();
    obj1.output_base_path = "a".into();
    obj1.suffix_drills = "b".into();
    obj1.suffix_drills_npth = "c".into();
    obj1.suffix_drills_pth = "d".into();
    obj1.suffix_outlines = "e".into();
    obj1.suffix_copper_top = "f".into();
    obj1.suffix_copper_inner = "g".into();
    obj1.suffix_copper_bot = "h".into();
    obj1.suffix_solder_mask_top = "i".into();
    obj1.suffix_solder_mask_bot = "j".into();
    obj1.suffix_silkscreen_top = "k".into();
    obj1.suffix_silkscreen_bot = "l".into();
    obj1.suffix_solder_paste_top = "m".into();
    obj1.suffix_solder_paste_bot = "n".into();
    obj1.merge_drill_files = !obj1.merge_drill_files;
    obj1.use_g85_slot_command = !obj1.use_g85_slot_command;
    obj1.enable_solder_paste_top = !obj1.enable_solder_paste_top;
    obj1.enable_solder_paste_bot = !obj1.enable_solder_paste_bot;
    let bytes1 = to_bytes(&obj1);
    let sexpr = SExpression::parse(&bytes1, None, Mode::LibrePcb).unwrap();
    let obj2 = BoardFabricationOutputSettings::deserialize(&sexpr).unwrap();
    assert_eq!(bytes1, to_bytes(&obj2));
    assert_eq!(obj1, obj2);
}
