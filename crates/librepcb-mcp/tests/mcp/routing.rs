//! Applied routing must report DRC regressions even with zero air wires.
//! The tiny SES candidate is built from actual pad coordinates; no Java,
//! network access or external libraries are needed.

use std::path::Path;
use std::time::Duration;

use librepcb_editor::commands::export_specctra_dsn;
use librepcb_editor::routing_validation::capture_routing_state;
use librepcb_editor::{
    FreeroutingConfig, FreeroutingLauncher, FreeroutingOutput, FreeroutingRouter, routing_stats,
};
use librepcb_mcp::Session;
use librepcb_mcp::outcome::Outcome;
use librepcb_mcp::tools::{board_edit, circuit, library, project, schematic_edit, write};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn args<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}

fn board_session(dir: &Path) -> Session {
    let mut session = Session::new();
    let ws =
        project::workspace_open(&mut session, args(json!({"path": dir.join("ws")})), true).unwrap();
    crate::common::install_populated_library(Path::new(
        ws.result["libraries_path"].as_str().unwrap(),
    ));
    library::library_rescan(&session).unwrap();
    project::project_create(&mut session, args(json!({"name": "Routing"}))).unwrap();
    for (name, x) in [("R1", 5), ("R2", 15)] {
        schematic_edit::component_add(
            &mut session,
            args(json!({
                "part": "R-0805", "designator": name, "value": "1k"
            })),
        )
        .unwrap();
        board_edit::device_place(
            &mut session,
            args(json!({
                "component": name, "position": {"x": x, "y": 10}, "rotation": 90
            })),
        )
        .unwrap();
    }
    for (net, pins) in [("A", ["R1.1", "R2.1"]), ("B", ["R1.2", "R2.2"])] {
        schematic_edit::connect(&mut session, args(json!({"net": net, "pins": pins}))).unwrap();
    }
    board_edit::board_set_outline(&mut session, args(json!({"width": 30, "height": 20}))).unwrap();
    board_edit::design_rules_set(&mut session, args(json!({"min_copper_width": 0.2}))).unwrap();
    session
}

fn candidate(session: &Session, width_nm: i64) -> String {
    let board = librepcb_mcp::tools::layout::board_get(session, Default::default()).unwrap();
    let coordinate = |item: &Value, axis: &str| {
        (item["position"][axis].as_f64().unwrap() * 1_000_000.0).round() as i64
    };
    let placement: String = board.result["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|device| {
            let name = device["designator"].as_str().unwrap();
            format!(
                "(component {name} (place {name} {} {} front {}))",
                coordinate(device, "x"),
                coordinate(device, "y"),
                device["rotation"]
            )
        })
        .collect();
    let mut nets = String::new();
    for name in ["A", "B"] {
        let pads: Vec<_> = board.result["devices"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|device| device["pads"].as_array().unwrap())
            .filter(|pad| pad["net"] == name)
            .collect();
        assert_eq!(pads.len(), 2);
        nets.push_str(&format!(
            "(net {name} (wire (path top_cu {width_nm} {} {} {} {})))",
            coordinate(pads[0], "x"),
            coordinate(pads[0], "y"),
            coordinate(pads[1], "x"),
            coordinate(pads[1], "y")
        ));
    }
    format!(
        "(session Routing (base_design Routing) (placement (resolution mm 1000000) {placement}) (routes (resolution mm 1000000) (network_out {nets})))"
    )
}

#[test]
fn imported_routing_is_checked_after_application_for_both_entry_points() {
    for freerouting in [false, true] {
        for width_nm in [150_000, 250_000] {
            let tmp = tempfile::tempdir().unwrap();
            let mut session = board_session(tmp.path());
            let ses = candidate(&session, width_nm);
            let netlist = circuit::netlist(&session).unwrap().result;
            let history = session
                .project()
                .unwrap()
                .editor
                .undo_stack()
                .history()
                .len();
            let p = session.project().unwrap().project();
            let board = p.boards()[0].id();
            let out = if freerouting {
                let validation_before =
                    capture_routing_state(&mut session.project_mut().unwrap().editor, board)
                        .unwrap();
                let p = session.project().unwrap().project();
                let export = export_specctra_dsn(p, Some(board)).unwrap();
                let job = board_edit::FreeroutingJob {
                    // The runner is deliberately not executed: test a returned SES candidate.
                    router: FreeroutingRouter::new(FreeroutingConfig::new(
                        FreeroutingLauncher::Executable("unused".into()),
                    )),
                    dsn: export.dsn,
                    manifest: export.manifest,
                    before: routing_stats(p, board),
                    validation_before,
                    args: board_edit::AutorouteArgs {
                        backend: board_edit::RouterBackend::Freerouting,
                        ..Default::default()
                    },
                };
                board_edit::autoroute_freerouting_finish(
                    &mut session,
                    job,
                    Ok(FreeroutingOutput {
                        session: ses,
                        elapsed: Duration::ZERO,
                        log: String::new(),
                    }),
                )
                .unwrap()
            } else {
                let path = tmp.path().join("candidate.ses");
                std::fs::write(&path, ses).unwrap();
                let revision = session.project().unwrap().project().revision();
                board_edit::specctra_import(
                    &mut session,
                    args(json!({"path": path, "expected_revision": revision})),
                )
                .unwrap()
            };
            let report = &out.result["validation"];
            assert_eq!(out.result["unrouted_after"], 0, "{out:?}");
            assert_eq!(report["netlist_preserved"], true);
            assert_eq!(report["before"]["copper_errors"], 0, "{out:?}");
            assert_eq!(report["after"]["complete"], true);
            assert_eq!(circuit::netlist(&session).unwrap().result, netlist);
            if width_nm == 150_000 {
                assert_eq!(out.outcome, Outcome::Partial, "{out:?}");
                assert_eq!(report["acceptable"], false);
                assert_eq!(report["drc_clean"], false);
                assert_eq!(report["new_copper_error_count"], 2, "{out:?}");
                assert!(
                    report["new_errors"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|e| e["kind"] == "MinimumWidthViolation")
                );
            } else {
                assert_eq!(out.outcome, Outcome::Complete, "{out:?}");
                assert_eq!(report["acceptable"], true);
                assert_eq!(report["drc_clean"], true, "{out:?}");
            }
            assert_eq!(
                session
                    .project()
                    .unwrap()
                    .editor
                    .undo_stack()
                    .history()
                    .len(),
                history + 1
            );
            write::undo_redo(&mut session, Default::default(), false).unwrap();
            assert_eq!(
                board_edit::unrouted(&session, Default::default())
                    .unwrap()
                    .result["count"],
                2
            );
        }
    }
}

#[test]
fn builtin_routing_reports_actual_model_checks_and_preserves_one_undo_step() {
    let tmp = tempfile::tempdir().unwrap();
    let mut session = board_session(tmp.path());
    let revision = session.project().unwrap().project().revision();
    let history = session
        .project()
        .unwrap()
        .editor
        .undo_stack()
        .history()
        .len();
    let out = board_edit::autoroute(
        &mut session,
        args(json!({"backend": "builtin", "expected_revision": revision})),
    )
    .unwrap();
    assert_eq!(out.outcome, Outcome::Complete, "{out:?}");
    assert_eq!(out.result["validation"]["acceptable"], true);
    assert_eq!(out.result["validation"]["drc_clean"], true);
    assert_eq!(out.result["unrouted_before"], 2);
    assert_eq!(out.result["unrouted_after"], 0);
    assert_eq!(
        session
            .project()
            .unwrap()
            .editor
            .undo_stack()
            .history()
            .len(),
        history + 1
    );
}
