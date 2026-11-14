//! Official KiCad snapshots with a local GitLab-compatible server.

use foilfern_network::kicad_source::{OfficialKiCad, Repository};
use foilfern_network::part_source::{CachePolicy, Error};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn exact_revision_and_files_replay_offline_and_detect_tampering() {
    let server = MockServer::start().await;
    let provider = OfficialKiCad::with_base(super::nam(), &server.uri()).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let commit = "1".repeat(40);
    Mock::given(path_regex(".*/commits/10.0.8$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":commit})))
        .mount(&server)
        .await;
    let revision = provider
        .resolve(
            Repository::Symbols,
            "10.0.8",
            cache.path(),
            CachePolicy::PreferCache,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(revision.commit, commit);
    assert!(
        revision
            .source_url
            .contains("kicad%2Flibraries%2Fkicad-symbols")
    );
    let path = "Interface_USB.kicad_symdir/CH340G.kicad_sym";
    let original = "(kicad_symbol_lib (version 20251024))";
    Mock::given(path_regex(".*/files/.*raw$"))
        .and(query_param("ref", commit.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_string(original))
        .mount(&server)
        .await;
    let file = provider
        .file(
            &revision,
            path,
            cache.path(),
            CachePolicy::PreferCache,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&file.file).unwrap(), original);
    assert!(
        file.source_url
            .contains("Interface_USB.kicad_symdir%2FCH340G.kicad_sym")
    );
    server.reset().await;
    let revision = provider
        .resolve(
            Repository::Symbols,
            "10.0.8",
            cache.path(),
            CachePolicy::Offline,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(revision.from_cache);
    assert!(
        provider
            .file(
                &revision,
                path,
                cache.path(),
                CachePolicy::Offline,
                CancellationToken::new()
            )
            .await
            .unwrap()
            .from_cache
    );
    assert!(server.received_requests().await.unwrap().is_empty());
    std::fs::write(&file.file, b"corrupt").unwrap();
    assert!(matches!(
        provider
            .file(
                &revision,
                path,
                cache.path(),
                CachePolicy::Offline,
                CancellationToken::new()
            )
            .await,
        Err(Error::Invalid(_))
    ));
}

#[tokio::test]
async fn failed_or_mismatched_revision_never_falls_back_to_another_branch() {
    let server = MockServer::start().await;
    let provider = OfficialKiCad::with_base(super::nam(), &server.uri()).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let commit = "2".repeat(40);
    Mock::given(path_regex(".*/commits/.*$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"3".repeat(40)})))
        .mount(&server)
        .await;
    assert!(matches!(
        provider
            .resolve(
                Repository::Symbols,
                &commit,
                cache.path(),
                CachePolicy::PreferCache,
                CancellationToken::new()
            )
            .await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    server.reset().await;
    assert!(matches!(
        provider
            .resolve(
                Repository::Symbols,
                &commit,
                cache.path(),
                CachePolicy::Offline,
                CancellationToken::new()
            )
            .await,
        Err(Error::Offline(_))
    ));
    assert!(
        provider
            .resolve(
                Repository::Symbols,
                "../bad",
                cache.path(),
                CachePolicy::PreferCache,
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    let revision = foilfern_network::kicad_source::Revision {
        repository: Repository::Symbols,
        reference: "test".into(),
        commit,
        source_url: String::new(),
        from_cache: false,
    };
    for path in [
        "/absolute",
        "../escape",
        "x/../escape",
        "x\\escape",
        "x//y",
        "",
    ] {
        assert!(
            provider
                .file(
                    &revision,
                    path,
                    cache.path(),
                    CachePolicy::PreferCache,
                    CancellationToken::new()
                )
                .await
                .is_err()
        );
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        provider
            .resolve(
                Repository::Symbols,
                "10.0.8",
                cache.path(),
                CachePolicy::PreferCache,
                cancel
            )
            .await,
        Err(Error::Cancelled)
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
#[ignore = "requires public GitLab source endpoints"]
async fn official_ch340g_snapshot_online() {
    let cache = tempfile::tempdir().unwrap();
    let provider = OfficialKiCad::new(super::nam()).unwrap();
    let revision = provider
        .resolve(
            Repository::Symbols,
            "10.0.8",
            cache.path(),
            CachePolicy::PreferCache,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let file = provider
        .file(
            &revision,
            "Interface_USB.kicad_symdir/CH340G.kicad_sym",
            cache.path(),
            CachePolicy::PreferCache,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(
        std::fs::read_to_string(&file.file)
            .unwrap()
            .contains("(symbol \"CH340G\"")
    );
    eprintln!("{} {} {}", revision.commit, file.sha256, file.source_url);
    for (repository, path, prefix) in [
        (
            Repository::Footprints,
            "Package_SO.pretty/SOIC-16_3.9x9.9mm_P1.27mm.kicad_mod",
            b"(footprint".as_slice(),
        ),
        (
            Repository::Models,
            "Package_SO.3dshapes/SOIC-16_3.9x9.9mm_P1.27mm.step",
            b"ISO-10303-21;".as_slice(),
        ),
    ] {
        let revision = provider
            .resolve(
                repository,
                "10.0.8",
                cache.path(),
                CachePolicy::PreferCache,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let file = provider
            .file(
                &revision,
                path,
                cache.path(),
                CachePolicy::PreferCache,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(
            std::fs::read(&file.file)
                .unwrap()
                .trim_ascii_start()
                .starts_with(prefix)
        );
        eprintln!("{} {} {}", revision.commit, file.sha256, file.source_url);
    }
}
