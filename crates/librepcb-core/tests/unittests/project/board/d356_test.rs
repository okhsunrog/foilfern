//! Board-specific IPC-D-356A transforms and standalone pad numbering.

use super::*;
use librepcb_core::export::Timestamp;
use librepcb_core::project::board::BoardD356NetlistExport;

#[test]
fn mirrored_footprint_pads_use_physical_positions_and_bottom_access() {
    let mut f = Fixture::new();
    let mut device = f.device(f.r1, pt(10.0, 20.0));
    device.set_rotation(Angle::DEG90);
    device.set_mirrored(true);
    f.p.add_board_device(f.board, device).unwrap();
    let content = BoardD356NetlistExport::new(&f.p, f.board(), "test", Timestamp::now())
        .generate()
        .unwrap();
    let pad = content
        .lines()
        .find(|line| line.contains("R1    -1 "))
        .unwrap();
    assert!(pad.starts_with("327GND{"), "{pad}");
    assert!(
        pad.contains("A02X+010000Y+021000X1000Y1000R270 S1"),
        "{pad}"
    );
    let tht = content
        .lines()
        .find(|line| line.contains("R1    -3 "))
        .unwrap();
    assert!(tht.starts_with("317N/C"), "{tht}");
    assert!(tht.contains("D0500PA00"), "{tht}");
}

#[test]
fn standalone_pads_are_numbered_in_segment_order_and_use_only_first_hole() {
    let mut f = Fixture::new();
    let mut tht = pad(Uuid::new_random(), Point::ORIGIN, ComponentSide::Top, true);
    let mut holes = tht.holes().clone();
    holes.push(PadHole::new(
        Uuid::new_random(),
        pos_mm(0.8),
        NonEmptyPath::from_point(Point::ORIGIN),
    ));
    tht.set_holes(holes);
    let mut smt = pad(
        Uuid::new_random(),
        pt(1.0, 2.0),
        ComponentSide::Bottom,
        false,
    );
    smt.set_rotation(Angle::DEG90);
    for (index, pad) in [tht, smt].into_iter().enumerate() {
        let uuid = format!("00000000-0000-4000-8000-{:012}", index + 1)
            .parse()
            .unwrap();
        let mut segment = BoardNetSegment::new(uuid, None);
        segment.insert_pad(BoardPadData::new(pad, false));
        f.p.add_board_net_segment(f.board, segment).unwrap();
    }
    let content = BoardD356NetlistExport::new(&f.p, f.board(), "test", Timestamp::now())
        .generate()
        .unwrap();
    let records: Vec<_> = content
        .lines()
        .filter(|line| line.starts_with("317") || line.starts_with("327"))
        .collect();
    assert_eq!(records.len(), 2);
    assert!(
        records[0].contains("BOARD -P1   D0500PA00"),
        "{}",
        records[0]
    );
    assert!(
        records[1].contains("BOARD -P2         A02X+001000Y+002000X1000Y1000R090 S1"),
        "{}",
        records[1]
    );
}

#[test]
fn via_records_preserve_logical_layers_even_when_drill_span_is_disabled() {
    use librepcb_core::geometry::Via;
    use librepcb_core::project::board::ViaView;
    let mut f = Fixture::new(); // No enabled inner layers.
    let via = Via::new(
        Uuid::new_random(),
        Layer::inner_copper(1).unwrap(),
        Layer::inner_copper(4).unwrap(),
        Point::ORIGIN,
        Some(pos_mm(0.25)),
        None,
        MaskConfig::Off,
    )
    .unwrap();
    assert_eq!(
        ViaView::new(&via, f.board(), None, f.p.circuit())
            .unwrap()
            .drill_layer_span(),
        None
    );
    let mut segment = BoardNetSegment::new(Uuid::new_random(), None);
    segment.insert_via(via);
    f.p.add_board_net_segment(f.board, segment).unwrap();
    let content = BoardD356NetlistExport::new(&f.p, f.board(), "test", Timestamp::now())
        .generate()
        .unwrap();
    let record = content
        .lines()
        .find(|line| line.starts_with("307"))
        .unwrap();
    assert!(record.contains("D0250P"), "{record}");
    assert!(record.ends_with("S3L02L05"), "{record}");
}
