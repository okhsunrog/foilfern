//! Applied routing must report DRC regressions even with zero air wires.
//! The tiny SES candidate is built from actual pad coordinates; no Java,
//! network access or external libraries are needed.

use std::path::Path;
use std::time::Duration;

use foilfern_editor::commands::export_specctra_dsn;
use foilfern_editor::routing_validation::capture_routing_state;
use foilfern_editor::{
    FreeroutingConfig, FreeroutingLauncher, FreeroutingOutput, FreeroutingRouter, routing_stats,
};
use foilfern_mcp::Session;
use foilfern_mcp::outcome::Outcome;
use foilfern_mcp::tools::{board_edit, circuit, library, project, schematic_edit, write};
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
    let board = foilfern_mcp::tools::layout::board_get(session, Default::default()).unwrap();
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
            assert_finding_accounting(report);
            assert_eq!(
                report["finding_delta"]["missing_connections"]["resolved"],
                2
            );
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

#[test]
fn grouped_trace_width_edits_preserve_anchors_and_report_clearance_regressions() {
    use foilfern_core::types::{Length, PositiveLength, Uuid};
    use foilfern_mcp::ErrorKind;
    use std::collections::BTreeMap;

    let tmp = tempfile::tempdir().unwrap();
    let mut session = board_session(tmp.path());
    let path = tmp.path().join("thin.ses");
    std::fs::write(&path, candidate(&session, 150_000)).unwrap();
    board_edit::specctra_import(&mut session, args(json!({"path": path}))).unwrap();
    let p = session.project().unwrap().project();
    let board = p.boards()[0].id();
    let original: BTreeMap<_, _> = p
        .board(board)
        .unwrap()
        .net_segments()
        .values()
        .flat_map(|s| s.traces().values().map(|t| (t.uuid(), t.clone())))
        .collect();
    let revision = p.revision();
    let history = session
        .project()
        .unwrap()
        .editor
        .undo_stack()
        .history()
        .len();
    let updated = board_edit::trace_update(
        &mut session,
        args(json!({
            "nets": ["A", "B"], "width": 0.25, "expected_revision": revision
        })),
    )
    .unwrap();
    assert_eq!(updated.outcome, Outcome::Complete, "{updated:?}");
    assert_eq!(updated.result["traces"].as_array().unwrap().len(), 2);
    assert_finding_accounting(&updated.result["validation"]);
    assert_eq!(updated.result["validation"]["resolved_error_count"], 2);
    assert_eq!(updated.result["validation"]["drc_clean"], true);
    assert_eq!(updated.result["validation"]["netlist_preserved"], true);
    let p = session.project().unwrap().project();
    for trace in p
        .board(board)
        .unwrap()
        .net_segments()
        .values()
        .flat_map(|s| s.traces().values())
    {
        let mut expected = original[&trace.uuid()].clone();
        expected.set_width(PositiveLength::new(Length::from_mm(0.25).unwrap()).unwrap());
        assert_eq!(trace, &expected);
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
    let noop = board_edit::trace_update(
        &mut session,
        args(json!({"nets": ["A", "B"], "width": 0.25})),
    )
    .unwrap();
    assert_eq!(noop.result["traces"], json!([]));
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

    let uuid = updated.result["traces"][0]["uuid"].as_str().unwrap();
    let too_wide =
        board_edit::trace_update(&mut session, args(json!({"uuids": [uuid], "width": 6}))).unwrap();
    assert_eq!(too_wide.outcome, Outcome::Partial, "{too_wide:?}");
    assert_finding_accounting(&too_wide.result["validation"]);
    assert_eq!(too_wide.result["unrouted_after"], 0);
    assert!(
        too_wide.result["validation"]["new_copper_error_count"]
            .as_u64()
            .unwrap()
            > 0
    );
    write::undo_redo(&mut session, Default::default(), false).unwrap();

    let before = session
        .project()
        .unwrap()
        .project()
        .board(board)
        .unwrap()
        .net_segments()
        .clone();
    let current_revision = session.project().unwrap().project().revision();
    for (value, kind) in [
        (
            json!({"uuids": [uuid, Uuid::new_random()], "width": 0.3}),
            ErrorKind::NotFound,
        ),
        (
            json!({"uuids": [uuid], "width": 0.3, "expected_revision": revision}),
            ErrorKind::StaleRevision,
        ),
        (
            json!({"uuids": [uuid], "width": 0}),
            ErrorKind::InvalidArgument,
        ),
    ] {
        let error = board_edit::trace_update(&mut session, args(value)).unwrap_err();
        assert_eq!(error.kind, kind);
        let p = session.project().unwrap().project();
        assert_eq!(p.board(board).unwrap().net_segments(), &before);
        assert_eq!(p.revision(), current_revision);
    }
    write::undo_redo(&mut session, Default::default(), false).unwrap();
    let p = session.project().unwrap().project();
    assert!(
        p.board(board)
            .unwrap()
            .net_segments()
            .values()
            .flat_map(|s| s.traces().values())
            .all(|t| t == &original[&t.uuid()])
    );
    write::undo_redo(&mut session, Default::default(), true).unwrap();
    let drc = foilfern_mcp::tools::output::drc_run(&mut session, Default::default()).unwrap();
    assert_eq!(drc.result["errors"], 0, "{drc:?}");
}

/// All categories use native approval identity and the same unapproved scope.
fn assert_finding_accounting(report: &Value) {
    let delta = &report["finding_delta"];
    assert!(delta.is_object(), "{report:#}");
    for (category, summary) in [
        ("errors", "errors"),
        ("non_connectivity_errors", "non_connectivity_errors"),
        ("copper_errors", "copper_errors"),
        ("missing_connections", "missing_connection_errors"),
        ("warnings", "warnings"),
    ] {
        let d = &delta[category];
        let count = |key: &str| d[key].as_u64().unwrap();
        assert_eq!(
            d["before"], report["before"][summary],
            "{category}: {report:#}"
        );
        assert_eq!(
            d["after"], report["after"][summary],
            "{category}: {report:#}"
        );
        assert_eq!(count("before"), count("resolved") + count("unchanged"));
        assert_eq!(count("after"), count("added") + count("unchanged"));
    }
    for field in ["before", "after", "added", "resolved", "unchanged"] {
        assert_eq!(
            delta["errors"][field].as_u64().unwrap(),
            delta["non_connectivity_errors"][field].as_u64().unwrap()
                + delta["missing_connections"][field].as_u64().unwrap()
        );
    }
    assert_eq!(
        report["new_error_count"],
        delta["non_connectivity_errors"]["added"]
    );
    assert_eq!(
        report["resolved_error_count"],
        delta["non_connectivity_errors"]["resolved"]
    );
    assert_eq!(
        report["new_copper_error_count"],
        delta["copper_errors"]["added"]
    );
    assert_eq!(
        report.get("new_warning_count").cloned().unwrap_or(json!(0)),
        delta["warnings"]["added"]
    );
}

#[test]
fn routing_check_status_retains_warnings_approvals_and_incomplete_coverage() {
    use foilfern_core::project::{BoardId, BoardMutation, Mutation};
    use foilfern_core::types::Uuid;
    use foilfern_editor::commands::AddVia;
    use foilfern_editor::routing_validation::RoutingValidation;

    let tmp = tempfile::tempdir().unwrap();
    let mut session = board_session(tmp.path());
    let path = tmp.path().join("routed.ses");
    std::fs::write(&path, candidate(&session, 250_000)).unwrap();
    board_edit::specctra_import(&mut session, args(json!({"path":path}))).unwrap();
    // A detached, netless via gives a real inherited warning without an air wire.
    session
        .project_mut()
        .unwrap()
        .editor
        .execute(args::<AddVia>(json!({
            "position":{"x":20_000_000,"y":15_000_000}
        })))
        .unwrap();
    let board = session.project().unwrap().project().boards()[0].id();
    let before = capture_routing_state(&mut session.project_mut().unwrap().editor, board).unwrap();
    let after = capture_routing_state(&mut session.project_mut().unwrap().editor, board).unwrap();
    let report = serde_json::to_value(RoutingValidation::compare(before.clone(), after)).unwrap();
    assert_finding_accounting(&report);
    assert_eq!(report["acceptable"], true, "{report:#}");
    assert_eq!(report["drc_clean"], true, "{report:#}");
    assert_eq!(report["drc_status"], "warnings_only", "{report:#}");
    assert!(
        report["finding_delta"]["warnings"]["unchanged"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(report["finding_delta"]["warnings"]["resolved"], 0);

    let failed = capture_routing_state(
        &mut session.project_mut().unwrap().editor,
        BoardId(Uuid::new_random()),
    )
    .unwrap();
    let validation = RoutingValidation::compare(before.clone(), failed);
    assert!(!validation.drc_clean && !validation.acceptable);
    assert!(validation.finding_delta.is_none());
    assert_eq!(
        serde_json::to_value(&validation).unwrap()["drc_status"],
        "incomplete"
    );
    assert_eq!(validation.resolved_error_count, 0);

    // Approved warnings are excluded exactly as they are in drc_run. Do not
    // confuse approved findings with absent raw physical warnings.
    let result = session
        .project_mut()
        .unwrap()
        .editor
        .update_derived_data(|p| p.run_drc(board, None, false, &|_| {}))
        .unwrap()
        .unwrap();
    let approvals: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.message().severity() == foilfern_core::rule_check::Severity::Warning)
        .map(|m| {
            Mutation::Board(BoardMutation::SetDrcApproval {
                board,
                approval: m.approval().clone(),
                approved: true,
            })
        })
        .collect();
    assert!(!approvals.is_empty());
    session
        .project_mut()
        .unwrap()
        .editor
        .apply_mutations("Approve warnings", approvals)
        .unwrap();
    let approved =
        capture_routing_state(&mut session.project_mut().unwrap().editor, board).unwrap();
    let report = serde_json::to_value(RoutingValidation::compare(before, approved)).unwrap();
    assert_finding_accounting(&report);
    assert_eq!(report["drc_status"], "no_errors_or_warnings");
    assert_eq!(report["after"]["warnings"], 0);
    assert!(
        report["finding_delta"]["warnings"]["resolved"]
            .as_u64()
            .unwrap()
            > 0
    );
}
