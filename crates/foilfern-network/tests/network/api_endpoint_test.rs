//! Tests of `ApiEndpoint` (no upstream unit tests; upstream tests it through
//! the funq tests with the same local server data).

use std::sync::Arc;

use foilfern_core::fileio::FilePath;
use foilfern_network::{ApiEndpoint, ChecksumAlgorithm, Error, FileDownload, Library, Part};
use serde_json::json;
use tokio::sync::Semaphore;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::{data_dir, nam, server, url};

/// Serves the library list with URLs rewritten to the mock server.
async fn mount_library_list(server: &MockServer) {
    let list = std::fs::read_to_string(data_dir().join("server/api/v1/libraries/v2")).unwrap();
    let list = list.replace("http://localhost:50080", &server.uri());
    Mock::given(path("/api/v1/libraries/v2"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(list, "application/json"))
        .mount(server)
        .await;
}

async fn library_list(endpoint: &ApiEndpoint) -> Result<Vec<Vec<Library>>, Error> {
    let mut pages = Vec::new();
    endpoint
        .request_library_list(&nam(), |libs| pages.push(libs))
        .await
        .map(|()| pages)
}

#[tokio::test]
async fn test_request_library_list() {
    let server = server().await;
    mount_library_list(&server).await;
    let endpoint = ApiEndpoint::new(url(&server, "/"));
    let pages = library_list(&endpoint).await.unwrap();
    assert_eq!(pages.len(), 1);
    let libs = &pages[0];
    assert_eq!(libs.len(), 3);

    let base = &libs[0];
    assert_eq!(
        base.uuid.to_string(),
        "a9ddf0c6-9b1c-4730-b300-01b4f192ad40"
    );
    assert_eq!(base.name, "LibrePCB Base");
    assert_eq!(base.description, "Official LibrePCB Base Library");
    assert_eq!(base.author, "LibrePCB");
    assert_eq!(base.version.to_string(), "0.0.3");
    assert!(base.recommended);
    assert!(base.dependencies.is_empty());
    assert_eq!(base.download_size, Some(184972));
    assert_eq!(
        base.download_sha256,
        "5f85fa43c057e3cff9206cc086f3023125f8a41d0ed706211d755d5e289314c5"
    );
    assert_eq!(
        base.download_url.as_ref().unwrap().as_str(),
        format!("{}/blobs/LibrePCB_Base.zip", server.uri())
    );

    let connectors = &libs[1];
    assert_eq!(connectors.dependencies.len(), 1);
    assert!(connectors.dependencies.contains(&base.uuid));

    // Download a library like the library manager does.
    let tmp = tempfile::tempdir().unwrap();
    let dir = FilePath::new(tmp.path()).unwrap();
    let lib_dir = dir.path_to("LibrePCB_Base.lplib");
    let downloaded = FileDownload::new(
        base.download_url.clone().unwrap(),
        dir.path_to("LibrePCB_Base.zip"),
        Arc::new(Semaphore::new(4)),
    )
    .expected_reply_content_size(base.download_size.unwrap())
    .expected_checksum(
        ChecksumAlgorithm::Sha256,
        hex::decode(&base.download_sha256).unwrap(),
    )
    .zip_extraction_directory(
        lib_dir.clone(),
        Some(Box::new(|tmp: &FilePath| {
            // Strip the root folder of the ZIP.
            let dirs = foilfern_core::fileio::file_utils::find_directories(tmp);
            Ok(dirs.into_iter().next().unwrap_or_else(|| tmp.clone()))
        })),
    )
    .download(&nam())
    .await
    .unwrap();
    assert_eq!(downloaded.extracted_to, Some(lib_dir.clone()));
    assert!(lib_dir.path_to(".gitattributes").is_existing_file());
    assert!(!dir.path_to("LibrePCB_Base.zip").is_existing_file());
}

#[tokio::test]
async fn test_request_library_list_pagination() {
    let server = MockServer::start().await;
    let lib = |uuid: &str, version: &str| {
        json!({
            "uuid": uuid,
            "name": {"default": "Lib"},
            "version": version,
            "recommended": null,
            "icon_url": null,
            "download_size": -1,
        })
    };
    Mock::given(path("/api/v1/libraries/v2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "next": format!("{}/page2", server.uri()),
            "results": [
                lib("a9ddf0c6-9b1c-4730-b300-01b4f192ad40", "1.0"),
                lib("invalid uuid", "1.0"),
                lib("6ccc516c-21b7-4cd5-9cf2-7a04cfa361c6", "invalid version"),
            ],
        })))
        .mount(&server)
        .await;
    Mock::given(path("/page2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "next": null,
            "results": [lib("6ccc516c-21b7-4cd5-9cf2-7a04cfa361c6", "2.0")],
        })))
        .mount(&server)
        .await;
    let endpoint = ApiEndpoint::new(url(&server, ""));
    let pages = library_list(&endpoint).await.unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].len(), 1); // Invalid entries are skipped.
    assert_eq!(pages[1].len(), 1);
    let lib = &pages[0][0];
    assert_eq!(lib.name, "Lib");
    assert_eq!(lib.description, "");
    assert!(!lib.recommended);
    assert_eq!(lib.icon_url, None);
    assert_eq!(lib.download_size, None);
}

#[tokio::test]
async fn test_request_library_list_errors() {
    let server = MockServer::start().await;
    let endpoint = ApiEndpoint::new(url(&server, ""));

    Mock::given(path("/api/v1/libraries/v2"))
        .respond_with(ResponseTemplate::new(200).set_body_string("no json"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    assert!(matches!(
        library_list(&endpoint).await,
        Err(Error::InvalidJson)
    ));

    Mock::given(path("/api/v1/libraries/v2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": null})))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    assert!(matches!(
        library_list(&endpoint).await,
        Err(Error::NoResults)
    ));

    // Nothing mounted anymore -> 404.
    assert!(matches!(
        library_list(&endpoint).await,
        Err(Error::Http { status: 404, .. })
    ));
}

#[tokio::test]
async fn test_request_parts_information() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/parts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "provider_name": "Test",
            "query_url": format!("{}/api/v1/parts/query", server.uri()),
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/parts/query"))
        .and(body_json(json!({
            "parts": [{"mpn": "1N4148", "manufacturer": "Vishay"}],
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"parts": []})))
        .mount(&server)
        .await;

    let endpoint = ApiEndpoint::new(url(&server, ""));
    let status = endpoint
        .request_parts_information_status(&nam())
        .await
        .unwrap();
    assert_eq!(status["provider_name"], "Test");

    let query_url = status["query_url"].as_str().unwrap().parse().unwrap();
    let parts = [Part {
        mpn: "1N4148".into(),
        manufacturer: "Vishay".into(),
    }];
    let info = endpoint
        .request_parts_information(&nam(), query_url, &parts)
        .await
        .unwrap();
    assert_eq!(info["parts"], json!([]));
}
