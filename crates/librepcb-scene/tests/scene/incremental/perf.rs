//! Timing of single moves on the largest upstream test board (and on the
//! same board with a large generated net segment): the incremental update
//! must be much cheaper than a full rebuild and touch only a few items.

use std::time::{Duration, Instant};

use librepcb_core::geometry::{Junction, Trace, TraceAnchor};
use librepcb_core::project::board::{BoardNetSegment, BoardSegmentElements};
use librepcb_core::project::{
    BoardId, BoardMutation, BoardNetSegmentRef, ComponentInstanceId, Mutation, NetSegmentId,
    Project,
};
use librepcb_core::types::{Layer, Length, PositiveLength, Uuid};
use librepcb_editor::ProjectEditor;
use librepcb_editor::commands::{ApplyMutations, MoveDevice};
use librepcb_scene::{BoardScene, BoardSide, ColorScheme, SceneSync};

use super::helpers::{assert_same, dump_board, mm, open_upstream, projects, upstream_editor};

/// The board with the most scene items of all upstream test projects.
fn largest_board() -> (std::path::PathBuf, BoardId) {
    let mut best: Option<(std::path::PathBuf, BoardId, usize)> = None;
    for dir in projects() {
        let project = open_upstream(&dir);
        for board in project.boards() {
            let scene = BoardScene::build(
                &project,
                board.id(),
                BoardSide::Top,
                &ColorScheme::BOARD_DARK,
            )
            .unwrap();
            let items = scene.scene().len();
            if best.as_ref().is_none_or(|(_, _, n)| items > *n) {
                best = Some((dir.clone(), board.id(), items));
            }
        }
    }
    let (dir, board, _) = best.expect("a board");
    (dir, board)
}

/// The device with the most traces at its pads.
fn most_connected_device(p: &Project, board: BoardId) -> ComponentInstanceId {
    let b = p.board(board).unwrap();
    let traces_at = |c: &ComponentInstanceId| {
        b.net_segments()
            .values()
            .flat_map(|s| s.traces().values())
            .filter(|t| {
                [t.p1(), t.p2()].iter().any(
                    |a| matches!(a, TraceAnchor::FootprintPad { device, .. } if *device == c.0),
                )
            })
            .count()
    };
    *b.devices()
        .keys()
        .max_by_key(|c| traces_at(c))
        .expect("a device")
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn apply(editor: &mut ProjectEditor, mutation: BoardMutation) {
    editor
        .execute(ApplyMutations {
            text: None,
            mutations: vec![Mutation::Board(mutation)],
        })
        .unwrap();
}

/// Adds a net segment without net: a grid of `n`×`n` junctions connected
/// by traces. Returns the segment and a junction in the middle.
fn add_grid(editor: &mut ProjectEditor, board: BoardId, n: usize) -> (NetSegmentId, Uuid) {
    let pitch = 0.5;
    let width = PositiveLength::new(Length::new(200_000)).unwrap();
    let ids: Vec<Vec<Uuid>> = (0..n)
        .map(|_| (0..n).map(|_| Uuid::new_random()).collect())
        .collect();
    let junctions = (0..n).flat_map(|i| {
        let ids = &ids;
        (0..n).map(move |j| Junction::new(ids[i][j], mm(i as f64 * pitch, j as f64 * pitch)))
    });
    let mut traces = Vec::new();
    for i in 0..n {
        for j in 0..n {
            let a = TraceAnchor::Junction(ids[i][j]);
            if i + 1 < n {
                let b = TraceAnchor::Junction(ids[i + 1][j]);
                traces.push(Trace::new(
                    Uuid::new_random(),
                    Layer::TOP_COPPER,
                    width,
                    a,
                    b,
                ));
            }
            if j + 1 < n {
                let b = TraceAnchor::Junction(ids[i][j + 1]);
                traces.push(Trace::new(
                    Uuid::new_random(),
                    Layer::TOP_COPPER,
                    width,
                    a,
                    b,
                ));
            }
        }
    }
    let segment =
        BoardNetSegment::with_elements(Uuid::new_random(), None, [], [], junctions, traces);
    let id = segment.id();
    apply(editor, BoardMutation::AddNetSegment { board, segment });
    (id, ids[n / 2][n / 2])
}

struct Timing {
    items: usize,
    full: Duration,
    incremental: Duration,
    operations: Vec<u64>,
}

/// Times full builds and incremental updates after each of `steps`
/// modifications, and checks the result against a fresh build.
fn measure(
    editor: &mut ProjectEditor,
    board: BoardId,
    steps: usize,
    mut modify: impl FnMut(&mut ProjectEditor, usize),
) -> Timing {
    let build = |p: &Project| {
        BoardScene::build(p, board, BoardSide::Top, &ColorScheme::BOARD_DARK).unwrap()
    };
    editor
        .update_derived_data(|p| p.rebuild_air_wires(board).unwrap())
        .unwrap();
    let full: Vec<(Duration, usize)> = (0..5)
        .map(|_| {
            let t = Instant::now();
            let scene = build(editor.project());
            (t.elapsed(), scene.scene().len())
        })
        .collect();
    let items = full[0].1;
    let full = median(full.into_iter().map(|(d, _)| d).collect());
    let mut scene = build(editor.project());
    let mut sync = SceneSync::new(editor.project());
    let mut incremental = Vec::new();
    let mut operations = Vec::new();
    for i in 0..steps {
        modify(editor, i);
        editor
            .update_derived_data(|p| p.rebuild_air_wires(board).unwrap())
            .unwrap();
        let rev = scene.scene().rev();
        let t = Instant::now();
        let rebuilt = sync.sync(editor.project(), &mut scene).unwrap();
        incremental.push(t.elapsed());
        operations.push(scene.scene().rev() - rev);
        assert!(!rebuilt, "a move rebuilt the scene");
    }
    assert_same(
        "perf",
        &dump_board(&scene),
        &dump_board(&build(editor.project())),
    );
    Timing {
        items,
        full,
        incremental: median(incremental),
        operations,
    }
}

fn report(what: &str, t: &Timing) {
    println!(
        "{what} ({} items): full build {:?}, incremental {:?} ({:.0}x faster, {:?} scene \
         operations per step)",
        t.items,
        t.full,
        t.incremental,
        t.full.as_secs_f64() / t.incremental.as_secs_f64().max(1e-9),
        t.operations,
    );
}

fn move_device(
    board: BoardId,
    component: ComponentInstanceId,
) -> impl FnMut(&mut ProjectEditor, usize) {
    move |editor, i| {
        let position = editor
            .project()
            .board(board)
            .unwrap()
            .device(component)
            .unwrap()
            .position();
        let d = if i % 2 == 0 { 2.54 } else { -2.54 };
        editor
            .execute(MoveDevice {
                component: component.into(),
                board: Some(board),
                position: Some(position + mm(d, d)),
                rotation: None,
                mirrored: None,
                locked: None,
            })
            .unwrap();
    }
}

#[test]
fn test_move_is_incremental_and_fast() {
    let (dir, board) = largest_board();
    let name = dir.file_name().unwrap().to_string_lossy().into_owned();
    let mut editor = upstream_editor(&dir);
    let component = most_connected_device(editor.project(), board);

    // The largest test board as it is.
    let t = measure(&mut editor, board, 6, move_device(board, component));
    report(&format!("{name}: move device"), &t);
    assert!(t.operations.iter().all(|n| (*n as usize) < t.items / 4));
    assert!(
        t.incremental * 2 < t.full,
        "{:?} vs {:?}",
        t.incremental,
        t.full
    );

    // With a generated segment of 80x80 junctions (12640 traces).
    let (segment, junction) = add_grid(&mut editor, board, 80);
    let t = measure(&mut editor, board, 6, move_device(board, component));
    report(&format!("{name} + grid: move device"), &t);
    assert!(t.operations.iter().all(|n| *n < 200));
    assert!(
        t.incremental * 10 < t.full,
        "{:?} vs {:?}",
        t.incremental,
        t.full
    );

    let t = measure(&mut editor, board, 6, |editor, i| {
        let b = editor.project().board(board).unwrap();
        let mut j = b.net_segment(segment).unwrap().junctions()[&junction].clone();
        let d = if i % 2 == 0 { 0.2 } else { -0.2 };
        j.set_position(j.position() + mm(d, d));
        apply(
            editor,
            BoardMutation::UpdateNetSegmentElements {
                segment: BoardNetSegmentRef { board, segment },
                elements: BoardSegmentElements {
                    junctions: vec![j],
                    ..BoardSegmentElements::default()
                },
            },
        );
    });
    report(&format!("{name} + grid: move junction"), &t);
    // The 4 traces at the junction.
    assert!(t.operations.iter().all(|n| *n <= 8), "{:?}", t.operations);
    assert!(
        t.incremental * 10 < t.full,
        "{:?} vs {:?}",
        t.incremental,
        t.full
    );
}
