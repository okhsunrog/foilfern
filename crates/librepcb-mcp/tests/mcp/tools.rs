//! In-process tests of the tool implementations on a session (no MCP
//! transport), using the upstream test data.

use std::path::Path;

use librepcb_mcp::tools::circuit::{self, ComponentGetArgs, NetListArgs};
use librepcb_mcp::tools::layout::{self, BoardGetArgs, SchematicGetArgs};
use librepcb_mcp::tools::library::{self, LibKind, LibraryElementArgs, LibrarySearchArgs};
use librepcb_mcp::tools::mutation::{self, MutationApplyArgs, MutationSchemaArgs};
use librepcb_mcp::tools::output::{
    self, ErcArgs, ExportBomArgs, ExportFabricationArgs, ExportNetlistArgs, ExportPickPlaceArgs,
    RenderArgs, RenderTarget, SideArg,
};
use librepcb_mcp::tools::project::{
    self, ProjectCloseArgs, ProjectCreateArgs, ProjectOpenArgs, ProjectSaveArgs, WorkspacePathArgs,
};
use librepcb_mcp::{ErrorKind, Session, resolve};
use serde_json::{Value, json};

fn open_project(dir: &Path, name: &str) -> (Session, std::path::PathBuf) {
    let lpp = crate::common::copy_test_project(name, dir);
    let mut session = Session::new();
    project::project_open(
        &mut session,
        ProjectOpenArgs {
            path: lpp.to_string_lossy().into_owned(),
        },
    )
    .unwrap();
    (session, lpp)
}

fn names(list: &Value, key: &str) -> Vec<String> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|v| v[key].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[test]
fn workspace_library_tools() {
    let tmp = tempfile::tempdir().unwrap();
    let ws_dir = tmp.path().join("workspace");
    let mut session = Session::new();

    // No workspace yet.
    let err = library::library_list(&session).unwrap_err();
    assert_eq!(err.kind, ErrorKind::NoWorkspace);

    let out = project::workspace_open(
        &mut session,
        WorkspacePathArgs {
            path: ws_dir.to_string_lossy().into_owned(),
        },
        true,
    )
    .unwrap();
    let libraries_path = out.result["libraries_path"].as_str().unwrap().to_owned();
    crate::common::install_populated_library(Path::new(&libraries_path));

    let out = library::library_rescan(&session).unwrap();
    assert_eq!(out.result["library_count"], 1);
    assert!(out.result["element_count"].as_u64().unwrap() > 10);

    let out = library::library_list(&session).unwrap();
    assert_eq!(
        names(&out.result["libraries"], "name"),
        ["Populated Library"]
    );
    assert_eq!(out.result["libraries"][0]["source"], "local");

    // Search devices.
    let out = library::library_search(
        &session,
        LibrarySearchArgs {
            query: "R-0805".into(),
            kind: Some(LibKind::Device),
            limit: None,
        },
    )
    .unwrap();
    let elements = out.result["elements"].as_array().unwrap();
    assert_eq!(elements.len(), 1, "{elements:#?}");
    let device = &elements[0];
    assert_eq!(device["kind"], "device");
    assert!(device["component"]["name"].is_string());
    assert!(device["package"]["name"].is_string());
    assert_eq!(device["match"], "whole");

    // Multi-word queries: all words (here: component name + device name).
    let out = library::library_search(
        &session,
        LibrarySearchArgs {
            query: "resistor 0805".into(),
            kind: Some(LibKind::Device),
            limit: None,
        },
    )
    .unwrap();
    let elements = out.result["elements"].as_array().unwrap();
    assert_eq!(elements[0]["uuid"], device["uuid"], "{elements:#?}");
    assert_eq!(elements[0]["match"], "all_words");

    // Device details: pad-signal map and footprints with pads.
    let out = library::library_element(
        &session,
        LibraryElementArgs {
            uuid: device["uuid"].as_str().unwrap().into(),
            kind: None,
        },
    )
    .unwrap();
    let d = &out.result;
    assert_eq!(d["kind"], "device");
    let pads = d["pads"].as_array().unwrap();
    assert_eq!(pads.len(), 2);
    assert!(pads.iter().all(|p| p["pad_name"].is_string()));
    assert!(pads.iter().all(|p| p["signal_name"].is_string()));
    // With the geometry of the default footprint.
    assert!(
        pads.iter()
            .all(|p| p["size"]["width"].as_f64().unwrap() > 0.0)
    );
    assert!(pads.iter().all(|p| p["position"]["x"].is_number()));
    assert!(pads.iter().all(|p| p["shape"].is_string()));
    let footprint_pads = d["footprints"][0]["pads"].as_array().unwrap();
    assert_eq!(footprint_pads.len(), 2);
    assert!(footprint_pads[0]["width"].as_f64().unwrap() > 0.0);
    assert!(footprint_pads[0]["position"]["x"].is_number());
    assert!(footprint_pads[0]["rotation"].is_number());
    assert!(footprint_pads[0]["height"].as_f64().unwrap() > 0.0);
    assert!(footprint_pads[0].get("drill").is_some());

    // Component details: signals and gates with pin names.
    let component = d["component"]["uuid"].as_str().unwrap().to_owned();
    let out = library::library_element(
        &session,
        LibraryElementArgs {
            uuid: component,
            kind: Some(LibKind::Component),
        },
    )
    .unwrap();
    let c = &out.result;
    assert_eq!(c["kind"], "component");
    assert!(!c["signals"].as_array().unwrap().is_empty());
    let gate = &c["symbol_variants"][0]["gates"][0];
    assert!(gate["symbol"]["name"].is_string());
    assert!(gate["pins"][0]["pin_name"].is_string());
    assert!(!c["devices"].as_array().unwrap().is_empty());

    // Unknown element.
    let err = library::library_element(
        &session,
        LibraryElementArgs {
            uuid: "00000000-0000-4000-8000-000000000000".into(),
            kind: None,
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);

    // New project in the workspace: like upstream's wizard.
    let out = project::project_create(
        &mut session,
        ProjectCreateArgs {
            name: "My Board".into(),
            directory: None,
            author: Some("Agent".into()),
            create_schematic: true,
            schematic_name: None,
            create_board: true,
            board_name: None,
            eagle_schematic: None,
            eagle_board: None,
        },
    )
    .unwrap();
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let lpp = ws_dir.join("projects/My_Board/My_Board.lpp");
    assert!(lpp.is_file(), "{}", out.summary);
    assert_eq!(out.result["name"], "My Board");
    assert_eq!(out.result["author"], "Agent");
    assert_eq!(names(&out.result["schematics"], "name"), ["Main"]);
    assert_eq!(names(&out.result["boards"], "name"), ["default"]);
    assert_eq!(out.result["unsaved_changes"], false);
    // The stroke font: from LIBREPCB_SHARE if set, else embedded into the
    // binary (identical to upstream's).
    let font =
        std::fs::read(ws_dir.join("projects/My_Board/resources/fontobene/newstroke.bene")).unwrap();
    if std::env::var_os("LIBREPCB_SHARE").is_none() {
        assert_eq!(
            font,
            std::fs::read(
                Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
                    .join("share/librepcb/fontobene/newstroke.bene")
            )
            .unwrap()
        );
    }
    let out = layout::board_get(&session, BoardGetArgs::default()).unwrap();
    let outline = &out.result["outlines"][0]["path"];
    assert_eq!(outline.as_array().unwrap().len(), 5);
    assert_eq!(outline[2], json!({ "x": 100.0, "y": 80.0 }));

    // A second project cannot be opened while one is open.
    let err = project::project_create(
        &mut session,
        ProjectCreateArgs {
            name: "Other".into(),
            directory: None,
            author: None,
            create_schematic: false,
            schematic_name: None,
            create_board: false,
            board_name: None,
            eagle_schematic: None,
            eagle_board: None,
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);
    project::project_close(&mut session, ProjectCloseArgs::default()).unwrap();

    // The created project opens in upstream LibrePCB.
    if let Some(cli) = crate::common::upstream_cli() {
        let (ok, output) =
            crate::common::run_cli(&cli, &["open-project", "--erc", lpp.to_str().unwrap()]);
        assert!(ok, "{output}");
    }
}

#[test]
fn project_read_tools() {
    let tmp = tempfile::tempdir().unwrap();
    let (session, _) = open_project(tmp.path(), "Nested Planes");

    let out = project::project_summary(&session).unwrap();
    assert_eq!(out.result["counts"]["components"], 9);
    assert_eq!(out.result["revision"], 0);
    assert_eq!(out.result["unsaved_changes"], false);

    let out = circuit::component_list(&session).unwrap();
    let designators = names(&out.result["components"], "designator");
    assert_eq!(
        designators,
        ["R1", "R2", "R3", "R4", "R5", "R6", "R7", "SUP1", "SUP2"]
    );

    let out = circuit::component_get(
        &session,
        ComponentGetArgs {
            component: "R1".into(),
        },
    )
    .unwrap();
    let c = &out.result;
    assert_eq!(c["designator"], "R1");
    let signals = c["signals"].as_array().unwrap();
    assert_eq!(signals.len(), 2);
    assert!(
        signals
            .iter()
            .all(|s| s["pin"].as_str().unwrap().starts_with("R1."))
    );
    assert_eq!(c["symbols"].as_array().unwrap().len(), 1);
    assert_eq!(c["symbols"][0]["pins"].as_array().unwrap().len(), 2);
    let device = &c["devices"][0];
    assert_eq!(device["side"], "top");
    let pads = device["pads"].as_array().unwrap();
    assert_eq!(pads.len(), 2);
    assert!(
        pads.iter()
            .all(|p| p["pad"].as_str().unwrap().starts_with("R1."))
    );

    // Case-insensitive designator and UUID addressing.
    let uuid = c["uuid"].as_str().unwrap().to_owned();
    for key in ["r1", uuid.as_str()] {
        let out = circuit::component_get(
            &session,
            ComponentGetArgs {
                component: key.into(),
            },
        )
        .unwrap();
        assert_eq!(out.result["designator"], "R1");
    }
    let err = circuit::component_get(
        &session,
        ComponentGetArgs {
            component: "X99".into(),
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);

    // Pin references by signal name and by pad name.
    let p = &session.project.as_ref().unwrap().project();
    let signal_name = signals[0]["name"].as_str().unwrap();
    let by_signal = resolve::pin(p, &format!("R1.{signal_name}")).unwrap();
    let pad_name = pads[0]["name"].as_str().unwrap();
    let by_pad = resolve::pin(p, &format!("R1.{pad_name}")).unwrap();
    assert_eq!(by_pad.signal.component, by_signal.signal.component);
    assert_eq!(
        resolve::pin(p, "R1.nonexistent").unwrap_err().kind,
        ErrorKind::NotFound
    );
    assert_eq!(
        resolve::pin(p, "R1").unwrap_err().kind,
        ErrorKind::InvalidArgument
    );
    assert_eq!(resolve::net(p, "GND").unwrap().1.name().as_str(), "GND");
    assert_eq!(resolve::board(p, Some("0")).unwrap().0, 0);
    assert_eq!(resolve::schematic(p, None).unwrap().0, 0);
    assert_eq!(
        resolve::board(p, Some("nope")).unwrap_err().kind,
        ErrorKind::NotFound
    );

    let out = circuit::net_list(
        &session,
        NetListArgs {
            net: Some("GND".into()),
        },
    )
    .unwrap();
    let gnd = &out.result["nets"][0];
    assert_eq!(gnd["name"], "GND");
    assert!(!gnd["pins"].as_array().unwrap().is_empty());
    assert!(gnd["planes"].as_u64().unwrap() > 0);

    let out = circuit::netlist(&session).unwrap();
    assert!(out.result["nets"]["VCC"].is_array());
    assert!(out.summary.contains("VCC: "));

    let out = layout::schematic_list(&session).unwrap();
    assert_eq!(out.result["schematics"].as_array().unwrap().len(), 1);
    let out = layout::schematic_get(&session, SchematicGetArgs::default()).unwrap();
    assert_eq!(out.result["symbols"].as_array().unwrap().len(), 9);
    let segments = out.result["net_segments"].as_array().unwrap();
    assert!(!segments.is_empty());
    let line = &segments
        .iter()
        .find(|s| !s["lines"].as_array().unwrap().is_empty())
        .unwrap()["lines"][0];
    assert!(line["from"]["position"]["x"].is_number());

    let out = layout::board_get(&session, BoardGetArgs::default()).unwrap();
    let b = &out.result;
    assert_eq!(b["copper_layers"], 2);
    assert_eq!(b["planes"].as_array().unwrap().len(), 9);
    assert!(!b["devices"].as_array().unwrap().is_empty());
    assert!(!b["traces"].as_array().unwrap().is_empty());
    assert!(!b["outlines"].as_array().unwrap().is_empty());
    let trace = &b["traces"][0];
    assert!(trace["from"]["position"]["x"].is_number());
    assert!(b["air_wires"].is_array());

    let out = output::jobs_list(&session).unwrap();
    assert!(out.result["jobs"].is_array());
}

#[test]
fn checks_exports_and_render() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut session, lpp) = open_project(tmp.path(), "Nested Planes");
    let project_dir = lpp.parent().unwrap().to_owned();

    let out = output::erc_run(&mut session, ErcArgs::default()).unwrap();
    assert_eq!(out.result["ran"], true);
    assert!(out.result["messages"].is_array());

    let gerber_dir = tmp.path().join("gerber");
    let out = output::export_fabrication(
        &mut session,
        ExportFabricationArgs {
            board: None,
            output_dir: Some(gerber_dir.to_string_lossy().into_owned()),
        },
    )
    .unwrap();
    let files = out.result["files"].as_array().unwrap();
    assert!(files.len() >= 8, "{files:?}");
    for f in files {
        assert!(Path::new(f.as_str().unwrap()).is_file(), "{f}");
        assert!(
            f.as_str()
                .unwrap()
                .starts_with(gerber_dir.to_str().unwrap())
        );
    }

    // The plane rebuild changes derived data only: nothing to save.
    assert!(!session.project.as_ref().unwrap().has_unsaved_changes());

    let out = output::export_bom(&session, ExportBomArgs::default()).unwrap();
    let bom = Path::new(out.result["file"].as_str().unwrap()).to_owned();
    assert!(bom.is_file());
    assert!(bom.starts_with(&project_dir));
    assert!(out.result["csv"].as_str().unwrap().contains("R1"));

    let pnp = tmp.path().join("pnp.csv");
    let out = output::export_pick_place(
        &session,
        ExportPickPlaceArgs {
            side: Some(SideArg::Top),
            output: Some(pnp.to_string_lossy().into_owned()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(pnp.is_file());
    assert!(out.result["csv"].as_str().unwrap().contains("R1"));

    let d356 = tmp.path().join("netlist.d356");
    output::export_netlist(
        &session,
        ExportNetlistArgs {
            board: None,
            output: Some(d356.to_string_lossy().into_owned()),
        },
    )
    .unwrap();
    assert!(d356.is_file());

    for target in [RenderTarget::Schematic, RenderTarget::Board] {
        let out = output::render(
            &session,
            RenderArgs {
                target: Some(target),
                width: Some(400),
                height: Some(300),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(out.images.len(), 1);
        assert!(out.images[0].starts_with(b"\x89PNG"));
    }
    // Without target, `side` or `board` select the board.
    let out = output::render(
        &session,
        RenderArgs {
            side: Some(SideArg::Bottom),
            width: Some(200),
            height: Some(150),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(out.summary.contains("board"), "{}", out.summary);
    let out = output::render(
        &session,
        RenderArgs {
            width: Some(200),
            height: Some(150),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(out.summary.contains("schematic"), "{}", out.summary);
    let err = output::export_bom(
        &session,
        ExportBomArgs {
            assembly_variant: Some("nope".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
}

#[test]
fn mutations_revisions_and_locking() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut session, lpp) = open_project(tmp.path(), "Nested Planes");

    // The examples of mutation_schema are valid Mutation JSON.
    let out = mutation::mutation_schema(&session, MutationSchemaArgs::default()).unwrap();
    for example in out.result["examples"].as_array().unwrap() {
        let m: librepcb_core::project::Mutation =
            serde_json::from_value(example["mutation"].clone()).unwrap();
        assert_eq!(serde_json::to_value(&m).unwrap(), example["mutation"]);
    }

    // Prefilled update of a net, renamed through mutation_apply.
    let out = mutation::mutation_schema(
        &session,
        MutationSchemaArgs {
            entity: Some("N1".into()),
        },
    )
    .unwrap();
    assert_eq!(out.result["entity"]["kind"], "net");
    let mut update = out.result["entity"]["update_mutation"].clone();
    update["UpdateNetSignal"]["name"] = json!("MY_NET");
    update["UpdateNetSignal"]["has_auto_name"] = json!(false);

    // Wrong revision: nothing changes.
    let err = mutation::mutation_apply(
        &mut session,
        MutationApplyArgs {
            mutation: update.clone(),
            expected_revision: Some(42),
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::StaleRevision);

    // Malformed mutation.
    let err = mutation::mutation_apply(
        &mut session,
        MutationApplyArgs {
            mutation: json!({ "NoSuchMutation": 1 }),
            expected_revision: None,
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidArgument);

    // Removing a used net fails (validated by the core, nothing changes).
    let gnd = circuit::net_list(
        &session,
        NetListArgs {
            net: Some("GND".into()),
        },
    )
    .unwrap();
    let err = mutation::mutation_apply(
        &mut session,
        MutationApplyArgs {
            mutation: json!({ "RemoveNetSignal": gnd.result["nets"][0]["uuid"] }),
            expected_revision: None,
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict, "{err}");
    assert_eq!(session.project.as_ref().unwrap().project().revision(), 0);

    let out = mutation::mutation_apply(
        &mut session,
        MutationApplyArgs {
            mutation: update,
            expected_revision: Some(0),
        },
    )
    .unwrap();
    assert!(!out.result["changes"].as_array().unwrap().is_empty());
    let open = session.project.as_ref().unwrap();
    assert!(open.project().revision() > 0);
    assert_eq!(open.editor.undo_stack().history().len(), 1);
    assert!(open.has_unsaved_changes());
    let out = circuit::net_list(
        &session,
        NetListArgs {
            net: Some("MY_NET".into()),
        },
    )
    .unwrap();
    assert_eq!(out.result["nets"][0]["auto_name"], false);

    // The project is locked: a second session cannot open it.
    let mut other = Session::new();
    let err = project::project_open(
        &mut other,
        ProjectOpenArgs {
            path: lpp.to_string_lossy().into_owned(),
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict, "{err}");

    // Closing with unsaved changes needs discard or save.
    let err = project::project_close(&mut session, ProjectCloseArgs::default()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);
    let err = project::project_save(
        &mut session,
        ProjectSaveArgs {
            expected_revision: Some(0),
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::StaleRevision);
    project::project_save(&mut session, ProjectSaveArgs::default()).unwrap();
    project::project_close(&mut session, ProjectCloseArgs::default()).unwrap();
    assert_eq!(
        project::project_summary(&session).unwrap_err().kind,
        ErrorKind::NoProject
    );

    // After closing, the lock is released and the change is on disk.
    project::project_open(
        &mut other,
        ProjectOpenArgs {
            path: lpp.parent().unwrap().to_string_lossy().into_owned(),
        },
    )
    .unwrap();
    let out = circuit::net_list(
        &other,
        NetListArgs {
            net: Some("MY_NET".into()),
        },
    )
    .unwrap();
    assert_eq!(out.result["nets"].as_array().unwrap().len(), 1);
}

/// `board_add` with `copy_from` copies a board with all its items.
#[test]
fn board_add_copy() {
    use librepcb_mcp::tools::board_edit::{self, BoardAddArgs};
    let tmp = tempfile::tempdir().unwrap();
    let (mut session, _) = open_project(tmp.path(), "Nested Planes");
    let p = session.project().unwrap().project();
    let original = p.boards()[0].name().to_string();
    let planes = p.boards()[0].planes().len();
    assert!(planes > 0);
    let out = board_edit::board_add(
        &mut session,
        BoardAddArgs {
            name: "copy".into(),
            copy_from: Some(original),
            expected_revision: None,
        },
    )
    .unwrap();
    assert_eq!(out.result["planes"], planes);
    let p = session.project().unwrap().project();
    assert_eq!(p.boards().len(), 2);
    assert_eq!(p.boards()[1].devices(), p.boards()[0].devices());
    let err = board_edit::board_add(
        &mut session,
        BoardAddArgs {
            name: "other".into(),
            copy_from: Some("no such board".into()),
            expected_revision: None,
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound, "{err:?}");
}

#[test]
fn migrated_project_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let lpp = crate::common::copy_test_project("v0.1", tmp.path());
    let mut session = Session::new();
    let out = project::project_open(
        &mut session,
        ProjectOpenArgs {
            path: lpp.to_string_lossy().into_owned(),
        },
    )
    .unwrap();
    assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
    assert_eq!(out.result["unsaved_changes"], true);
    let out = project::server_info(&session).unwrap();
    assert_eq!(out.result["project"]["unsaved_changes"], true);
    project::project_close(
        &mut session,
        ProjectCloseArgs {
            discard_changes: true,
        },
    )
    .unwrap();
}
