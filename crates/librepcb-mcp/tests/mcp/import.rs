//! Tests of `library_import` and the EAGLE project import of
//! `project_create` (upstream test data).

use std::path::Path;

use librepcb_mcp::tools::import::{self, ImportFormat, LibraryImportArgs};
use librepcb_mcp::tools::library::{self, LibKind, LibrarySearchArgs};
use librepcb_mcp::tools::project::{self, ProjectCreateArgs, WorkspacePathArgs};
use librepcb_mcp::{ErrorKind, Session};

use crate::common::test_data_dir;

fn data(path: &str) -> String {
    test_data_dir()
        .join("unittests")
        .join(path)
        .to_string_lossy()
        .into_owned()
}

fn args(format: ImportFormat, source: &str) -> LibraryImportArgs {
    LibraryImportArgs {
        format,
        source: data(source),
        shapes_3d: None,
        library: None,
        dry_run: true,
        elements: None,
        add_name_prefix: None,
        add_to_category: None,
        component_category: None,
        package_category: None,
    }
}

/// A session with a new workspace containing the "Populated Library".
fn workspace(dir: &Path) -> Session {
    let mut session = Session::new();
    let out = project::workspace_open(
        &mut session,
        WorkspacePathArgs {
            path: dir.join("workspace").to_string_lossy().into_owned(),
        },
        true,
    )
    .unwrap();
    let libraries_path = out.result["libraries_path"].as_str().unwrap().to_owned();
    crate::common::install_populated_library(Path::new(&libraries_path));
    library::library_rescan(&session).unwrap();
    session
}

#[test]
fn eagle_library_import() {
    let tmp = tempfile::tempdir().unwrap();
    let session = workspace(tmp.path());

    // Dry run: everything selected by default, nothing written.
    let out = import::library_import(
        &session,
        args(ImportFormat::Eagle, "eagleimport/resistor.lbr"),
    )
    .unwrap();
    let count = out.result["element_count"].as_u64().unwrap();
    assert_eq!(count, out.result["selected_count"].as_u64().unwrap());
    let elements = out.result["elements"].as_array().unwrap();
    let device = elements.iter().find(|e| e["kind"] == "device").unwrap()["name"]
        .as_str()
        .unwrap()
        .to_owned();

    // Unknown elements and libraries.
    let err = import::library_import(
        &session,
        LibraryImportArgs {
            elements: Some(vec!["nope".into()]),
            ..args(ImportFormat::Eagle, "eagleimport/resistor.lbr")
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    let err = import::library_import(
        &session,
        LibraryImportArgs {
            library: Some("Unknown Library".into()),
            dry_run: false,
            ..args(ImportFormat::Eagle, "eagleimport/resistor.lbr")
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);

    // Import one device (with its dependencies) into a workspace library.
    let out = import::library_import(
        &session,
        LibraryImportArgs {
            library: Some("Populated Library".into()),
            dry_run: false,
            elements: Some(vec![device.clone()]),
            add_name_prefix: Some(true),
            ..args(ImportFormat::Eagle, "eagleimport/resistor.lbr")
        },
    )
    .unwrap();
    // Device, component, package and symbol (categories are not counted).
    assert_eq!(out.result["imported"], 4, "{}", out.summary);
    assert_eq!(out.result["failed"], 0);
    assert!(out.result["index_element_count"].as_u64().is_some());

    // The imported device is in the index.
    let out = library::library_search(
        &session,
        LibrarySearchArgs {
            query: "EAGLE".into(),
            kind: Some(LibKind::Device),
            limit: None,
        },
    )
    .unwrap();
    let names: Vec<&str> = out.result["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert!(names.contains(&"EAGLE_R-EU_R0805"), "{names:?}");
}

#[test]
fn kicad_library_import() {
    let tmp = tempfile::tempdir().unwrap();
    let session = workspace(tmp.path());

    let out = import::library_import(&session, args(ImportFormat::Kicad, "kicadimport")).unwrap();
    // 3 symbols and 3 footprints.
    assert_eq!(6, out.result["element_count"], "{}", out.result);

    // Target library by path.
    let lib = tmp
        .path()
        .join("workspace/data/libraries/local/Populated Library.lplib");
    let out = import::library_import(
        &session,
        LibraryImportArgs {
            library: Some(lib.to_string_lossy().into_owned()),
            dry_run: false,
            add_name_prefix: Some(true),
            ..args(ImportFormat::Kicad, "kicadimport")
        },
    )
    .unwrap();
    assert_eq!(out.result["failed"], 0, "{:?}", out.warnings);
    assert_eq!(out.result["imported"], out.result["total"]);
    assert!(out.result["imported"].as_u64().unwrap() > 6);

    // Importing again: the elements are known from the index now.
    let out = import::library_import(&session, args(ImportFormat::Kicad, "kicadimport")).unwrap();
    let elements = out.result["elements"].as_array().unwrap();
    assert!(
        elements.iter().all(|e| e["already_imported"] == true),
        "{elements:?}"
    );
}

#[test]
fn eagle_project_import() {
    let tmp = tempfile::tempdir().unwrap();
    let mut session = workspace(tmp.path());
    let out = project::project_create(
        &mut session,
        ProjectCreateArgs {
            name: "Imported".into(),
            directory: None,
            author: None,
            create_schematic: true,
            schematic_name: None,
            create_board: true,
            board_name: None,
            eagle_schematic: Some(data("eagleimport/testproject/testproject.sch")),
            eagle_board: Some(data("eagleimport/testproject/testproject.brd")),
            kicad_schematic: None,
            kicad_board: None,
        },
    )
    .unwrap();
    let open = session.project().unwrap();
    assert_eq!(1, open.project().schematics().len(), "{}", out.summary);
    assert_eq!(1, open.project().boards().len());
    assert_eq!(4, open.project().circuit().buses().len());
    assert!(!open.has_unsaved_changes());
}

#[test]
fn kicad_project_import_preserves_numbered_unnamed_pins_and_direct_labels() {
    use librepcb_mcp::tools::{circuit, output};
    use serde_json::json;

    let temp = tempfile::tempdir().unwrap();
    let schematic = temp.path().join("bench.kicad_sch");
    std::fs::write(
        &schematic,
        r#"
(kicad_sch (version 20260306) (generator "eeschema")
  (lib_symbols
    (symbol "Fixture:Part"
      (property "Reference" "U" (at 0 0 0) (effects (font (size 1 1))))
      (symbol "Part_1_1"
        (pin passive line (at 0 2 270) (length 1)
          (name "" (effects (font (size 1 0)))) (number "1" (effects (font (size 1 0.8)))))
        (pin passive line (at 0 -2 90) (length 1)
          (name "" (effects (font (size 1 1)))) (number "2" (effects (font (size 1 1))))))))
  (symbol (lib_id "Fixture:Part") (at 20 20 270) (mirror x) (unit 1)
    (property "Reference" "U1" (at 20 20 90) (effects (font (size 1 1)))))
  (symbol (lib_id "Fixture:Part") (at 40 20 0) (unit 1) (dnp yes)
    (property "Reference" "U2" (at 40 20 0) (effects (font (size 1 1)))))
  (global_label "LINK" (shape input) (at 18 20 0) (effects (font (size 1 1))))
  (global_label "LINK" (shape input) (at 40 18 0) (effects (font (size 1 1))))
  (no_connect (at 22 20))
  (no_connect (at 40 22)))
"#,
    )
    .unwrap();
    let mut session = Session::new();
    let request = serde_json::from_value(json!({
        "name":"KiCad test", "directory":temp.path().join("native"),
        "kicad_schematic":schematic, "create_board":false
    }))
    .unwrap();
    let created = project::project_create(&mut session, request).unwrap();
    assert!(created.warnings.is_empty(), "{:?}", created.warnings);
    let netlist = circuit::netlist(&session).unwrap().result;
    assert_eq!(netlist["nets"], json!({"LINK":["U1.2","U2.1"]}));
    assert_eq!(netlist["unconnected_pins"], json!(["U1.1", "U2.2"]));
    let open = session.project().unwrap();
    let native = open.project();
    let sheet = &native.schematics()[0];
    let appearance =
        librepcb_core::project::schematic::import_appearance::ImportAppearance::load(native, sheet);
    assert_eq!(appearance.global_labels.len(), 2);
    assert_eq!(
        appearance
            .no_connects
            .values()
            .map(|pins| pins.len())
            .sum::<usize>(),
        2
    );
    assert_eq!(appearance.unpopulated.len(), 1);
    assert_eq!(
        appearance
            .pin_numbers
            .values()
            .map(|pins| pins.len())
            .sum::<usize>(),
        4
    );
    for (symbol, pins) in &appearance.pin_numbers {
        for (pin, number) in pins {
            let heights = appearance.pin_text_heights(*symbol, *pin);
            let (name_height, number_height) = match number.as_str() {
                "1" => (0, 1_200_000),
                "2" => (1_500_000, 1_500_000),
                other => panic!("unexpected pin number {other}"),
            };
            assert_eq!(heights.name.unwrap().to_nm(), name_height);
            assert_eq!(heights.number.unwrap().to_nm(), number_height);
        }
    }
    let rotated = sheet
        .symbols()
        .values()
        .find(|symbol| symbol.name(native.view()).unwrap() == "U1")
        .unwrap();
    assert_eq!(
        rotated.texts().values().next().unwrap().rotation(),
        librepcb_core::types::Angle::DEG0
    );
    let erc = output::erc_run(&mut session, Default::default())
        .unwrap()
        .result;
    assert_eq!(erc["warnings"], 0, "{erc}");
    assert_eq!(erc["errors"], 0, "{erc}");

    let board_path = temp.path().join("bench.kicad_pcb");
    std::fs::write(
        &board_path,
        r#"
(kicad_pcb (version 20260206)
  (general (thickness 1.6))
  (layers (0 "F.Cu" signal) (4 "In1.Cu" power) (6 "In2.Cu" signal) (2 "B.Cu" signal))
  (gr_line (start 0 0) (end 50 0) (layer "Edge.Cuts"))
  (gr_line (start 50 0) (end 50 40) (layer "Edge.Cuts"))
  (gr_line (start 50 40) (end 0 40) (layer "Edge.Cuts"))
  (gr_line (start 0 40) (end 0 0) (layer "Edge.Cuts"))
  (footprint "Fixture:Part" (layer "F.Cu") (at 20 20) (attr smd)
    (property "Reference" "U1" (at 0 2 0) (layer "F.SilkS") (effects (font (size 1 1))))
    (pad "1" smd rect (at -1 0) (size 0.6 0.8) (layers "F.Cu" "F.Mask"))
    (pad "2" smd rect (at 1 0) (size 0.6 0.8) (layers "F.Cu" "F.Mask") (net "LINK")))
  (footprint "Fixture:Part" (layer "B.Cu") (at 30 20 90) (attr smd)
    (property "Reference" "U2" (at 0 2 90) (layer "B.SilkS") (effects (font (size 1 1))))
    (pad "1" smd rect (at -1 0 90) (size 0.6 0.8) (layers "B.Cu" "B.Mask") (net "LINK"))
    (pad "2" smd rect (at 1 0 90) (size 0.6 0.8) (layers "B.Cu" "B.Mask")))
  (segment (start 21 20) (end 30 21) (width 0.5) (layer "F.Cu") (net "LINK")))
"#,
    )
    .unwrap();
    std::fs::write(
        temp.path().join("bench.kicad_pro"),
        r#"{
        "net_settings": {
          "classes":[{"name":"Default","track_width":0.2},{"name":"Power","track_width":0.5}],
          "netclass_patterns":[{"pattern":"LINK","netclass":"Power"}]
        }
    }"#,
    )
    .unwrap();
    let mut session = Session::new();
    let request = serde_json::from_value(json!({
        "name":"KiCad routing setup", "directory":temp.path().join("native-board"),
        "kicad_schematic":schematic, "kicad_board":board_path
    }))
    .unwrap();
    project::project_create(&mut session, request).unwrap();
    let out = librepcb_mcp::tools::layout::board_get(&session, Default::default())
        .unwrap()
        .result;
    let device = out["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["designator"] == "U2")
        .unwrap();
    let pad = device["pads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "1")
        .unwrap();
    assert_eq!(device["side"], "bottom");
    assert_eq!(pad["position"], json!({"x":30.0, "y":-21.0}));
    assert_eq!(pad["net"], "LINK");
    assert_eq!(out["traces"], json!([]));
    let p = session.project().unwrap().project();
    let class = p.circuit().net_class_by_name("Power").unwrap().1;
    assert_eq!(class.default_trace_width().unwrap().to_nm(), 500_000);
    assert_eq!(p.boards()[0].copper_layers().len(), 4);
}
