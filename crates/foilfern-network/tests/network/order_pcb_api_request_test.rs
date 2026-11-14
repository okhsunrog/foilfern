//! Tests of `OrderPcbApiRequest` (no upstream unit tests).

use base64::Engine;
use foilfern_network::{Error, OrderPcbApiRequest, Progress};
use serde_json::{Value, json};
use tokio::sync::watch;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::{nam, url};

async fn mount_info(server: &MockServer, info: Value) {
    Mock::given(method("GET"))
        .and(path("/api/v1/order"))
        .respond_with(ResponseTemplate::new(200).set_body_json(info))
        .mount(server)
        .await;
}

#[tokio::test]
async fn test_order() {
    let server = MockServer::start().await;
    mount_info(
        &server,
        json!({
            "info_url": "https://example.com/info",
            "upload_url": format!("{}/upload", server.uri()),
            "max_size": 1000,
        }),
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/upload"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"redirect_url": "https://example.com/order/42"})),
        )
        .mount(&server)
        .await;

    let mut request = OrderPcbApiRequest::new(url(&server, ""));
    assert!(!request.is_ready_for_upload());
    let info = request.request_info(&nam()).await.unwrap();
    assert!(request.is_ready_for_upload());
    assert_eq!(info.info_url.unwrap().as_str(), "https://example.com/info");
    assert_eq!(info.max_file_size, Some(1000));

    let (tx, rx) = watch::channel(Progress::default());
    let redirect = request
        .upload(
            &nam(),
            b"lppz content",
            Some("boards/default/board.lp"),
            Some(&tx),
        )
        .await
        .unwrap();
    assert_eq!(redirect.as_str(), "https://example.com/order/42");
    assert_eq!(request.redirect_url(), Some(&redirect));
    assert_eq!(rx.borrow().state, "Request successfully finished.");

    let requests = server.received_requests().await.unwrap();
    let upload = requests.iter().find(|r| r.url.path() == "/upload").unwrap();
    let body: Value = upload.body_json().unwrap();
    assert_eq!(body["board"], "boards/default/board.lp");
    let project = base64::engine::general_purpose::STANDARD
        .decode(body["project"].as_str().unwrap())
        .unwrap();
    assert_eq!(project, b"lppz content");

    // Too large project.
    let err = request
        .upload(&nam(), &[0; 1001], None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::ProjectTooLarge(_)), "{err}");
}

#[tokio::test]
async fn test_upload_without_info() {
    let server = MockServer::start().await;
    let mut request = OrderPcbApiRequest::new(url(&server, ""));
    let err = request.upload(&nam(), b"x", None, None).await.unwrap_err();
    assert!(matches!(err, Error::UploadUrlUnknown), "{err}");
}

#[tokio::test]
async fn test_service_not_available() {
    let server = MockServer::start().await;
    mount_info(&server, json!({"info_url": "https://example.com/info"})).await;
    let mut request = OrderPcbApiRequest::new(url(&server, ""));
    let err = request.request_info(&nam()).await.unwrap_err();
    assert!(matches!(err, Error::ServiceNotAvailable), "{err}");
    assert!(!request.is_ready_for_upload());
    assert_eq!(
        request.info_url().unwrap().as_str(),
        "https://example.com/info"
    );
}

#[tokio::test]
async fn test_invalid_upload_response() {
    let server = MockServer::start().await;
    mount_info(
        &server,
        json!({"upload_url": format!("{}/upload", server.uri())}),
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/upload"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"redirect_url": "no url"})))
        .mount(&server)
        .await;
    let mut request = OrderPcbApiRequest::new(url(&server, ""));
    let info = request.request_info(&nam()).await.unwrap();
    assert_eq!(info.info_url, None);
    assert_eq!(info.max_file_size, None);
    let err = request.upload(&nam(), b"x", None, None).await.unwrap_err();
    assert!(matches!(err, Error::InvalidOrderRedirect), "{err}");
    assert_eq!(request.redirect_url(), None);
}
