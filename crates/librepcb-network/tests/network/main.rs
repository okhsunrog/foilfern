//! Port of tests/unittests/core/network/*.cpp, plus tests of the API
//! classes (not covered upstream).
//!
//! Upstream uses `file://` URLs only; those vectors are kept, and the HTTP
//! code paths are tested against a local [`wiremock`] server which serves
//! `tests/data/server` like the funq test server of upstream (no internet
//! access).

mod api_endpoint_test;
mod file_download_test;
mod library_download_test;
mod network_request_test;
mod order_pcb_api_request_test;

use std::path::{Path, PathBuf};

use librepcb_network::{ClientInfo, NetworkAccessManager, Progress, Url};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Returns the upstream test data directory (see `LIBREPCB_UPSTREAM_DIR` in
/// `.cargo/config.toml`).
pub fn data_dir() -> PathBuf {
    Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data")
}

/// Returns a `file://` URL for a file in the test data directory.
pub fn data_url(rel: &str) -> Url {
    Url::from_file_path(data_dir().join(rel).canonicalize().unwrap()).unwrap()
}

/// Creates a network access manager with test client information.
pub fn nam() -> NetworkAccessManager {
    NetworkAccessManager::new(ClientInfo {
        app_version: "2.0.0-test".into(),
        git_revision: "0123abc".into(),
        file_format_version: "2".parse().unwrap(),
        locale: "de_CH".into(),
    })
    .unwrap()
}

/// Serves files of a directory (like Python's `SimpleHTTPRequestHandler`).
struct DirResponder(PathBuf);

impl Respond for DirResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let rel = request.url.path().trim_start_matches('/');
        match std::fs::read(self.0.join(rel)) {
            Ok(content) => ResponseTemplate::new(200).set_body_bytes(content),
            Err(_) => ResponseTemplate::new(404),
        }
    }
}

/// Starts a local HTTP server serving `tests/data/server` (lowest priority,
/// so tests can mount more specific mocks).
pub async fn server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(DirResponder(data_dir().join("server")))
        .with_priority(u8::MAX)
        .mount(&server)
        .await;
    server
}

/// Returns a URL on the mock server.
pub fn url(server: &MockServer, path: &str) -> Url {
    Url::parse(&format!("{}{path}", server.uri())).unwrap()
}

/// Records all published progress states (upstream: counting signals).
pub fn record_progress(mut rx: watch::Receiver<Progress>) -> JoinHandle<Vec<Progress>> {
    tokio::spawn(async move {
        let mut all = Vec::new();
        while rx.changed().await.is_ok() {
            let p = rx.borrow_and_update().clone();
            assert!(!p.state.is_empty());
            assert!(p.percent <= 100);
            all.push(p);
        }
        all
    })
}
