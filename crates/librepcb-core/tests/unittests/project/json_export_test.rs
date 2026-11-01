//! Tests of upstream's project JSON schema and manufacturing statistics.

use super::*;
use librepcb_core::geometry::{
    Circle, Junction, NonEmptyPath, PadHole, Path, Polygon, Trace, TraceAnchor, Vertex, Via,
};
use librepcb_core::project::board::{
    BoardHoleData, BoardItem, BoardNetSegment, BoardPadData, BoardPlane,
};
use librepcb_core::project::{BoardMutation, ProjectJsonExport as Export};
use librepcb_core::types::{Angle, Layer, Length, MaskConfig, Point, UnsignedLength};
use serde_json::json;

#[test]
fn empty_project_document_has_upstream_layout_and_schema() {
    let p = new_project();
    let bytes = Export::to_utf8(&p).unwrap();
    let variant = p.circuit().assembly_variants().first().unwrap();
    let expected = format!(
        r#"{{
    "format": {{
        "major": 1,
        "minor": 0,
        "type": "librepcb-project"
    }},
    "project": {{
        "author": "",
        "boards": [
        ],
        "created": "{}",
        "filename": "test.lpp",
        "locales": [
        ],
        "name": "Unnamed",
        "norms": [
        ],
        "uuid": "{}",
        "variants": [
            {{
                "description": "Standard assembly",
                "name": "Std",
                "uuid": "{}"
            }}
        ],
        "version": "v1"
    }}
}}
"#,
        p.metadata().created.format("%Y-%m-%dT%H:%M:%SZ"),
        p.uuid(),
        variant.uuid()
    );
    assert_eq!(std::str::from_utf8(&bytes).unwrap(), expected);
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed["project"], Export::project(&p).unwrap());
}

#[test]
fn board_defaults_and_footprint_holes() {
    let f = board::Fixture::with_devices();
    let value = Export::board(f.board(), f.p.view()).unwrap();
    assert_eq!(
        value,
        json!({
            "uuid": f.board.0.to_string(), "name": "default", "directory": "default",
            "inner_layers": 0, "pcb_thickness": 1.6, "solder_resist": "green",
            "silkscreen_top": "white", "silkscreen_bottom": "white", "bounding_box": null,
            "vias_tht": {"count": 0, "diameters": []}, "vias_blind": {"count": 0, "diameters": []},
            "vias_buried": {"count": 0, "diameters": []}, "pth_drills": {"count": 2, "diameters": [0.5]},
            "pth_slots": {"count": 0, "diameters": []}, "npth_drills": {"count": 0, "diameters": []},
            "npth_slots": {"count": 0, "diameters": []}, "plated_cutouts": 0, "min_copper_width": null,
        })
    );
}

fn set_settings(
    f: &mut board::Fixture,
    modify: impl FnOnce(&mut librepcb_core::project::board::BoardSettings),
) {
    let mut settings = f.board().settings().clone();
    modify(&mut settings);
    f.p.apply(Mutation::Board(BoardMutation::SetSettings {
        board: f.board,
        settings: Box::new(settings),
    }))
    .unwrap();
}

#[test]
fn via_types_layer_clipping_and_automatic_drill_defaults() {
    let mut f = board::Fixture::new();
    set_settings(&mut f, |s| {
        s.inner_layer_count = 3;
        s.solder_resist = None;
        s.silkscreen_layers_bot.clear();
    });
    let mut class =
        f.p.circuit()
            .net_classes()
            .get(&default_net_class(&f.p))
            .unwrap()
            .clone();
    class.set_default_via_drill(Some(board::pos_mm(0.42)));
    f.p.apply(Mutation::UpdateNetClass(class)).unwrap();
    let inner = |n| Layer::inner_copper(n).unwrap();
    for (start, end, drill, net) in [
        (Layer::TOP_COPPER, Layer::BOT_COPPER, None, Some(f.gnd)),
        (Layer::TOP_COPPER, inner(4), Some(board::pos_mm(0.2)), None), // clipped to layer 3
        (inner(1), inner(3), Some(board::pos_mm(0.15)), None),
        (inner(4), Layer::BOT_COPPER, Some(board::pos_mm(0.1)), None), // disabled start
        (inner(3), inner(4), Some(board::pos_mm(0.1)), None),          // empty clipped span
        (Layer::TOP_COPPER, Layer::BOT_COPPER, None, None),            // board default
    ] {
        let mut segment = BoardNetSegment::new(Uuid::new_random(), net);
        segment.insert_via(
            Via::new(
                Uuid::new_random(),
                start,
                end,
                Point::ORIGIN,
                drill,
                None,
                MaskConfig::Automatic,
            )
            .unwrap(),
        );
        f.p.add_board_net_segment(f.board, segment).unwrap();
    }
    let value = Export::board(f.board(), f.p.view()).unwrap();
    assert_eq!(
        value["vias_tht"],
        json!({"count": 2, "diameters": [0.3, 0.42]})
    );
    assert_eq!(value["vias_blind"], json!({"count": 1, "diameters": [0.2]}));
    assert_eq!(
        value["vias_buried"],
        json!({"count": 1, "diameters": [0.15]})
    );
    assert_eq!(value["solder_resist"], "none");
    assert_eq!(value["silkscreen_bottom"], "none");
}

#[test]
fn drills_slots_copper_widths_and_plated_cutouts() {
    let mut f = board::Fixture::new();
    let slot = NonEmptyPath::new(Path::new(vec![
        Vertex::new(Point::ORIGIN, Angle::DEG0),
        Vertex::new(board::pt(1.0, 0.0), Angle::DEG0),
    ]))
    .unwrap();
    let mut pad = board::pad(
        Uuid::new_random(),
        Point::ORIGIN,
        librepcb_core::geometry::ComponentSide::Top,
        false,
    );
    pad.set_holes(
        vec![PadHole::new(
            Uuid::new_random(),
            board::pos_mm(0.6),
            slot.clone(),
        )]
        .into(),
    );
    let mut segment = BoardNetSegment::new(Uuid::new_random(), None);
    segment.insert_pad(BoardPadData::new(pad, false));
    f.p.add_board_net_segment(f.board, segment).unwrap();
    for path in [NonEmptyPath::from_point(Point::ORIGIN), slot] {
        f.p.add_board_item(
            f.board,
            BoardItem::Hole(BoardHoleData::new(
                Uuid::new_random(),
                board::pos_mm(0.8),
                path,
                MaskConfig::Off,
                false,
            )),
        )
        .unwrap();
    }
    let mut plane = BoardPlane::new(Uuid::new_random(), Layer::TOP_COPPER, None, Path::default());
    plane.set_min_width(UnsignedLength::new(Length::new(100_000)).unwrap());
    f.p.add_board_plane(f.board, plane).unwrap();
    let mut segment = BoardNetSegment::new(Uuid::new_random(), None);
    let (a, b) = (Uuid::new_random(), Uuid::new_random());
    segment.insert_junction(Junction::new(a, Point::ORIGIN));
    segment.insert_junction(Junction::new(b, board::pt(1.0, 0.0)));
    segment.insert_trace(Trace::new(
        Uuid::new_random(),
        Layer::TOP_COPPER,
        board::pos_mm(0.2),
        TraceAnchor::Junction(a),
        TraceAnchor::Junction(b),
    ));
    f.p.add_board_net_segment(f.board, segment).unwrap();
    let polygon = librepcb_core::project::board::BoardPolygonData::new(
        Uuid::new_random(),
        Layer::BOARD_PLATED_CUTOUTS,
        UnsignedLength::ZERO,
        Path::default(),
        false,
        false,
        false,
    );
    f.p.add_board_item(f.board, BoardItem::Polygon(polygon))
        .unwrap();
    let value = Export::board(f.board(), f.p.view()).unwrap();
    assert_eq!(value["pth_slots"], json!({"count": 1, "diameters": [0.6]}));
    assert_eq!(
        value["npth_drills"],
        json!({"count": 1, "diameters": [0.8]})
    );
    assert_eq!(value["npth_slots"], json!({"count": 1, "diameters": [0.8]}));
    assert_eq!(value["plated_cutouts"], 1);
    assert_eq!(value["min_copper_width"], 0.1);
}

#[test]
fn board_bounds_include_transformed_footprint_outlines_and_circles() {
    let mut f = board::Fixture::new();
    let mut package = f.p.remove_library_package(&f.lib_package).unwrap();
    let footprint = package.footprints_mut().by_uuid_mut(&f.footprint).unwrap();
    footprint.circles_mut().push(Circle::new(
        Uuid::new_random(),
        Layer::BOARD_OUTLINES,
        UnsignedLength::ZERO,
        false,
        false,
        Point::ORIGIN,
        board::pos_mm(2.0),
    ));
    footprint.polygons_mut().push(Polygon::new(
        Uuid::new_random(),
        Layer::BOARD_OUTLINES,
        UnsignedLength::ZERO,
        false,
        false,
        Path::new(vec![
            Vertex::new(board::pt(-2.0, 0.0), Angle::DEG0),
            Vertex::new(board::pt(2.0, 0.0), Angle::DEG0),
        ]),
    ));
    f.p.add_library_package(package).unwrap();
    let mut device = f.device(f.r1, board::pt(10.0, 20.0));
    device.set_rotation(Angle::DEG90);
    device.set_mirrored(true);
    f.p.add_board_device(f.board, device).unwrap();
    assert_eq!(
        Export::board(f.board(), f.p.view()).unwrap()["bounding_box"],
        json!({"x": 9, "y": 18, "width": 2, "height": 4})
    );
}
