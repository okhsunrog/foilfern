//! Official KiCad source assembly through a local GitLab-compatible endpoint.

use foilfern_mcp::tools::{import, library, project};
use foilfern_mcp::{ErrorKind, Session};
use foilfern_network::kicad_source::OfficialKiCad;
use foilfern_network::{ClientInfo, NetworkAccessManager};
use serde_json::json;
use wiremock::matchers::{path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CHILD: &str = r#"(kicad_symbol_lib(version 20251024)(generator test)
 (symbol "CHILD"(extends "BASE")(property "Value" "CHILD"(at 0 2 0)(effects(font(size 1 1))))))"#;
const BASE: &str = r#"(kicad_symbol_lib(version 20251024)(generator test)
 (symbol "BASE"(property "Reference" "U"(at 0 4 0)(effects(font(size 1 1))))
 (property "Value" "BASE"(at 0 2 0)(effects(font(size 1 1))))(property "Footprint" "Test:Foot")
 (symbol "BASE_1_1"
 (pin input line(at -2.54 0 0)(length 2.54)(name "A"(effects(font(size 1 1))))(number "1"(effects(font(size 1 1)))))
 (pin output line(at 2.54 0 180)(length 2.54)(name "B"(effects(font(size 1 1))))(number "2"(effects(font(size 1 1))))))))"#;
const FOOTPRINT: &str = r#"(footprint "Foot"(version 20251024)(generator test)(layer "F.Cu")
 (pad "1" smd rect(at -1 0)(size 1 1)(layers "F.Cu" "F.Paste" "F.Mask"))
 (pad "2" smd rect(at 1 0)(size 1 1)(layers "F.Cu" "F.Paste" "F.Mask"))
 (model "${KICAD10_3DMODEL_DIR}/Test.3dshapes/Foot.step"(offset(xyz 0 0 0))(scale(xyz 1 1 1))(rotate(xyz 0 0 0))))"#;

fn args(dry_run: bool, cache: &str) -> import::LibraryImportArgs {
    serde_json::from_value(json!({"format":"kicad","source":"official:Test:CHILD","dry_run":dry_run,"source_cache":cache})).unwrap()
}

#[tokio::test]
async fn official_sources_merge_parents_import_models_and_keep_license_provenance() {
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
    let target = import::official::target(&session).unwrap();
    let server = MockServer::start().await;
    let commit = "a".repeat(40);
    Mock::given(path_regex(".*/commits/10.0.8$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":commit})))
        .mount(&server)
        .await;
    for (suffix, body) in [
        ("CHILD\\.kicad_sym", CHILD),
        ("BASE\\.kicad_sym", BASE),
        ("Foot\\.kicad_mod", FOOTPRINT),
        ("LICENSE\\.md", "Original test license and attribution"),
    ] {
        Mock::given(path_regex(format!(".*/files/.*{suffix}/raw$")))
            .and(query_param("ref", commit.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;
    }
    Mock::given(path_regex(".*/files/.*Foot\\.step/raw$"))
        .respond_with(
            ResponseTemplate::new(200).set_body_bytes(
                std::fs::read(
                    crate::common::test_data_dir()
                        .join("unittests/librepcbcommon/OccModelTest/model.step"),
                )
                .unwrap(),
            ),
        )
        .mount(&server)
        .await;
    let provider = OfficialKiCad::with_base(
        NetworkAccessManager::new(ClientInfo {
            app_version: "test".into(),
            git_revision: String::new(),
            file_format_version: foilfern_core::application::file_format_version(),
            locale: "en_US".into(),
        })
        .unwrap(),
        &server.uri(),
    )
    .unwrap();
    let prepared = import::official::prepare(&target, args(true, "prefer_cache"), provider.clone())
        .await
        .unwrap();
    let preview = import::official::finish(&session, target.clone(), prepared).unwrap();
    assert!(
        !session
            .workspace()
            .unwrap()
            .local_libraries_path()
            .path_to("KiCadImports.lplib")
            .is_existing_dir()
    );
    assert_eq!(preview.result["official_source"]["symbol"], "Test:CHILD");
    assert_eq!(
        preview.result["official_source"]["sources"]
            .as_array()
            .unwrap()
            .len(),
        7
    );
    server.reset().await;
    let prepared = import::official::prepare(&target, args(false, "offline"), provider.clone())
        .await
        .unwrap();
    let output = import::official::finish(&session, target.clone(), prepared).unwrap();
    assert_eq!(
        output.result["official_source"]["bundle_id"],
        preview.result["official_source"]["bundle_id"]
    );
    assert!(server.received_requests().await.unwrap().is_empty());
    let device = library::library_search(
        &session,
        library::LibrarySearchArgs {
            query: "CHILD".into(),
            kind: Some(library::LibKind::Device),
            exact: true,
            limit: None,
        },
    )
    .unwrap_or_else(|error| panic!("{error}: {}; {:?}", output.summary, output.warnings));
    assert_eq!(
        device.result["elements"][0]["pads"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        device.result["elements"][0]["models"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let root = session
        .workspace()
        .unwrap()
        .local_libraries_path()
        .path_to("KiCadImports.lplib");
    let archive = root.path_to(&format!(
        "sources/{}",
        output.result["official_source"]["bundle_id"]
            .as_str()
            .unwrap()
    ));
    assert_eq!(
        std::fs::read_to_string(archive.path_to("LICENSE-kicad-symbols.md").as_path()).unwrap(),
        "Original test license and attribution"
    );
    let prepared = import::official::prepare(&target, args(false, "offline"), provider.clone())
        .await
        .unwrap();
    import::official::finish(&session, target.clone(), prepared).unwrap();
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
    let mut invalid = args(true, "offline");
    invalid.source = "official:../escape:CHILD".into();
    assert_eq!(
        import::official::prepare(&target, invalid, provider)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidArgument
    );
}

#[tokio::test]
#[ignore = "requires official KiCad GitLab repositories"]
async fn official_ch340g_imports_through_registry_and_replays_offline() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = std::env::var_os("FOILFERN_KICAD_WORKSPACE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| temporary.path().join("workspace"));
    let runner =
        foilfern_mcp::local::LocalRunner::new(foilfern_mcp::FoilfernMcp::new(Session::new()));
    let opened = runner
        .call(
            "workspace_create",
            json!({"path":workspace}).as_object().unwrap().clone(),
        )
        .await
        .unwrap();
    assert_ne!(opened.is_error, Some(true), "{opened:?}");
    let request = json!({"format":"kicad","source":"official:Interface_USB:CH340G","dry_run":true});
    let preview = runner
        .call("library_import", request.as_object().unwrap().clone())
        .await
        .unwrap();
    assert_ne!(preview.is_error, Some(true), "{preview:?}");
    let expected =
        preview.structured_content.unwrap()["result"]["official_source"]["bundle_id"].clone();
    let imported=runner.call("library_import",json!({"format":"kicad","source":"official:Interface_USB:CH340G","source_cache":"offline"}).as_object().unwrap().clone()).await.unwrap();
    assert_ne!(imported.is_error, Some(true), "{imported:?}");
    let imported = imported.structured_content.unwrap();
    assert_eq!(imported["result"]["official_source"]["bundle_id"], expected);
    let native_library = imported["result"]["library"].as_str().unwrap().to_owned();
    let device = runner
        .call(
            "library_search",
            json!({"query":"CH340G","exact":true,"kind":"device"})
                .as_object()
                .unwrap()
                .clone(),
        )
        .await
        .unwrap();
    assert_ne!(device.is_error, Some(true), "{device:?}");
    let device = device.structured_content.unwrap();
    assert_eq!(
        device["result"]["elements"][0]["pads"]
            .as_array()
            .unwrap()
            .len(),
        16
    );
    assert!(
        device["result"]["elements"][0]["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|model| model["available"] == true)
    );
    runner.close().await.unwrap();
    if let Some(cli) = crate::common::upstream_cli() {
        let (ok, output) = crate::common::run_cli(
            &cli,
            &["open-library", "--all", "--strict", &native_library],
        );
        assert!(ok, "{output}");
    }
}
