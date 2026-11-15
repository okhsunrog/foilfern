//! Initial placement uses native text/mask geometry and the ordinary undo journal.
use super::helpers::{create_project, lib, library_source, mm};
use foilfern_core::project::{BoardMutation, Mutation};
use foilfern_core::types::{Angle, ElementName};
use foilfern_editor::commands::{AddBoard, AddComponent, AddDevice, SetBoardOutline};
use foilfern_editor::legend_placement::initial::{
    self, InitialLegendStatus, PlaceInitialDeviceLegend,
};
use foilfern_editor::{Error, ProjectEditor};
use std::sync::Arc;

#[test]
fn initial_legend_respects_locks_and_cancelled_search_rolls_back() {
    let tmp = tempfile::tempdir().unwrap();
    let mut editor =
        ProjectEditor::with_source(create_project(tmp.path()), Arc::new(library_source()));
    let board = editor
        .execute(AddBoard::new(ElementName::new("Board").unwrap()))
        .unwrap()
        .board;
    editor
        .execute(SetBoardOutline::rect(Some(board), mm(0., 0.), mm(40., 30.)))
        .unwrap();
    let mut components = Vec::new();
    for x in [10., 20.] {
        let component = editor
            .execute(AddComponent {
                device: Some(lib::c0805()),
                ..AddComponent::new(lib::capacitor())
            })
            .unwrap()
            .component;
        editor
            .execute(AddDevice {
                component: component.into(),
                board: Some(board),
                device: None,
                footprint: None,
                position: mm(x, 15.),
                rotation: Angle::DEG0,
                mirrored: false,
            })
            .unwrap();
        components.push(component);
    }
    let before = editor.project().board(board).unwrap().clone();
    let revision = editor.project().revision();
    let mut checkpoints = 0;
    let error = editor
        .execute_atomic("Cancelled initial legend", |tx| {
            initial::place(tx, board, components.clone(), || {
                checkpoints += 1;
                if checkpoints == 2 {
                    Err(Error::InvalidArgument("cancelled".into()))
                } else {
                    Ok(())
                }
            })
        })
        .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert_eq!(editor.project().board(board).unwrap(), &before);
    assert_eq!(editor.project().revision(), revision);
    let mut locked = before.device(components[0]).unwrap().clone();
    for mut text in locked.stroke_texts().values().cloned().collect::<Vec<_>>() {
        text.set_locked(true);
        locked.insert_stroke_text(text);
    }
    editor
        .apply_mutations(
            "Lock text",
            vec![Mutation::Board(BoardMutation::UpdateDevice {
                board,
                device: locked.clone(),
            })],
        )
        .unwrap();
    let reports = editor
        .execute(PlaceInitialDeviceLegend {
            board,
            components: components.clone(),
        })
        .unwrap();
    assert_eq!(reports[0].status, InitialLegendStatus::Locked);
    assert!(!reports[0].verified_clear());
    assert_eq!(
        editor
            .project()
            .board(board)
            .unwrap()
            .device(components[0])
            .unwrap(),
        &locked
    );
    assert_eq!(reports[1].status, InitialLegendStatus::Moved);
    assert!(reports[1].verified_clear());
    let mut restored = editor
        .project()
        .board(board)
        .unwrap()
        .device(components[1])
        .unwrap()
        .clone();
    for text in before
        .device(components[1])
        .unwrap()
        .stroke_texts()
        .values()
    {
        restored.insert_stroke_text(text.clone());
    }
    assert_eq!(&restored, before.device(components[1]).unwrap());
    let revision = editor.project().revision();
    let reports = editor
        .execute(PlaceInitialDeviceLegend {
            board,
            components: vec![components[1]],
        })
        .unwrap();
    assert_eq!(reports[0].status, InitialLegendStatus::Unchanged);
    assert_eq!(reports[0].attempts, 0);
    assert_eq!(editor.project().revision(), revision);
}
