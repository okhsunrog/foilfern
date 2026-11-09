//! Tests of the Specctra session import: port of the upstream
//! `CmdBoardSpecctraImportTest`
//! (tests/unittests/editor/project/board/cmdboardspecctraimporttest.cpp),
//! plus checks of the imported routes, undo and the strict import.

use std::sync::Arc;

use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::geometry::TraceAnchor;
use librepcb_core::project::{Project, ProjectLoader};
use librepcb_editor::ProjectEditor;
use librepcb_editor::commands::*;

use crate::helpers::test_data_dir;

fn open_gerber_test() -> Project {
    let dir = test_data_dir().join("projects/Gerber Test");
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(&dir).unwrap()).unwrap();
    ProjectLoader::new()
        .open(TransactionalDirectory::new(Arc::new(fs), ""), "project.lpp")
        .unwrap()
}

fn session(name: &str) -> String {
    std::fs::read_to_string(
        test_data_dir()
            .join("unittests/librepcbproject/BoardSpecctraExportTest")
            .join(name),
    )
    .unwrap()
}

fn import(editor: &mut ProjectEditor, name: &str) -> SpecctraImportResult {
    editor
        .execute(ImportSpecctraSession {
            board: None,
            session: session(name),
            manifest: None,
        })
        .unwrap()
}

#[test]
fn test_upstream_sessions() {
    for name in [
        "session_freerouting_old.ses",
        "session_freerouting_2.4.0.ses",
    ] {
        let mut editor = ProjectEditor::new(open_gerber_test());
        let before = editor
            .project()
            .board(editor.project().boards()[0].id())
            .unwrap()
            .clone();
        let result = import(&mut editor, name);
        assert!(editor.project().is_ref_index_consistent(), "{name}");
        let board = editor.project().board(result.board).unwrap();
        for segment in board.net_segments().values() {
            assert!(segment.is_cohesive(), "{name}");
        }
        // Board pads are kept.
        let pads = |b: &librepcb_core::project::board::Board| -> usize {
            b.net_segments().values().map(|s| s.pads().len()).sum()
        };
        assert_eq!(pads(board), pads(&before), "{name}");

        // Undo restores the board.
        assert!(editor.undo().unwrap());
        let after_undo = editor.project().board(result.board).unwrap();
        assert_eq!(after_undo.net_segments(), before.net_segments(), "{name}");
        assert_eq!(after_undo.devices(), before.devices(), "{name}");
    }
}

#[test]
fn test_old_session_routes() {
    let mut editor = ProjectEditor::new(open_gerber_test());
    let result = import(&mut editor, "session_freerouting_old.ses");
    // The session was created from this board: all components found, none
    // moved.
    assert_eq!(result.updated_components, 0);
    assert_eq!(result.unmodified_components, 2);
    assert!(result.reused_objects > 0);
    assert!(
        !result
            .messages
            .iter()
            .any(|m| m.message.contains("does not exist")),
        "{:?}",
        result.messages
    );
    let p = editor.project();
    let board = p.board(result.board).unwrap();
    let gnd = p.circuit().net_signal_by_name("GND").unwrap().0;
    let n3 = p.circuit().net_signal_by_name("N3").unwrap().0;
    let traces = |net| -> usize {
        board
            .net_segments()
            .values()
            .filter(|s| s.net() == Some(net))
            .map(|s| s.traces().len())
            .sum()
    };
    let vias = |net| -> usize {
        board
            .net_segments()
            .values()
            .filter(|s| s.net() == Some(net))
            .map(|s| s.vias().len())
            .sum()
    };
    assert_eq!(traces(gnd), 5);
    assert_eq!(vias(gnd), 3);
    assert_eq!(traces(n3), 3);
    // The N3 wire starts at a footprint pad.
    assert!(
        board
            .net_segments()
            .values()
            .filter(|s| s.net() == Some(n3))
            .flat_map(|s| s.traces().values())
            .any(|t| matches!(t.p1(), TraceAnchor::FootprintPad { .. })
                || matches!(t.p2(), TraceAnchor::FootprintPad { .. }))
    );
}

#[test]
fn test_invalid_session() {
    let mut editor = ProjectEditor::new(open_gerber_test());
    let err = editor
        .execute(ImportSpecctraSession {
            board: None,
            session: "(pcb foo)".into(),
            manifest: None,
        })
        .unwrap_err();
    assert!(err.to_string().contains("not a Specctra session"), "{err}");
    assert!(editor.undo_stack().history().is_empty());
}

#[test]
fn test_strict_import() {
    let mut editor = ProjectEditor::new(open_gerber_test());
    let export = export_specctra_dsn(editor.project(), None).unwrap();
    let manifest = export.manifest;
    assert_eq!(manifest.components.len(), 2);
    assert!(manifest.nets.contains("GND"));
    assert!(manifest.via_padstacks.contains("via-1.0-2.0-tht"));

    let strict = |editor: &mut ProjectEditor, session: String| {
        editor.execute(ImportSpecctraSession {
            board: None,
            session,
            manifest: Some(manifest.clone()),
        })
    };
    let old = session("session_freerouting_old.ses");

    // A moved component, an unknown net and an unknown via pad stack are
    // rejected before anything is modified.
    for (from, to) in [
        ("76000000 60000000 back", "77000000 60000000 back"),
        ("62000000 front 180", "62000000 back 180"),
        ("(net N3", "(net FOOBAR"),
        ("\"via-1.0-2.0-tht\"", "\"via-9.0-9.0-tht\""),
    ] {
        assert!(old.contains(from));
        let err = strict(&mut editor, old.replace(from, to)).unwrap_err();
        assert!(err.to_string().contains("Specctra session"), "{err}");
        assert!(editor.undo_stack().history().is_empty());
    }

    // The session was created from this board and passes.
    let result = strict(&mut editor, old.clone()).unwrap();
    assert_eq!(result.updated_components, 0);

    // After a modification of the project, the manifest is outdated.
    let err = strict(&mut editor, old).unwrap_err();
    assert!(err.to_string().contains("modified since"), "{err}");
}
