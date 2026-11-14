//! `BoardNetSegmentSplitter` (upstream has no unit test; behavior of
//! `boardnetsegmentsplitter.cpp`).

use foilfern_core::geometry::Via;
use foilfern_core::project::board::BoardNetSegmentSplitter;

use super::*;

fn via(pos: Point) -> Via {
    Via::new(
        Uuid::new_random(),
        Layer::TOP_COPPER,
        Layer::BOT_COPPER,
        pos,
        None,
        None,
        MaskConfig::Off,
    )
    .unwrap()
}

#[test]
fn split_into_cohesive_segments() {
    let j1 = Junction::new(Uuid::new_random(), pt(0.0, 0.0));
    let j2 = Junction::new(Uuid::new_random(), pt(1.0, 0.0));
    let j3 = Junction::new(Uuid::new_random(), pt(5.0, 0.0));
    let j4 = Junction::new(Uuid::new_random(), pt(6.0, 0.0));
    let unused = Junction::new(Uuid::new_random(), pt(9.0, 9.0));
    let v1 = via(pt(2.0, 0.0));
    let v2 = via(pt(8.0, 8.0));
    let a = |j: &Junction| TraceAnchor::Junction(j.uuid());
    let t1 = trace(a(&j1), a(&j2));
    let t2 = trace(a(&j2), TraceAnchor::Via(v1.uuid()));
    let t3 = trace(a(&j3), a(&j4));

    let mut splitter = BoardNetSegmentSplitter::new();
    for j in [&j1, &j2, &j3, &j4, &unused] {
        splitter.add_junction(j.clone());
    }
    splitter.add_via(v1.clone(), false);
    splitter.add_via(v2.clone(), false);
    for t in [&t1, &t2, &t3] {
        splitter.add_trace(t, Uuid::new_random);
    }
    let segments = splitter.split();
    assert_eq!(segments.len(), 3);
    let first = &segments[0];
    let mut junctions: Vec<_> = first.junctions.iter().map(Junction::uuid).collect();
    junctions.sort();
    let mut expected = vec![j1.uuid(), j2.uuid()];
    expected.sort();
    assert_eq!(junctions, expected);
    assert_eq!(first.vias, vec![v1]);
    assert_eq!(first.traces.len(), 2);
    assert_eq!(segments[1].traces, vec![t3]);
    assert_eq!(segments[1].junctions.len(), 2);
    assert_eq!(segments[2].vias, vec![v2]);
    assert!(segments[2].traces.is_empty());
}

#[test]
fn replace_via_by_junctions() {
    let j1 = Junction::new(Uuid::new_random(), pt(0.0, 0.0));
    let j2 = Junction::new(Uuid::new_random(), pt(4.0, 0.0));
    let v = via(pt(2.0, 0.0));
    let top = trace(TraceAnchor::Junction(j1.uuid()), TraceAnchor::Via(v.uuid()));
    let bot = trace_on(
        Layer::BOT_COPPER,
        TraceAnchor::Via(v.uuid()),
        TraceAnchor::Junction(j2.uuid()),
    );
    let mut splitter = BoardNetSegmentSplitter::new();
    splitter.add_junction(j1);
    splitter.add_junction(j2);
    splitter.add_via(v.clone(), true);
    splitter.add_trace(&top, Uuid::new_random);
    splitter.add_trace(&bot, Uuid::new_random);
    // The via is replaced by one junction per layer, so the segment falls
    // apart into two.
    let segments = splitter.split();
    assert_eq!(segments.len(), 2);
    for segment in &segments {
        assert!(segment.vias.is_empty());
        assert_eq!(segment.junctions.len(), 2);
        assert!(
            segment
                .junctions
                .iter()
                .any(|j| j.position() == v.position())
        );
    }
}
