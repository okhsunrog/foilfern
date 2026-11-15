//! Supplier acquisition through a local HTTP source, native install and replay.

use foilfern_mcp::tools::part::{PartCache, PartImportArgs};
use foilfern_mcp::tools::{part, project};
use foilfern_mcp::{ErrorKind, Session};
use foilfern_network::part_source::EasyEdaProvider;
use serde_json::{Value, json};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn source() -> Value {
    json!({"success":true,"result":{
        "dataStr":{"head":{"x":100,"y":100,"c_para":{"pre":"R?","Manufacturer Part":"TEST-123","Manufacturer":"Test maker","Supplier Part":"C123"}},"shape":[
            "P~show~0~1~90.25~100~180~pin1~0^^90.25~100^^M90.25,100h5~#000^^1~90~100~0~A~start",
            "P~show~0~2~110~100~0~pin2~0^^110~100^^M110,100h-5~#000^^1~110~100~0~B~start",
            "R~95~95~0~0~10~10~#000~0.5~0~none~box"]},
        "packageDetail":{"dataStr":{"head":{"x":200,"y":200},"shape":[
            "PAD~RECT~195.125~200~2~3~1~~1~0~~0~pad1~0",
            "PAD~OVAL~205~200~2~3~1~~2~0~~0~pad2~0"]}}
    }})
}

fn args(cache: PartCache) -> PartImportArgs {
    PartImportArgs {
        lcsc: Some("C123".into()),
        mpn: None,
        cache,
        require_model: false,
        dry_run: false,
        expected_source_sha256: None,
        expected_model_sha256: None,
    }
}

async fn mount(server: &MockServer, data: Value) {
    Mock::given(path("/api/products/C123/components"))
        .respond_with(ResponseTemplate::new(200).set_body_json(data))
        .mount(server)
        .await;
}

#[tokio::test]
async fn acquisition_installs_once_replays_offline_and_refuses_changed_sources() {
    let tmp = tempfile::tempdir().unwrap();
    let mut session = Session::new();
    project::workspace_open(
        &mut session,
        project::WorkspacePathArgs {
            path: tmp.path().join("workspace").to_string_lossy().into_owned(),
        },
        true,
    )
    .unwrap();
    let target = part::target(&session).unwrap();
    let server = MockServer::start().await;
    mount(&server, source()).await;
    Mock::given(path("/api/search"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"components":[{"lcsc":123,"mfr":"TEST-123"}]})),
        )
        .mount(&server)
        .await;
    // Reuse the official HTTP client identity with a test endpoint.
    let provider = EasyEdaProvider::with_base(
        foilfern_network::NetworkAccessManager::new(foilfern_network::ClientInfo {
            app_version: "test".into(),
            git_revision: String::new(),
            file_format_version: foilfern_core::application::file_format_version(),
            locale: "en_US".into(),
        })
        .unwrap(),
        &server.uri(),
    )
    .unwrap()
    .with_model_base(&server.uri())
    .unwrap()
    .with_search_base(&server.uri())
    .unwrap();
    let acquired = part::acquire(
        &target,
        PartImportArgs {
            lcsc: None,
            mpn: Some("TEST-123".into()),
            dry_run: true,
            ..args(PartCache::PreferCache)
        },
        provider.clone(),
    )
    .await
    .unwrap();
    let preview = part::finish(&session, target.clone(), acquired).unwrap();
    assert_eq!(preview.result["electrical_metadata"]["unspecified_pins"], 2);
    assert_eq!(preview.images.len(), 2);
    assert!(
        preview
            .images
            .iter()
            .all(|png| png.starts_with(b"\x89PNG\r\n\x1a\n"))
    );
    assert_eq!(preview.result["mapping"].as_array().unwrap().len(), 2);
    assert!(
        !session
            .workspace()
            .unwrap()
            .local_libraries_path()
            .path_to("SupplierParts.lplib")
            .is_existing_dir()
    );
    let install: PartImportArgs =
        serde_json::from_value(preview.result["install"].clone()).unwrap();
    let error = part::acquire(
        &target,
        PartImportArgs {
            expected_source_sha256: Some("0".repeat(64)),
            ..args(PartCache::Offline)
        },
        provider.clone(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Conflict);
    let acquired = part::acquire(
        &target,
        PartImportArgs {
            cache: PartCache::Offline,
            ..install
        },
        provider.clone(),
    )
    .await
    .unwrap();
    let out = part::finish(&session, target.clone(), acquired).unwrap();
    assert_eq!(out.result["reused"], false);
    assert_eq!(out.result["device"]["pads"].as_array().unwrap().len(), 2);
    assert_eq!(
        out.result["add_component"]["selected_part"]["mpn"],
        "TEST-123"
    );
    assert_eq!(
        out.result["add_component"]["selected_part"]["attributes"]["LCSC"],
        "C123"
    );
    assert_eq!(out.result["model"]["available"], false);
    let device = out.result["device"]["uuid"].clone();
    assert_eq!(out.outcome, foilfern_mcp::outcome::Outcome::Partial);
    // A user edit to the native selected part survives a replay and is what
    // the returned ready-to-use entry describes.
    {
        use foilfern_core::fileio::{TransactionalDirectory, TransactionalFileSystem};
        use foilfern_core::library::{LibraryBaseElement, dev::Device};
        let uuid = device.as_str().unwrap().parse().unwrap();
        let path = session
            .workspace()
            .unwrap()
            .library_db()
            .latest(foilfern_core::workspace::ElementKind::Device, uuid)
            .unwrap()
            .unwrap();
        let fs = std::sync::Arc::new(TransactionalFileSystem::open_rw(&path).unwrap());
        let mut device = Device::open(TransactionalDirectory::new(fs.clone(), "")).unwrap();
        device
            .parts_mut()
            .get_mut(0)
            .unwrap()
            .set_mpn(foilfern_core::types::SimpleString::new("TEST-MANUAL").unwrap());
        device.save().unwrap();
        fs.save().unwrap();
    }
    server.reset().await;
    let acquired = part::acquire(&target, args(PartCache::Offline), provider.clone())
        .await
        .unwrap();
    let again = part::finish(&session, target.clone(), acquired).unwrap();
    assert_eq!(again.result["reused"], true);
    assert_eq!(again.result["device"]["uuid"], device);
    assert_eq!(
        again.result["add_component"]["selected_part"]["mpn"],
        "TEST-MANUAL"
    );
    assert!(server.received_requests().await.unwrap().is_empty());
    let invalid = part::acquire(
        &target,
        PartImportArgs {
            mpn: Some("TEST-123".into()),
            ..args(PartCache::Offline)
        },
        provider.clone(),
    )
    .await
    .unwrap_err();
    assert_eq!(invalid.kind, ErrorKind::InvalidArgument);
    let mut changed = source();
    changed["result"]["dataStr"]["head"]["c_para"]["Manufacturer Part"] = json!("TEST-CHANGED");
    mount(&server, changed).await;
    let acquired = part::acquire(&target, args(PartCache::Refresh), provider.clone())
        .await
        .unwrap();
    let error = part::finish(&session, target.clone(), acquired).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Conflict);
    assert_eq!(
        session
            .workspace()
            .unwrap()
            .library_db()
            .all(foilfern_core::workspace::ElementKind::Device, None, None)
            .unwrap()
            .len(),
        1
    );
    let error = part::acquire(
        &target,
        PartImportArgs {
            require_model: true,
            ..args(PartCache::Offline)
        },
        provider,
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::NotFound);
}

#[tokio::test]
#[ignore = "requires the live EasyEDA source and model endpoints"]
async fn c8734_imports_through_registry_with_step_and_replays_offline() {
    let tmp = tempfile::tempdir().unwrap();
    let runner =
        foilfern_mcp::local::LocalRunner::new(foilfern_mcp::FoilfernMcp::new(Session::new()));
    let created = runner
        .call(
            "workspace_create",
            json!({"path":tmp.path().join("workspace")})
                .as_object()
                .unwrap()
                .clone(),
        )
        .await
        .unwrap();
    assert_ne!(created.is_error, Some(true), "{created:?}");
    let preview = runner
        .call(
            "part_import",
            json!({"mpn":"STM32F103C8T6","require_model":true,"dry_run":true})
                .as_object()
                .unwrap()
                .clone(),
        )
        .await
        .unwrap();
    assert_ne!(preview.is_error, Some(true), "{preview:?}");
    assert_eq!(
        preview
            .content
            .iter()
            .filter(|content| content.as_image().is_some())
            .count(),
        2
    );
    if let Ok(directory) = std::env::var("FOILFERN_PART_PREVIEW_DIR") {
        use base64::Engine;
        std::fs::create_dir_all(&directory).unwrap();
        for (index, image) in preview
            .content
            .iter()
            .filter_map(|content| content.as_image())
            .enumerate()
        {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&image.data)
                .unwrap();
            std::fs::write(
                std::path::Path::new(&directory).join(format!("C8734-{index}.png")),
                bytes,
            )
            .unwrap();
        }
    }
    let install = preview.structured_content.unwrap()["result"]["install"]
        .as_object()
        .unwrap()
        .clone();
    let imported = runner.call("part_import", install).await.unwrap();
    assert_ne!(imported.is_error, Some(true), "{imported:?}");
    let envelope = imported.structured_content.unwrap();
    assert_eq!(
        envelope["result"]["device"]["pads"]
            .as_array()
            .unwrap()
            .len(),
        48
    );
    assert_eq!(envelope["result"]["model"]["available"], true);
    assert_eq!(
        envelope["result"]["add_component"]["selected_part"]["mpn"],
        "STM32F103C8T6"
    );
    let replay = runner
        .call(
            "part_import",
            json!({"lcsc":"C8734","cache":"offline","require_model":true})
                .as_object()
                .unwrap()
                .clone(),
        )
        .await
        .unwrap();
    assert_ne!(replay.is_error, Some(true), "{replay:?}");
    let replay = replay.structured_content.unwrap();
    assert_eq!(replay["result"]["reused"], true);
    assert_eq!(
        replay["result"]["device"]["uuid"],
        envelope["result"]["device"]["uuid"]
    );
    runner.close().await.unwrap();
}

#[tokio::test]
async fn acquired_part_handoff_selects_identity_without_design_documents() {
    use foilfern_core::fileio::{TransactionalDirectory, TransactionalFileSystem};
    use foilfern_core::library::{LibraryBaseElement, dev::Device};
    use foilfern_mcp::tools::{board_edit, output, schematic_edit, write};
    let tmp = tempfile::tempdir().unwrap();
    let mut session = Session::new();
    project::workspace_open(
        &mut session,
        serde_json::from_value(json!({"path":tmp.path().join("ws")})).unwrap(),
        true,
    )
    .unwrap();
    let target = part::target(&session).unwrap();
    let mock = MockServer::start().await;
    mount(&mock, source()).await;
    let provider = EasyEdaProvider::with_base(
        foilfern_network::NetworkAccessManager::new(foilfern_network::ClientInfo {
            app_version: "test".into(),
            git_revision: String::new(),
            file_format_version: foilfern_core::application::file_format_version(),
            locale: "en_US".into(),
        })
        .unwrap(),
        &mock.uri(),
    )
    .unwrap();
    let acquired = part::acquire(&target, args(PartCache::PreferCache), provider)
        .await
        .unwrap();
    let imported = part::finish(&session, target, acquired).unwrap();
    assert_eq!(
        imported.result["component"]["electrical_metadata"]["source_format"],
        "easyeda_std"
    );
    assert_eq!(
        imported.result["component"]["electrical_metadata"]["unspecified_pins"],
        2
    );
    let device = imported.result["device"]["uuid"]
        .as_str()
        .unwrap()
        .to_owned();
    let created = project::project_create(
        &mut session,
        serde_json::from_value(
            json!({"name":"Procurement","directory":tmp.path().join("project")}),
        )
        .unwrap(),
    )
    .unwrap();
    let project_path = created.result["path"].as_str().unwrap().to_owned();
    // The import's advertised arguments are executable as-is, with only the
    // optional instance name added by the caller.
    let mut ready = imported.result["add_component"].clone();
    ready["designator"] = json!("U1");
    let added = schematic_edit::component_add(&mut session, serde_json::from_value(ready).unwrap())
        .unwrap();
    assert_eq!(added.result["electrical_metadata"]["status"], "recorded");
    assert_eq!(
        added.result["signals"][0]["source_pins"][0]["source_type"],
        "0"
    );
    assert!(!added.warnings.is_empty());
    let erc = output::erc_run(&mut session, Default::default()).unwrap();
    assert_eq!(erc.result["errors"], 0);
    assert_eq!(erc.result["warnings"], 0);
    assert_eq!(erc.outcome, foilfern_mcp::outcome::Outcome::Partial);
    assert!(!erc.warnings.is_empty());
    assert_eq!(erc.result["coverage"]["has_metadata_gaps"], true);
    assert_eq!(
        erc.result["coverage"]["pin_role_compatibility_checked"],
        false
    );
    assert_eq!(erc.result["coverage"]["datasheet_verified"], false);
    assert_eq!(
        added.result["part_selection"]["selected"]["mpn"],
        "TEST-123"
    );
    assert_eq!(
        added.result["part_selection"]["selected"]["attributes"]["LCSC"],
        "C123"
    );
    write::undo_redo(&mut session, Default::default(), false).unwrap();
    assert!(
        session
            .project()
            .unwrap()
            .project()
            .circuit()
            .component_instances()
            .is_empty()
    );
    write::undo_redo(&mut session, Default::default(), true).unwrap();
    // One library part needs no explicit selector and still selects purchasing data.
    let automatic = schematic_edit::component_add(
        &mut session,
        serde_json::from_value(json!({"part":device,"designator":"U2"})).unwrap(),
    )
    .unwrap();
    assert_eq!(
        automatic.result["part_selection"]["selected"],
        added.result["part_selection"]["selected"]
    );
    // Another manufacturer with the same MPN must never be chosen arbitrarily.
    let path = session
        .workspace()
        .unwrap()
        .library_db()
        .latest(
            foilfern_core::workspace::ElementKind::Device,
            device.parse().unwrap(),
        )
        .unwrap()
        .unwrap();
    let fs = std::sync::Arc::new(TransactionalFileSystem::open_rw(&path).unwrap());
    let mut native = Device::open(TransactionalDirectory::new(fs.clone(), "")).unwrap();
    let mut alternative = native.parts().get(0).unwrap().clone();
    alternative.set_manufacturer(foilfern_core::types::SimpleString::new("Other maker").unwrap());
    native.parts_mut().push(alternative);
    native.save().unwrap();
    fs.save().unwrap();
    drop(native);
    drop(fs);
    let revision = session.project().unwrap().project().revision();
    let failed = schematic_edit::component_add(
        &mut session,
        serde_json::from_value(
            json!({"part":device,"designator":"U3","selected_part":{"mpn":"TEST-123"}}),
        )
        .unwrap(),
    )
    .unwrap_err();
    assert_eq!(failed.kind, ErrorKind::InvalidArgument);
    assert!(failed.message.contains("matches 2"));
    assert_eq!(session.project().unwrap().project().revision(), revision);
    assert!(
        session
            .project()
            .unwrap()
            .project()
            .circuit()
            .component_instance_by_name("U3")
            .is_none()
    );
    let ambiguous = schematic_edit::component_add(
        &mut session,
        serde_json::from_value(json!({"part":device,"designator":"U3"})).unwrap(),
    )
    .unwrap();
    assert!(ambiguous.result["part_selection"]["selected"].is_null());
    assert_eq!(
        ambiguous.result["part_selection"]["candidates"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(!ambiguous.warnings.is_empty());
    schematic_edit::component_update(&mut session, serde_json::from_value(json!({"component":"U3","selected_part":{"mpn":"TEST-123","manufacturer":"Test maker"}})).unwrap()).unwrap();
    let before_value = session
        .project()
        .unwrap()
        .project()
        .circuit()
        .component_instance_by_name("U3")
        .unwrap()
        .1
        .value()
        .to_owned();
    let revision = session.project().unwrap().project().revision();
    let error = schematic_edit::component_update(
        &mut session,
        serde_json::from_value(
            json!({"component":"U3","value":"must roll back","selected_part":{"mpn":"missing"}}),
        )
        .unwrap(),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidArgument);
    assert_eq!(session.project().unwrap().project().revision(), revision);
    assert_eq!(
        session
            .project()
            .unwrap()
            .project()
            .circuit()
            .component_instance_by_name("U3")
            .unwrap()
            .1
            .value(),
        &before_value
    );
    // Placement and board BOM preserve the exact selected native part.
    board_edit::device_place(
        &mut session,
        serde_json::from_value(json!({"component":"U1","position":{"x":15,"y":15}})).unwrap(),
    )
    .unwrap();
    let bom = output::export_bom(
        &session,
        serde_json::from_value(json!({"additional_attributes":["LCSC[]"]})).unwrap(),
    )
    .unwrap();
    let csv = bom.result["csv"].as_str().unwrap();
    assert!(
        csv.contains("TEST-123") && csv.contains("Test maker") && csv.contains("C123"),
        "{csv}"
    );
    project::project_save(&mut session, serde_json::from_value(json!({})).unwrap()).unwrap();
    project::project_close(&mut session, serde_json::from_value(json!({})).unwrap()).unwrap();
    project::project_open(
        &mut session,
        serde_json::from_value(json!({"path":project_path})).unwrap(),
    )
    .unwrap();
    let bom = output::export_bom(
        &session,
        serde_json::from_value(json!({"additional_attributes":["LCSC[]"]})).unwrap(),
    )
    .unwrap();
    assert!(bom.result["csv"].as_str().unwrap().contains("C123"));
    let revision = session.project().unwrap().project().revision();
    let erc = output::erc_run(&mut session, Default::default()).unwrap();
    assert_eq!(erc.outcome, foilfern_mcp::outcome::Outcome::Partial);
    assert_eq!(
        erc.result["coverage"]["import_metadata"][0]["metadata"]["status"],
        "recorded"
    );
    assert_eq!(
        erc.result["coverage"]["import_metadata"][0]["metadata"]["unspecified_pins"],
        2
    );
    assert_eq!(session.project().unwrap().project().revision(), revision);
    assert!(!session.project().unwrap().has_unsaved_changes());
    let schematic = session.project().unwrap().project().schematics()[0]
        .name()
        .as_str()
        .to_owned();
    let preview = foilfern_mcp::tools::design::preview(
        &mut session,
        serde_json::from_value(json!({"version":1,"schematic":schematic})).unwrap(),
    )
    .unwrap();
    assert_eq!(
        preview.result["erc"]["coverage"]["after"]["has_metadata_gaps"],
        true
    );
    assert_eq!(
        preview.result["erc"]["coverage"]["after"]["import_metadata"][0]["metadata"]["status"],
        "recorded"
    );
    assert!(!preview.warnings.is_empty());
}
