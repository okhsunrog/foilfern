//! Serde (JSON) representation of boards, board mutations and changes
//! (see `docs/project-model-design.md`, "Serde representation").

use foilfern_core::geometry::{PadShape, TraceAnchor, Via, ZoneRules};
use foilfern_core::project::board::{
    Board, BoardChange, BoardSegmentElements, BoardSegmentItems, BoardSettings, PlaneConnectStyle,
};
use foilfern_core::project::{BoardMutation, BoardNetSegmentRef, Change, Mutation};
use foilfern_core::types::{MaskConfig, PcbColor};

use super::*;

fn roundtrip<T>(value: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let json = serde_json::to_string(value).unwrap();
    serde_json::from_str(&json).unwrap()
}

#[test]
fn value_types() {
    assert_eq!(
        serde_json::to_string(&Ratio::new(500_000)).unwrap(),
        "500000"
    );
    assert_eq!(
        serde_json::to_string(&MaskConfig::Manual(Length::new(100))).unwrap(),
        "{\"Manual\":100}"
    );
    assert_eq!(serde_json::to_string(&MaskConfig::Off).unwrap(), "\"Off\"");
    assert_eq!(
        serde_json::to_string(&PcbColor::Green).unwrap(),
        "\"green\""
    );
    assert_eq!(
        serde_json::to_string(&PadShape::RoundedRect).unwrap(),
        "\"roundrect\""
    );
    assert_eq!(
        serde_json::to_string(&PlaneConnectStyle::ThermalRelief).unwrap(),
        "\"thermal\""
    );
    assert_eq!(
        serde_json::to_string(&(ZoneRules::NO_COPPER | ZoneRules::NO_DEVICES)).unwrap(),
        "[\"NO_COPPER\",\"NO_DEVICES\"]"
    );
    assert_eq!(
        serde_json::from_str::<ZoneRules>("[\"NO_PLANES\"]").unwrap(),
        ZoneRules::NO_PLANES
    );
    assert!(serde_json::from_str::<ZoneRules>("[\"FOO\"]").is_err());
    let anchor = TraceAnchor::FootprintPad {
        device: "d2c30518-5cd1-4ce9-a569-44f783a3f66a".parse().unwrap(),
        pad: "d2c30518-5cd1-4ce9-a569-44f783a3f66b".parse().unwrap(),
    };
    assert_eq!(
        serde_json::to_string(&anchor).unwrap(),
        "{\"FootprintPad\":{\"device\":\"d2c30518-5cd1-4ce9-a569-44f783a3f66a\",\
         \"pad\":\"d2c30518-5cd1-4ce9-a569-44f783a3f66b\"}}"
    );
    assert_eq!(
        roundtrip(&BoardSettings::default()),
        BoardSettings::default()
    );
    // Validation on deserialization.
    assert!(serde_json::from_str::<UnsignedLimitedRatio>("2000000").is_err());
    let via = Via::new(
        Uuid::new_random(),
        Layer::TOP_COPPER,
        Layer::BOT_COPPER,
        pt(1.0, 1.0),
        None,
        None,
        MaskConfig::Off,
    )
    .unwrap();
    let mut json: serde_json::Value = serde_json::to_value(&via).unwrap();
    json["start_layer"] = "bot_cu".into();
    json["end_layer"] = "top_cu".into();
    assert!(serde_json::from_value::<Via>(json).is_err());
}

#[test]
fn board_and_mutations() {
    let mut f = Fixture::with_devices();
    let board = f.board;
    let (segment, junction) = gnd_segment(&f);
    let seg_ref = BoardNetSegmentRef {
        board,
        segment: segment.id(),
    };
    f.p.add_board_net_segment(board, segment.clone()).unwrap();
    for item in super::mutation_test::sample_items() {
        f.p.add_board_item(board, item).unwrap();
    }
    let value: &Board = f.board();
    assert_eq!(&roundtrip(value), value);

    let mutations = vec![
        Mutation::Board(BoardMutation::AddNetSegment {
            board,
            segment: segment.clone(),
        }),
        Mutation::Board(BoardMutation::AddNetSegmentElements {
            segment: seg_ref,
            elements: BoardSegmentElements {
                junctions: vec![junction.clone()],
                traces: segment.traces().values().cloned().collect(),
                ..Default::default()
            },
        }),
        Mutation::Board(BoardMutation::RemoveNetSegmentElements {
            segment: seg_ref,
            elements: BoardSegmentItems {
                junctions: vec![junction.uuid()],
                ..Default::default()
            },
        }),
        Mutation::Board(BoardMutation::SetSettings {
            board,
            settings: Box::default(),
        }),
        Mutation::Board(BoardMutation::AddDevice {
            board,
            device: f.board().device(f.r1).unwrap().clone(),
        }),
    ];
    for m in &mutations {
        assert_eq!(&roundtrip(m), m);
    }
    let change = Change::Board {
        id: board,
        change: BoardChange::NetSegmentElementsAdded {
            segment: segment.id(),
            items: BoardSegmentItems {
                traces: vec![Uuid::new_random()],
                ..Default::default()
            },
        },
    };
    assert_eq!(roundtrip(&change), change);

    // A map key which doesn't match the item UUID is rejected.
    let mut json = serde_json::to_value(&segment).unwrap();
    let junctions = json["junctions"].as_object_mut().unwrap();
    let value = junctions.remove(&junction.uuid().to_string()).unwrap();
    junctions.insert(Uuid::new_random().to_string(), value);
    let error = serde_json::from_value::<BoardNetSegment>(json).unwrap_err();
    assert!(
        error.to_string().contains("does not match the UUID"),
        "{error}"
    );
}
