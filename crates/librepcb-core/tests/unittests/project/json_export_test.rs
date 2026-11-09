//! Port of tests/unittests/core/project/projectjsonexporttest.cpp.
//!
//! Upstream compares both sides after re-formatting them with
//! `QJsonDocument`; here both sides are formatted with
//! [`json_export::to_indented_json()`] (which reproduces
//! `QJsonDocument::toJson()`), and the complete file of `toUtf8()` is also
//! compared byte for byte.

use std::collections::BTreeSet;

use chrono::{TimeZone, Utc};
use librepcb_core::fileio::TransactionalDirectory;
use librepcb_core::geometry::{Path, Vertex};
use librepcb_core::project::board::{Board, BoardItem, BoardPolygonData};
use librepcb_core::project::circuit::AssemblyVariant;
use librepcb_core::project::json_export::{self, BoundingBox, to_indented_json};
use librepcb_core::project::{AssemblyVariantId, Mutation, Project};
use librepcb_core::types::{
    ElementName, FileProofName, Layer, Length, PcbColor, Point, PositiveLength, UnsignedLength,
    Uuid,
};

fn uuid(s: &str) -> Uuid {
    s.parse().unwrap()
}

fn fmt(json: &str) -> String {
    to_indented_json(&serde_json::from_str(json).unwrap())
}

fn fmt_value(value: &serde_json::Value) -> String {
    to_indented_json(value)
}

fn create_assembly_variant() -> AssemblyVariant {
    AssemblyVariant::new(
        uuid("bb0d66f1-2f21-4592-b923-d853867a6124"),
        FileProofName::new("AV0").unwrap(),
        "Hello World!",
    )
}

fn create_board() -> Board {
    let mut board = Board::new(
        uuid("1ff89be5-dd83-4b08-8d95-d09e0fd72b25"),
        ElementName::new("New Board").unwrap(),
        "board",
    );
    let mut settings = board.settings().clone();
    settings.inner_layer_count = 5;
    settings.pcb_thickness = PositiveLength::new(Length::new(1_500_000)).unwrap();
    settings.solder_resist = Some(PcbColor::Black);
    settings.silkscreen_color = PcbColor::Blue;
    settings.silkscreen_layers_top = vec![Layer::TOP_LEGEND];
    settings.silkscreen_layers_bot = vec![];
    board.set_settings(settings);
    board
}

fn create_project() -> Project {
    let mut first = true;
    let mut p = Project::create(
        TransactionalDirectory::new_temporary().unwrap(),
        "project.lpp",
        || {
            if std::mem::take(&mut first) {
                uuid("7b3985b2-91ad-4e93-8d15-7668869ed45d")
            } else {
                Uuid::new_random()
            }
        },
    )
    .unwrap();
    let mut metadata = p.metadata().clone();
    metadata.name = ElementName::new("New Project").unwrap();
    metadata.author = "New Author".to_owned();
    metadata.version = FileProofName::new("New-Version.1").unwrap();
    metadata.created = Utc.with_ymd_and_hms(2000, 1, 2, 1, 2, 3).unwrap();
    p.apply(Mutation::SetProjectMetadata(metadata)).unwrap();
    p.apply(Mutation::AddAssemblyVariant {
        variant: create_assembly_variant(),
        index: None,
    })
    .unwrap();
    while p.circuit().assembly_variants().len() > 1 {
        let first = p
            .circuit()
            .assembly_variants()
            .iter()
            .next()
            .unwrap()
            .uuid();
        p.apply(Mutation::RemoveAssemblyVariant(AssemblyVariantId(first)))
            .unwrap();
    }
    p.add_board(create_board(), None).unwrap();
    p
}

fn pt(x: i64, y: i64) -> Point {
    Point::new(Length::new(x), Length::new(y))
}

#[test]
fn test_string_list() {
    let none: [&str; 0] = [];
    assert_eq!(
        fmt("[]"),
        fmt_value(&json_export::string_list_to_json(&none))
    );
    assert_eq!(
        fmt("[\"foo\"]"),
        fmt_value(&json_export::string_list_to_json(&["foo"]))
    );
    assert_eq!(
        fmt("[\"foo\", \"bar\"]"),
        fmt_value(&json_export::string_list_to_json(&["foo", "bar"]))
    );
}

#[test]
fn test_length() {
    assert_eq!(
        serde_json::json!(-5.5),
        json_export::length_to_json(Length::new(-5_500_000))
    );
}

#[test]
fn test_optional_length() {
    assert!(json_export::optional_length_to_json(None).is_null());
    assert_eq!(
        serde_json::json!(-5.5),
        json_export::optional_length_to_json(Some(Length::new(-5_500_000)))
    );
}

#[test]
fn test_length_set() {
    assert_eq!(
        fmt("[]"),
        fmt_value(&json_export::length_set_to_json(&BTreeSet::new()))
    );
    assert_eq!(
        fmt("[0.1]"),
        fmt_value(&json_export::length_set_to_json(&BTreeSet::from([
            Length::new(100_000)
        ])))
    );
    assert_eq!(
        fmt("[-0.1, 0.1]"),
        fmt_value(&json_export::length_set_to_json(&BTreeSet::from([
            Length::new(100_000),
            Length::new(-100_000)
        ])))
    );
}

#[test]
fn test_pcb_color() {
    assert_eq!(
        serde_json::json!("none"),
        json_export::pcb_color_to_json(None)
    );
    assert_eq!(
        serde_json::json!("black"),
        json_export::pcb_color_to_json(Some(PcbColor::Black))
    );
}

#[test]
fn test_assembly_variant() {
    let expected = r#"{
        "uuid": "bb0d66f1-2f21-4592-b923-d853867a6124",
        "name": "AV0",
        "description": "Hello World!"
    }"#;
    assert_eq!(
        fmt(expected),
        fmt_value(&json_export::assembly_variant_to_json(
            &create_assembly_variant()
        ))
    );
}

#[test]
fn test_bounding_box() {
    let make_box = |x0: i64, y0: i64, x1: i64, y1: i64| BoundingBox(Some((pt(x0, y0), pt(x1, y1))));
    assert!(json_export::bounding_box_to_json(&BoundingBox(None)).is_null());
    let expected = r#"{"x": 0, "y": 0, "width": 0, "height": 0}"#;
    assert_eq!(
        fmt(expected),
        fmt_value(&json_export::bounding_box_to_json(&make_box(0, 0, 0, 0)))
    );
    let expected = r#"{"x": -1.1, "y": 2.2, "width": 5.5, "height": 6.6}"#;
    for bbox in [
        make_box(-1_100_000, 2_200_000, 4_400_000, 8_800_000),
        make_box(4_400_000, 8_800_000, -1_100_000, 2_200_000),
        make_box(-1_100_000, 8_800_000, 4_400_000, 2_200_000),
    ] {
        assert_eq!(
            fmt(expected),
            fmt_value(&json_export::bounding_box_to_json(&bbox))
        );
    }
}

#[test]
fn test_board() {
    let mut project = create_project();
    let board = project.boards()[0].id();
    let polygon = |layer: Layer, points: [(i64, i64); 4]| {
        BoardItem::Polygon(BoardPolygonData::new(
            Uuid::new_random(),
            layer,
            UnsignedLength::ZERO,
            Path::new(
                points
                    .iter()
                    .map(|(x, y)| Vertex::new(pt(*x, *y), Default::default()))
                    .collect(),
            ),
            false,
            false,
            false,
        ))
    };
    project
        .add_board_item(
            board,
            polygon(
                Layer::BOARD_OUTLINES,
                [
                    (5_000_000, 6_000_000),
                    (5_000_000, 10_000_000),
                    (7_000_000, 6_000_000),
                    (5_000_000, 6_000_000),
                ],
            ),
        )
        .unwrap();
    project
        .add_board_item(
            board,
            polygon(
                Layer::BOARD_PLATED_CUTOUTS,
                [
                    (5_500_000, 5_500_000),
                    (5_500_000, 95_000_000),
                    (6_500_000, 5_500_000),
                    (5_500_000, 5_500_000),
                ],
            ),
        )
        .unwrap();

    let expected = r#"{
        "uuid": "1ff89be5-dd83-4b08-8d95-d09e0fd72b25",
        "name": "New Board",
        "directory": "board",
        "inner_layers": 5,
        "pcb_thickness": 1.5,
        "solder_resist": "black",
        "silkscreen_top": "blue",
        "silkscreen_bottom": "none",
        "bounding_box": {"x": 5.0, "y": 6.0, "width": 2.0, "height": 4.0},
        "vias_tht": {"count": 0, "diameters": []},
        "vias_blind": {"count": 0, "diameters": []},
        "vias_buried": {"count": 0, "diameters": []},
        "pth_drills": {"count": 0, "diameters": []},
        "pth_slots": {"count": 0, "diameters": []},
        "npth_drills": {"count": 0, "diameters": []},
        "npth_slots": {"count": 0, "diameters": []},
        "plated_cutouts": 1,
        "min_copper_width": null
    }"#;
    assert_eq!(
        fmt(expected),
        fmt_value(&json_export::board_to_json(&project, &project.boards()[0]))
    );
}

const PROJECT_JSON: &str = r#"{
    "filename": "project.lpp",
    "uuid": "7b3985b2-91ad-4e93-8d15-7668869ed45d",
    "name": "New Project",
    "author": "New Author",
    "version": "New-Version.1",
    "created": "2000-01-02T01:02:03Z",
    "locales": [],
    "norms": [],
    "variants": [
        {
            "uuid": "bb0d66f1-2f21-4592-b923-d853867a6124",
            "name": "AV0",
            "description": "Hello World!"
        }
    ],
    "boards": [
        {
            "uuid": "1ff89be5-dd83-4b08-8d95-d09e0fd72b25",
            "name": "New Board",
            "directory": "board",
            "inner_layers": 5,
            "pcb_thickness": 1.5,
            "solder_resist": "black",
            "silkscreen_top": "blue",
            "silkscreen_bottom": "none",
            "bounding_box": null,
            "vias_tht": {"count": 0, "diameters": []},
            "vias_blind": {"count": 0, "diameters": []},
            "vias_buried": {"count": 0, "diameters": []},
            "pth_drills": {"count": 0, "diameters": []},
            "pth_slots": {"count": 0, "diameters": []},
            "npth_drills": {"count": 0, "diameters": []},
            "npth_slots": {"count": 0, "diameters": []},
            "plated_cutouts": 0,
            "min_copper_width": null
        }
    ]
}"#;

#[test]
fn test_project() {
    let project = create_project();
    assert_eq!(
        fmt(PROJECT_JSON),
        fmt_value(&json_export::project_to_json(&project))
    );
}

#[test]
fn test_project_to_utf8() {
    let project = create_project();
    let expected = format!(
        r#"{{
            "format": {{
                "major": 1,
                "minor": 0,
                "type": "librepcb-project"
            }},
            "project": {PROJECT_JSON}
        }}"#
    );
    let actual = json_export::to_utf8(&project);
    assert_eq!(fmt(&expected), fmt(std::str::from_utf8(&actual).unwrap()));
    // Upstream writes exactly the `QJsonDocument` formatting.
    assert_eq!(fmt(&expected).as_bytes(), actual.as_slice());
}
