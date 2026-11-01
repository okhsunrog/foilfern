//! Port of tests/unittests/core/project/board/boardplanefragmentsbuildertest.cpp.
//! Test data stays in the pinned upstream checkout.

use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::project::board::{BoardPlane, BoardPlaneFragmentsBuilder};
use librepcb_core::project::{BoardMutation, Mutation, Project, ProjectLoader};
use librepcb_core::serialization::{List, Mode, SExpression, SerializeObject};
use librepcb_core::types::{Layer, Uuid};
use std::collections::BTreeSet;
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::Arc;

fn nested_project() -> Project {
    let root = Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data/projects/Nested Planes");
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(root).unwrap()).unwrap();
    ProjectLoader::new()
        .open(TransactionalDirectory::new(Arc::new(fs), ""), "project.lpp")
        .unwrap()
}

#[test]
fn test_fragments() {
    let mut project = nested_project();
    let id = project.boards()[0].id();
    let result =
        BoardPlaneFragmentsBuilder::build(&project, project.board(id).unwrap(), None).unwrap();
    project.rebuild_planes(id, None).unwrap();
    assert_eq!(
        project.board(id).unwrap().derived().plane_fragments(),
        result.fragments()
    );
    let mut root = List::new("actual");
    for (id, fragments) in result.fragments() {
        root.ensure_line_break();
        let child = root.append_list("plane");
        child.append_value(id);
        for fragment in fragments {
            child.ensure_line_break();
            fragment.serialize(child.append_list("fragment"));
        }
        child.ensure_line_break();
    }
    root.ensure_line_break();
    let actual = SExpression::List(root)
        .to_string_with_mode(Mode::LibrePcb)
        .unwrap();
    let expected =
        std::fs::read_to_string(Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join(
            "tests/data/unittests/librepcbproject/BoardPlaneFragmentsBuilderTest/expected.lp",
        ))
        .unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn cancellation_is_atomic_and_filtered_rebuild_clears_dirty_layers() {
    let mut project = nested_project();
    let id = project.boards()[0].id();
    let revision = project.revision();
    let dirty = project
        .board(id)
        .unwrap()
        .derived()
        .dirty_plane_layers()
        .clone();
    assert!(
        project
            .rebuild_planes_with_progress(id, None, |_, _| ControlFlow::Break(()))
            .is_err()
    );
    assert_eq!(project.revision(), revision);
    assert_eq!(
        project.board(id).unwrap().derived().dirty_plane_layers(),
        &dirty
    );
    assert!(
        project
            .board(id)
            .unwrap()
            .derived()
            .plane_fragments()
            .is_empty()
    );
    let layers = BTreeSet::from([Layer::TOP_COPPER]);
    project.rebuild_planes(id, Some(&layers)).unwrap();
    assert!(
        !project
            .board(id)
            .unwrap()
            .derived()
            .dirty_plane_layers()
            .contains(&Layer::TOP_COPPER)
    );
    let revision = project.revision();
    project.rebuild_planes(id, Some(&layers)).unwrap();
    assert_eq!(project.revision(), revision);
}

#[test]
fn test_many_threads() {
    let mut project = nested_project();
    let id = project.boards()[0].id();
    let mut settings = project.board(id).unwrap().settings().clone();
    settings.inner_layer_count = 40;
    project
        .apply(Mutation::Board(BoardMutation::SetSettings {
            board: id,
            settings: Box::new(settings),
        }))
        .unwrap();
    let planes: Vec<_> = project
        .board(id)
        .unwrap()
        .planes()
        .values()
        .cloned()
        .collect();
    let layers = project.board(id).unwrap().copper_layers();
    for plane in planes {
        for layer in layers.iter().filter(|layer| **layer != Layer::TOP_COPPER) {
            project
                .add_board_plane(
                    id,
                    BoardPlane::new(
                        Uuid::new_random(),
                        *layer,
                        plane.net(),
                        plane.outline().clone(),
                    ),
                )
                .unwrap();
        }
    }
    // Keep upstream's 40-inner-layer fixture; exercise simultaneous read-only
    // builds rather than Qt's asynchronous builder lifecycle.
    let board = project.board(id).unwrap();
    std::thread::scope(|scope| {
        let first =
            scope.spawn(|| BoardPlaneFragmentsBuilder::build(&project, board, None).unwrap());
        let second =
            scope.spawn(|| BoardPlaneFragmentsBuilder::build(&project, board, None).unwrap());
        assert_eq!(first.join().unwrap(), second.join().unwrap());
    });
}
