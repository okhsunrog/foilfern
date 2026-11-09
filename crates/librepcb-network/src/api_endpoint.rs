//! Port of libs/librepcb/core/network/apiendpoint.{h,cpp}.
//!
//! Access to a LibrePCB API server (`/api/v1/...`). JSON is parsed with
//! `serde`; like upstream, library list entries with an invalid UUID or
//! version are skipped (and logged), missing or mistyped fields fall back to
//! empty values.

use std::collections::HashSet;

use librepcb_core::types::{Uuid, Version};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::network_access_manager::NetworkAccessManager;
use crate::network_request::NetworkRequest;

/// A part to request information for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Part {
    /// Manufacturer part number.
    pub mpn: String,
    /// Manufacturer name.
    pub manufacturer: String,
}

/// A library provided by the API server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    /// Library UUID.
    pub uuid: Uuid,
    /// Name (default locale).
    pub name: String,
    /// Description (default locale).
    pub description: String,
    /// Author.
    pub author: String,
    /// Library version.
    pub version: Version,
    /// Whether the library is recommended.
    pub recommended: bool,
    /// UUIDs of libraries this one depends on.
    pub dependencies: HashSet<Uuid>,
    /// URL of the library icon.
    pub icon_url: Option<Url>,
    /// URL of the library ZIP.
    pub download_url: Option<Url>,
    /// Size of the library ZIP in bytes, if known.
    pub download_size: Option<u64>,
    /// SHA-256 of the library ZIP (hex string, may be empty).
    pub download_sha256: String,
    /// URL of the library's source repository (the API's `url`, e.g. a
    /// GitHub repository), if any.
    pub repository_url: Option<Url>,
}

/// Localized string as sent by the server (`{"default": "..."}`).
#[derive(Debug, Default, Deserialize)]
struct LocalizedString {
    #[serde(default)]
    default: Option<String>,
}

/// Library list entry as sent by the server; everything is optional to
/// tolerate missing or `null` values (upstream `QJsonValue` semantics).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawLibrary {
    uuid: Option<String>,
    name: Option<LocalizedString>,
    description: Option<LocalizedString>,
    author: Option<String>,
    version: Option<String>,
    recommended: Option<bool>,
    dependencies: Option<Vec<Value>>,
    icon_url: Option<String>,
    download_url: Option<String>,
    download_size: Option<Value>,
    download_sha256: Option<String>,
    url: Option<String>,
}

impl RawLibrary {
    fn into_library(self) -> Option<Library> {
        let Some(uuid) = self.uuid.as_deref().and_then(|s| s.parse::<Uuid>().ok()) else {
            log::error!("Invalid UUID received: {:?}", self.uuid);
            return None;
        };
        let Some(version) = self
            .version
            .as_deref()
            .and_then(|s| s.parse::<Version>().ok())
        else {
            log::error!("Invalid version received: {:?}", self.version);
            return None;
        };
        let dependencies = self
            .dependencies
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| {
                let uuid = value.as_str().and_then(|s| s.parse::<Uuid>().ok());
                if uuid.is_none() {
                    log::warn!("Invalid library dependency UUID: {value}");
                }
                uuid
            })
            .collect();
        let url = |s: Option<String>| s.and_then(|s| Url::parse(&s).ok());
        Some(Library {
            uuid,
            name: self.name.and_then(|n| n.default).unwrap_or_default(),
            description: self.description.and_then(|n| n.default).unwrap_or_default(),
            author: self.author.unwrap_or_default(),
            version,
            recommended: self.recommended.unwrap_or(false),
            dependencies,
            icon_url: url(self.icon_url),
            download_url: url(self.download_url),
            // Like QJsonValue::toInt(-1): only integral numbers are valid.
            download_size: self.download_size.as_ref().and_then(Value::as_u64),
            download_sha256: self.download_sha256.unwrap_or_default(),
            repository_url: url(self.url),
        })
    }
}

/// Parses a non-empty JSON object (upstream: `QJsonDocument::fromJson()` +
/// checks).
pub(crate) fn parse_object(data: &[u8]) -> Option<Map<String, Value>> {
    match serde_json::from_slice(data) {
        Ok(Value::Object(obj)) if !obj.is_empty() => Some(obj),
        _ => None,
    }
}

/// Adds the JSON `Accept` headers to a request.
fn accept_json(request: NetworkRequest) -> NetworkRequest {
    request
        .header("Accept", "application/json;charset=UTF-8")
        .header("Accept-Charset", "UTF-8")
}

/// A LibrePCB API endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiEndpoint {
    url: Url,
}

impl ApiEndpoint {
    /// Creates an endpoint for the given server URL (e.g.
    /// `https://api.librepcb.org`).
    pub fn new(url: Url) -> Self {
        Self { url }
    }

    /// Returns the server URL.
    pub fn url(&self) -> &Url {
        &self.url
    }

    fn endpoint(&self, path: &str) -> Result<Url> {
        let url = format!("{}{path}", self.url.as_str().trim_end_matches('/'));
        Url::parse(&url).map_err(|e| Error::Transport(e.to_string()))
    }

    /// Requests the list of libraries for the file format version of `nam`.
    ///
    /// The list is paginated; `on_libraries` is called for each received
    /// page (upstream `libraryListReceived()`), so partial results are
    /// delivered even if a later page fails.
    pub async fn request_library_list(
        &self,
        nam: &NetworkAccessManager,
        mut on_libraries: impl FnMut(Vec<Library>),
    ) -> Result<()> {
        let path = format!("/api/v1/libraries/v{}", nam.info().file_format_version);
        let mut url = Some(self.endpoint(&path)?);
        while let Some(current) = url.take() {
            let reply = accept_json(NetworkRequest::get(current)).send(nam).await?;
            let obj = parse_object(&reply.data).ok_or(Error::InvalidJson)?;
            if let Some(Value::String(next)) = obj.get("next") {
                match Url::parse(next) {
                    Ok(next) => {
                        log::debug!("Request more results from API endpoint {next}...");
                        url = Some(next);
                    }
                    Err(_) => log::warn!("Invalid URL in received JSON object: {next}"),
                }
            }
            let Some(Value::Array(results)) = obj.get("results") else {
                return Err(Error::NoResults);
            };
            let libraries = results
                .iter()
                .filter_map(|item| {
                    RawLibrary::deserialize(item)
                        .unwrap_or_default()
                        .into_library()
                })
                .collect();
            on_libraries(libraries);
        }
        Ok(())
    }

    /// Requests the parts information status (`/api/v1/parts`).
    pub async fn request_parts_information_status(
        &self,
        nam: &NetworkAccessManager,
    ) -> Result<Map<String, Value>> {
        let request = accept_json(NetworkRequest::get(self.endpoint("/api/v1/parts")?));
        let reply = request.send(nam).await?;
        parse_object(&reply.data).ok_or(Error::InvalidJson)
    }

    /// Requests information about parts from `url` (as provided by the
    /// parts information status).
    pub async fn request_parts_information(
        &self,
        nam: &NetworkAccessManager,
        url: Url,
        parts: &[Part],
    ) -> Result<Map<String, Value>> {
        #[derive(Serialize)]
        struct Body<'a> {
            parts: &'a [Part],
        }
        let post_data =
            serde_json::to_vec(&Body { parts }).map_err(|e| Error::Transport(e.to_string()))?;
        let request = accept_json(NetworkRequest::post(url, post_data))
            .header("Content-Type", "application/json");
        let reply = request.send(nam).await?;
        parse_object(&reply.data).ok_or(Error::InvalidJson)
    }
}
