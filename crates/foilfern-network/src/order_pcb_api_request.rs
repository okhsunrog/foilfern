//! Port of libs/librepcb/core/network/orderpcbapirequest.{h,cpp}.
//!
//! Two-step PCB order through a LibrePCB API server: first request the
//! service information ([`OrderPcbApiRequest::request_info()`]), then upload
//! the project ([`OrderPcbApiRequest::upload()`]) and open the returned URL
//! in the browser.

use base64::Engine;
use reqwest::Url;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::watch;

use crate::api_endpoint::parse_object;
use crate::error::{Error, Result};
use crate::network_access_manager::NetworkAccessManager;
use crate::network_request::{NetworkRequest, Progress, format_file_size};

/// Service information received from the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderInfo {
    /// URL with information about the service, if provided.
    pub info_url: Option<Url>,
    /// Maximum project size in bytes, if limited.
    pub max_file_size: Option<u64>,
}

/// PCB order request (see the [module docs](self)).
#[derive(Debug, Clone)]
pub struct OrderPcbApiRequest {
    api_server_url: Url,
    info_url: Option<Url>,
    upload_url: Option<Url>,
    max_file_size: Option<u64>,
    redirect_url: Option<Url>,
}

impl OrderPcbApiRequest {
    /// Creates a request for the given API server (workspace setting).
    pub fn new(api_server_url: Url) -> Self {
        Self {
            api_server_url,
            info_url: None,
            upload_url: None,
            max_file_size: None,
            redirect_url: None,
        }
    }

    /// Returns whether the information for the upload was received.
    pub fn is_ready_for_upload(&self) -> bool {
        self.upload_url.is_some()
    }

    /// Returns the received service information URL.
    pub fn info_url(&self) -> Option<&Url> {
        self.info_url.as_ref()
    }

    /// Returns the received upload URL.
    pub fn upload_url(&self) -> Option<&Url> {
        self.upload_url.as_ref()
    }

    /// Returns the received maximum project size in bytes.
    pub fn max_file_size(&self) -> Option<u64> {
        self.max_file_size
    }

    /// Returns the received URL where to continue the order process.
    pub fn redirect_url(&self) -> Option<&Url> {
        self.redirect_url.as_ref()
    }

    /// Requests the upload information from the API server.
    pub async fn request_info(&mut self, nam: &NetworkAccessManager) -> Result<OrderInfo> {
        let url = format!(
            "{}/api/v1/order",
            self.api_server_url.as_str().trim_end_matches('/')
        );
        let url = Url::parse(&url).map_err(|e| Error::Transport(e.to_string()))?;
        let reply = NetworkRequest::get(url)
            .header("Accept", "application/json;charset=UTF-8")
            .header("Accept-Charset", "UTF-8")
            .send(nam)
            .await?;
        log_json(&reply.data);
        let obj = parse_object(&reply.data).ok_or(Error::InvalidOrderJson)?;
        let url = |key| {
            obj.get(key)
                .and_then(Value::as_str)
                .and_then(|s| Url::parse(s).ok())
        };
        self.info_url = url("info_url");
        self.upload_url = url("upload_url");
        self.max_file_size = obj.get("max_size").and_then(Value::as_u64);
        if self.upload_url.is_none() {
            // No or invalid upload_url -> consider it as "service not
            // available" so the server can just remove upload_url when out
            // of service.
            return Err(Error::ServiceNotAvailable);
        }
        Ok(OrderInfo {
            info_url: self.info_url.clone(),
            max_file_size: self.max_file_size,
        })
    }

    /// Uploads the project (`*.lppz` content) and returns the URL to be
    /// opened in the web browser.
    ///
    /// `board_path` is the pre-selected board (e.g.
    /// `boards/default/board.lp`), if known. Upload progress is published to
    /// `progress`, if given.
    pub async fn upload(
        &mut self,
        nam: &NetworkAccessManager,
        lppz: &[u8],
        board_path: Option<&str>,
        progress: Option<&watch::Sender<Progress>>,
    ) -> Result<Url> {
        self.redirect_url = None;

        let upload_url = self.upload_url.clone().ok_or(Error::UploadUrlUnknown)?;
        if let Some(max) = self.max_file_size
            && max > 0
            && lppz.len() as u64 > max
        {
            return Err(Error::ProjectTooLarge(format_file_size(lppz.len() as u64)));
        }

        #[derive(Serialize)]
        struct Body<'a> {
            board: Option<&'a str>,
            project: String,
        }
        let body = Body {
            board: board_path.filter(|p| !p.is_empty()),
            project: base64::engine::general_purpose::STANDARD.encode(lppz),
        };
        let post_data = serde_json::to_vec(&body).map_err(|e| Error::Transport(e.to_string()))?;
        let request = NetworkRequest::post(upload_url, post_data)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json;charset=UTF-8")
            .header("Accept-Charset", "UTF-8");
        let reply = match progress {
            Some(sender) => {
                let mut rx = request.subscribe_progress();
                let final_rx = rx.clone();
                let forward = async {
                    while rx.changed().await.is_ok() {
                        sender.send_replace(rx.borrow_and_update().clone());
                    }
                    std::future::pending::<()>().await;
                };
                let reply = tokio::select! {
                    reply = request.send(nam) => reply,
                    () = forward => Err(Error::Task("progress forwarding ended".into())),
                };
                sender.send_replace(final_rx.borrow().clone());
                reply
            }
            None => request.send(nam).await,
        }?;
        log_json(&reply.data);
        let obj = parse_object(&reply.data).ok_or(Error::InvalidOrderJson)?;
        let redirect_url = obj
            .get("redirect_url")
            .and_then(Value::as_str)
            .and_then(|s| Url::parse(s).ok())
            .ok_or(Error::InvalidOrderRedirect)?;
        self.redirect_url = Some(redirect_url.clone());
        Ok(redirect_url)
    }
}

fn log_json(data: &[u8]) {
    let text = String::from_utf8_lossy(&data[..data.len().min(500)]).replace('\n', " ");
    log::debug!("Received JSON: {text}");
}
