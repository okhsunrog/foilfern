//! Port of libs/librepcb/core/network/networkaccessmanager.{h,cpp}.
//!
//! Upstream runs a `QNetworkAccessManager` in a dedicated thread. Here it is
//! a cheaply clonable wrapper around a [`reqwest::Client`] (connection pool)
//! which runs on the caller's tokio runtime, plus the client information
//! sent with every request (user agent and `X-LibrePCB-*` headers, which
//! upstream takes from `Application`).
//!
//! Not ported: the HTTP disk cache (`QNetworkDiskCache`, cache load control
//! and minimum cache time), see COMPAT.md.

use foilfern_core::types::Version;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::error::{Error, Result};

/// Information about the application, sent to servers with every request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInfo {
    /// Application version (upstream `Application::getVersion()`).
    pub app_version: String,
    /// Git revision (upstream `Application::getGitRevision()`).
    pub git_revision: String,
    /// File format version (upstream
    /// `Application::getFileFormatVersion()`).
    pub file_format_version: Version,
    /// Locale name for `Accept-Language`, e.g. `de_CH` (upstream
    /// `QLocale().name()`).
    pub locale: String,
}

impl ClientInfo {
    /// Returns the default user agent, e.g.
    /// `LibrePCB/2.0.0 (linux; x86_64; de_CH)`.
    pub fn user_agent(&self) -> String {
        format!(
            "LibrePCB/{} ({}; {}; {})",
            self.app_version,
            std::env::consts::OS,
            std::env::consts::ARCH,
            self.locale
        )
    }
}

/// User agent used by [`NetworkRequest::browser_user_agent()`](crate::NetworkRequest::browser_user_agent).
pub(crate) const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0";

/// Shared HTTP client for all requests (see the [module docs](self)).
#[derive(Debug, Clone)]
pub struct NetworkAccessManager {
    client: reqwest::Client,
    info: ClientInfo,
}

impl NetworkAccessManager {
    /// Creates a new client.
    ///
    /// Redirections are not followed automatically, since requests handle
    /// them like upstream (loop detection, progress reporting).
    pub fn new(info: ClientInfo) -> Result<Self> {
        let header = |name: &'static str, value: &str| -> Result<(HeaderName, HeaderValue)> {
            let value =
                HeaderValue::from_str(value).map_err(|_| Error::InvalidHeader(name.to_owned()))?;
            Ok((HeaderName::from_static(name), value))
        };
        let headers: HeaderMap = [
            header("accept-language", &info.locale)?,
            header("x-librepcb-appversion", &info.app_version)?,
            header("x-librepcb-gitrevision", &info.git_revision)?,
            header(
                "x-librepcb-fileformatversion",
                &info.file_format_version.to_string(),
            )?,
        ]
        .into_iter()
        .collect();
        let client = reqwest::Client::builder()
            .user_agent(info.user_agent())
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self { client, info })
    }

    /// Returns the client information.
    pub fn info(&self) -> &ClientInfo {
        &self.info
    }

    /// Returns the underlying HTTP client.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }
}
