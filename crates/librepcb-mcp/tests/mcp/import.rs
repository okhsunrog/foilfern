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
        },
    )
    .unwrap();
    let open = session.project().unwrap();
    assert_eq!(1, open.project().schematics().len(), "{}", out.summary);
    assert_eq!(1, open.project().boards().len());
    assert_eq!(4, open.project().circuit().buses().len());
    assert!(!open.has_unsaved_changes());
}
