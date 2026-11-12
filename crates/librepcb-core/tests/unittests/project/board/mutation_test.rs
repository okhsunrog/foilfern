//! Board mutations: inverses, validation errors (model unchanged), reverse
//! index, change journal, dirty sets of the derived data.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::geometry::{Junction, TraceAnchor, Via, ZoneRules};
use librepcb_core::geometry::{NonEmptyPath, Path};
use librepcb_core::project::board::{
    BoardChange, BoardHoleData, BoardItem, BoardItemKind, BoardNetSegment, BoardPadData,
    BoardPlane, BoardPolygonData, BoardSegmentElements, BoardSegmentItems, BoardStrokeTextData,
    BoardZoneData,
};
use librepcb_core::project::{
    BoardMutation, BoardNetSegmentRef, Change, ChangesSince, ComponentSignalRef, DeviceRef,
    EntityKind, Error, Mutation, NetUse, PlaneRef,
};
use librepcb_core::serialization::SExpression;
use librepcb_core::types::{
    Alignment, Angle, HAlign, Layer, MaskConfig, StrokeTextSpacing, UnsignedLength, Uuid, VAlign,
};

use super::super::{assert_fails, assert_inverse};
use super::*;

fn board_mutation(m: BoardMutation) -> Mutation {
    Mutation::Board(m)
}

fn changes(p: &Project, since: u64) -> Vec<Change> {
    match p.changes_since(since) {
        ChangesSince::Changes(c) => c.to_vec(),
        ChangesSince::Resync => panic!("resync"),
    }
}

#[test]
fn devices() {
    let mut f = Fixture::new();
    let d1 = f.device(f.r1, pt(0.0, 0.0));
    let board = f.board;
    assert_inverse(
        &mut f.p,
        board_mutation(BoardMutation::AddDevice {
            board,
            device: d1.clone(),
        }),
    );
    let revision = f.p.revision();
    f.p.add_board_device(board, d1.clone()).unwrap();
    assert_eq!(
        changes(&f.p, revision),
        vec![Change::Board {
            id: board,
            change: BoardChange::DeviceAdded(f.r1)
        }]
    );
    assert!(f.p.is_component_used(f.r1));
    assert_eq!(
        f.p.component_uses(f.r1).unwrap().devices,
        BTreeSet::from([board])
    );
    // The component can't be removed while it has a device.
    assert!(matches!(
        assert_fails(&mut f.p, Mutation::RemoveComponentInstance(f.r1)),
        Error::ComponentInUse(_)
    ));

    // Footprint pad views.
    let pads = f
        .board()
        .device(f.r1)
        .unwrap()
        .pads(f.p.library(), f.p.circuit())
        .unwrap();
    assert_eq!(pads.len(), 3);
    assert_eq!(pads[0].position(), pt(-1.0, 0.0));
    assert_eq!(pads[0].net(), Some(f.gnd));
    assert_eq!(pads[1].net(), Some(f.vcc));
    assert_eq!(pads[2].net(), None);
    assert_eq!(pads[0].text(), "1:S1\nGND");
    assert_eq!(pads[2].text(), "3");
    assert_eq!(pads[0].solder_layer(), Layer::TOP_COPPER);
    assert!(pads[2].is_on_layer(Layer::BOT_COPPER));

    // Update (move, mirror, attributes).
    let mut moved = d1.clone();
    moved.set_position(pt(3.0, 4.0));
    moved.set_rotation(Angle::DEG90);
    moved.set_mirrored(true);
    assert_inverse(
        &mut f.p,
        board_mutation(BoardMutation::UpdateDevice {
            board,
            device: moved.clone(),
        }),
    );
    f.p.apply(board_mutation(BoardMutation::UpdateDevice {
        board,
        device: moved,
    }))
    .unwrap();
    let pads = f
        .board()
        .device(f.r1)
        .unwrap()
        .pads(f.p.library(), f.p.circuit())
        .unwrap();
    // Mirrored, rotated by 90° and translated: (-1, 0) -> (1, 0) -> (0, 1).
    assert_eq!(pads[0].position(), pt(3.0, 5.0));
    assert_eq!(pads[0].solder_layer(), Layer::BOT_COPPER);

    assert_inverse(
        &mut f.p,
        board_mutation(BoardMutation::RemoveDevice(DeviceRef {
            board,
            component: f.r1,
        })),
    );

    // Errors.
    assert!(matches!(
        assert_fails(
            &mut f.p,
            board_mutation(BoardMutation::AddDevice {
                board,
                device: d1.clone()
            })
        ),
        Error::DuplicateDevice(_)
    ));
    let unknown = f.device(
        librepcb_core::project::ComponentInstanceId::new_random(),
        pt(0.0, 0.0),
    );
    assert!(matches!(
        assert_fails(
            &mut f.p,
            board_mutation(BoardMutation::AddDevice {
                board,
                device: unknown
            })
        ),
        Error::InexistentDeviceComponent(_)
    ));
    let mut bad = f.device(f.r2, pt(0.0, 0.0));
    bad.set_lib_device(Uuid::new_random());
    let error = assert_fails(
        &mut f.p,
        board_mutation(BoardMutation::AddDevice { board, device: bad }),
    );
    assert!(matches!(error, Error::MissingLibraryDevice(_)));
    assert!(error.to_string().contains("found in the project's library"));
    let mut bad = f.device(f.r2, pt(0.0, 0.0));
    bad.set_lib_footprint(Uuid::new_random());
    assert!(matches!(
        assert_fails(
            &mut f.p,
            board_mutation(BoardMutation::AddDevice { board, device: bad })
        ),
        Error::Serialization(_)
    ));
    let mut bad = f.device(f.r2, pt(0.0, 0.0));
    bad.set_lib_model(Some(Uuid::new_random()));
    assert!(matches!(
        assert_fails(
            &mut f.p,
            board_mutation(BoardMutation::AddDevice { board, device: bad })
        ),
        Error::Serialization(_)
    ));
    assert!(matches!(
        assert_fails(
            &mut f.p,
            board_mutation(BoardMutation::RemoveDevice(DeviceRef {
                board,
                component: f.r2
            }))
        ),
        Error::NotFound {
            kind: EntityKind::Device,
            ..
        }
    ));
    let d2 = f.device(f.r2, pt(0.0, 0.0));
    assert!(matches!(
        assert_fails(
            &mut f.p,
            board_mutation(BoardMutation::UpdateDevice { board, device: d2 })
        ),
        Error::NotFound {
            kind: EntityKind::Device,
            ..
        }
    ));
}

#[test]
fn device_in_use() {
    let mut f = Fixture::with_devices();
    let (segment, _) = gnd_segment(&f);
    f.p.add_board_net_segment(f.board, segment).unwrap();
    let board = f.board;
    let error = assert_fails(
        &mut f.p,
        board_mutation(BoardMutation::RemoveDevice(DeviceRef {
            board,
            component: f.r1,
        })),
    );
    assert_eq!(
        error.to_string(),
        format!(
            "The device \"{}\" cannot be modified because it is still in use!",
            f.r1
        )
    );
    let mut mirrored = f.board().device(f.r1).unwrap().clone();
    mirrored.set_mirrored(true);
    assert!(matches!(
        assert_fails(
            &mut f.p,
            board_mutation(BoardMutation::UpdateDevice {
                board,
                device: mirrored
            })
        ),
        Error::ItemInUse {
            kind: EntityKind::Device,
            ..
        }
    ));
    // Moving is fine.
    let mut moved = f.board().device(f.r1).unwrap().clone();
    moved.set_position(pt(1.0, 1.0));
    assert_inverse(
        &mut f.p,
        board_mutation(BoardMutation::UpdateDevice {
            board,
            device: moved,
        }),
    );
    // The net of a wired component signal can't change.
    let signal = gnd_signal(&f, f.r1);
    assert!(matches!(
        assert_fails(
            &mut f.p,
            Mutation::SetComponentSignalNet {
                signal: ComponentSignalRef {
                    component: f.r1,
                    signal,
                },
                net: None,
            }
        ),
        Error::ComponentSignalInUse { .. }
    ));
    // The net can't be removed while used by the segment.
    assert!(matches!(
        assert_fails(&mut f.p, Mutation::RemoveNetSignal(f.gnd)),
        Error::NetSignalInUse(_)
    ));
}

#[test]
fn net_segments() {
    let mut f = Fixture::with_devices();
    let board = f.board;
    let (segment, junction) = gnd_segment(&f);
    let seg_ref = BoardNetSegmentRef {
        board,
        segment: segment.id(),
    };
    assert_inverse(
        &mut f.p,
        board_mutation(BoardMutation::AddNetSegment {
            board,
            segment: segment.clone(),
        }),
    );
    f.p.add_board_net_segment(board, segment.clone()).unwrap();
    assert!(
        f.p.net_signal_uses(f.gnd)
            .any(|u| *u == NetUse::BoardSegment(board, segment.id()))
    );
    assert_eq!(
        f.board().footprint_pad_segment(f.r1, f.fpt_pads[0]),
        Some(segment.id())
    );
    assert_eq!(
        f.board().anchor_trace_layers(f.pad_anchor(f.r2, 0)),
        BTreeSet::from([Layer::TOP_COPPER])
    );
    assert_eq!(
        f.board().anchor_position(
            &segment,
            f.pad_anchor(f.r2, 0),
            f.p.library(),
            f.p.circuit()
        ),
        Some(pt(9.0, 0.0))
    );

    // Add elements: a via connected to the junction.
    let via = Via::new(
        Uuid::new_random(),
        Layer::TOP_COPPER,
        Layer::BOT_COPPER,
        pt(5.0, 5.0),
        None,
        None,
        MaskConfig::Off,
    )
    .unwrap();
    let to_via = trace(
        TraceAnchor::Junction(junction.uuid()),
        TraceAnchor::Via(via.uuid()),
    );
    let elements = BoardSegmentElements {
        vias: vec![via.clone()],
        traces: vec![to_via.clone()],
        ..Default::default()
    };
    let inverse = assert_inverse(
        &mut f.p,
        board_mutation(BoardMutation::AddNetSegmentElements {
            segment: seg_ref,
            elements: elements.clone(),
        }),
    );
    assert_eq!(
        inverse,
        board_mutation(BoardMutation::RemoveNetSegmentElements {
            segment: seg_ref,
            elements: BoardSegmentItems {
                vias: vec![via.uuid()],
                traces: vec![to_via.uuid()],
                ..Default::default()
            }
        })
    );
    f.p.apply(board_mutation(BoardMutation::AddNetSegmentElements {
        segment: seg_ref,
        elements,
    }))
    .unwrap();

    // Update: move the via, widen a trace.
    let mut moved = via.clone();
    moved.set_position(pt(6.0, 6.0));
    let mut wide = to_via.clone();
    wide.set_width(pos_mm(1.0));
    assert_inverse(
        &mut f.p,
        board_mutation(BoardMutation::UpdateNetSegmentElements {
            segment: seg_ref,
            elements: BoardSegmentElements {
                vias: vec![moved],
                traces: vec![wide],
                ..Default::default()
            },
        }),
    );
    // Changing the via layers while a trace is connected on another layer.
    let mut buried = via.clone();
    let inner = Layer::inner_copper(1).unwrap();
    buried.set_layers(inner, Layer::BOT_COPPER).unwrap();
    assert!(matches!(
        assert_fails(
            &mut f.p,
            board_mutation(BoardMutation::UpdateNetSegmentElements {
                segment: seg_ref,
                elements: BoardSegmentElements {
                    vias: vec![buried],
                    ..Default::default()
                },
            })
        ),
        Error::ViaLayersInUse(_)
    ));

    // Removing the trace to the via keeps the segment cohesive only if the
    // via is removed too.
    let error = assert_fails(
        &mut f.p,
        board_mutation(BoardMutation::RemoveNetSegmentElements {
            segment: seg_ref,
            elements: BoardSegmentItems {
                traces: vec![to_via.uuid()],
                ..Default::default()
            },
        }),
    );
    assert!(matches!(
        error,
        Error::NetSegmentNotCohesive {
            after_removal: true,
            ..
        }
    ));
    assert_eq!(
        error.to_string(),
        format!(
            "The netsegment with the UUID \"{}\" is not cohesive!",
            segment.uuid()
        )
    );
    assert_inverse(
        &mut f.p,
        board_mutation(BoardMutation::RemoveNetSegmentElements {
            segment: seg_ref,
            elements: BoardSegmentItems {
                vias: vec![via.uuid()],
                traces: vec![to_via.uuid()],
                ..Default::default()
            },
        }),
    );
    // Removing a junction still used by traces.
    assert!(matches!(
        assert_fails(
            &mut f.p,
            board_mutation(BoardMutation::RemoveNetSegmentElements {
                segment: seg_ref,
                elements: BoardSegmentItems {
                    junctions: vec![junction.uuid()],
                    ..Default::default()
                },
            })
        ),
        Error::InexistentNetPoint(_)
    ));
    assert!(matches!(
        assert_fails(
            &mut f.p,
            board_mutation(BoardMutation::RemoveNetSegmentElements {
                segment: seg_ref,
                elements: BoardSegmentItems {
                    vias: vec![Uuid::new_random()],
                    ..Default::default()
                },
            })
        ),
        Error::NotFound {
            kind: EntityKind::Via,
            ..
        }
    ));
    assert!(matches!(
        assert_fails(
            &mut f.p,
            board_mutation(BoardMutation::AddNetSegmentElements {
                segment: seg_ref,
                elements: BoardSegmentElements {
                    junctions: vec![junction.clone()],
                    ..Default::default()
                },
            })
        ),
        Error::DuplicateUuid {
            kind: EntityKind::NetPoint,
            ..
        }
    ));

    // The net of a used segment can't change.
    assert!(matches!(
        assert_fails(
            &mut f.p,
            board_mutation(BoardMutation::SetNetSegmentNet {
                segment: seg_ref,
                net: Some(f.vcc)
            })
        ),
        Error::ItemInUse {
            kind: EntityKind::NetSegment,
            ..
        }
    ));
    assert!(matches!(
        assert_fails(
            &mut f.p,
            board_mutation(BoardMutation::AddNetSegment {
                board,
                segment: segment.clone()
            })
        ),
        Error::DuplicateUuid {
            kind: EntityKind::NetSegment,
            ..
        }
    ));
    assert_inverse(
        &mut f.p,
        board_mutation(BoardMutation::RemoveNetSegment(seg_ref)),
    );
}

#[test]
fn standalone_pad_segment() {
    let mut f = Fixture::with_devices();
    let board = f.board;
    let pad = BoardPadData::new(
        pad(Uuid::new_random(), pt(20.0, 0.0), ComponentSide::Top, false),
        false,
    );
    let segment = BoardNetSegment::with_elements(
        Uuid::new_random(),
        None,
        vec![pad.clone()],
        vec![],
        vec![],
        vec![],
    );
    let seg_ref = BoardNetSegmentRef {
        board,
        segment: segment.id(),
    };
    f.p.add_board_net_segment(board, segment).unwrap();
    // A segment with only pads may change its net.
    assert_inverse(
        &mut f.p,
        board_mutation(BoardMutation::SetNetSegmentNet {
            segment: seg_ref,
            net: Some(f.vcc),
        }),
    );
    assert!(matches!(
        assert_fails(
            &mut f.p,
            board_mutation(BoardMutation::SetNetSegmentNet {
                segment: seg_ref,
                net: Some(librepcb_core::project::NetSignalId::new_random())
            })
        ),
        Error::InexistentNetSignal(_)
    ));
    // A trace on the bottom layer can't connect to the SMT top pad.
    let junction = Junction::new(Uuid::new_random(), pt(20.0, 5.0));
    let bottom = trace_on(
        Layer::BOT_COPPER,
        TraceAnchor::Pad(pad.uuid()),
        TraceAnchor::Junction(junction.uuid()),
    );
    let error = assert_fails(
        &mut f.p,
        board_mutation(BoardMutation::AddNetSegmentElements {
            segment: seg_ref,
            elements: BoardSegmentElements {
                junctions: vec![junction.clone()],
                traces: vec![bottom],
                ..Default::default()
            },
        }),
    );
    assert!(matches!(error, Error::TraceLayerMismatch { .. }));
    // On top copper it works; the pad then can't move to the bottom side.
    let top = trace(
        TraceAnchor::Pad(pad.uuid()),
        TraceAnchor::Junction(junction.uuid()),
    );
    f.p.apply(board_mutation(BoardMutation::AddNetSegmentElements {
        segment: seg_ref,
        elements: BoardSegmentElements {
            junctions: vec![junction],
            traces: vec![top],
            ..Default::default()
        },
    }))
    .unwrap();
    let mut flipped = pad.clone();
    flipped.pad_mut().set_component_side(ComponentSide::Bottom);
    assert!(matches!(
        assert_fails(
            &mut f.p,
            board_mutation(BoardMutation::UpdateNetSegmentElements {
                segment: seg_ref,
                elements: BoardSegmentElements {
                    pads: vec![flipped],
                    ..Default::default()
                },
            })
        ),
        Error::TraceLayerMismatch { .. }
    ));
}

#[test]
fn segment_validation_errors() {
    let mut f = Fixture::with_devices();
    let board = f.board;
    let add =
        |segment: BoardNetSegment| board_mutation(BoardMutation::AddNetSegment { board, segment });
    let segment = |net, junctions: Vec<Junction>, traces| {
        BoardNetSegment::with_elements(
            Uuid::new_random(),
            net,
            Vec::<BoardPadData>::new(),
            vec![],
            junctions,
            traces,
        )
    };
    let j1 = Junction::new(Uuid::new_random(), pt(5.0, 0.0));
    let j2 = Junction::new(Uuid::new_random(), pt(5.0, 5.0));
    let a1 = TraceAnchor::Junction(j1.uuid());
    let a2 = TraceAnchor::Junction(j2.uuid());
    let (r1_s1, r1_s2, r1_tht) = (
        f.pad_anchor(f.r1, 0),
        f.pad_anchor(f.r1, 1),
        f.pad_anchor(f.r1, 2),
    );
    let r2_tht = f.pad_anchor(f.r2, 2);

    // Pad of another net (R1.S2 is VCC).
    let error = assert_fails(
        &mut f.p,
        add(segment(
            Some(f.gnd),
            vec![j1.clone()],
            vec![trace(r1_s2, a1)],
        )),
    );
    assert_eq!(
        error.to_string(),
        "Trace of net \"GND\" is not allowed to be connected to pad \"2\" of device \"R1\" \
         (DEV) since it is connected to the net \"VCC\"."
    );
    // Pad on another layer.
    let error = assert_fails(
        &mut f.p,
        add(segment(
            Some(f.gnd),
            vec![j1.clone()],
            vec![trace_on(Layer::BOT_COPPER, r1_s1, a1)],
        )),
    );
    assert_eq!(
        error.to_string(),
        "Trace on layer \"Bottom Copper\" cannot be connected to the pad \"1\" of device \
         \"R1\" (DEV) since it is on layer \"Top Copper\"."
    );
    // Non-copper layer.
    assert!(matches!(
        assert_fails(
            &mut f.p,
            add(segment(
                Some(f.gnd),
                vec![j1.clone(), j2.clone()],
                vec![trace_on(Layer::TOP_LEGEND, a1, a2)],
            ))
        ),
        Error::InvalidTraceLayer { .. }
    ));
    // Degenerate trace.
    assert!(matches!(
        assert_fails(
            &mut f.p,
            add(segment(Some(f.gnd), vec![j1.clone()], vec![trace(a1, a1)]))
        ),
        Error::DegenerateLine {
            kind: EntityKind::Trace,
            ..
        }
    ));
    assert_eq!(
        Error::DegenerateLine {
            kind: EntityKind::Trace,
            uuid: j1.uuid()
        }
        .to_string(),
        "BI_NetLine: both endpoints are the same."
    );
    // Missing anchors.
    let error = assert_fails(
        &mut f.p,
        add(segment(Some(f.gnd), vec![j1.clone()], vec![trace(a1, a2)])),
    );
    assert_eq!(
        error.to_string(),
        format!("Net point '{}' does not exist in schematic.", j2.uuid())
    );
    let missing_device = TraceAnchor::FootprintPad {
        device: Uuid::new_random(),
        pad: f.fpt_pads[0],
    };
    assert!(matches!(
        assert_fails(
            &mut f.p,
            add(segment(
                Some(f.gnd),
                vec![j1.clone()],
                vec![trace(a1, missing_device)]
            ))
        ),
        Error::InexistentTraceAnchor {
            device_missing: true,
            ..
        }
    ));
    let missing_pad = TraceAnchor::FootprintPad {
        device: f.r1.0,
        pad: Uuid::new_random(),
    };
    assert!(matches!(
        assert_fails(
            &mut f.p,
            add(segment(
                Some(f.gnd),
                vec![j1.clone()],
                vec![trace(a1, missing_pad)]
            ))
        ),
        Error::InexistentTraceAnchor {
            device_missing: false,
            ..
        }
    ));
    // Traces of a junction on different layers (through the THT pad).
    assert!(matches!(
        assert_fails(
            &mut f.p,
            add(segment(
                None,
                vec![j1.clone()],
                vec![trace(r1_tht, a1), trace_on(Layer::BOT_COPPER, a1, r2_tht),],
            ))
        ),
        Error::JunctionLayerMismatch(_)
    ));
    // Not cohesive.
    let error = assert_fails(
        &mut f.p,
        add(segment(
            Some(f.gnd),
            vec![j1.clone(), j2.clone()],
            vec![trace(r1_s1, a1)],
        )),
    );
    assert!(matches!(
        error,
        Error::NetSegmentNotCohesive {
            after_removal: false,
            ..
        }
    ));
    // Inexistent net.
    assert!(matches!(
        assert_fails(
            &mut f.p,
            add(segment(
                Some(librepcb_core::project::NetSignalId::new_random()),
                vec![],
                vec![]
            ))
        ),
        Error::InexistentNetSignal(_)
    ));
    // Traces of two segments on one pad.
    let (first, _) = gnd_segment(&f);
    f.p.add_board_net_segment(board, first).unwrap();
    let error = assert_fails(
        &mut f.p,
        add(segment(
            Some(f.gnd),
            vec![j2.clone()],
            vec![trace(r1_s1, a2)],
        )),
    );
    assert_eq!(
        error.to_string(),
        "There are traces from multiple net segments connected to the pad \"1\" of device \
         \"R1\" (DEV)."
    );
    // Blind via without the trace layer.
    let mut settings = f.board().settings().clone();
    settings.inner_layer_count = 2;
    f.p.apply(board_mutation(BoardMutation::SetSettings {
        board,
        settings: Box::new(settings),
    }))
    .unwrap();
    let blind = Via::new(
        Uuid::new_random(),
        Layer::TOP_COPPER,
        Layer::inner_copper(1).unwrap(),
        pt(5.0, 5.0),
        None,
        None,
        MaskConfig::Off,
    )
    .unwrap();
    let error = assert_fails(
        &mut f.p,
        add(BoardNetSegment::with_elements(
            Uuid::new_random(),
            None,
            Vec::<BoardPadData>::new(),
            vec![blind.clone()],
            vec![j2.clone()],
            vec![trace_on(
                Layer::BOT_COPPER,
                TraceAnchor::Via(blind.uuid()),
                a2,
            )],
        )),
    );
    assert!(matches!(error, Error::TraceViaLayerMismatch(_)));
    assert!(
        error
            .to_string()
            .starts_with("Failed to connect trace to via")
    );
}

#[test]
fn planes() {
    let mut f = Fixture::with_devices();
    let board = f.board;
    let plane = BoardPlane::new(
        Uuid::new_random(),
        Layer::TOP_COPPER,
        Some(f.gnd),
        Path::rect(pt(0.0, 0.0), pt(10.0, 10.0)),
    );
    let plane_ref = PlaneRef {
        board,
        plane: plane.id(),
    };
    assert_inverse(
        &mut f.p,
        board_mutation(BoardMutation::AddPlane {
            board,
            plane: plane.clone(),
        }),
    );
    f.p.add_board_plane(board, plane.clone()).unwrap();
    assert!(
        f.p.net_signal_uses(f.gnd)
            .any(|u| *u == NetUse::Plane(board, plane.id()))
    );
    let mut changed = plane.clone();
    changed.set_net(Some(f.vcc));
    changed.set_layer(Layer::BOT_COPPER);
    changed.set_priority(5);
    changed.set_visible(false);
    assert_inverse(
        &mut f.p,
        board_mutation(BoardMutation::UpdatePlane {
            board,
            plane: changed,
        }),
    );
    assert!(matches!(
        assert_fails(
            &mut f.p,
            board_mutation(BoardMutation::AddPlane {
                board,
                plane: plane.clone()
            })
        ),
        Error::DuplicateUuid {
            kind: EntityKind::Plane,
            ..
        }
    ));
    let mut bad = plane.with_uuid(Uuid::new_random());
    bad.set_net(Some(librepcb_core::project::NetSignalId::new_random()));
    assert!(matches!(
        assert_fails(
            &mut f.p,
            board_mutation(BoardMutation::AddPlane { board, plane: bad })
        ),
        Error::InexistentNetSignal(_)
    ));
    assert!(matches!(
        assert_fails(&mut f.p, Mutation::RemoveNetSignal(f.gnd)),
        Error::NetSignalInUse(_)
    ));
    assert_inverse(
        &mut f.p,
        board_mutation(BoardMutation::RemovePlane(plane_ref)),
    );
}

/// One zone, polygon, stroke text and hole.
pub fn sample_items() -> Vec<BoardItem> {
    let outline = Path::rect(pt(0.0, 0.0), pt(10.0, 10.0));
    vec![
        BoardItem::Zone(
            BoardZoneData::new(
                Uuid::new_random(),
                BTreeSet::from([Layer::TOP_COPPER]),
                ZoneRules::NO_COPPER | ZoneRules::NO_DEVICES,
                outline.clone(),
                false,
            )
            .unwrap(),
        ),
        BoardItem::Polygon(BoardPolygonData::new(
            Uuid::new_random(),
            Layer::BOARD_OUTLINES,
            UnsignedLength::ZERO,
            outline,
            false,
            false,
            true,
        )),
        BoardItem::StrokeText(BoardStrokeTextData::new(
            Uuid::new_random(),
            Layer::TOP_LEGEND,
            "Hello",
            pt(1.0, 2.0),
            Angle::DEG90,
            pos_mm(1.0),
            UnsignedLength::ZERO,
            StrokeTextSpacing::Auto,
            StrokeTextSpacing::Auto,
            Alignment::new(HAlign::Left, VAlign::Bottom),
            false,
            true,
            false,
        )),
        BoardItem::Hole(BoardHoleData::new(
            Uuid::new_random(),
            pos_mm(3.0),
            NonEmptyPath::from_point(pt(5.0, 5.0)),
            MaskConfig::Automatic,
            false,
        )),
    ]
}

#[test]
fn zones_polygons_texts_holes() {
    let mut f = Fixture::new();
    let board = f.board;
    for item in sample_items() {
        let (kind, uuid) = (item.kind(), item.uuid());
        assert_inverse(
            &mut f.p,
            board_mutation(BoardMutation::AddItem {
                board,
                item: item.clone(),
            }),
        );
        f.p.add_board_item(board, item.clone()).unwrap();
        assert_eq!(f.board().item(kind, &uuid), Some(item.clone()));
        let updated = match item.clone() {
            BoardItem::Zone(mut z) => {
                z.set_locked(true);
                BoardItem::Zone(z)
            }
            BoardItem::Polygon(mut p) => {
                p.set_line_width(UnsignedLength::new(pos_mm(0.2).get()).unwrap());
                BoardItem::Polygon(p)
            }
            BoardItem::StrokeText(mut t) => {
                t.set_text("World".to_owned());
                BoardItem::StrokeText(t)
            }
            BoardItem::Hole(mut h) => {
                h.set_diameter(pos_mm(1.0));
                BoardItem::Hole(h)
            }
        };
        assert_inverse(
            &mut f.p,
            board_mutation(BoardMutation::UpdateItem {
                board,
                item: updated,
            }),
        );
        assert!(matches!(
            assert_fails(
                &mut f.p,
                board_mutation(BoardMutation::AddItem {
                    board,
                    item: item.clone()
                })
            ),
            Error::DuplicateUuid { .. }
        ));
        assert_inverse(
            &mut f.p,
            board_mutation(BoardMutation::RemoveItem { board, kind, uuid }),
        );
        let error = assert_fails(
            &mut f.p,
            board_mutation(BoardMutation::RemoveItem {
                board,
                kind,
                uuid: Uuid::new_random(),
            }),
        );
        assert!(matches!(error, Error::NotFound { .. }));
        assert_eq!(
            error.to_string().split(' ').nth(3),
            Some(match kind {
                BoardItemKind::Zone => "zone",
                BoardItemKind::Polygon => "polygon",
                BoardItemKind::StrokeText => "stroke",
                _ => "hole",
            })
        );
    }
    assert!(matches!(
        assert_fails(
            &mut f.p,
            board_mutation(BoardMutation::RemoveItem {
                board,
                kind: BoardItemKind::Via,
                uuid: Uuid::new_random(),
            })
        ),
        Error::NotFound {
            kind: EntityKind::Via,
            ..
        }
    ));
    // A zone on a non-copper layer is rejected.
    assert!(
        BoardZoneData::new(
            Uuid::new_random(),
            BTreeSet::from([Layer::TOP_LEGEND]),
            ZoneRules::empty(),
            Path::default(),
            false
        )
        .is_err()
    );
}

#[test]
fn settings_and_approvals() {
    let mut f = Fixture::new();
    let board = f.board;
    let mut settings = f.board().settings().clone();
    settings.inner_layer_count = 4;
    settings.solder_resist = None;
    settings.design_rules.set_pad_inner_auto_annular_ring(false);
    assert_inverse(
        &mut f.p,
        board_mutation(BoardMutation::SetSettings {
            board,
            settings: Box::new(settings.clone()),
        }),
    );
    f.p.apply(board_mutation(BoardMutation::SetSettings {
        board,
        settings: Box::new(settings),
    }))
    .unwrap();
    assert_eq!(f.board().copper_layers().len(), 6);
    assert!(
        f.board()
            .derived()
            .dirty_plane_layers()
            .contains(&Layer::inner_copper(4).unwrap())
    );
    let taken =
        f.p.take_dirty_plane_layers(board, &BTreeSet::from([Layer::TOP_COPPER]))
            .unwrap();
    assert_eq!(taken, BTreeSet::from([Layer::TOP_COPPER]));
    assert!(
        !f.board()
            .derived()
            .dirty_plane_layers()
            .contains(&Layer::TOP_COPPER)
    );

    let approval = SExpression::parse(b"(approved foo (bar 1))", None, Default::default()).unwrap();
    assert_inverse(
        &mut f.p,
        board_mutation(BoardMutation::SetDrcApproval {
            board,
            approval: approval.clone(),
            approved: true,
        }),
    );
    assert_inverse(
        &mut f.p,
        board_mutation(BoardMutation::SetDrcApprovals {
            board,
            version: "1".parse().unwrap(),
            approvals: BTreeSet::from([approval]),
        }),
    );
    assert_inverse(
        &mut f.p,
        board_mutation(BoardMutation::SetLayersVisibility {
            board,
            visibility: BTreeMap::from([("top_cu".to_owned(), false)]),
        }),
    );
}

#[test]
fn batch_rollback() {
    let mut f = Fixture::with_devices();
    let board = f.board;
    let (segment, _) = gnd_segment(&f);
    let before = super::super::snapshot(&f.p);
    let error =
        f.p.apply(Mutation::Batch(vec![
            board_mutation(BoardMutation::AddNetSegment {
                board,
                segment: segment.clone(),
            }),
            board_mutation(BoardMutation::AddNetSegment { board, segment }),
        ]))
        .unwrap_err();
    assert!(matches!(error, Error::DuplicateUuid { .. }));
    assert_eq!(super::super::snapshot(&f.p), before);
    assert!(f.p.is_ref_index_consistent());
    assert!(
        f.board()
            .footprint_pad_segment(f.r1, f.fpt_pads[0])
            .is_none()
    );
}

#[test]
fn large_batch_rollback_preserves_published_journal_and_board_files() {
    use librepcb_core::project::{ChangesSince, DEFAULT_CHANGE_LOG_CAPACITY};

    let mut f = Fixture::with_devices();
    let path = format!("boards/{}/extra.txt", f.board().directory_name());
    let fs = f.p.directory().file_system().clone();
    fs.write(&path, b"unsaved board asset").unwrap();
    let before = fs.save_state();
    let revision = f.p.revision();
    let ChangesSince::Changes(changes) = f.p.changes_since(revision - 1) else {
        panic!("expected the last published event");
    };
    let published = changes.to_vec();
    let mut mutations =
        vec![Mutation::SetProjectSettings(f.p.settings().clone()); DEFAULT_CHANGE_LOG_CAPACITY + 1];
    mutations.extend([
        Mutation::RemoveBoard(f.board),
        Mutation::RemoveBoard(f.board),
    ]);
    assert!(f.p.apply(Mutation::Batch(mutations)).is_err());
    assert!(f.p.board(f.board).is_some());
    assert!(f.p.is_ref_index_consistent());
    assert_eq!(fs.save_state(), before);
    assert_eq!(f.p.revision(), revision);
    let ChangesSince::Changes(changes) = f.p.changes_since(revision - 1) else {
        panic!("failed batch evicted a published event");
    };
    assert_eq!(changes, published);
}
