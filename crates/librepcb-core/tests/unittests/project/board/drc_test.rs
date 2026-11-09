//! Port of tests/unittests/core/project/board/boarddesignrulechecktest.cpp.
//!
//! `test_messages` runs the DRC on every board of the upstream test project
//! `projects/DRC` and compares the approvals of the emitted messages with
//! the approvals stored in the boards (each board contains the approvals of
//! exactly the messages upstream emits for it).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path as StdPath, PathBuf};
use std::sync::Arc;

use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::project::board::drc::{DrcProgress, DrcResult};
use librepcb_core::project::{Project, ProjectLoader};
use librepcb_core::serialization::{List, Mode, SExpression};

fn projects_dir() -> PathBuf {
    StdPath::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data/projects")
}

/// Opens a project of the upstream test data (read-only).
pub fn open_project(dir: &str, file: &str) -> Project {
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(projects_dir().join(dir)).unwrap())
        .unwrap();
    ProjectLoader::new()
        .open(TransactionalDirectory::new(Arc::new(fs), ""), file)
        .unwrap()
}

fn no_progress(_: DrcProgress<'_>) {}

/// Runs the full DRC on all boards with their own settings.
pub fn run_all(project: &mut Project) -> Vec<(String, BTreeSet<SExpression>, DrcResult)> {
    let boards: Vec<_> = project
        .boards_with_ids()
        .map(|(id, b)| {
            (
                id,
                b.properties().name.to_string(),
                b.drc_approvals().clone(),
            )
        })
        .collect();
    boards
        .into_iter()
        .map(|(id, name, approvals)| {
            let result = project.run_drc(id, None, false, &no_progress).unwrap();
            (name, approvals, result)
        })
        .collect()
}

fn to_string(approvals: &BTreeSet<SExpression>) -> String {
    let mut node = List::new("node");
    for approval in approvals {
        node.ensure_line_break();
        node.push(approval.clone());
    }
    node.ensure_line_break();
    SExpression::List(node)
        .to_string_with_mode(Mode::LibrePcb)
        .unwrap()
}

#[test]
fn test_messages() {
    // Ignore certain messages in all boards, except on whitelisted ones.
    let whitelist: BTreeMap<&str, &[&str]> = BTreeMap::from([
        ("missing_device", &["checkForUnplacedComponents"][..]),
        ("missing_connection", &["checkForMissingConnections"][..]),
        ("unused_layer", &["checkUsedLayers"][..]),
        (
            "useless_via",
            &["checkVias", "checkVias2", "checkVias3"][..],
        ),
        ("plated_cutouts", &["checkBoardCutouts"][..]),
    ]);

    let mut project = open_project("DRC", "project.lpp");
    let mut summary = Vec::new();
    let mut failures = Vec::new();
    for (name, expected, result) in run_all(&mut project) {
        assert!(result.errors.is_empty(), "{name}: {:?}", result.errors);

        // Filter messages, get approvals and check uniqueness of their
        // approval.
        let mut approvals = BTreeSet::new();
        for msg in &result.messages {
            let msg_type = msg.approval().children()[0].value().unwrap().to_owned();
            if let Some(boards) = whitelist.get(msg_type.as_str())
                && !boards.contains(&name.as_str())
            {
                continue;
            }
            assert!(
                approvals.insert(msg.approval().clone()),
                "{name}: ambiguous approval for message '{}':\n{}",
                msg.message().message(),
                msg.approval().to_string_with_mode(Mode::LibrePcb).unwrap()
            );
        }

        let line = format!(
            " * {name}: Emitted {} messages, {} approved",
            approvals.len(),
            expected.len()
        );
        summary.push(line);
        let (expected, actual) = (to_string(&expected), to_string(&approvals));
        if expected != actual {
            failures.push((name, expected, actual));
        }
    }
    for (name, expected, actual) in &failures {
        eprintln!("FAILED board '{name}':\n--- expected\n{expected}\n--- actual\n{actual}");
    }
    eprintln!("Summary:\n{}", summary.join("\n"));
    assert!(failures.is_empty(), "DRC approvals differ from upstream");
}

/// A full check rebuilds the plane fragments first (like upstream), so the
/// planes of `checkCopperBoardClearances` violate the board clearance even
/// though no fragments were calculated before.
#[test]
fn full_check_rebuilds_planes() {
    let mut project = open_project("DRC", "project.lpp");
    let id = project
        .boards_with_ids()
        .find(|(_, b)| *b.properties().name == *"checkCopperBoardClearances")
        .unwrap()
        .0;
    let fragment_count = |project: &Project| -> usize {
        let board = project.board(id).unwrap();
        board
            .planes()
            .keys()
            .map(|plane| board.derived().fragments_of(*plane).len())
            .sum()
    };
    assert_eq!(fragment_count(&project), 0);
    let result = project.run_drc(id, None, false, &no_progress).unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(fragment_count(&project) > 0);
    let approvals: BTreeSet<SExpression> = result
        .messages
        .iter()
        .map(|m| m.approval().clone())
        .collect();
    for plane in [
        "2cf859b9-a14e-4c32-b9e4-9b687c09f9ee",
        "81180634-835f-407d-9360-c7213176ad55",
    ] {
        let expected = format!("(approved copper_board_clearance_violation\n (plane {plane})\n)\n");
        let expected = SExpression::parse(expected.as_bytes(), None, Mode::LibrePcb).unwrap();
        assert!(approvals.contains(&expected), "plane {plane} not reported");
    }
}

/// The quick check runs only a subset of the checks and reports progress
/// up to 100% (no upstream counterpart).
#[test]
fn quick_check_and_progress() {
    use std::sync::Mutex;
    let mut project = open_project("DRC", "project.lpp");
    let id = project
        .boards_with_ids()
        .find(|(_, b)| *b.properties().name == *"checkMinimumCopperWidth")
        .unwrap()
        .0;
    let percents = Mutex::new(Vec::new());
    let progress = |p: DrcProgress<'_>| {
        if let DrcProgress::Percent(p) = p {
            percents.lock().unwrap().push(p);
        }
    };
    let quick = project.run_drc(id, None, true, &progress).unwrap();
    assert!(quick.quick && quick.errors.is_empty());
    let percents = percents.into_inner().unwrap();
    assert_eq!(percents.first(), Some(&1));
    assert_eq!(percents.last(), Some(&100));
    assert!(percents.iter().all(|p| *p <= 100));
    let full = project.run_drc(id, None, false, &no_progress).unwrap();
    assert!(!full.quick);
    let kinds = |r: &DrcResult| -> BTreeSet<String> {
        r.messages
            .iter()
            .map(|m| m.approval().children()[0].value().unwrap().to_owned())
            .collect()
    };
    assert!(kinds(&quick).contains("minimum_width_violation"));
    assert!(!kinds(&quick).contains("missing_device"));
    assert!(kinds(&full).contains("missing_device"));
}

#[test]
fn test_multithreading() {
    let mut project = open_project("Gerber Test", "project.lpp");
    let board = project.boards_with_ids().next().unwrap().0;
    // Run several times to heavily test multithreading.
    for _ in 0..10 {
        let result = project.run_drc(board, None, false, &no_progress).unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }
}

/// `Project::drc_approvals_update()` (upstream
/// `Board::updateDrcMessageApprovals()`): obsolete approvals are removed
/// after full checks only, approvals unknown to this session are kept, and
/// the first full check after a file format upgrade removes everything not
/// occurring anymore.
#[test]
fn drc_approvals_update() {
    use librepcb_core::project::{BoardMutation, Mutation};
    let mut project = open_project("DRC", "project.lpp");
    let id = project
        .boards_with_ids()
        .find(|(_, b)| *b.properties().name == *"checkMinimumCopperWidth")
        .unwrap()
        .0;
    let approvals = |p: &Project| p.board(id).unwrap().drc_approvals().clone();
    let version = |p: &Project| p.board(id).unwrap().drc_approvals_version().to_string();
    let unknown =
        SExpression::parse(b"(approved future_check (foo bar))", None, Mode::LibrePcb).unwrap();
    project
        .apply(Mutation::Board(BoardMutation::SetDrcApproval {
            board: id,
            approval: unknown.clone(),
            approved: true,
        }))
        .unwrap();
    let initial = approvals(&project);
    assert_eq!(version(&project), "2");

    // A full check with all messages: nothing to remove (the unknown
    // approval never occurred in this session).
    let full = project.run_drc(id, None, false, &no_progress).unwrap();
    assert_eq!(project.drc_approvals_update(id, &full).unwrap(), None);
    assert!(
        project
            .board(id)
            .unwrap()
            .derived()
            .supported_drc_approvals()
            .contains(full.messages[0].approval())
    );

    // A message disappears: after a quick check nothing is removed, after
    // a full check its approval is removed.
    let approved = full
        .messages
        .iter()
        .find(|m| initial.contains(m.approval()))
        .unwrap()
        .approval()
        .clone();
    let mut reduced = full.clone();
    reduced.messages.retain(|m| *m.approval() != approved);
    let mut quick = reduced.clone();
    quick.quick = true;
    assert_eq!(project.drc_approvals_update(id, &quick).unwrap(), None);
    let mutation = project.drc_approvals_update(id, &reduced).unwrap().unwrap();
    project.apply(mutation).unwrap();
    let mut expected = initial.clone();
    expected.remove(&approved);
    assert_eq!(approvals(&project), expected);
    assert!(approvals(&project).contains(&unknown));
    assert_eq!(version(&project), "2");

    // First full check after a file format upgrade: everything not
    // occurring is removed and the version is updated.
    project
        .apply(Mutation::Board(BoardMutation::SetDrcApprovals {
            board: id,
            version: "1".parse().unwrap(),
            approvals: initial.clone(),
        }))
        .unwrap();
    let mut quick = full.clone();
    quick.quick = true;
    assert_eq!(project.drc_approvals_update(id, &quick).unwrap(), None);
    let mutation = project.drc_approvals_update(id, &full).unwrap().unwrap();
    project.apply(mutation).unwrap();
    let mut expected = initial.clone();
    expected.remove(&unknown);
    assert_eq!(approvals(&project), expected);
    assert_eq!(version(&project), "2");
}

/// On all boards of the upstream DRC test project, whose approvals are
/// exactly the emitted messages, a full check removes nothing except the
/// messages ignored by `test_messages`; boards with an older approvals
/// version get the current version.
#[test]
fn drc_approvals_update_on_test_project() {
    let mut project = open_project("DRC", "project.lpp");
    let boards: Vec<_> = project.boards_with_ids().map(|(id, _)| id).collect();
    for id in boards {
        let before = project.board(id).unwrap().drc_approvals().clone();
        let old_version = project.board(id).unwrap().drc_approvals_version().clone();
        let result = project.run_drc(id, None, false, &no_progress).unwrap();
        let emitted: BTreeSet<_> = result
            .messages
            .iter()
            .map(|m| m.approval().clone())
            .collect();
        match project.drc_approvals_update(id, &result).unwrap() {
            None => assert_eq!(old_version.to_string(), "2"),
            Some(m) => {
                project.apply(m).unwrap();
                let board = project.board(id).unwrap();
                assert_eq!(board.drc_approvals_version().to_string(), "2");
                let expected: BTreeSet<_> = before.intersection(&emitted).cloned().collect();
                assert_eq!(*board.drc_approvals(), expected);
            }
        }
    }
}
