//! Tests of the library scanner and the library database facade with the
//! upstream test libraries (no upstream unit tests; upstream tests the
//! scanner only through the GUI tests).

use std::sync::atomic::AtomicBool;

use librepcb_core::fileio::{FilePath, VersionFile, file_utils};
use librepcb_core::types::Uuid;
use librepcb_core::workspace::{
    ElementKind, OutputJobKind, ScanEvent, ScanOutcome, SearchQuery, Workspace,
};

use crate::helpers::{TempDir, test_data_dir};

const POPULATED_LIB_UUID: &str = "a7cb5051-9f37-4500-be38-33eade6e621e";
const DEMO_DEVICE: &str = "4f5ee784-4b1b-407c-802b-44625163d90f";
const R0805_DEVICE: &str = "078650d3-483c-4b9e-a848-b14f1aad2edc";
const RESISTOR_COMPONENT: &str = "ef80cd5e-2689-47ee-8888-31d04fc99174";

fn uuid(s: &str) -> Uuid {
    s.parse().unwrap()
}

fn test_lib(name: &str) -> FilePath {
    FilePath::new(test_data_dir().join("libraries").join(name)).unwrap()
}

/// Creates a workspace with the upstream test libraries copied into it.
fn setup() -> (TempDir, Workspace) {
    let tmp = TempDir::new();
    let ws = Workspace::open_or_create(&tmp.path().path_to("workspace"), None).unwrap();
    let local = ws.local_libraries_path();
    let remote = ws.remote_libraries_path();
    file_utils::copy_dir_recursively(
        &test_lib("Populated Library.lplib"),
        &local.path_to("Populated Library.lplib"),
    )
    .unwrap();
    file_utils::copy_dir_recursively(&test_lib("v1.lplib"), &local.path_to("v1.lplib")).unwrap();
    file_utils::copy_dir_recursively(&test_lib("v0.1.lplib"), &remote.path_to("v0.1.lplib"))
        .unwrap();
    file_utils::copy_dir_recursively(
        &test_lib("Empty Library.lplib"),
        &remote.path_to("Empty Library.lplib"),
    )
    .unwrap();
    // Ignored: an empty directory and a directory which is not a library.
    file_utils::make_path(&local.path_to("empty")).unwrap();
    file_utils::write_file(&local.path_to("no library/file.txt"), b"").unwrap();
    (tmp, ws)
}

fn scan(ws: &Workspace) -> (ScanOutcome, Vec<ScanEvent>) {
    let mut events = Vec::new();
    let outcome = ws
        .library_db()
        .rescan(&AtomicBool::new(false), &mut |e| events.push(e))
        .unwrap();
    (outcome, events)
}

#[test]
fn test_scan_and_query() {
    let (_tmp, ws) = setup();
    let db = ws.library_db();
    let (outcome, events) = scan(&ws);

    // Populated Library: 67 elements, v1: 64 elements, v0.1: 6 elements.
    assert_eq!(outcome, ScanOutcome::Succeeded { element_count: 137 });
    assert_eq!(events[0], ScanEvent::Started);
    assert!(events.contains(&ScanEvent::LibraryListUpdated { library_count: 4 }));
    assert_eq!(events.last(), Some(&ScanEvent::Progress(100)));
    let percents: Vec<u8> = events
        .iter()
        .filter_map(|e| match e {
            ScanEvent::Progress(p) => Some(*p),
            _ => None,
        })
        .collect();
    assert!(percents.windows(2).all(|w| w[0] <= w[1]), "{percents:?}");

    // The v0.1 library was upgraded on disk.
    let v01 = ws.remote_libraries_path().path_to("v0.1.lplib");
    let version =
        VersionFile::from_bytes(&file_utils::read_file(&v01.path_to(".librepcb-lib")).unwrap())
            .unwrap();
    assert_eq!(version.version().to_string(), "2");

    // Libraries.
    let libs = db.all(ElementKind::Library, None, None).unwrap();
    assert_eq!(libs.len(), 4);
    let populated = ws.local_libraries_path().path_to("Populated Library.lplib");
    let info = db.library_metadata(&populated).unwrap().unwrap();
    assert_eq!(info.manufacturer, "Demo Manufacturer");
    assert!(info.icon_png.starts_with(b"\x89PNG"));
    let meta = db
        .metadata(ElementKind::Library, &populated)
        .unwrap()
        .unwrap();
    assert_eq!(meta.uuid, uuid(POPULATED_LIB_UUID));
    assert_eq!(
        db.translations(ElementKind::Library, &populated, &["de_DE"])
            .unwrap()
            .unwrap()
            .name,
        "Populated Library"
    );
    assert_eq!(
        db.all_in_library(ElementKind::Device, &populated)
            .unwrap()
            .len(),
        8
    );

    // Elements (duplicates across libraries).
    let devices = db
        .all(ElementKind::Device, Some(uuid(R0805_DEVICE)), None)
        .unwrap();
    assert_eq!(devices.len(), 2);
    let dev_dir = db
        .element_dir(ElementKind::Device, uuid(DEMO_DEVICE))
        .unwrap()
        .unwrap();
    assert!(dev_dir.is_located_in_dir(ws.libraries_path()));
    assert!(dev_dir.path_to("device.lp").is_existing_file());
    let dev_info = db.device_metadata(&dev_dir).unwrap().unwrap();
    assert_eq!(
        dev_info.component_uuid,
        uuid("45022bef-9310-4aa2-92ef-acec1b95aa4e")
    );
    assert!(
        db.component_devices(uuid(RESISTOR_COMPONENT))
            .unwrap()
            .contains(&uuid(R0805_DEVICE))
    );
    assert_eq!(
        db.generated(ElementKind::Device, "Some Generator")
            .unwrap()
            .len(),
        1
    );

    // Parts.
    let parts = db.device_parts(uuid(DEMO_DEVICE)).unwrap();
    let mpns: Vec<&str> = parts.iter().map(|p| p.mpn().as_str()).collect();
    assert_eq!(mpns, ["DEMO1", "DEMO2", "DEMO3"]);
    assert_eq!(parts[0].attributes().get(0).unwrap().value(), "VALUE1");
    assert_eq!(
        db.find_devices_of_parts("MANU1").unwrap(),
        vec![uuid(DEMO_DEVICE)]
    );

    // Search facade.
    let results = db.search(&SearchQuery::new("R-0805")).unwrap();
    assert_eq!(results.len(), 1);
    let r = &results[0];
    assert_eq!(r.kind, ElementKind::Device);
    assert_eq!(r.uuid, uuid(R0805_DEVICE));
    assert_eq!(r.name, "R-0805");
    assert_eq!(r.device.unwrap().component_uuid, uuid(RESISTOR_COMPONENT));
    assert_eq!(
        r.categories,
        [uuid("1039f038-20a6-4bfe-89c1-99f34fbb45bd")].into()
    );
    let results = db
        .search(&SearchQuery {
            keyword: "resistor".to_owned(),
            kinds: vec![ElementKind::Component, ElementKind::Symbol],
            ..SearchQuery::default()
        })
        .unwrap();
    assert!(!results.is_empty());
    assert!(
        results
            .iter()
            .all(|r| r.name.to_lowercase().contains("resistor")
                || r.keywords.to_lowercase().contains("resistor"))
    );
    assert_eq!(results[0].kind, ElementKind::Component);
    // Devices found by their parts.
    let results = db.search(&SearchQuery::new("MANU1")).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].uuid, uuid(DEMO_DEVICE));
    let results = db
        .search(&SearchQuery {
            keyword: "MANU1".to_owned(),
            include_parts: false,
            ..SearchQuery::default()
        })
        .unwrap();
    assert!(results.is_empty());
    // Limit and JSON serialization.
    let results = db
        .search(&SearchQuery {
            limit: Some(3),
            ..SearchQuery::default()
        })
        .unwrap();
    assert_eq!(results.len(), 3);
    let json = serde_json::to_value(&results[0]).unwrap();
    assert!(json["directory"].is_string());
    assert!(json["kind"].is_string());
    // Packages by alternative name.
    let results = db
        .search(&SearchQuery {
            keyword: "TO220".to_owned(),
            kinds: vec![ElementKind::Package],
            ..SearchQuery::default()
        })
        .unwrap();
    assert_eq!(results.len(), 1);
    // All tokens (not an upstream function): device by its component name.
    assert!(
        db.find(ElementKind::Device, "resistor 0805")
            .unwrap()
            .is_empty()
    );
    let found = db
        .find_all_tokens(ElementKind::Device, "  Resistor   0805 ")
        .unwrap();
    assert_eq!(found.first(), Some(&uuid(R0805_DEVICE)), "{found:?}");
    assert!(
        db.find_all_tokens(ElementKind::Device, "resistor nonexistingtoken")
            .unwrap()
            .is_empty()
    );
    assert!(
        db.find_all_tokens(ElementKind::Device, " ")
            .unwrap()
            .is_empty()
    );
    let results = db
        .search_all_tokens(&SearchQuery {
            keyword: "manu1 demo1".to_owned(),
            kinds: vec![ElementKind::Device],
            ..SearchQuery::default()
        })
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].uuid, uuid(DEMO_DEVICE));

    // Category tree with translations.
    let tree = db
        .category_tree(ElementKind::ComponentCategory, &["de_DE"])
        .unwrap();
    let names: Vec<&str> = tree.iter().map(|n| n.name.as_str()).collect();
    assert!(names.contains(&"Widerstände"), "{names:?}");
    let opto = tree.iter().find(|n| n.name == "Optoelectronics").unwrap();
    assert_eq!(opto.children.len(), 1);
    assert_eq!(opto.children[0].name, "LEDs");
    let summary = db
        .element_summary(ElementKind::Component, uuid(RESISTOR_COMPONENT), &["de_DE"])
        .unwrap()
        .unwrap();
    assert_eq!(summary.name, "Widerstand");
    assert!(
        db.element_summary(ElementKind::Symbol, uuid(RESISTOR_COMPONENT), &["de_DE"])
            .unwrap()
            .is_none()
    );

    // Organizations.
    let orgs = db.all_latest_organizations(&["en_US"], true, true).unwrap();
    let names: Vec<&str> = orgs.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["PCBWay", "AISLER"]); // Sorted by priority.
    let pcbway = &orgs[0];
    assert_eq!(pcbway.country, "CN");
    assert_eq!(pcbway.fabs, ["CN"]);
    assert_eq!(pcbway.url, "https://www.pcbway.com");
    assert!(pcbway.is_sponsor);
    assert_eq!(pcbway.priority, 34);
    assert_eq!(pcbway.pcb_design_rules.len(), 1);
    assert_eq!(pcbway.pcb_design_rules[0].name, "Standard");
    assert_eq!(pcbway.pcb_design_rules[0].max_layer_count, 12);
    let aisler = &orgs[1];
    assert_eq!(aisler.pcb_design_rules.len(), 2);
    assert_eq!(
        aisler
            .output_jobs
            .iter()
            .filter(|j| j.kind == OutputJobKind::Assembly)
            .count(),
        2
    );
    assert!(
        aisler
            .output_jobs
            .iter()
            .any(|j| j.kind == OutputJobKind::Pcb && j.job_type == "gerber_excellon")
    );
    let orgs = db
        .all_latest_organizations(&["en_US"], false, false)
        .unwrap();
    assert!(orgs[0].pcb_design_rules.is_empty() && orgs[0].output_jobs.is_empty());
}

#[test]
fn test_rescan_updates_database() {
    let (_tmp, ws) = setup();
    let db = ws.library_db();
    scan(&ws);
    let lib_ids_before = db.all(ElementKind::Library, None, None).unwrap();

    // Rescan without changes.
    let (outcome, _) = scan(&ws);
    assert_eq!(outcome, ScanOutcome::Succeeded { element_count: 137 });
    assert_eq!(
        db.all(ElementKind::Library, None, None).unwrap(),
        lib_ids_before
    );

    // Remove a library.
    file_utils::remove_dir_recursively(&ws.local_libraries_path().path_to("v1.lplib")).unwrap();
    let (outcome, _) = scan(&ws);
    assert_eq!(outcome, ScanOutcome::Succeeded { element_count: 73 });
    assert_eq!(db.all(ElementKind::Library, None, None).unwrap().len(), 3);
    assert_eq!(
        db.all(ElementKind::Device, Some(uuid(R0805_DEVICE)), None)
            .unwrap()
            .len(),
        1
    );

    // Reset and rescan.
    let outcome = db
        .reset_and_rescan(&AtomicBool::new(false), &mut |_| {})
        .unwrap();
    assert_eq!(outcome, ScanOutcome::Succeeded { element_count: 73 });
}

#[test]
fn test_abort_scan() {
    let (_tmp, ws) = setup();
    let db = ws.library_db();
    let outcome = db.rescan(&AtomicBool::new(true), &mut |_| {}).unwrap();
    assert_eq!(outcome, ScanOutcome::Aborted);
    // The library list is updated anyway, but no elements are added.
    assert_eq!(db.all(ElementKind::Library, None, None).unwrap().len(), 4);
    assert!(db.all(ElementKind::Device, None, None).unwrap().is_empty());
}

#[test]
fn test_scan_on_other_thread() {
    let (_tmp, ws) = setup();
    let scanner = ws.library_db().scanner();
    let handle = std::thread::spawn(move || scanner.scan(&AtomicBool::new(false), &mut |_| {}));
    assert_eq!(
        handle.join().unwrap().unwrap(),
        ScanOutcome::Succeeded { element_count: 137 }
    );
    assert_eq!(
        ws.library_db()
            .all(ElementKind::Symbol, None, None)
            .unwrap()
            .len(),
        39
    );
}
