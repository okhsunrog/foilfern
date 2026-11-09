//! Round trip with FreeRouting: an unrouted board built with the editor
//! commands is exported as Specctra DSN, routed by FreeRouting, and the
//! session is imported. Afterwards all connections are routed, our DRC
//! reports no clearance violations, and the official `librepcb-cli` accepts
//! the saved project (`--drc`). Skipped if FreeRouting (or Java >= 25) is
//! not available (see `scripts/cloud-setup.sh`).

use std::time::Duration;

use librepcb_core::project::board::drc::DrcMessageKind;
use librepcb_core::project::{BoardId, ProjectLoader};
use librepcb_core::types::{Angle, CircuitIdentifier, ElementName};
use librepcb_editor::commands::*;
use librepcb_editor::{FreeroutingError, FreeroutingRouter, ProjectEditor, routing_stats};

use crate::helpers::{copy_dir, create_editor, lib, mm};

fn ident(s: &str) -> CircuitIdentifier {
    CircuitIdentifier::new(s).unwrap()
}

/// Builds an unrouted board: R1..R4 and C1, C2 (0805) on a 30 x 20 mm board
/// with the nets VCC, GND, A, B.
fn build_unrouted(editor: &mut ProjectEditor) -> BoardId {
    let schematic = editor
        .execute(AddSchematic {
            name: ElementName::new("Main").unwrap(),
            index: None,
        })
        .unwrap();
    let board = editor
        .execute(AddBoard::new(ElementName::new("default").unwrap()))
        .unwrap()
        .board;
    let parts = [
        ("R1", lib::resistor(), lib::r0805()),
        ("R2", lib::resistor(), lib::r0805()),
        ("R3", lib::resistor(), lib::r0805()),
        ("R4", lib::resistor(), lib::r0805()),
        ("C1", lib::capacitor(), lib::c0805()),
        ("C2", lib::capacitor(), lib::c0805()),
    ];
    for (i, (name, component, device)) in parts.iter().enumerate() {
        let added = editor
            .execute(AddComponent {
                device: Some(*device),
                place: Some(SymbolPlacement {
                    schematic: Some(schematic),
                    position: mm(20.32 * (i as f64 + 1.0), 50.8),
                    rotation: Angle::DEG0,
                    mirrored: false,
                    gate_offset: None,
                }),
                ..AddComponent::new(*component)
            })
            .unwrap();
        assert_eq!(added.name, *name);
    }
    // VCC: R1.1, R3.1, C1.1, C2.1; GND: R2.2, R4.2, C1.2, C2.2;
    // A: R1.2, R2.1; B: R3.2, R4.1.
    for (component, pin, net) in [
        ("R1", "1", "VCC"),
        ("R3", "1", "VCC"),
        ("C1", "1", "VCC"),
        ("C2", "1", "VCC"),
        ("R2", "2", "GND"),
        ("R4", "2", "GND"),
        ("C1", "2", "GND"),
        ("C2", "2", "GND"),
        ("R1", "2", "A"),
        ("R2", "1", "A"),
        ("R3", "2", "B"),
        ("R4", "1", "B"),
    ] {
        editor
            .execute(ConnectPinToNet {
                pin: PinRef::new(component, pin),
                net: ident(net),
                label: true,
                stub_length: None,
            })
            .unwrap();
    }
    // Devices in two rows, rotated differently.
    for (component, x, y, rotation) in [
        ("R1", 6.0, 6.0, Angle::DEG0),
        ("R2", 14.0, 6.0, Angle::DEG90),
        ("R3", 22.0, 6.0, Angle::DEG0),
        ("R4", 6.0, 14.0, Angle::DEG90),
        ("C1", 14.0, 14.0, Angle::DEG0),
        ("C2", 22.0, 14.0, Angle::DEG180),
    ] {
        editor
            .execute(AddDevice {
                component: component.into(),
                board: Some(board),
                device: None,
                footprint: None,
                position: mm(x, y),
                rotation,
                mirrored: false,
            })
            .unwrap();
    }
    editor
        .execute(SetBoardOutline::rect(
            Some(board),
            mm(0.0, 0.0),
            mm(28.0, 20.0),
        ))
        .unwrap();
    board
}

/// Runs the official `librepcb-cli open-project --drc` on a copy of the
/// project and returns the non-approved messages. Returns `None` if the CLI
/// is not available.
fn cli_drc_messages(dir: &std::path::Path) -> Option<Vec<String>> {
    let cli = std::env::var_os("LIBREPCB_CLI").unwrap_or_else(|| "librepcb-cli".into());
    let copy = tempfile::tempdir().unwrap();
    copy_dir(dir, copy.path());
    let output = std::process::Command::new(cli)
        .arg("open-project")
        .arg("--drc")
        .arg(copy.path().join("test.lpp"))
        .output()
        .ok()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("Run DRC..."), "{text}");
    let mut messages: Vec<String> = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("- "))
        .map(str::to_owned)
        .collect();
    messages.sort();
    assert_eq!(output.status.success(), messages.is_empty(), "{text}");
    Some(messages)
}

#[test]
fn test_freerouting_round_trip() {
    let router = match FreeroutingRouter::detect() {
        Ok(router) => router,
        Err(FreeroutingError::NotFound(reason)) => {
            eprintln!("FreeRouting not available, skipping: {reason}");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    let mut config = router.config().clone();
    config.max_passes = Some(30);
    // One thread: deterministic routing.
    config.threads = Some(1);
    config.timeout = Duration::from_secs(600);
    // For inspecting the DSN/SES files and the log: LIBREPCB_FREEROUTING_TEST_DIR=<dir>.
    config.work_dir = std::env::var_os("LIBREPCB_FREEROUTING_TEST_DIR").map(Into::into);
    let router = FreeroutingRouter::new(config);

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("project");
    let mut editor = create_editor(&dir);
    let board = build_unrouted(&mut editor);
    let before = routing_stats(editor.project(), board);
    assert_eq!(before.unrouted_nets, 4);
    assert!(before.air_wires >= 8, "{before:?}");
    let history = editor.undo_stack().history().len();
    // DRC messages of the official CLI before routing.
    editor.save().unwrap();
    let cli_before = cli_drc_messages(&dir);

    let report = router.route(&mut editor, Some(board)).unwrap();
    assert_eq!(report.before, before);
    assert_eq!(report.after.air_wires, 0, "{report:#?}");
    assert_eq!(report.after.unrouted_nets, 0);
    assert_eq!(report.import.updated_components, 0);
    assert_eq!(report.import.unmodified_components, 6);
    // One undo group.
    assert_eq!(editor.undo_stack().history().len(), history + 1);
    let p = editor.project();
    assert!(p.is_ref_index_consistent());
    let b = p.board(board).unwrap();
    assert!(b.net_segments().values().all(|s| s.is_cohesive()));
    assert!(b.net_segments().values().all(|s| !s.traces().is_empty()));

    // Our DRC: no clearance violations and no missing connections.
    let mut project = {
        editor.save().unwrap();
        let fs = librepcb_core::fileio::TransactionalFileSystem::open_ro(
            &librepcb_core::fileio::FilePath::new(&dir).unwrap(),
        )
        .unwrap();
        ProjectLoader::new()
            .open(
                librepcb_core::fileio::TransactionalDirectory::new(std::sync::Arc::new(fs), ""),
                "test.lpp",
            )
            .unwrap()
    };
    let drc = project.run_drc(board, None, false, &|_| {}).unwrap();
    assert!(drc.errors.is_empty(), "{:?}", drc.errors);
    let problems: Vec<String> = drc
        .messages
        .iter()
        .filter(|m| {
            matches!(
                m.kind(),
                DrcMessageKind::CopperCopperClearanceViolation
                    | DrcMessageKind::CopperBoardClearanceViolation
                    | DrcMessageKind::CopperHoleClearanceViolation
                    | DrcMessageKind::MissingConnection
                    | DrcMessageKind::MinimumWidthViolation
            )
        })
        .map(|m| m.message().message().to_owned())
        .collect();
    assert!(problems.is_empty(), "{problems:#?}");

    // The official CLI reports no missing connections and no new messages
    // (the parts of the upstream test library have silkscreen texts close
    // to their pads, reported before and after routing).
    match (cli_before, cli_drc_messages(&dir)) {
        (Some(before), Some(after)) => {
            assert!(
                before.iter().any(|m| m.contains("Missing connection")),
                "{before:#?}"
            );
            for m in &after {
                assert!(
                    before.contains(m) && !m.contains("Missing connection"),
                    "new DRC message after routing: {m}\nbefore: {before:#?}\nafter: {after:#?}"
                );
            }
        }
        _ => eprintln!("librepcb-cli not found, skipping the upstream check"),
    }

    // Undo restores the unrouted board.
    assert!(editor.undo().unwrap());
    assert_eq!(routing_stats(editor.project(), board), before);
}
