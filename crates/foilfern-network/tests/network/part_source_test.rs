//! Exact part source acquisition with a local provider, no live dependency.

use foilfern_network::part_source::{CachePolicy, EasyEdaProvider, Error, canonical_lcsc};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn response(mpn: &str, lcsc: &str) -> serde_json::Value {
    json!({"success":true,"result":{
        "dataStr":{"head":{"c_para":{"Manufacturer Part":mpn,"Manufacturer":"Test maker","Supplier Part":lcsc}},"shape":[]},
        "packageDetail":{"dataStr":{"head":{"license":"preserve source attribution"},"shape":[]}}
    }})
}

async fn mount(server: &MockServer, data: serde_json::Value) {
    Mock::given(path("/api/products/C8734/components"))
        .respond_with(ResponseTemplate::new(200).set_body_json(data))
        .mount(server)
        .await;
}

#[test]
fn identities_cannot_be_used_as_paths_or_urls() {
    assert_eq!(canonical_lcsc(" c008734 ").unwrap(), "C8734");
    for value in [
        "C0",
        "8734",
        "C../../bad",
        "C1?q=x",
        "C",
        "C1/2",
        "C-1",
        "C999999999999999999999999999",
    ] {
        assert!(canonical_lcsc(value).is_err(), "{value}");
    }
}

#[tokio::test]
async fn source_cache_replays_offline_and_keeps_changed_snapshots() {
    let server = MockServer::start().await;
    mount(&server, response("TEST-PART-1", "C8734")).await;
    let provider = EasyEdaProvider::with_base(super::nam(), &server.uri()).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let source = provider
        .acquire(
            "C8734",
            cache.path(),
            CachePolicy::PreferCache,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(source.mpn, "TEST-PART-1");
    assert!(!source.from_cache);
    assert!(source.snapshot.is_file());
    assert_eq!(
        source.raw["packageDetail"]["dataStr"]["head"]["license"],
        "preserve source attribution"
    );
    let offline = provider
        .acquire(
            "C8734",
            cache.path(),
            CachePolicy::Offline,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(offline.from_cache);
    assert_eq!(source.sha256, offline.sha256);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    server.reset().await;
    mount(&server, response("TEST-PART-2", "C8734")).await;
    let refreshed = provider
        .acquire(
            "C8734",
            cache.path(),
            CachePolicy::Refresh,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        refreshed.changed_from.as_deref(),
        Some(source.sha256.as_str())
    );
    assert!(source.snapshot.is_file());
    assert!(refreshed.snapshot.is_file());
    assert_ne!(source.snapshot, refreshed.snapshot);
    std::fs::write(&refreshed.snapshot, b"tampered").unwrap();
    let error = provider
        .acquire(
            "C8734",
            cache.path(),
            CachePolicy::Offline,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Invalid(_)));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
#[ignore = "requires the live EasyEDA product endpoint"]
async fn acquire_c8734_online() {
    let cache = tempfile::tempdir().unwrap();
    let provider = EasyEdaProvider::new(super::nam()).unwrap();
    let source = provider
        .acquire(
            "C8734",
            cache.path(),
            CachePolicy::Refresh,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(source.mpn, "STM32F103C8T6");
    assert!(source.raw["dataStr"]["shape"].as_array().unwrap().len() >= 48);
    assert!(source.snapshot.is_file());
    eprintln!("C8734 source SHA-256: {}", source.sha256);
}

#[tokio::test]
async fn invalid_sources_missing_cache_and_cancellation_do_not_publish_snapshots() {
    let server = MockServer::start().await;
    mount(&server, response("WRONG-PART", "C123")).await;
    let provider = EasyEdaProvider::with_base(super::nam(), &server.uri()).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let error = provider
        .acquire(
            "C8734",
            cache.path(),
            CachePolicy::Offline,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Offline(_)));
    assert!(server.received_requests().await.unwrap().is_empty());
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        provider
            .acquire("C8734", cache.path(), CachePolicy::Refresh, cancel)
            .await,
        Err(Error::Cancelled)
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
    assert!(matches!(
        provider
            .acquire(
                "C8734",
                cache.path(),
                CachePolicy::Refresh,
                CancellationToken::new()
            )
            .await,
        Err(Error::Invalid(_))
    ));
    assert!(!cache.path().join("easyeda-v1/C8734/current").exists());
}
