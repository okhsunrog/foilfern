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
        lcsc: "C123".into(),
        cache,
        require_model: false,
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
    .unwrap();
    let acquired = part::acquire(&target, args(PartCache::PreferCache), provider.clone())
        .await
        .unwrap();
    let out = part::finish(&session, target.clone(), acquired).unwrap();
    assert_eq!(out.result["reused"], false);
    assert_eq!(out.result["device"]["pads"].as_array().unwrap().len(), 2);
    assert_eq!(out.result["add_component"]["part"]["mpn"], "TEST-123");
    assert_eq!(
        out.result["add_component"]["part"]["attributes"]["LCSC"],
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
    assert_eq!(again.result["add_component"]["part"]["mpn"], "TEST-MANUAL");
    assert!(server.received_requests().await.unwrap().is_empty());
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
    let imported = runner
        .call(
            "part_import",
            json!({"lcsc":"C8734","require_model":true})
                .as_object()
                .unwrap()
                .clone(),
        )
        .await
        .unwrap();
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
        envelope["result"]["add_component"]["part"]["mpn"],
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
