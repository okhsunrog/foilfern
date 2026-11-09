//! End-to-end design from the official LibrePCB libraries through the
//! workspace library database: a 2-pin connector powering a LED with a
//! series resistor, with supply symbols (forced net names), autorouted,
//! with a GND plane, checked by ERC and DRC, exported and opened with the
//! official `librepcb-cli`.
//!
//! The libraries ("LibrePCB Base" and "LibrePCB Connectors") are taken
//! from the directory in `LIBREPCB_TEST_LIBRARIES_DIR` (containing the
//! `*.lplib` directories, e.g. git clones of
//! `https://github.com/LibrePCB-Libraries/LibrePCB_Base.lplib`); the test
//! is skipped if it is not set.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use librepcb_core::fileio::FilePath;
use librepcb_core::project::board::{ExportInfo, export_fabrication_data};
use librepcb_core::project::erc::run_erc;
use librepcb_core::project::{LibraryElementKind, Project};
use librepcb_core::rule_check::Severity;
use librepcb_core::types::{Angle, ElementName, Layer, Uuid};
use librepcb_core::workspace::LibraryDb;
use librepcb_editor::commands::*;
use librepcb_editor::{LibraryElementSource, ProjectEditor};

use crate::autoroute_test::{air_wire_count, check_cli_without_errors, run_drc};
use crate::helpers::{create_project, mm};

fn uuid(s: &str) -> Uuid {
    s.parse().unwrap()
}

// LibrePCB Base.
const RESISTOR: &str = "ef80cd5e-2689-47ee-8888-31d04fc99174";
const RESISTOR_0805: &str = "078650d3-483c-4b9e-a848-b14f1aad2edc";
const LED: &str = "2b24b18d-bd95-4fb4-8fe6-bce1d020ead4";
const LED_3MM_RED: &str = "bc3f8aef-328a-449f-9922-5ede7afb0b65";
const SUPPLY_GND: &str = "8076f6be-bfab-4fc1-9772-5d54465dd7e1";
const SUPPLY_VCC: &str = "58c3c6cd-11eb-4557-aa3f-d3e05874afde";
// LibrePCB Connectors.
const PIN_HEADER_1X02: &str = "ab344e7c-b740-4f2a-b21a-f7b1c433f990";
const PIN_HEADER_1X02_DEVICE: &str = "9ac8e65e-83a7-42c7-9ebe-8017995828f3";

/// Creates a workspace library database in `dir` with (symbolic links to)
/// all libraries of `LIBREPCB_TEST_LIBRARIES_DIR` and scans it.
fn workspace_library_db(dir: &Path) -> Option<LibraryDb> {
    let source = PathBuf::from(std::env::var_os("LIBREPCB_TEST_LIBRARIES_DIR")?);
    let local = dir.join("local");
    std::fs::create_dir_all(&local).unwrap();
    for entry in std::fs::read_dir(&source).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "lplib") {
            #[cfg(unix)]
            std::os::unix::fs::symlink(&path, local.join(path.file_name().unwrap())).unwrap();
            #[cfg(not(unix))]
            return None;
        }
    }
    let db = LibraryDb::open(&FilePath::new(dir).unwrap()).unwrap();
    db.rescan(&AtomicBool::new(false), &mut |_| {}).unwrap();
    Some(db)
}

fn net_name_of_pin(p: &Project, component: &str, signal: &str) -> String {
    let (_, c) = p.circuit().component_instance_by_name(component).unwrap();
    let lib = p.library().component(&c.lib_component()).unwrap();
    let signal = lib.signals().by_name(signal, true).unwrap().uuid();
    let net = c.signal(&signal).unwrap().net().unwrap();
    p.circuit().net_signal(net).unwrap().name().to_string()
}

#[test]
fn test_led_board_from_official_libraries() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(db) = workspace_library_db(&tmp.path().join("libraries")) else {
        eprintln!("LIBREPCB_TEST_LIBRARIES_DIR not set, skipping");
        return;
    };
    let missing: Vec<_> = [
        (LibraryElementKind::Component, RESISTOR),
        (LibraryElementKind::Device, RESISTOR_0805),
        (LibraryElementKind::Component, LED),
        (LibraryElementKind::Device, LED_3MM_RED),
        (LibraryElementKind::Component, SUPPLY_GND),
        (LibraryElementKind::Component, SUPPLY_VCC),
        (LibraryElementKind::Component, PIN_HEADER_1X02),
        (LibraryElementKind::Device, PIN_HEADER_1X02_DEVICE),
    ]
    .into_iter()
    .filter(|(kind, id)| db.element_directory(*kind, &uuid(id)).is_none())
    .collect();
    if !missing.is_empty() {
        eprintln!("Libraries incomplete ({missing:?} missing), skipping");
        return;
    }
    let dir = tmp.path().join("project");
    let mut editor = ProjectEditor::with_source(create_project(&dir), Arc::new(db));

    // Schematic.
    let schematic = editor
        .execute(AddSchematic {
            name: ElementName::new("Main").unwrap(),
            index: None,
        })
        .unwrap();
    let add = |editor: &mut ProjectEditor, cmp: &str, dev: Option<&str>, x, y, rotation| {
        editor
            .execute(AddComponent {
                device: dev.map(uuid),
                place: Some(SymbolPlacement {
                    schematic: Some(schematic),
                    position: mm(x, y),
                    rotation,
                    mirrored: false,
                    gate_offset: None,
                }),
                ..AddComponent::new(uuid(cmp))
            })
            .unwrap()
            .name
    };
    let j1 = add(
        &mut editor,
        PIN_HEADER_1X02,
        Some(PIN_HEADER_1X02_DEVICE),
        20.32,
        50.8,
        Angle::DEG0,
    );
    let r1 = add(
        &mut editor,
        RESISTOR,
        Some(RESISTOR_0805),
        45.72,
        60.96,
        Angle::DEG0,
    );
    let d1 = add(
        &mut editor,
        LED,
        Some(LED_3MM_RED),
        66.04,
        50.8,
        Angle::DEG270,
    );
    let vcc = add(&mut editor, SUPPLY_VCC, None, 30.48, 71.12, Angle::DEG0);
    let gnd1 = add(&mut editor, SUPPLY_GND, None, 30.48, 35.56, Angle::DEG0);
    let gnd2 = add(&mut editor, SUPPLY_GND, None, 66.04, 35.56, Angle::DEG0);
    assert_eq!(
        [&j1, &r1, &d1, &vcc, &gnd1, &gnd2],
        ["J1", "R1", "LED1", "VCC1", "GND1", "GND2"]
    );
    editor
        .execute(EditComponent {
            component: r1.as_str().into(),
            name: None,
            value: Some("330R".into()),
            attributes: None,
            assembly_options: None,
            lock_assembly: None,
        })
        .unwrap();

    let connect = |editor: &mut ProjectEditor, a: (&str, &str), b: (&str, &str)| {
        editor
            .execute(ConnectPins {
                a: PinRef::new(a.0, a.1),
                b: PinRef::new(b.0, b.1),
                mode: WireMode::HV,
                net: None,
            })
            .unwrap()
    };
    // Forced net names of the supply symbols name the nets.
    let w = connect(&mut editor, (&vcc, "Net"), (&j1, "1"));
    assert_eq!(
        editor
            .project()
            .circuit()
            .net_signal(w.net)
            .unwrap()
            .name()
            .as_str(),
        "VCC"
    );
    connect(&mut editor, (&r1, "1"), (&j1, "1"));
    connect(&mut editor, (&r1, "2"), (&d1, "A"));
    connect(&mut editor, (&d1, "C"), (&gnd2, "Net"));
    // The second GND symbol joins the existing GND net.
    connect(&mut editor, (&j1, "2"), (&gnd1, "Net"));
    let p = editor.project();
    assert_eq!(net_name_of_pin(p, &j1, "1"), "VCC");
    assert_eq!(net_name_of_pin(p, &r1, "1"), "VCC");
    assert_eq!(net_name_of_pin(p, &j1, "2"), "GND");
    assert_eq!(net_name_of_pin(p, &d1, "C"), "GND");
    assert_eq!(p.circuit().net_signals().len(), 3);
    let erc = run_erc(p);
    for m in &erc {
        eprintln!(
            "ERC: [{:?}] {}",
            m.message().severity(),
            m.message().message()
        );
    }
    assert!(
        erc.iter()
            .all(|m| m.message().severity() != Severity::Error),
        "ERC errors"
    );

    // Board.
    let board = editor
        .execute(AddBoard::new(ElementName::new("default").unwrap()))
        .unwrap()
        .board;
    editor
        .execute(SetBoardOutline::rect(
            Some(board),
            mm(0.0, 0.0),
            mm(32.0, 20.0),
        ))
        .unwrap();
    for (component, x, y, rotation) in [
        (&j1, 5.0, 10.0, Angle::DEG90),
        (&r1, 16.0, 14.0, Angle::DEG0),
        (&d1, 26.0, 10.0, Angle::DEG270),
    ] {
        editor
            .execute(AddDevice {
                component: component.as_str().into(),
                board: Some(board),
                device: None,
                footprint: None,
                position: mm(x, y),
                rotation,
                mirrored: false,
            })
            .unwrap();
    }
    let routed = editor.execute(Autoroute::default()).unwrap();
    eprintln!("Autoroute: {routed:?}");
    assert_eq!(routed.unrouted, 0, "{routed:?}");
    assert!(routed.remaining_air_wires.is_empty(), "{routed:?}");
    editor
        .execute(AddPlane {
            board: Some(board),
            net: Some("GND".into()),
            layer: Some(Layer::BOT_COPPER),
            outline: None,
            settings: PlaneSettings::default(),
        })
        .unwrap();
    editor.save().unwrap();

    // Checks and outputs on the saved project.
    let mut project = editor.into_project();
    let drc = run_drc(&mut project, board);
    for (severity, message) in &drc {
        eprintln!("DRC: [{severity:?}] {message}");
    }
    assert!(
        drc.iter().all(|(s, _)| *s != Severity::Error),
        "DRC errors: {drc:#?}"
    );
    assert_eq!(air_wire_count(&project, board), 0);
    let files =
        export_fabrication_data(&mut project, board, None, &ExportInfo::now("test")).unwrap();
    assert!(files.len() >= 8, "{files:?}");
    assert!(files.iter().all(|f| f.is_existing_file()));

    // For manual inspection: LIBREPCB_EDITOR_TEST_OUTPUT=<dir>.
    if let Some(out) = std::env::var_os("LIBREPCB_EDITOR_TEST_OUTPUT") {
        crate::helpers::copy_dir(&dir, Path::new(&out));
    }
    if let Some(output) =
        check_cli_without_errors(&dir, &["--erc", "--drc", "--export-pcb-fabrication-data"])
    {
        eprintln!("librepcb-cli:\n{output}");
    }
}

#[test]
fn test_workspace_library_db_source() {
    let tmp = tempfile::tempdir().unwrap();
    let libraries = tmp.path().join("libraries");
    let local = libraries.join("local");
    std::fs::create_dir_all(&local).unwrap();
    crate::helpers::copy_dir(
        &crate::helpers::test_data_dir().join("libraries/Populated Library.lplib"),
        &local.join("Populated Library.lplib"),
    );
    let db = LibraryDb::open(&FilePath::new(&libraries).unwrap()).unwrap();
    db.rescan(&AtomicBool::new(false), &mut |_| {}).unwrap();
    let resistor = crate::helpers::lib::resistor();
    let dir = db
        .element_directory(LibraryElementKind::Component, &resistor)
        .unwrap();
    assert!(dir.is_located_in_dir(&FilePath::new(&local).unwrap()));
    assert!(
        db.element_directory(LibraryElementKind::Device, &Uuid::new_random())
            .is_none()
    );

    // Adding a component through an editor with the database as source.
    let project = create_project(&tmp.path().join("project"));
    let mut editor = ProjectEditor::with_source(project, Arc::new(db));
    let added = editor.execute(AddComponent::new(resistor)).unwrap();
    assert_eq!(added.name, "R1");
    assert_eq!(added.library_elements.len(), 1);
}
