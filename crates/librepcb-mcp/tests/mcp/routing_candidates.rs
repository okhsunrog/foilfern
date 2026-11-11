//! Routing candidates are isolated previews and exact, revision-bound transactions.

use std::path::Path;

use librepcb_core::project::Mutation;
use librepcb_editor::ProjectEditor;
use librepcb_mcp::Session;
use librepcb_mcp::error::ErrorKind;
use librepcb_mcp::tools::{board_edit, circuit, library, project, routing, schematic_edit, write};
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
    project::project_create(&mut session, args(json!({"name": "Candidates"}))).unwrap();
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

fn edit_author(session: &mut Session) {
    let mut metadata = session.project().unwrap().project().metadata().clone();
    metadata.author.push_str(" changed");
    session
        .project_mut()
        .unwrap()
        .editor
        .apply_mutations(
            "Change author",
            vec![Mutation::SetProjectMetadata(metadata)],
        )
        .unwrap();
}

fn board_value(session: &Session) -> Value {
    serde_json::to_value(session.project().unwrap().project().boards()).unwrap()
}

#[test]
fn preparation_and_paginated_preview_leave_live_state_and_redo_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let mut session = board_session(temp.path());
    edit_author(&mut session);
    write::undo_redo(&mut session, Default::default(), false).unwrap();
    let open = session.project().unwrap();
    let revision = open.project().revision();
    let history = open.editor.undo_stack().history();
    let history_index = open.editor.undo_stack().index();
    let filesystem = open.file_system.save_state();
    let dirty = open.has_unsaved_changes();
    let board = board_value(&session);
    let netlist = circuit::netlist(&session).unwrap().result;
    assert!(open.editor.undo_stack().can_redo());

    let prepared =
        routing::routing_prepare(&mut session, args(json!({"expected_revision": revision})))
            .unwrap();
    assert_eq!(prepared.result["base_revision"], revision);
    assert_eq!(
        prepared.result["validation"]["acceptable"], true,
        "{prepared:?}"
    );
    assert_eq!(prepared.result["validation"]["after"]["unrouted"], 0);
    let candidate = prepared.result["candidate"].clone();
    let count = prepared.result["mutation_count"].as_u64().unwrap() as usize;
    assert!(count > 1);
    let mut mutations = Vec::new();
    let mut offset = 0;
    loop {
        let preview = routing::routing_preview(
            &session,
            args(json!({
                "candidate": candidate, "offset": offset, "limit": 1
            })),
        )
        .unwrap();
        assert_eq!(preview.result["stale"], false);
        assert_eq!(preview.result["offset"], offset);
        let page = preview.result["mutations"].as_array().unwrap();
        assert!(page.len() <= 1);
        mutations.extend(page.iter().cloned());
        match preview.result["next_offset"].as_u64() {
            Some(next) => {
                assert!(next > offset as u64);
                offset = next as usize;
            }
            None => break,
        }
    }
    assert_eq!(mutations.len(), count);
    let open = session.project().unwrap();
    assert_eq!(open.project().revision(), revision);
    assert_eq!(open.editor.undo_stack().history(), history);
    assert_eq!(open.editor.undo_stack().index(), history_index);
    assert!(open.editor.undo_stack().can_redo());
    assert_eq!(open.file_system.save_state(), filesystem);
    assert_eq!(open.has_unsaved_changes(), dirty);
    assert_eq!(board_value(&session), board);
    assert_eq!(circuit::netlist(&session).unwrap().result, netlist);
    write::undo_redo(&mut session, Default::default(), true).unwrap();
    assert!(
        session
            .project()
            .unwrap()
            .project()
            .metadata()
            .author
            .ends_with(" changed")
    );
}

#[test]
fn apply_replays_exact_preview_with_one_undo_group_and_consumes_candidate() {
    let temp = tempfile::tempdir().unwrap();
    let mut session = board_session(temp.path());
    let baseline = board_value(&session);
    let revision = session.project().unwrap().project().revision();
    let history = session
        .project()
        .unwrap()
        .editor
        .undo_stack()
        .history()
        .len();
    let prepared = routing::routing_prepare(&mut session, args(json!({}))).unwrap();
    let candidate = prepared.result["candidate"].clone();
    let preview = routing::routing_preview(
        &session,
        args(json!({"candidate": candidate, "limit": 200})),
    )
    .unwrap();
    assert!(preview.result["next_offset"].is_null());
    let mutations: Vec<Mutation> =
        serde_json::from_value(preview.result["mutations"].clone()).unwrap();
    let mut expected = ProjectEditor::new(session.project().unwrap().project().snapshot().unwrap());
    expected
        .apply_mutations("Exact preview", mutations)
        .unwrap();
    let expected_board = serde_json::to_value(expected.project().boards()).unwrap();
    let applied = routing::routing_apply(
        &mut session,
        args(json!({
            "candidate": candidate, "expected_revision": revision
        })),
    )
    .unwrap();
    assert_eq!(
        applied.result["validation"]["acceptable"], true,
        "{applied:?}"
    );
    assert_eq!(applied.result["revision_before"], revision);
    assert_eq!(board_value(&session), expected_board);
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
    assert_eq!(
        board_edit::unrouted(&session, Default::default())
            .unwrap()
            .result["count"],
        0
    );
    assert_eq!(
        routing::routing_apply(&mut session, args(json!({"candidate": candidate})))
            .unwrap_err()
            .kind,
        ErrorKind::NotFound
    );
    write::undo_redo(&mut session, Default::default(), false).unwrap();
    assert_eq!(board_value(&session), baseline);
    write::undo_redo(&mut session, Default::default(), true).unwrap();
    assert_eq!(board_value(&session), expected_board);
}

#[test]
fn edit_then_undo_still_invalidates_candidate_and_failed_apply_changes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let mut session = board_session(temp.path());
    let prepared = routing::routing_prepare(&mut session, args(json!({}))).unwrap();
    let candidate = prepared.result["candidate"].clone();
    edit_author(&mut session);
    write::undo_redo(&mut session, Default::default(), false).unwrap();
    let preview =
        routing::routing_preview(&session, args(json!({"candidate": candidate}))).unwrap();
    assert_eq!(preview.result["stale"], true);
    let revision = session.project().unwrap().project().revision();
    let history = session.project().unwrap().editor.undo_stack().history();
    let filesystem = session.project().unwrap().file_system.save_state();
    let board = board_value(&session);
    assert_eq!(
        routing::routing_apply(
            &mut session,
            args(json!({
                "candidate": candidate, "expected_revision": revision
            }))
        )
        .unwrap_err()
        .kind,
        ErrorKind::StaleRevision
    );
    assert_eq!(session.project().unwrap().project().revision(), revision);
    assert_eq!(
        session.project().unwrap().editor.undo_stack().history(),
        history
    );
    assert_eq!(
        session.project().unwrap().file_system.save_state(),
        filesystem
    );
    assert_eq!(board_value(&session), board);
}

#[test]
fn candidate_storage_is_bounded_and_discard_releases_capacity() {
    let temp = tempfile::tempdir().unwrap();
    let mut session = board_session(temp.path());
    let mut candidates = Vec::new();
    for _ in 0..8 {
        candidates.push(
            routing::routing_prepare(&mut session, args(json!({})))
                .unwrap()
                .result["candidate"]
                .clone(),
        );
    }
    assert_eq!(
        routing::routing_prepare(&mut session, args(json!({})))
            .unwrap_err()
            .kind,
        ErrorKind::Refused
    );
    let discarded = candidates.remove(0);
    routing::routing_discard(&mut session, args(json!({"candidate": discarded}))).unwrap();
    assert_eq!(
        routing::routing_preview(&session, args(json!({"candidate": discarded})))
            .unwrap_err()
            .kind,
        ErrorKind::NotFound
    );
    assert_eq!(
        routing::routing_discard(&mut session, args(json!({"candidate": discarded})))
            .unwrap_err()
            .kind,
        ErrorKind::NotFound
    );
    routing::routing_prepare(&mut session, args(json!({}))).unwrap();
    let invalid = routing::routing_preview(
        &session,
        args(json!({"candidate": candidates[0], "limit": 201})),
    )
    .unwrap_err();
    assert_eq!(invalid.kind, ErrorKind::InvalidArgument);
}

#[test]
fn candidate_arguments_are_strict_and_revision_is_checked_before_preparation() {
    assert!(
        serde_json::from_value::<routing::RoutingPrepareArgs>(json!({"backend": "freerouting"}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<routing::RoutingPreviewArgs>(
            json!({"candidate": "bad", "unknown": true})
        )
        .is_err()
    );
    assert!(serde_json::from_value::<routing::RoutingCandidateArgs>(json!({})).is_err());
    let temp = tempfile::tempdir().unwrap();
    let mut session = board_session(temp.path());
    let revision = session.project().unwrap().project().revision();
    assert_eq!(
        routing::routing_prepare(
            &mut session,
            args(json!({"expected_revision": revision + 1}))
        )
        .unwrap_err()
        .kind,
        ErrorKind::StaleRevision
    );
    assert_eq!(session.project().unwrap().project().revision(), revision);
    assert_eq!(
        routing::routing_preview(&session, args(json!({"candidate": "malformed"})))
            .unwrap_err()
            .kind,
        ErrorKind::InvalidArgument
    );
}

#[test]
fn detached_job_completion_rejects_concurrent_edits_without_publishing_candidate() {
    let temp = tempfile::tempdir().unwrap();
    let mut session = board_session(temp.path());
    let job = routing::prepare_job(&session, args(json!({}))).unwrap();
    edit_author(&mut session);
    let revision = session.project().unwrap().project().revision();
    let history = session.project().unwrap().editor.undo_stack().history();
    let candidate = routing::run_job(job).unwrap();
    assert_eq!(
        routing::finish_prepare(&mut session, candidate)
            .unwrap_err()
            .kind,
        ErrorKind::StaleRevision
    );
    assert_eq!(session.project().unwrap().project().revision(), revision);
    assert_eq!(
        session.project().unwrap().editor.undo_stack().history(),
        history
    );
    // The stale completion did not reserve a storage slot.
    for _ in 0..8 {
        routing::routing_prepare(&mut session, args(json!({}))).unwrap();
    }
}

#[test]
fn detached_job_cannot_be_published_into_reopened_project_with_same_uuid() {
    use librepcb_editor::{NoLibrarySource, OpenProject};
    use std::sync::Arc;

    let temp = tempfile::tempdir().unwrap();
    let mut session = board_session(temp.path());
    let job = routing::prepare_job(&session, args(json!({}))).unwrap();
    let snapshot = session.project().unwrap().project().snapshot().unwrap();
    let filesystem = Arc::clone(snapshot.directory().file_system());
    let source = Arc::new(NoLibrarySource);
    project::project_close(&mut session, args(json!({"discard_changes": true}))).unwrap();
    session
        .attach_project(OpenProject::new(snapshot, filesystem, false, source).into_shared())
        .unwrap();
    let revision = session.project().unwrap().project().revision();
    let board = board_value(&session);
    let candidate = routing::run_job(job).unwrap();
    assert_eq!(
        routing::finish_prepare(&mut session, candidate)
            .unwrap_err()
            .kind,
        ErrorKind::Conflict
    );
    assert_eq!(session.project().unwrap().project().revision(), revision);
    assert_eq!(board_value(&session), board);
    assert!(
        session
            .project()
            .unwrap()
            .editor
            .undo_stack()
            .history()
            .is_empty()
    );
}

#[test]
fn already_routed_board_produces_empty_candidate_without_spurious_history() {
    let temp = tempfile::tempdir().unwrap();
    let mut session = board_session(temp.path());
    board_edit::autoroute(&mut session, args(json!({"backend": "builtin"}))).unwrap();
    let revision = session.project().unwrap().project().revision();
    let history = session.project().unwrap().editor.undo_stack().history();
    let prepared = routing::routing_prepare(&mut session, args(json!({}))).unwrap();
    assert_eq!(prepared.result["mutation_count"], 0);
    assert_eq!(prepared.result["validation"]["after"]["unrouted"], 0);
    routing::routing_apply(
        &mut session,
        args(json!({"candidate": prepared.result["candidate"]})),
    )
    .unwrap();
    assert_eq!(session.project().unwrap().project().revision(), revision);
    assert_eq!(
        session.project().unwrap().editor.undo_stack().history(),
        history
    );
}

#[test]
fn save_time_change_invalidates_physical_text_context_without_a_model_edit() {
    let temp = tempfile::tempdir().unwrap();
    let mut session = board_session(temp.path());
    let prepared = routing::routing_prepare(&mut session, Default::default()).unwrap();
    let id = prepared.result["candidate"].clone();
    let revision = session.project().unwrap().project().revision();
    project::project_save(&mut session, args(json!({}))).unwrap();
    assert_eq!(session.project().unwrap().project().revision(), revision);
    let preview = routing::routing_preview(&session, args(json!({"candidate":id}))).unwrap();
    assert_eq!(preview.result["stale"], true);
    let board = board_value(&session);
    let history = session.project().unwrap().editor.undo_stack().history();
    let error = routing::routing_apply(&mut session, args(json!({"candidate":id}))).unwrap_err();
    assert_eq!(error.kind, ErrorKind::StaleRevision);
    assert_eq!(board_value(&session), board);
    assert_eq!(
        session.project().unwrap().editor.undo_stack().history(),
        history
    );
}

#[test]
fn diagnostics_report_grid_access_and_local_blockers_without_edits() {
    let temp = tempfile::tempdir().unwrap();
    let session = board_session(temp.path());
    let open = session.project().unwrap();
    let revision = open.project().revision();
    let before = board_value(&session);
    let report = routing::routing_diagnose(
        &session,
        args(json!({
            "net":"A", "grid_pitch":0.1, "trace_width":0.2, "probe_length":2.0,
            "image":{"endpoint":0, "layer":"top"},
        })),
    )
    .unwrap();
    assert_eq!(report.result["endpoint_count"], 2);
    for endpoint in report.result["endpoints"].as_array().unwrap() {
        assert!(endpoint["access_count"].as_u64().unwrap() > 0);
        assert_eq!(endpoint["grid_pitch_mm"], 0.1);
        assert_eq!(endpoint["width_mm"], 0.2);
        let blocker = endpoint["straight_probes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|probe| &probe["blocker"])
            .find(|blocker| blocker["kind"] == "terminal" && blocker["net"] == "B")
            .unwrap_or_else(|| panic!("No adjacent pad blocker: {endpoint:#}"));
        assert_eq!(blocker["object"]["kind"], "pad");
        assert_eq!(blocker["object"]["uuid"], blocker["anchor"]["pad_uuid"]);
        let device: librepcb_core::types::Uuid =
            serde_json::from_value(blocker["object"]["device_uuid"].clone()).unwrap();
        assert!(
            open.project()
                .boards()
                .iter()
                .any(|b| b.device(device.into()).is_some())
        );
    }
    assert_eq!(report.result["image"]["endpoint"], 0);
    assert_eq!(report.result["image"]["layer"], "top_cu");
    assert_eq!(report.images.len(), 1);
    assert!(report.images[0].starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(&report.images[0][16..24], &[0, 0, 2, 128, 0, 0, 2, 128]);
    assert_eq!(open.project().revision(), revision);
    assert_eq!(board_value(&session), before);
}

#[test]
fn diagnostics_report_rule_precedence_and_effective_dimensions() {
    use librepcb_core::types::{Length, UnsignedLength};

    let temp = tempfile::tempdir().unwrap();
    let mut session = board_session(temp.path());
    board_edit::design_rules_set(
        &mut session,
        args(json!({
            "default_trace_width":0.42, "min_copper_width":0.2, "min_copper_clearance":0.21,
            "default_via_drill":0.3, "min_drill_diameter":0.35, "min_annular_ring":0.23
        })),
    )
    .unwrap();
    schematic_edit::net_class_set(
        &mut session,
        args(json!({
            "net_class":"Signal", "nets":["A"], "default_trace_width":0.15, "default_via_drill":0.7
        })),
    )
    .unwrap();
    schematic_edit::net_class_set(
        &mut session,
        args(json!({"net_class":"Power", "nets":["B"]})),
    )
    .unwrap();
    let circuit = session.project().unwrap().project().circuit();
    let mut signal = circuit.net_class_by_name("Signal").unwrap().1.clone();
    let mut power = circuit.net_class_by_name("Power").unwrap().1.clone();
    let signal_id = signal.uuid();
    let power_id = power.uuid();
    let unsigned = |nm| UnsignedLength::new(Length::new(nm)).unwrap();
    signal.set_min_copper_width(unsigned(350_000));
    signal.set_min_via_drill_diameter(unsigned(450_000));
    power.set_min_copper_copper_clearance(unsigned(330_000));
    session
        .project_mut()
        .unwrap()
        .editor
        .apply_mutations(
            "Set net class limits",
            vec![
                Mutation::UpdateNetClass(signal),
                Mutation::UpdateNetClass(power),
            ],
        )
        .unwrap();

    let diagnose = |args| routing::routing_diagnose(&session, args).unwrap().result;
    let automatic = diagnose(args(json!({"net":"A"})));
    let rule = &automatic["rules"];
    assert_eq!(
        rule["trace_width"]["base"]["source"],
        json!({
            "kind":"net_class","uuid":signal_id,"field":"default_trace_width"
        })
    );
    assert_eq!(rule["trace_width"]["base"]["mm"], 0.15);
    assert_eq!(rule["trace_width"]["effective_mm"], 0.2);
    assert_eq!(rule["trace_width"]["clamped"], true);
    assert_eq!(
        automatic["endpoints"][0]["width_mm"],
        rule["trace_width"]["effective_mm"]
    );
    assert_eq!(rule["net_class_drc"]["min_copper_width_mm"], 0.35);
    assert_eq!(rule["net_class_drc"]["min_via_drill_diameter_mm"], 0.45);
    assert_eq!(rule["via_drill"]["base"]["source"]["kind"], "board_design");
    assert_eq!(rule["via_drill"]["effective_mm"], 0.35);
    assert_eq!(
        rule["via_diameter"]["base"]["source"]["field"],
        "via_annular_ring"
    );
    assert_eq!(rule["via_diameter"]["effective_mm"], 0.81);
    assert_eq!(rule["via_storage"]["inherited_drill"]["mm"], 0.7);
    assert_eq!(rule["via_storage"]["explicit"], true);
    assert_eq!(rule["board_copper_minimum"]["mm"], 0.21);
    assert_eq!(
        rule["copper_clearance"]["base"]["source"]["uuid"],
        json!(power_id)
    );
    assert_eq!(rule["copper_clearance"]["margin_mm"], 0.01);
    assert_eq!(rule["copper_clearance"]["effective_mm"], 0.34);
    assert_eq!(rule["grid_pitch"]["source"]["kind"], "builtin_auto");
    assert_eq!(
        rule["grid_pitch"]["effective_mm"],
        automatic["endpoints"][0]["grid_pitch_mm"]
    );
    assert_eq!(
        diagnose(args(json!({"net":"B"})))["rules"]["trace_width"]["effective_mm"],
        0.42
    );

    let requested = diagnose(args(json!({
        "net":"A","trace_width":0.32,"via_drill":0.2,"via_size":0.5,"grid_pitch":0.15,"layers":["top"]
    })));
    let rule = &requested["rules"];
    for (key, effective) in [
        ("trace_width", 0.32),
        ("via_drill", 0.35),
        ("via_diameter", 0.81),
        ("grid_pitch", 0.15),
    ] {
        assert_eq!(rule[key]["effective_mm"], effective);
    }
    assert_eq!(rule["trace_width"]["base"]["source"]["kind"], "request");
    assert_eq!(rule["via_drill"]["base"]["mm"], 0.2);
    assert_eq!(rule["via_diameter"]["base"]["mm"], 0.5);
    assert_eq!(rule["trace_layers"]["effective"], json!(["top_cu"]));
}

#[test]
fn candidate_preserves_failed_request_and_unchanged_air_wire() {
    let temp = tempfile::tempdir().unwrap();
    let mut session = board_session(temp.path());
    let prepared = routing::routing_prepare(
        &mut session,
        args(json!({"nets": ["A"], "layers": ["bottom"]})),
    )
    .unwrap();
    assert_eq!(prepared.result["mutation_count"], 0);
    let report = &prepared.result["routing"];
    let failed = report["failed_requests"].as_array().unwrap();
    assert_eq!(report["unrouted"], 1);
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["net_name"], "A");
    assert_eq!(failed[0]["reason"], "no_access");
    assert!(failed[0].get("soft_reason").is_none());
    assert_eq!(report["remaining_air_wires"], report["failed_requests"]);
}

#[test]
fn reviewed_edit_preserves_anchors_and_rejects_new_clearance_errors() {
    use librepcb_mcp::tools::layout::board_get;

    let temp = tempfile::tempdir().unwrap();
    let mut session = board_session(temp.path());
    let board = board_get(&session, Default::default()).unwrap().result;
    let pad = |net: &str| {
        board["devices"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|device| device["pads"].as_array().unwrap())
            .find(|pad| pad["net"] == net)
            .unwrap()["position"]
            .clone()
    };
    let y = pad("A")["y"].as_f64().unwrap();
    let via = board_edit::via_add(
        &mut session,
        args(json!({
            "position":{"x":10.0,"y":y}, "net":"A", "drill":0.3,"size":0.6
        })),
    )
    .unwrap()
    .result["uuid"]
        .clone();
    board_edit::trace_add(
        &mut session,
        args(json!({
            "from":"R1.1","to":via,"points":[{"x":8.0,"y":y}],"width":0.2
        })),
    )
    .unwrap();
    board_edit::trace_add(
        &mut session,
        args(json!({
            "from":via,"to":"R2.1","width":0.2
        })),
    )
    .unwrap();
    let before = board_get(&session, Default::default()).unwrap().result;
    let junction = before["traces"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|trace| [&trace["from"], &trace["to"]])
        .find_map(|anchor| anchor["junction"].as_str())
        .unwrap();
    let prepare = |session: &mut Session, request| {
        let job = routing::prepare_edit_job(session, args(request)).unwrap();
        routing::finish_prepare(session, routing::run_job(job).unwrap())
            .unwrap()
            .result
    };
    let baseline_revision = session.project().unwrap().project().revision();
    let bad = prepare(
        &mut session,
        json!({
            "moves":[{"kind":"via","uuid":via,"position":pad("B")}]
        }),
    );
    assert_eq!(bad["validation"]["acceptable"], false, "{bad:#}");
    assert!(
        bad["validation"]["new_copper_error_count"]
            .as_u64()
            .unwrap()
            > 0
    );
    let error = routing::routing_apply(
        &mut session,
        args(json!({
            "candidate":bad["candidate"]
        })),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Refused);
    assert_eq!(
        session.project().unwrap().project().revision(),
        baseline_revision
    );
    assert_eq!(
        board_get(&session, Default::default()).unwrap().result,
        before
    );

    let planned = prepare(
        &mut session,
        json!({
            "vias":[{"uuid":via,"drill":0.3,"size":0.7}],
            "moves":[
                {"kind":"via","uuid":via,"position":{"x":10.25,"y":y}},
                {"kind":"junction","uuid":junction,"position":{"x":8.25,"y":y}}
            ]
        }),
    );
    assert_eq!(planned["validation"]["acceptable"], true, "{planned:#}");
    let preview = routing::routing_preview(
        &session,
        args(json!({
            "candidate":planned["candidate"]
        })),
    )
    .unwrap()
    .result;
    assert_eq!(preview["mutation_count"], 1);
    assert_eq!(
        board_get(&session, Default::default()).unwrap().result,
        before
    );
    let history = session.project().unwrap().editor.undo_stack().index();
    routing::routing_apply(
        &mut session,
        args(json!({"candidate":planned["candidate"]})),
    )
    .unwrap();
    let after = board_get(&session, Default::default()).unwrap().result;
    assert_eq!(after["vias"][0]["uuid"], via);
    assert_eq!(after["vias"][0]["size"], 0.7);
    assert_eq!(after["vias"][0]["position"]["x"], 10.25);
    assert_eq!(after["air_wires"], before["air_wires"]);
    for (a, b) in after["traces"]
        .as_array()
        .unwrap()
        .iter()
        .zip(before["traces"].as_array().unwrap())
    {
        assert_eq!(a["uuid"], b["uuid"]);
    }
    let mut ends = after["traces"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|trace| [&trace["from"], &trace["to"]]);
    assert!(ends.any(|end| end["junction"] == junction && end["position"]["x"] == 8.25));
    assert_eq!(
        session.project().unwrap().editor.undo_stack().index(),
        history + 1
    );
    write::undo_redo(&mut session, Default::default(), false).unwrap();
    assert_eq!(
        board_get(&session, Default::default()).unwrap().result,
        before
    );
    write::undo_redo(&mut session, Default::default(), true).unwrap();
    assert_eq!(
        board_get(&session, Default::default()).unwrap().result,
        after
    );
}

#[test]
fn reviewed_replacement_preserves_connections_and_rejects_branches_and_drc_errors() {
    use librepcb_mcp::tools::layout::board_get;

    let temp = tempfile::tempdir().unwrap();
    let mut session = board_session(temp.path());
    let layout = |session: &Session| board_get(session, Default::default()).unwrap().result;
    let board = layout(&session);
    let pads = board["devices"][0]["pads"].as_array().unwrap();
    let y = |net| {
        pads.iter().find(|pad| pad["net"] == net).unwrap()["position"]["y"]
            .as_f64()
            .unwrap()
    };
    let (y_a, y_b) = (y("A"), y("B"));
    let away = (y_a - y_b).signum();
    let add_trace = |session: &mut Session, request| {
        board_edit::trace_add(session, args(request))
            .unwrap()
            .result
    };
    let old = add_trace(
        &mut session,
        json!({
            "from":"R1.1","to":"R2.1","width":0.2,"points":[{"x":10,"y":y_a}]
        }),
    );
    add_trace(&mut session, json!({"from":"R1.2","to":"R2.2","width":0.2}));
    let traces: Vec<_> = old["traces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["uuid"].clone())
        .collect();
    let before = layout(&session);
    let junction = before["traces"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|trace| [&trace["from"], &trace["to"]])
        .find_map(|end| end.get("junction"))
        .unwrap();
    let request = |points| json!({"replacements":[{"traces":traces,"points":points}]});
    let prepare = |session: &mut Session, points| {
        let job = routing::prepare_edit_job(session, args(request(points)))?;
        let candidate = routing::run_job(job)?;
        routing::finish_prepare(session, candidate).map(|out| out.result)
    };
    let waypoints = json!([{"x":8,"y":y_a+away},{"x":12,"y":y_a+away}]);
    add_trace(
        &mut session,
        json!({
            "from":junction,"to":{"x":10,"y":y_a+away},"width":0.2
        }),
    );
    let branched = layout(&session);
    let error = prepare(&mut session, waypoints.clone()).unwrap_err();
    assert!(error.message.contains("branch"), "{error}");
    assert_eq!(layout(&session), branched);
    write::undo_redo(&mut session, Default::default(), false).unwrap();
    assert_eq!(layout(&session), before);
    let history = session.project().unwrap().editor.undo_stack().index();
    let revision = session.project().unwrap().project().revision();

    let collision = prepare(&mut session, json!([{"x":10,"y":y_b}])).unwrap();
    assert_eq!(
        collision["validation"]["acceptable"], false,
        "{collision:#}"
    );
    assert!(
        collision["validation"]["new_copper_error_count"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(
        routing::routing_apply(
            &mut session,
            args(json!({
                "candidate":collision["candidate"]
            }))
        )
        .unwrap_err()
        .kind,
        ErrorKind::Refused
    );
    assert_eq!(session.project().unwrap().project().revision(), revision);
    assert_eq!(layout(&session), before);

    let prepared = prepare(&mut session, waypoints).unwrap();
    assert_eq!(prepared["validation"]["acceptable"], true, "{prepared:#}");
    assert_eq!(prepared["validation"]["after"]["unrouted"], 0);
    let candidate = prepared["candidate"].clone();
    let preview = routing::routing_preview(&session, args(json!({"candidate":candidate})))
        .unwrap()
        .result;
    assert_eq!(preview["mutation_count"], 2);
    let patch: Vec<Mutation> = serde_json::from_value(preview["mutations"].clone()).unwrap();
    let [
        Mutation::Board(librepcb_core::project::BoardMutation::AddNetSegmentElements {
            elements,
            ..
        }),
        ..,
    ] = patch.as_slice()
    else {
        panic!("Preview must contain the replacement geometry");
    };
    let new_ids: Vec<_> = elements
        .traces
        .iter()
        .map(|trace| json!(trace.uuid()))
        .collect();
    assert_eq!(layout(&session), before);
    routing::routing_apply(&mut session, args(json!({"candidate":candidate}))).unwrap();
    let after = layout(&session);
    assert_eq!(after["air_wires"], json!([]));
    for trace in after["traces"].as_array().unwrap() {
        assert!(!traces.contains(&trace["uuid"]));
        if trace["net"] == "A" {
            assert!(new_ids.contains(&trace["uuid"]));
            assert_eq!(trace["segment"], old["segment"]);
            assert_eq!(trace["width"], 0.2);
            assert_eq!(trace["layer"], old["layer"]);
        } else {
            assert!(before["traces"].as_array().unwrap().contains(trace));
        }
    }
    assert_eq!(
        session.project().unwrap().editor.undo_stack().index(),
        history + 1
    );
    write::undo_redo(&mut session, Default::default(), false).unwrap();
    assert_eq!(layout(&session), before);
    write::undo_redo(&mut session, Default::default(), true).unwrap();
    assert_eq!(layout(&session), after);
}

#[test]
fn edit_and_route_freezes_both_patches_and_applies_them_together() {
    use librepcb_mcp::tools::layout::board_get;

    let temp = tempfile::tempdir().unwrap();
    let mut session = board_session(temp.path());
    let trace = board_edit::trace_add(
        &mut session,
        args(json!({"from":"R1.1","to":"R2.1","width":0.2})),
    )
    .unwrap()
    .result["traces"][0]
        .clone();
    let before = board_get(&session, Default::default()).unwrap().result;
    let y_a = trace["from"]["y"].as_f64().unwrap();
    let y_b = before["air_wires"][0]["from"]["position"]["y"]
        .as_f64()
        .unwrap();
    let revision = session.project().unwrap().project().revision();
    let history = session.project().unwrap().editor.undo_stack().index();
    let job = routing::prepare_edit_job(
        &session,
        args(json!({
            "expected_revision":revision,
            "replacements":[{"traces":[trace["uuid"]],"points":[{"x":10,"y":y_a+(y_a-y_b).signum()}]}],
            "route":{"nets":["B"],"layers":["top"],"trace_width":0.2}
        })),
    ).unwrap();
    let prepared = routing::finish_prepare(&mut session, routing::run_job(job).unwrap())
        .unwrap()
        .result;
    assert_eq!(prepared["validation"]["acceptable"], true, "{prepared:#}");
    assert_eq!(prepared["validation"]["before"]["unrouted"], 1);
    assert_eq!(prepared["validation"]["after"]["unrouted"], 0);
    assert_eq!(prepared["route"]["routing"]["routed"], 1);
    let candidate = prepared["candidate"].clone();
    let preview = routing::routing_preview(&session, args(json!({"candidate":candidate})))
        .unwrap()
        .result;
    assert!(preview["mutation_count"].as_u64().unwrap() > 2);
    assert_eq!(session.project().unwrap().project().revision(), revision);
    assert_eq!(
        board_get(&session, Default::default()).unwrap().result,
        before
    );

    let applied =
        routing::routing_apply(&mut session, args(json!({"candidate":candidate}))).unwrap();
    assert_eq!(applied.result["validation"]["after"]["unrouted"], 0);
    let after = board_get(&session, Default::default()).unwrap().result;
    let traces = after["traces"].as_array().unwrap();
    assert!(traces.iter().any(|t| t["net"] == "B"));
    assert_eq!(traces.iter().filter(|t| t["net"] == "A").count(), 2);
    assert!(traces.iter().all(|t| t["uuid"] != trace["uuid"]));
    assert_eq!(
        session.project().unwrap().editor.undo_stack().index(),
        history + 1
    );
    write::undo_redo(&mut session, Default::default(), false).unwrap();
    assert_eq!(
        board_get(&session, Default::default()).unwrap().result,
        before
    );
}

#[test]
fn regional_reroute_replaces_only_authorized_copper_and_preserves_connectivity() {
    use librepcb_mcp::tools::layout::board_get;

    let temp = tempfile::tempdir().unwrap();
    let mut session = board_session(temp.path());
    let layout = |session: &Session| board_get(session, Default::default()).unwrap().result;
    let board = layout(&session);
    let y = board["devices"][0]["pads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pad| pad["net"] == "A")
        .unwrap()["position"]["y"]
        .as_f64()
        .unwrap();
    let added = board_edit::trace_add(
        &mut session,
        args(json!({
            "from":"R1.1","to":"R2.1","width":0.2,"points":[{"x":8,"y":y},{"x":12,"y":y}]
        })),
    )
    .unwrap()
    .result;
    board_edit::trace_add(
        &mut session,
        args(json!({
            "from":"R1.2","to":"R2.2","width":0.2
        })),
    )
    .unwrap();
    let selected = &added["traces"][1]["uuid"];
    let before = layout(&session);
    let revision = session.project().unwrap().project().revision();
    let history = session.project().unwrap().editor.undo_stack().index();
    let request = |min_x, targets| {
        json!({
            "region":{"min":{"x":min_x,"y":y-0.6},"max":{"x":13,"y":y+0.6}},
            "route":{"nets":targets,"layers":["top"],"trace_width":0.2},
            "rip_up":[selected],
            "expected_revision":revision,
        })
    };
    for (min_x, targets, reason) in [(8.0, ["A"], "outside"), (7.0, ["B"], "target net")] {
        let job = routing::prepare_regional_job(&session, args(request(min_x, targets))).unwrap();
        let error = routing::run_job(job).unwrap_err();
        assert!(error.message.contains(reason), "{error}");
    }
    let job = routing::prepare_regional_job(&session, args(request(7.0, ["A"]))).unwrap();
    let prepared = routing::finish_prepare(&mut session, routing::run_job(job).unwrap())
        .unwrap()
        .result;
    assert_eq!(prepared["validation"]["acceptable"], true, "{prepared:#}");
    assert_eq!(prepared["validation"]["after"]["unrouted"], 0);
    let candidate = prepared["candidate"].clone();
    let preview = routing::routing_preview(&session, args(json!({"candidate":candidate})))
        .unwrap()
        .result;
    assert!(preview["mutation_count"].as_u64().unwrap() > 1);
    assert_eq!(layout(&session), before);
    assert_eq!(session.project().unwrap().project().revision(), revision);
    routing::routing_apply(&mut session, args(json!({"candidate":candidate}))).unwrap();
    let after = layout(&session);
    let traces = after["traces"].as_array().unwrap();
    assert_eq!(after["air_wires"], json!([]));
    assert!(traces.iter().all(|trace| trace["uuid"] != *selected));
    for trace in before["traces"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["uuid"] != *selected)
    {
        let next = traces.iter().find(|t| t["uuid"] == trace["uuid"]).unwrap();
        for key in ["from", "to", "width", "layer", "net"] {
            assert_eq!(trace[key], next[key]);
        }
    }
    assert_eq!(
        session.project().unwrap().editor.undo_stack().index(),
        history + 1
    );
    write::undo_redo(&mut session, Default::default(), false).unwrap();
    assert_eq!(layout(&session), before);
}

#[test]
fn reviewed_edit_rejects_losing_a_plane_connection() {
    use librepcb_mcp::tools::layout::board_get;

    let temp = tempfile::tempdir().unwrap();
    let mut session = board_session(temp.path());
    let board = board_get(&session, Default::default()).unwrap().result;
    let y = board["devices"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|device| device["pads"].as_array().unwrap())
        .find(|pad| pad["net"] == "A")
        .unwrap()["position"]["y"]
        .as_f64()
        .unwrap();
    let mut vias = Vec::new();
    for (pad, x) in [("R1.1", 7.0), ("R2.1", 13.0)] {
        let via = board_edit::via_add(
            &mut session,
            args(json!({
                "position":{"x":x,"y":y},"net":"A","drill":0.3,"size":0.6
            })),
        )
        .unwrap()
        .result["uuid"]
            .clone();
        board_edit::trace_add(
            &mut session,
            args(json!({
                "from":pad,"to":via,"layer":"top","width":0.2
            })),
        )
        .unwrap();
        vias.push(via);
    }
    board_edit::plane_add(
        &mut session,
        args(json!({
            "net":"A","layer":"bottom","connect_style":"solid","outline":[
                {"x":6.0,"y":y-1.0},{"x":14.0,"y":y-1.0},
                {"x":14.0,"y":y+1.0},{"x":6.0,"y":y+1.0}
            ]
        })),
    )
    .unwrap();
    let job = routing::prepare_edit_job(
        &session,
        args(json!({
            "moves":[{"kind":"via","uuid":vias[0],"position":{"x":5.5,"y":y}}]
        })),
    )
    .unwrap();
    let prepared = routing::finish_prepare(&mut session, routing::run_job(job).unwrap())
        .unwrap()
        .result;
    let validation = &prepared["validation"];
    assert_eq!(validation["before"]["unrouted"], 1, "{prepared:#}");
    assert_eq!(validation["after"]["unrouted"], 2, "{prepared:#}");
    assert_eq!(validation["new_copper_error_count"], 0, "{prepared:#}");
    assert_eq!(
        validation["unrouted_regressions"].as_array().unwrap().len(),
        1
    );
    assert_eq!(validation["acceptable"], false);
    let before = board_value(&session);
    let error = routing::routing_apply(
        &mut session,
        args(json!({
            "candidate":prepared["candidate"]
        })),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Refused);
    assert_eq!(board_value(&session), before);
}
