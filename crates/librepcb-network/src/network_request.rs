//! Port of libs/librepcb/core/network/networkrequestbase.{h,cpp} and
//! libs/librepcb/core/network/networkrequest.{h,cpp}.
//!
//! Upstream requests are `QObject`s moved into the network thread, reporting
//! through signals. Here a request is a builder whose `send()` future
//! performs it on the caller's tokio runtime:
//!
//! - progress (upstream `progressState()`, `progressPercent()`,
//!   `progress()`) is published through a [`tokio::sync::watch`] channel
//!   ([`NetworkRequest::subscribe_progress()`]);
//! - the result (upstream `succeeded()`/`errored()`/`dataReceived()`) is the
//!   return value;
//! - cancellation (upstream `abort()`/`aborted()`) is dropping the future, or
//!   a [`CancellationToken`] which makes the request fail with
//!   [`Error::Aborted`].
//!
//! Like upstream, redirections are followed manually (with loop detection
//! and a limit of 10), and `file://` URLs are supported besides HTTP(S).
//! Both are handwritten since `reqwest` supports neither upstream's
//! redirect rules nor `file://`; the progress estimation and
//! `format_file_size()` reproduce upstream's formulas and format.

use std::future::Future;
use std::sync::Arc;

use futures_util::StreamExt;
use librepcb_i18n::tr;
use reqwest::Url;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, LOCATION, USER_AGENT};
use tokio::io::AsyncReadExt;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::network_access_manager::{BROWSER_USER_AGENT, NetworkAccessManager};

/// Maximum reply size of a [`NetworkRequest`] (100 MB).
const MAX_REPLY_SIZE: usize = 100 * 1000 * 1000;

/// Chunk size for reading local files and uploading data.
const CHUNK_SIZE: usize = 64 * 1024;

/// Progress of a request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    /// Short description of the current state (upstream `progressState()`).
    pub state: String,
    /// Bytes received (or sent, for uploads) so far.
    pub bytes: u64,
    /// Total bytes if known.
    pub total: Option<u64>,
    /// Estimated progress in percent, 0..=100.
    pub percent: u8,
}

/// Publishes [`Progress`] updates.
#[derive(Debug, Clone)]
pub(crate) struct ProgressReporter(Arc<watch::Sender<Progress>>);

impl ProgressReporter {
    fn new() -> Self {
        Self(Arc::new(watch::Sender::new(Progress::default())))
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<Progress> {
        self.0.subscribe()
    }

    pub(crate) fn state(&self, state: String) {
        self.0.send_modify(|p| p.state = state);
    }

    fn transfer(&self, state: String, bytes: u64, total: Option<u64>, percent: u8) {
        self.0.send_replace(Progress {
            state,
            bytes,
            total,
            percent,
        });
    }

    /// Reports download progress (upstream `replyDownloadProgressSlot()`).
    fn download(&self, received: u64, total: Option<u64>, expected: Option<u64>) {
        let received_i = i64::try_from(received).unwrap_or(i64::MAX);
        let mut estimated = total
            .filter(|&t| t > 0)
            .or(expected)
            .map_or(-1, |t| i64::try_from(t).unwrap_or(i64::MAX));
        if estimated < received_i {
            estimated = received_i.saturating_add(10_000_000);
        }
        let percent = percent(received_i, estimated);
        let state = tr!(
            "librepcb::NetworkRequestBase",
            "Receive data: {0}",
            format_file_size(received)
        );
        self.transfer(state, received, total, percent);
    }

    /// Reports upload progress (upstream `uploadProgressSlot()`).
    fn upload(&self, sent: u64, total: u64) {
        let sent_i = i64::try_from(sent).unwrap_or(i64::MAX);
        let mut total_i = i64::try_from(total).unwrap_or(i64::MAX);
        if total_i < sent_i {
            total_i = sent_i.saturating_add(10_000_000);
        }
        let percent = if sent_i == total_i {
            100
        } else {
            percent(sent_i, total_i)
        };
        let state = tr!(
            "librepcb::NetworkRequestBase",
            "Send data: {0}",
            format_file_size(sent)
        );
        self.transfer(state, sent, Some(total), percent);
    }
}

fn percent(value: i64, total: i64) -> u8 {
    let p = (100 * i128::from(value)) / i128::from(total.max(1));
    u8::try_from(p.clamp(0, 100)).unwrap_or(100)
}

/// Formats a size like upstream `formatFileSize()`, e.g. `"1.50 MB"`.
pub(crate) fn format_file_size(bytes: u64) -> String {
    let mut num = bytes as f64;
    let mut unit = "Bytes";
    for next in ["KB", "MB", "GB", "TB"] {
        if num < 1024.0 {
            break;
        }
        unit = next;
        num /= 1024.0;
    }
    format!("{num:.2} {unit}")
}

/// Receiver of reply data (upstream `prepareRequest()` / `fetchNewData()`).
pub(crate) trait ReplySink {
    /// Called before each (also redirected) request.
    async fn begin(&mut self) -> Result<()>;
    /// Called for each received chunk.
    async fn write(&mut self, chunk: &[u8]) -> Result<()>;
}

/// The reply of a successful [`NetworkRequest`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reply {
    /// The received content.
    pub data: Vec<u8>,
    /// The `Content-Type` header, if any.
    pub content_type: Option<String>,
}

/// General purpose network request (up to 100 MB), see the
/// [module docs](self).
#[derive(Debug, Clone)]
pub struct NetworkRequest {
    url: Url,
    post_data: Option<Vec<u8>>,
    headers: Vec<(String, String)>,
    expected_content_size: Option<u64>,
    browser_user_agent: bool,
    cancel: Option<CancellationToken>,
    progress: ProgressReporter,
}

impl NetworkRequest {
    /// Creates a GET request.
    pub fn get(url: Url) -> Self {
        Self {
            url,
            post_data: None,
            headers: Vec::new(),
            expected_content_size: None,
            browser_user_agent: false,
            cancel: None,
            progress: ProgressReporter::new(),
        }
    }

    /// Creates a POST request with the given body.
    pub fn post(url: Url, data: impl Into<Vec<u8>>) -> Self {
        Self {
            post_data: Some(data.into()),
            ..Self::get(url)
        }
    }

    /// Returns the requested URL.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Sets an HTTP header field (upstream `setHeaderField()`).
    ///
    /// `Content-Length` of POST requests is set automatically.
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// Sets the expected reply size, used to estimate the progress if the
    /// server does not send a `Content-Length`.
    pub fn expected_reply_content_size(mut self, bytes: u64) -> Self {
        self.expected_content_size = Some(bytes);
        self
    }

    /// Uses a typical browser user agent (some servers block other clients).
    pub fn browser_user_agent(mut self) -> Self {
        self.browser_user_agent = true;
        self
    }

    /// Makes the request fail with [`Error::Aborted`] when `token` is
    /// cancelled.
    pub fn cancellation_token(mut self, token: CancellationToken) -> Self {
        self.cancel = Some(token);
        self
    }

    /// Returns a receiver for progress updates.
    pub fn subscribe_progress(&self) -> watch::Receiver<Progress> {
        self.progress.subscribe()
    }

    pub(crate) fn reporter(&self) -> &ProgressReporter {
        &self.progress
    }

    /// Performs the request and returns the received data.
    pub async fn send(self, nam: &NetworkAccessManager) -> Result<Reply> {
        let result = self
            .run(async {
                let mut sink = MemorySink::default();
                let content_type = self.execute(nam, &mut sink).await?;
                Ok(Reply {
                    data: sink.data,
                    content_type,
                })
            })
            .await;
        self.finish(result)
    }

    /// Runs `fut` with cancellation support.
    pub(crate) async fn run<T>(&self, fut: impl Future<Output = Result<T>>) -> Result<T> {
        match &self.cancel {
            None => fut.await,
            Some(token) => {
                tokio::select! {
                    result = fut => result,
                    () = token.cancelled() => {
                        self.progress.state(tr!("librepcb::NetworkRequestBase", "Abort request..."));
                        Err(Error::Aborted)
                    }
                }
            }
        }
    }

    /// Publishes the final state (upstream `finalize()`).
    pub(crate) fn finish<T>(&self, result: Result<T>) -> Result<T> {
        match &result {
            Ok(_) => {
                log::debug!("Request succeeded: {}", self.url);
                self.progress.state(tr!(
                    "librepcb::NetworkRequestBase",
                    "Request successfully finished."
                ));
            }
            Err(Error::Aborted) => {
                log::debug!("Request aborted: {}", self.url);
                self.progress
                    .state(tr!("librepcb::NetworkRequestBase", "Request aborted."));
            }
            Err(e) => {
                log::error!("Request failed: {}: {e}", self.url);
                self.progress.state(tr!(
                    "librepcb::NetworkRequestBase",
                    "Request failed: {0}",
                    e
                ));
            }
        }
        result
    }

    /// Performs the request (following redirections) and streams the reply
    /// into `sink`. Returns the content type.
    pub(crate) async fn execute<S: ReplySink>(
        &self,
        nam: &NetworkAccessManager,
        sink: &mut S,
    ) -> Result<Option<String>> {
        self.progress
            .state(tr!("librepcb::NetworkRequestBase", "Start request..."));
        let mut url = self.url.clone();
        let mut redirected_urls: Vec<Url> = Vec::new();
        loop {
            self.progress
                .state(tr!("librepcb::NetworkRequestBase", "Request started..."));
            sink.begin().await?;

            if url.scheme() == "file" {
                self.read_local_file(&url, sink).await?;
                return Ok(None);
            }
            if !matches!(url.scheme(), "http" | "https") {
                return Err(Error::UnsupportedScheme(url.scheme().to_owned()));
            }

            let response = self.build(nam, &url)?.send().await?;
            let status = response.status();

            // Follow redirections manually.
            if status.is_redirection()
                && let Some(location) = response.headers().get(LOCATION)
            {
                let location = location
                    .to_str()
                    .map_err(|_| Error::InvalidRedirect(format!("{location:?}")))?;
                let target = url
                    .join(location)
                    .map_err(|_| Error::InvalidRedirect(location.to_owned()))?;
                if redirected_urls.contains(&target) {
                    return Err(Error::RedirectLoop);
                } else if redirected_urls.len() > 10 {
                    return Err(Error::TooManyRedirects);
                }
                log::debug!("Redirect from {url} to {target}.");
                self.progress.state(tr!(
                    "librepcb::NetworkRequestBase",
                    "Redirect to {0}...",
                    target
                ));
                redirected_urls.push(std::mem::replace(&mut url, target));
                continue;
            }

            if status.is_client_error() || status.is_server_error() {
                return Err(Error::Http {
                    url: url.to_string(),
                    status: status.as_u16(),
                    reason: status.canonical_reason().unwrap_or_default().to_owned(),
                });
            }

            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let total = response.content_length();
            let mut received = 0u64;
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                received += chunk.len() as u64;
                sink.write(&chunk).await?;
                if self.post_data.is_none() {
                    self.progress
                        .download(received, total, self.expected_content_size);
                }
            }
            if received == 0 && self.post_data.is_none() {
                self.progress.download(0, total, self.expected_content_size);
            }
            return Ok(content_type);
        }
    }

    fn build(&self, nam: &NetworkAccessManager, url: &Url) -> Result<reqwest::RequestBuilder> {
        let client = nam.client();
        let mut builder = match &self.post_data {
            Some(data) => {
                let total = data.len() as u64;
                let reporter = self.progress.clone();
                let mut sent = 0u64;
                let chunks: Vec<Vec<u8>> = data.chunks(CHUNK_SIZE).map(<[u8]>::to_vec).collect();
                let stream = futures_util::stream::iter(chunks).map(move |chunk| {
                    sent += chunk.len() as u64;
                    reporter.upload(sent, total);
                    Ok::<_, std::io::Error>(chunk)
                });
                client
                    .post(url.clone())
                    .header(CONTENT_LENGTH, total)
                    .body(reqwest::Body::wrap_stream(stream))
            }
            None => client.get(url.clone()),
        };
        for (name, value) in &self.headers {
            if name.eq_ignore_ascii_case(CONTENT_LENGTH.as_str()) {
                continue; // Set automatically.
            }
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| Error::InvalidHeader(name.clone()))?;
            let value = reqwest::header::HeaderValue::from_str(value)
                .map_err(|_| Error::InvalidHeader(name.to_string()))?;
            builder = builder.header(name, value);
        }
        if self.browser_user_agent {
            builder = builder.header(USER_AGENT, BROWSER_USER_AGENT);
        }
        Ok(builder)
    }

    async fn read_local_file<S: ReplySink>(&self, url: &Url, sink: &mut S) -> Result<()> {
        let path = url.to_file_path().map_err(|()| Error::FileOpen {
            path: url.path().to_owned(),
            message: "Invalid local file URL".into(),
        })?;
        let open_err = |message: String| Error::FileOpen {
            path: path.display().to_string(),
            message,
        };
        let mut file = tokio::fs::File::open(&path)
            .await
            .map_err(|e| open_err(e.to_string()))?;
        let metadata = file.metadata().await.map_err(|e| open_err(e.to_string()))?;
        if metadata.is_dir() {
            return Err(open_err("Path is a directory".into()));
        }
        let total = Some(metadata.len());
        let mut received = 0u64;
        let mut buf = vec![0; CHUNK_SIZE];
        loop {
            let n = file
                .read(&mut buf)
                .await
                .map_err(|e| open_err(e.to_string()))?;
            if n == 0 {
                break;
            }
            received += n as u64;
            sink.write(&buf[..n]).await?;
            self.progress
                .download(received, total, self.expected_content_size);
        }
        if received == 0 {
            self.progress.download(0, total, self.expected_content_size);
        }
        Ok(())
    }
}

/// Collects the reply in memory (upstream `NetworkRequest::fetchNewData()`).
#[derive(Default)]
struct MemorySink {
    data: Vec<u8>,
}

impl ReplySink for MemorySink {
    async fn begin(&mut self) -> Result<()> {
        self.data.clear();
        Ok(())
    }

    async fn write(&mut self, chunk: &[u8]) -> Result<()> {
        if self.data.len() + chunk.len() > MAX_REPLY_SIZE {
            return Err(Error::SizeLimitExceeded);
        }
        self.data.extend_from_slice(chunk);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_file_size() {
        assert_eq!(format_file_size(0), "0.00 Bytes");
        assert_eq!(format_file_size(1023), "1023.00 Bytes");
        assert_eq!(format_file_size(1536), "1.50 KB");
        assert_eq!(format_file_size(5 * 1024 * 1024), "5.00 MB");
    }

    #[test]
    fn test_percent() {
        assert_eq!(percent(0, -1), 0);
        assert_eq!(percent(50, 100), 50);
        assert_eq!(percent(100, 100), 100);
    }
}
