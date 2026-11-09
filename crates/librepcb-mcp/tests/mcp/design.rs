//! In-process test of the write tools (MCP phase 2) with the upstream
//! "Populated Library": a small RC circuit designed end to end through the
//! tool implementations (schematic, board, routing, plane, checks,
//! outputs), with undo/redo and revision checks, then opened with the
//! official `librepcb-cli` if available.

use std::path::Path;

use librepcb_mcp::tools::board_edit::{
    self, AutorouteArgs, BoardArgs, BoardSetOutlineArgs, DesignRulesSetArgs, DeviceAutoPlaceArgs,
    DevicePlaceArgs, PlaneAddArgs, RouterBackend, TraceRemoveArgs,
};
use librepcb_mcp::tools::circuit::{self, NetListArgs};
use librepcb_mcp::tools::library;
use librepcb_mcp::tools::output::{self, DrcArgs, ErcArgs, ExportFabricationArgs, JobsRunArgs};
use librepcb_mcp::tools::project::{self, ProjectCreateArgs, ProjectSaveArgs, WorkspacePathArgs};
use librepcb_mcp::tools::schematic_edit::{
    self, ComponentAddArgs, ComponentRemoveArgs, ComponentUpdateArgs, ConnectArgs, DisconnectArgs,
    NetClassSetArgs, NetRenameArgs, SchematicAddArgs, SchematicTidyArgs, SymbolMoveArgs,
};
use librepcb_mcp::tools::write::{self, UndoArgs};
use librepcb_mcp::units::PointMm;
use librepcb_mcp::{ErrorKind, Session, ToolOutput};
use serde_json::Value;

fn add(session: &mut Session, part: &str) -> ToolOutput {
    schematic_edit::component_add(
        session,
        ComponentAddArgs {
            part: part.into(),
            device: None,
            designator: None,
            value: None,
            schematic: None,
            position: None,
            rotation: None,
            mirror: false,
            expected_revision: None,
        },
    )
    .unwrap_or_else(|e| panic!("component_add {part}: {e}"))
}

fn connect(session: &mut Session, net: Option<&str>, pins: &[&str]) -> ToolOutput {
    schematic_edit::connect(
        session,
        ConnectArgs {
            net: net.map(Into::into),
            pins: pins.iter().map(|p| p.to_string()).collect(),
            max_wire_length: None,
            expected_revision: None,
        },
    )
    .unwrap_or_else(|e| panic!("connect {net:?}: {e}"))
}

fn designator(out: &ToolOutput) -> String {
    out.result["designator"].as_str().unwrap().to_owned()
}

fn revision(session: &Session) -> u64 {
    session.project.as_ref().unwrap().project().revision()
}

fn net_pins(session: &Session, net: &str) -> Vec<String> {
    let out = circuit::net_list(
        session,
        NetListArgs {
            net: Some(net.into()),
        },
    )
    .unwrap();
    out.result["nets"][0]["pins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_owned())
        .collect()
}

/// Whether the bounding boxes of the symbols of a schematic overlap
/// (checks the automatic symbol placement).
fn symbols_overlap(session: &Session) -> bool {
    let out = librepcb_mcp::tools::layout::schematic_get(session, Default::default()).unwrap();
    let positions: Vec<(f64, f64)> = out.result["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["position"]["x"].as_f64().unwrap(),
                s["position"]["y"].as_f64().unwrap(),
            )
        })
        .collect();
    positions.iter().enumerate().any(|(i, a)| {
        positions[i + 1..]
            .iter()
            .any(|b| (a.0 - b.0).abs() < 5.0 && (a.1 - b.1).abs() < 5.0)
    })
}

#[test]
fn design_rc_circuit_with_populated_library() {
    let tmp = tempfile::tempdir().unwrap();
    let mut session = Session::new();
    let ws = project::workspace_open(
        &mut session,
        WorkspacePathArgs {
            path: tmp.path().join("ws").to_string_lossy().into_owned(),
        },
        true,
    )
    .unwrap();
    crate::common::install_populated_library(Path::new(
        ws.result["libraries_path"].as_str().unwrap(),
    ));
    library::library_rescan(&session).unwrap();
    project::project_create(
        &mut session,
        ProjectCreateArgs {
            name: "RC Filter".into(),
            directory: None,
            author: None,
            create_schematic: true,
            schematic_name: None,
            create_board: true,
            board_name: None,
            eagle_schematic: None,
            eagle_board: None,
        },
    )
    .unwrap();
    assert!(!session.project.as_ref().unwrap().has_unsaved_changes());

    // Schematic: parts by device name, component name (with automatic
    // device choice) and supply symbols.
    let r1 = add(&mut session, "R-0805");
    assert_eq!(designator(&r1), "R1");
    assert_eq!(r1.result["devices"].as_array().unwrap().len(), 0);
    assert!(
        r1.result["library_elements_added"]
            .as_array()
            .unwrap()
            .len()
            >= 2
    );
    assert!(!r1.result["changes"].as_array().unwrap().is_empty());
    let c1 = add(&mut session, "Capacitor Bipolar");
    assert_eq!(designator(&c1), "C1");
    assert_eq!(c1.result["part"], "C-0805");
    let c2 = add(&mut session, "Capacitor Bipolar");
    assert_eq!(designator(&c2), "C2");
    let vcc = designator(&add(&mut session, "Supply VCC"));
    let gnd = designator(&add(&mut session, "Supply GND"));
    assert!(!symbols_overlap(&session), "symbols overlap");

    // Ambiguous and unknown parts.
    let err = schematic_edit::component_add(
        &mut session,
        ComponentAddArgs {
            part: "Diode".into(),
            device: None,
            designator: None,
            value: None,
            schematic: None,
            position: None,
            rotation: None,
            mirror: false,
            expected_revision: None,
        },
    );
    // "Diode" matches the component "Diode" exactly.
    assert!(err.is_ok(), "{err:?}");
    let d1 = designator(&err.unwrap());
    schematic_edit::component_remove(
        &mut session,
        ComponentRemoveArgs {
            component: d1,
            expected_revision: None,
        },
    )
    .unwrap();
    let err = schematic_edit::component_add(
        &mut session,
        ComponentAddArgs {
            part: "No Such Part".into(),
            device: None,
            designator: None,
            value: None,
            schematic: None,
            position: None,
            rotation: None,
            mirror: false,
            expected_revision: None,
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);

    // Nets: the supply symbols name their nets (a wrong name is replaced
    // by the forced one, with a warning).
    let out = connect(
        &mut session,
        Some("POWER"),
        &["R1.1", "C2.1", &format!("{vcc}.Net")],
    );
    assert_eq!(out.result["name"], "VCC");
    assert!(!out.warnings.is_empty());
    connect(&mut session, Some("OUT"), &["R1.2", "C1.1"]);
    // The unconnected GND symbol is attached to net "GND" automatically.
    let out = connect(&mut session, Some("GND"), &["C1.2", "C2.2"]);
    assert_eq!(out.result["name"], "GND");
    assert_eq!(
        out.result["attached_supplies"][0].as_str(),
        Some(gnd.as_str())
    );
    assert!(out.summary.contains(&gnd), "{}", out.summary);
    // A designator alone addresses the only pin of a supply symbol; other
    // components need the pin.
    let out = connect(&mut session, Some("GND"), &[&gnd]);
    assert_eq!(out.result["name"], "GND");
    let err = schematic_edit::connect(
        &mut session,
        ConnectArgs {
            net: Some("GND".into()),
            pins: vec!["R1".into()],
            max_wire_length: None,
            expected_revision: None,
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidArgument);
    assert!(
        err.message.contains("R1.<") || err.message.contains("signals"),
        "{}",
        err.message
    );
    assert_eq!(net_pins(&session, "OUT"), ["C1.1", "R1.2"]);
    assert_eq!(
        net_pins(&session, "GND"),
        ["C1.2", "C2.2", &format!("{gnd}.Net")]
    );
    let erc = output::erc_run(&mut session, ErcArgs::default()).unwrap();
    assert_eq!(erc.result["ran"], true);
    assert_eq!(erc.result["errors"], 0, "{}", erc.summary);
    assert_eq!(erc.result["warnings"], 0, "{}", erc.summary);

    // Value, rename, net class, symbol move.
    schematic_edit::component_update(
        &mut session,
        ComponentUpdateArgs {
            component: "R1".into(),
            designator: None,
            value: Some("1k".into()),
            attributes: Some([("TOLERANCE".to_owned(), Some("1%".to_owned()))].into()),
            expected_revision: None,
        },
    )
    .unwrap();
    let out = schematic_edit::net_rename(
        &mut session,
        NetRenameArgs {
            net: "OUT".into(),
            name: "FILTERED".into(),
            merge: false,
            expected_revision: None,
        },
    )
    .unwrap();
    assert_eq!(out.result["name"], "FILTERED");
    let out = schematic_edit::net_class_set(
        &mut session,
        NetClassSetArgs {
            net_class: "Power".into(),
            nets: vec!["VCC".into(), "GND".into()],
            default_trace_width: Some(0.5),
            default_via_drill: None,
            expected_revision: None,
        },
    )
    .unwrap();
    assert_eq!(out.result["nets"].as_array().unwrap().len(), 2);
    schematic_edit::symbol_move(
        &mut session,
        SymbolMoveArgs {
            component: "C1".into(),
            gate: None,
            position: None,
            rotation: Some(90.0),
            mirror: None,
            auto: false,
            expected_revision: None,
        },
    )
    .unwrap();
    // Tidy the page: nets unchanged, still ERC clean, no overlaps.
    let netlist_before = circuit::netlist(&session).unwrap().result;
    let out = schematic_edit::schematic_tidy(&mut session, SchematicTidyArgs::default()).unwrap();
    assert_eq!(out.result["supplies"], 2, "{}", out.summary);
    assert_eq!(circuit::netlist(&session).unwrap().result, netlist_before);
    assert!(!symbols_overlap(&session), "symbols overlap");
    let erc = output::erc_run(&mut session, ErcArgs::default()).unwrap();
    assert_eq!(erc.result["errors"], 0, "{}", erc.summary);
    assert_eq!(erc.result["warnings"], 0, "{}", erc.summary);

    let out = schematic_edit::schematic_add(
        &mut session,
        SchematicAddArgs {
            name: "Second".into(),
            expected_revision: None,
        },
    )
    .unwrap();
    assert_eq!(out.result["index"], 1);

    // Disconnect and undo it; stale revisions change nothing.
    let rev = revision(&session);
    schematic_edit::disconnect(
        &mut session,
        DisconnectArgs {
            pins: vec!["R1.2".into()],
            expected_revision: Some(rev),
        },
    )
    .unwrap();
    assert_eq!(net_pins(&session, "FILTERED"), ["C1.1"]);
    let err = schematic_edit::disconnect(
        &mut session,
        DisconnectArgs {
            pins: vec!["C1.1".into()],
            expected_revision: Some(rev),
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::StaleRevision);
    let out = write::undo_redo(&mut session, UndoArgs::default(), false).unwrap();
    assert_eq!(out.result["done"][0], "AI: disconnect");
    assert_eq!(net_pins(&session, "FILTERED"), ["C1.1", "R1.2"]);
    write::undo_redo(&mut session, UndoArgs::default(), true).unwrap();
    assert_eq!(net_pins(&session, "FILTERED"), ["C1.1"]);
    write::undo_redo(&mut session, UndoArgs::default(), false).unwrap();
    let history = write::history(&session).unwrap();
    let texts: Vec<&str> = history.result["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts[0], "AI: component_add");
    assert!(texts.contains(&"AI: connect"));
    assert_eq!(history.result["unsaved_changes"], true);

    // Board.
    board_edit::board_set_outline(
        &mut session,
        BoardSetOutlineArgs {
            width: Some(30.0),
            height: Some(20.0),
            corner_radius: Some(1.0),
            ..Default::default()
        },
    )
    .unwrap();
    board_edit::design_rules_set(
        &mut session,
        DesignRulesSetArgs {
            default_trace_width: Some(0.3),
            ..Default::default()
        },
    )
    .unwrap();
    let out = board_edit::device_auto_place(&mut session, DeviceAutoPlaceArgs::default()).unwrap();
    assert_eq!(out.result["placed"].as_array().unwrap().len(), 3, "{out:?}");
    assert_eq!(out.result["skipped"].as_array().unwrap().len(), 2);
    assert!(out.result["unrouted"].as_u64().unwrap() > 0);
    let out = board_edit::device_place(
        &mut session,
        DevicePlaceArgs {
            component: "C1".into(),
            board: None,
            position: Some(PointMm { x: 26.0, y: 4.0 }),
            rotation: Some(90.0),
            side: None,
            device: None,
            footprint: None,
            expected_revision: None,
        },
    )
    .unwrap();
    assert_eq!(out.result["rotation"], 90.0);
    let routed = board_edit::autoroute(
        &mut session,
        AutorouteArgs {
            backend: RouterBackend::Builtin,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(routed.result["unrouted"], 0, "{:#}", routed.result);
    let unrouted = board_edit::unrouted(&session, BoardArgs::default()).unwrap();
    assert_eq!(unrouted.result["count"], 0);
    // Remove and re-route one net.
    board_edit::trace_remove(
        &mut session,
        TraceRemoveArgs {
            nets: vec!["FILTERED".into()],
            ..Default::default()
        },
    )
    .unwrap();
    let unrouted = board_edit::unrouted(&session, BoardArgs::default()).unwrap();
    assert_eq!(unrouted.result["count"], 1);
    board_edit::autoroute(
        &mut session,
        AutorouteArgs {
            backend: RouterBackend::Builtin,
            nets: vec!["FILTERED".into()],
            ..Default::default()
        },
    )
    .unwrap();
    let err = board_edit::autoroute(
        &mut session,
        AutorouteArgs {
            backend: RouterBackend::Freerouting,
            nets: vec!["FILTERED".into()],
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidArgument);
    board_edit::plane_add(
        &mut session,
        PlaneAddArgs {
            net: "GND".into(),
            layer: Some("top".into()), // SMT only: the bottom has no GND pads
            outline: None,
            clearance: None,
            min_width: None,
            connect_style: None,
            priority: None,
            keep_islands: None,
            board: None,
            expected_revision: None,
        },
    )
    .unwrap();
    let planes = board_edit::planes_rebuild(&mut session, BoardArgs::default()).unwrap();
    assert_eq!(planes.result["planes"][0]["net"], "GND");
    assert!(planes.result["planes"][0]["fragments"].as_u64().unwrap() > 0);

    // Checks and outputs.
    let drc = output::drc_run(&mut session, DrcArgs::default()).unwrap();
    assert_eq!(drc.result["ran"], true);
    assert_eq!(drc.result["errors"], 0, "{}", drc.summary);
    let erc = output::erc_run(&mut session, ErcArgs::default()).unwrap();
    assert_eq!(erc.result["errors"], 0, "{}", erc.summary);
    let fab = output::export_fabrication(
        &mut session,
        ExportFabricationArgs {
            output_dir: Some(tmp.path().join("gerber").to_string_lossy().into_owned()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(fab.result["files"].as_array().unwrap().len() >= 8);
    // New projects have no output jobs (like upstream).
    let jobs = output::jobs_run(&mut session, JobsRunArgs::default()).unwrap();
    assert!(!jobs.warnings.is_empty(), "{jobs:?}");
    // For manual inspection: LIBREPCB_MCP_TEST_OUTPUT=<dir>.
    if let Some(out) = std::env::var_os("LIBREPCB_MCP_TEST_OUTPUT") {
        std::fs::create_dir_all(&out).unwrap();
        for (target, name) in [
            (output::RenderTarget::Schematic, "rc_schematic.png"),
            (output::RenderTarget::Board, "rc_board.png"),
        ] {
            let png = output::render(
                &session,
                output::RenderArgs {
                    target: Some(target),
                    schematic: None,
                    board: None,
                    side: None,
                    width: Some(1200),
                    height: Some(900),
                },
            )
            .unwrap();
            std::fs::write(Path::new(&out).join(name), &png.images[0]).unwrap();
        }
    }
    // Derived data does not make the project dirty; saving makes it clean.
    project::project_save(&mut session, ProjectSaveArgs::default()).unwrap();
    assert!(!session.project.as_ref().unwrap().has_unsaved_changes());
    let history = write::history(&session).unwrap();
    assert_eq!(history.result["unsaved_changes"], false);

    // Upstream opens the result: no ERC/DRC errors.
    let dir = Path::new(session.project.as_ref().unwrap().file_path().as_str())
        .parent()
        .unwrap()
        .to_owned();
    if let Some(cli) = crate::common::upstream_cli() {
        let lpp = Path::new(&session.project.as_ref().unwrap().file_path()).to_owned();
        let copy = tempfile::tempdir().unwrap();
        crate::common::copy_dir(&dir, copy.path());
        let _ = std::fs::remove_file(copy.path().join(".lock"));
        let lpp_copy = copy.path().join(lpp.file_name().unwrap());
        let (ok, text) = crate::common::run_cli(
            &cli,
            &[
                "open-project",
                "--erc",
                "--drc",
                "--export-pcb-fabrication-data",
                &lpp_copy.to_string_lossy(),
            ],
        );
        eprintln!("{text}");
        // Non-approved DRC warnings make the CLI exit with an error code.
        assert!(
            ok || text.contains("Finished with errors!"),
            "librepcb-cli failed:\n{text}"
        );
        assert!(
            !text.contains("[ERROR]") && !text.contains("ERROR:"),
            "{text}"
        );
        assert!(text.contains("COPPER-TOP.gbr"), "{text}");
    } else {
        eprintln!("librepcb-cli not found, skipping the upstream check");
    }

    let _: Value = serde_json::to_value(&history.result).unwrap();
}

#[test]
fn jobs_run_writes_the_output_job_files() {
    let tmp = tempfile::tempdir().unwrap();
    let lpp = crate::common::copy_test_project("Project With Two Boards", tmp.path());
    let mut session = Session::new();
    project::project_open(
        &mut session,
        project::ProjectOpenArgs {
            path: lpp.to_string_lossy().into_owned(),
        },
    )
    .unwrap();
    let out_dir = tmp.path().join("jobs");
    // Every output backend is installed, including graphics and mechanical STEP.
    let jobs = output::jobs_run(
        &mut session,
        JobsRunArgs {
            jobs: Vec::new(),
            output_dir: Some(out_dir.to_string_lossy().into_owned()),
        },
    )
    .unwrap();
    let files = jobs.result["files"].as_array().unwrap();
    assert!(!files.is_empty(), "{jobs:?}");
    let step_files: Vec<_> = files
        .iter()
        .filter_map(Value::as_str)
        .filter(|file| file.ends_with(".step") || file.ends_with(".stp"))
        .collect();
    assert!(!step_files.is_empty(), "MCP skipped STEP jobs: {jobs:?}");
    for file in step_files {
        let model = librepcb_step::StepModel::from_step(&std::fs::read(file).unwrap()).unwrap();
        assert!(model.solid_count() > 0, "{file}");
    }
    assert!(
        files
            .iter()
            .filter_map(Value::as_str)
            .any(|file| file.ends_with(".pdf")),
        "MCP skipped graphics jobs: {jobs:?}"
    );
    assert!(
        !jobs
            .warnings
            .iter()
            .any(|message| message.starts_with("Skipped the output job")),
        "{jobs:?}"
    );
    assert!(
        files
            .iter()
            .all(|f| Path::new(f.as_str().unwrap()).starts_with(&out_dir))
    );
    // Running jobs changes derived data only.
    assert!(
        !session.project.as_ref().unwrap().has_unsaved_changes() || {
            // The project may have been upgraded from an older file format.
            session.project.as_ref().unwrap().upgraded
        }
    );
    let err = output::jobs_run(
        &mut session,
        JobsRunArgs {
            jobs: vec!["No Such Job".into()],
            output_dir: None,
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
}
