//! Port of tests/unittests/core/network/networkrequesttest.cpp.

use foilfern_network::{Error, NetworkRequest, Url};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{body_bytes, header, method, path};
use wiremock::{Mock, ResponseTemplate};

use crate::{data_url, nam, record_progress, server, url};

struct Data {
    url: Url,
    accept: &'static str,
    content_start: &'static str,
    success: bool,
}

fn test_data() -> Vec<Data> {
    vec![
        Data {
            url: data_url("unittests/librepcbcommon/NetworkRequestTest/libraries"),
            accept: "application/json",
            content_start: "{",
            success: true,
        },
        Data {
            url: Url::parse("file:///some-invalid-url").unwrap(),
            accept: "text/html",
            content_start: "",
            success: false,
        },
    ]
}

#[test]
fn test_constructor_and_setters_and_destructor() {
    for data in test_data() {
        let request = NetworkRequest::get(data.url).expected_reply_content_size(5);
        drop(request);
    }
}

#[tokio::test]
async fn test_download() {
    let nam = nam();
    for data in test_data() {
        let mut request = NetworkRequest::get(data.url.clone());
        if !data.accept.is_empty() {
            request = request.header("Accept", data.accept);
        }
        let progress = record_progress(request.subscribe_progress());
        let result = request.send(&nam).await;
        let progress = progress.await.unwrap();

        assert!(!progress.is_empty());
        if data.success {
            let reply = result.unwrap();
            assert!(progress.iter().any(|p| p.bytes > 0));
            assert_eq!(
                progress.last().unwrap().state,
                "Request successfully finished."
            );
            assert!(!reply.data.is_empty());
            assert!(reply.content_type.is_none());
            let text = String::from_utf8(reply.data).unwrap();
            assert!(text.trim().starts_with(data.content_start));
        } else {
            let err = result.unwrap_err();
            assert!(!err.to_string().is_empty());
            assert!(err.to_string().starts_with("Error opening "), "{err}");
            assert!(
                progress
                    .last()
                    .unwrap()
                    .state
                    .starts_with("Request failed: ")
            );
        }
    }
}

#[tokio::test]
async fn test_http_get() {
    let server = server().await;
    let request = NetworkRequest::get(url(&server, "/api/v1/libraries/v2"))
        .header("Accept", "application/json");
    let progress = record_progress(request.subscribe_progress());
    let reply = request.send(&nam()).await.unwrap();
    let progress = progress.await.unwrap();
    assert!(String::from_utf8(reply.data).unwrap().starts_with('{'));
    assert!(progress.iter().any(|p| p.percent == 100 || p.bytes > 0));

    // Default headers sent to the server.
    let requests = server.received_requests().await.unwrap();
    let headers = &requests[0].headers;
    assert_eq!(headers.get("x-foilfern-appversion").unwrap(), "2.0.0-test");
    assert_eq!(headers.get("x-librepcb-gitrevision").unwrap(), "0123abc");
    assert_eq!(headers.get("x-librepcb-fileformatversion").unwrap(), "2");
    assert_eq!(headers.get("accept-language").unwrap(), "de_CH");
    assert_eq!(headers.get("accept").unwrap(), "application/json");
    assert!(
        headers
            .get("user-agent")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("LibrePCB/2.0.0-test (")
    );
}

#[tokio::test]
async fn test_http_content_type() {
    let server = server().await;
    Mock::given(path("/typed"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("{}", "application/json"))
        .mount(&server)
        .await;
    let reply = NetworkRequest::get(url(&server, "/typed"))
        .send(&nam())
        .await
        .unwrap();
    assert_eq!(reply.content_type.as_deref(), Some("application/json"));
}

#[tokio::test]
async fn test_browser_user_agent() {
    let server = server().await;
    NetworkRequest::get(url(&server, "/api/v1/libraries/v2"))
        .browser_user_agent()
        .send(&nam())
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    let agents: Vec<_> = requests[0].headers.get_all("user-agent").iter().collect();
    assert_eq!(agents.len(), 1);
    assert!(agents[0].to_str().unwrap().starts_with("Mozilla/5.0"));
}

#[tokio::test]
async fn test_http_not_found() {
    let server = server().await;
    let err = NetworkRequest::get(url(&server, "/nonexistent"))
        .send(&nam())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Http { status: 404, .. }), "{err}");
    assert!(err.to_string().contains("Not Found"), "{err}");
}

#[tokio::test]
async fn test_redirect() {
    let server = server().await;
    Mock::given(path("/redirect"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/api/v1/libraries/v2"))
        .mount(&server)
        .await;
    let reply = NetworkRequest::get(url(&server, "/redirect"))
        .send(&nam())
        .await
        .unwrap();
    assert!(String::from_utf8(reply.data).unwrap().starts_with('{'));
    let paths: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| r.url.path().to_owned())
        .collect();
    assert_eq!(paths, ["/redirect", "/api/v1/libraries/v2"]);
}

#[tokio::test]
async fn test_redirect_loop() {
    let server = server().await;
    Mock::given(path("/loop"))
        .respond_with(ResponseTemplate::new(301).insert_header("Location", "/loop"))
        .mount(&server)
        .await;
    let err = NetworkRequest::get(url(&server, "/loop"))
        .send(&nam())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::RedirectLoop), "{err}");
}

#[tokio::test]
async fn test_too_many_redirects() {
    let server = server().await;
    for i in 0..20 {
        Mock::given(path(format!("/r{i}")))
            .respond_with(
                ResponseTemplate::new(307).insert_header("Location", format!("/r{}", i + 1)),
            )
            .mount(&server)
            .await;
    }
    let err = NetworkRequest::get(url(&server, "/r0"))
        .send(&nam())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::TooManyRedirects), "{err}");
}

#[tokio::test]
async fn test_post_with_upload_progress() {
    let server = server().await;
    let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    Mock::given(method("POST"))
        .and(path("/echo"))
        .and(header("content-length", "200000"))
        .and(body_bytes(data.clone()))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;
    let request = NetworkRequest::post(url(&server, "/echo"), data).header("Content-Length", "1"); // Must be ignored.
    let progress = record_progress(request.subscribe_progress());
    let reply = request.send(&nam()).await.unwrap();
    assert_eq!(reply.data, b"ok");
    let progress = progress.await.unwrap();
    assert!(
        progress
            .iter()
            .any(|p| p.percent == 100 && p.bytes == 200_000)
    );
    assert!(progress.iter().any(|p| p.state.starts_with("Send data: ")));
}

#[tokio::test]
async fn test_cancellation_token() {
    let server = server().await;
    Mock::given(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(30)))
        .mount(&server)
        .await;
    let token = CancellationToken::new();
    let request = NetworkRequest::get(url(&server, "/slow")).cancellation_token(token.clone());
    let progress = record_progress(request.subscribe_progress());
    let task = tokio::spawn({
        let nam = nam();
        async move { request.send(&nam).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    token.cancel();
    let err = task.await.unwrap().unwrap_err();
    assert!(matches!(err, Error::Aborted), "{err}");
    let progress = progress.await.unwrap();
    assert_eq!(progress.last().unwrap().state, "Request aborted.");
}

#[tokio::test]
async fn test_unsupported_scheme() {
    let err = NetworkRequest::get(Url::parse("ftp://example.com/x").unwrap())
        .send(&nam())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::UnsupportedScheme(_)), "{err}");
}
