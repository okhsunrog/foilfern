//! Tests of the undo stack (upstream `tests/unittests/editor/undostacktest.cpp`
//! works on dummy commands; here groups of real mutations are used).

use librepcb_core::project::circuit::NetClass;
use librepcb_core::project::{Mutation, NetClassId};
use librepcb_core::types::{ElementName, Uuid};
use librepcb_editor::commands::{AddNetClass, ApplyMutations};
use librepcb_editor::{Error, ProjectEditor};

use crate::helpers::create_project;

fn net_class(name: &str) -> Mutation {
    Mutation::AddNetClass(NetClass::new(
        Uuid::new_random(),
        ElementName::new(name).unwrap(),
    ))
}

fn editor() -> (tempfile::TempDir, ProjectEditor) {
    let tmp = tempfile::tempdir().unwrap();
    let editor = ProjectEditor::new(create_project(&tmp.path().join("p")));
    (tmp, editor)
}

fn class_count(editor: &ProjectEditor) -> usize {
    editor.project().circuit().net_classes().len()
}

#[test]
fn test_initial_state() {
    let (_tmp, editor) = editor();
    let stack = editor.undo_stack();
    assert!(!stack.can_undo());
    assert!(!stack.can_redo());
    assert!(stack.is_clean());
    assert!(!stack.is_group_active());
    assert_eq!(stack.undo_text(), None);
    assert!(stack.history().is_empty());
}

#[test]
fn test_execute_undo_redo() {
    let (_tmp, mut editor) = editor();
    let initial = class_count(&editor);
    let id = editor
        .execute(AddNetClass {
            name: ElementName::new("power").unwrap(),
            default_trace_width: None,
            default_via_drill: None,
        })
        .unwrap();
    assert_eq!(class_count(&editor), initial + 1);
    assert!(editor.project().circuit().net_class(id).is_some());
    assert_eq!(editor.undo_stack().undo_text(), Some("Add netclass"));
    assert!(!editor.is_clean());

    let state = editor.undo_stack().state_id();
    assert!(editor.undo().unwrap());
    assert_eq!(class_count(&editor), initial);
    assert!(editor.is_clean());
    assert_eq!(editor.undo_stack().redo_text(), Some("Add netclass"));
    assert!(!editor.undo().unwrap());

    assert!(editor.redo().unwrap());
    assert_eq!(class_count(&editor), initial + 1);
    assert!(editor.project().circuit().net_class(id).is_some());
    assert_eq!(editor.undo_stack().state_id(), state);
    assert!(!editor.redo().unwrap());
}

#[test]
fn test_group_commit_and_abort() {
    let (_tmp, mut editor) = editor();
    let initial = class_count(&editor);
    editor.begin_group("Agent action").unwrap();
    assert!(matches!(editor.begin_group("x"), Err(Error::GroupActive)));
    editor.apply_mutations("a", vec![net_class("a")]).unwrap();
    editor.apply_mutations("b", vec![net_class("b")]).unwrap();
    assert!(!editor.undo_stack().can_undo());
    assert!(editor.commit_group().unwrap());
    assert_eq!(class_count(&editor), initial + 2);
    assert_eq!(editor.undo_stack().history().len(), 1);
    assert_eq!(editor.undo_stack().undo_text(), Some("Agent action"));

    // One undo reverts the whole group.
    editor.undo().unwrap();
    assert_eq!(class_count(&editor), initial);

    // Abort rolls back.
    editor.begin_group("aborted").unwrap();
    editor.apply_mutations("c", vec![net_class("c")]).unwrap();
    assert_eq!(class_count(&editor), initial + 1);
    editor.abort_group().unwrap();
    assert_eq!(class_count(&editor), initial);
    assert!(matches!(editor.abort_group(), Err(Error::NoGroupActive)));

    // Empty groups are dropped.
    editor.begin_group("empty").unwrap();
    assert!(!editor.commit_group().unwrap());
    // The redo entry is still there (upstream would have discarded it
    // when the aborted group was started).
    assert!(editor.undo_stack().can_redo());
}

#[test]
fn test_failed_command_rolls_back() {
    let (_tmp, mut editor) = editor();
    let before = editor.project().circuit().clone();
    let duplicate = NetClass::new(Uuid::new_random(), ElementName::new("dup").unwrap());
    let result = editor.execute(ApplyMutations {
        text: None,
        mutations: vec![
            net_class("ok"),
            Mutation::AddNetClass(duplicate.clone()),
            Mutation::AddNetClass(duplicate),
        ],
    });
    assert!(result.is_err());
    assert_eq!(*editor.project().circuit(), before);
    assert!(!editor.undo_stack().can_undo());

    // Inside a group, only the failed command is rolled back.
    editor.begin_group("group").unwrap();
    editor.apply_mutations("x", vec![net_class("x")]).unwrap();
    assert!(
        editor
            .apply_mutations("y", vec![net_class("y"), net_class("y")])
            .is_err()
    );
    assert!(editor.commit_group().unwrap());
    let names: Vec<_> = editor
        .project()
        .circuit()
        .net_classes()
        .values()
        .map(|c| c.name().to_string())
        .collect();
    assert!(names.contains(&"x".to_owned()));
    assert!(!names.contains(&"y".to_owned()));
}

#[test]
fn test_new_group_discards_redo_and_clean_state() {
    let (_tmp, mut editor) = editor();
    editor.apply_mutations("a", vec![net_class("a")]).unwrap();
    editor.save().unwrap();
    assert!(editor.is_clean());
    editor.undo().unwrap();
    assert!(!editor.is_clean());
    editor.apply_mutations("b", vec![net_class("b")]).unwrap();
    assert!(!editor.undo_stack().can_redo());
    // The saved state is no longer reachable.
    editor.undo().unwrap();
    assert!(!editor.is_clean());
    let _ = NetClassId(Uuid::new_random());
}

/// The DRC approval cleanup (upstream `Board::updateDrcMessageApprovals()`
/// via `BoardEditor::setDrcResult()`) is applied without undo step and
/// marks the project as modified until it is saved.
#[test]
fn test_update_drc_approvals_is_manual_modification() {
    use librepcb_core::project::BoardMutation;
    use librepcb_core::serialization::{Mode, SExpression};
    use librepcb_editor::commands::AddBoard;
    let (_tmp, mut editor) = editor();
    let board = editor
        .execute(AddBoard {
            name: ElementName::new("default").unwrap(),
            copy_from: None,
            default_outline: true,
        })
        .unwrap()
        .board;
    let obsolete =
        SExpression::parse(b"(approved obsolete (foo bar))", None, Mode::LibrePcb).unwrap();
    editor
        .apply_mutations(
            "approvals",
            vec![Mutation::Board(BoardMutation::SetDrcApprovals {
                board,
                version: "1".parse().unwrap(),
                approvals: [obsolete].into(),
            })],
        )
        .unwrap();
    editor.save().unwrap();
    assert!(editor.is_clean());
    let undo_index = editor.undo_stack().index();

    let result = editor
        .update_derived_data(|p| p.run_drc(board, None, false, &|_| {}))
        .unwrap()
        .unwrap();
    assert!(editor.update_drc_approvals(board, &result).unwrap());
    let b = editor.project().board(board).unwrap();
    assert!(b.drc_approvals().is_empty());
    assert_eq!(b.drc_approvals_version().to_string(), "2");
    assert_eq!(editor.undo_stack().index(), undo_index);
    assert!(editor.has_manual_modifications());
    assert!(!editor.is_clean());
    // Nothing left to clean up.
    assert!(!editor.update_drc_approvals(board, &result).unwrap());
    editor.save().unwrap();
    assert!(editor.is_clean());
}

/// ERC approval cleanup (upstream `ProjectEditor::runErc()`): approvals of
/// messages which disappeared during the session are removed without undo
/// step, approvals of messages never seen are kept.
#[test]
fn test_update_erc_approvals() {
    use librepcb_core::project::erc::run_erc;
    use librepcb_core::serialization::{Mode, SExpression};
    let (_tmp, mut editor) = editor();
    // An unused net class gives an ERC message.
    editor.apply_mutations("a", vec![net_class("a")]).unwrap();
    let messages = run_erc(editor.project());
    assert!(!messages.is_empty());
    let approval = messages[0].approval().clone();
    let unknown =
        SExpression::parse(b"(approved future_check (foo bar))", None, Mode::LibrePcb).unwrap();
    editor
        .apply_mutations(
            "approve",
            vec![Mutation::SetErcApprovals(
                [approval.clone(), unknown.clone()].into(),
            )],
        )
        .unwrap();
    editor.save().unwrap();
    assert!(!editor.update_erc_approvals(&messages).unwrap());
    assert!(editor.is_clean());

    // The message disappears.
    let undo_index = editor.undo_stack().index();
    assert!(editor.update_erc_approvals(&[]).unwrap());
    assert_eq!(*editor.project().erc_approvals(), [unknown].into());
    assert_eq!(editor.undo_stack().index(), undo_index);
    assert!(!editor.is_clean());
}
