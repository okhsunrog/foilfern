//! Port of the upstream `BoardPlaneFragmentsBuilderTest`
//! (tests/unittests/core/project/board/boardplanefragmentsbuildertest.cpp).

use std::collections::BTreeMap;
use std::sync::Arc;

use foilfern_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use foilfern_core::geometry::Path;
use foilfern_core::project::board::BoardPlane;
use foilfern_core::project::{BoardId, BoardMutation, Mutation, PlaneId, Project, ProjectLoader};
use foilfern_core::serialization::{List, Mode, SExpression, SerializeObject};
use foilfern_core::types::{Layer, Uuid};

use crate::helpers::test_data_dir;

/// Opens an upstream test project (read-only).
pub fn open_project(dir: &str, lpp: &str) -> Project {
    let dir = test_data_dir().join("projects").join(dir);
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(&dir).unwrap()).unwrap();
    ProjectLoader::new()
        .open(TransactionalDirectory::new(Arc::new(fs), ""), lpp)
        .unwrap()
}

fn serialize_fragments(result: &BTreeMap<PlaneId, Vec<Path>>) -> String {
    let mut root = List::new("actual");
    for (uuid, fragments) in result {
        let mut child = List::new("plane");
        child.append_value(&uuid.0);
        for fragment in fragments {
            child.ensure_line_break();
            fragment.serialize(child.append_list("fragment"));
        }
        child.ensure_line_break();
        root.ensure_line_break();
        root.push(SExpression::from(child));
    }
    root.ensure_line_break();
    String::from_utf8(
        SExpression::from(root)
            .to_byte_array(Mode::LibrePcb)
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn test_fragments() {
    let test_data =
        test_data_dir().join("unittests/librepcbproject/BoardPlaneFragmentsBuilderTest");
    let mut project = open_project("Nested Planes", "project.lpp");
    let board: BoardId = project.boards()[0].id();

    // Force planes rebuild.
    let result = project.rebuild_planes(board, None).unwrap();

    // Check if fragments have been applied.
    let b = project.board(board).unwrap();
    for plane in b.planes().values() {
        assert_eq!(
            b.derived().fragments_of(plane.id()),
            result.get(&plane.id()).map_or(&[][..], Vec::as_slice)
        );
    }
    // Only layers without planes are still scheduled for a rebuild.
    for plane in b.planes().values() {
        assert!(!b.derived().dirty_plane_layers().contains(&plane.layer()));
    }

    // Compare with expected plane fragments loaded from file.
    let actual = serialize_fragments(&result);
    let expected = std::fs::read_to_string(test_data.join("expected.lp")).unwrap();
    assert!(actual == expected, "plane fragments differ:\n{actual}");
}

#[test]
fn test_many_threads() {
    let mut project = open_project("Nested Planes", "project.lpp");
    let board: BoardId = project.boards()[0].id();

    // Copy planes on top layer to more layers, otherwise this test is quite
    // meaningless.
    let mut settings = project.board(board).unwrap().settings().clone();
    settings.inner_layer_count = 40;
    project
        .apply(Mutation::Board(BoardMutation::SetSettings {
            board,
            settings: Box::new(settings),
        }))
        .unwrap();
    let copper = project.board(board).unwrap().copper_layers();
    let planes: Vec<BoardPlane> = project
        .board(board)
        .unwrap()
        .planes()
        .values()
        .cloned()
        .collect();
    for plane in planes {
        for layer in copper.iter().filter(|l| **l != Layer::TOP_COPPER) {
            let new_plane = BoardPlane::new(
                Uuid::new_random(),
                *layer,
                plane.net(),
                plane.outline().clone(),
            );
            project.add_board_plane(board, new_plane).unwrap();
        }
    }

    // Run several times to test multithreading.
    let mut first = None;
    for _ in 0..3 {
        let job = project.plane_job(board, None).unwrap().unwrap();
        let result = job.run();
        assert_eq!(result.board, board);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(result.finished);
        match &first {
            None => first = Some(result.planes),
            Some(first) => assert!(result.planes == *first),
        }
    }
}

#[test]
fn stale_plane_result_preserves_newer_fragments() {
    use foilfern_core::types::Point;
    let mut project = open_project("Nested Planes", "project.lpp");
    let board = project.boards()[0].id();
    project.rebuild_planes(board, None).unwrap();
    let old_job = project.plane_job(board, None).unwrap().unwrap();
    let mut plane = project
        .board(board)
        .unwrap()
        .planes()
        .values()
        .next()
        .unwrap()
        .clone();
    plane.set_outline(plane.outline().translated(Point::from_nm(1_000_000, 0)));
    project
        .apply(Mutation::Board(BoardMutation::UpdatePlane { board, plane }))
        .unwrap();
    project.rebuild_planes(board, None).unwrap();
    let revision = project.revision();
    let before = project
        .board(board)
        .unwrap()
        .derived()
        .plane_fragments()
        .clone();
    let dirty = project
        .board(board)
        .unwrap()
        .derived()
        .dirty_plane_layers()
        .clone();
    assert!(!project.apply_plane_job_result(old_job.run()).unwrap());
    assert_eq!(project.revision(), revision);
    assert_eq!(
        project.board(board).unwrap().derived().plane_fragments(),
        &before
    );
    assert_eq!(
        project.board(board).unwrap().derived().dirty_plane_layers(),
        &dirty
    );
}

#[test]
fn aborted_and_failed_plane_results_preserve_dirty_layers() {
    use std::sync::atomic::AtomicBool;
    let mut project = open_project("Nested Planes", "project.lpp");
    let board = project.boards()[0].id();
    for aborted in [true, false] {
        let job = project.plane_job(board, None).unwrap().unwrap();
        let mut result = job.run_with_abort(&AtomicBool::new(aborted));
        if !aborted {
            result.errors.push("Injected plane failure".into());
        }
        let revision = project.revision();
        let before = project
            .board(board)
            .unwrap()
            .derived()
            .plane_fragments()
            .clone();
        let dirty = project
            .board(board)
            .unwrap()
            .derived()
            .dirty_plane_layers()
            .clone();
        assert!(!dirty.is_empty());
        assert!(!project.apply_plane_job_result(result).unwrap());
        assert_eq!(project.revision(), revision);
        assert_eq!(
            project.board(board).unwrap().derived().plane_fragments(),
            &before
        );
        assert_eq!(
            project.board(board).unwrap().derived().dirty_plane_layers(),
            &dirty
        );
    }
}
