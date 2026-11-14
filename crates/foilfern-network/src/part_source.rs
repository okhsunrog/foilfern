//! Acquisition of exact supplier part source snapshots (no upstream counterpart).
//!
//! Retains the provider's original JSON, including provenance/license metadata,
//! for offline conversion. Geometry conversion belongs in foilfern-import. The
//! source's free-form descriptions are data, never executable instructions.

use std::path::{Path, PathBuf};
use std::time::Duration;

use foilfern_core::fileio::{FilePath, file_utils};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::NetworkAccessManager;

const MAX_SOURCE_BYTES: usize = 32 * 1024 * 1024;

/// Failure to acquire or validate a supplier snapshot.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Invalid supplier identity or malformed/mismatched source data.
    #[error("Invalid part source: {0}")]
    Invalid(String),
    /// A snapshot was requested offline but is absent.
    #[error("No cached source for {0}; offline acquisition cannot fetch it.")]
    Offline(String),
    /// HTTP, TLS or timeout failure.
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    /// Invalid provider JSON.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Cache read/write failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Atomic cache write failure.
    #[error(transparent)]
    File(#[from] foilfern_core::fileio::Error),
    /// Acquisition was cancelled.
    #[error("Part acquisition cancelled.")]
    Cancelled,
    /// A cache worker failed unexpectedly.
    #[error("Part cache worker failed: {0}")]
    Worker(String),
}

/// Cache semantics for reproducible part acquisition.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CachePolicy {
    /// Use the current verified snapshot, fetching only when absent.
    #[default]
    PreferCache,
    /// Require a current snapshot and make no HTTP requests.
    Offline,
    /// Fetch a new snapshot, retaining previous content-addressed versions.
    Refresh,
}

/// A validated raw supplier snapshot; not yet an imported library element.
#[derive(Debug, Clone, Serialize)]
pub struct PartSource {
    /// Canonical LCSC ID.
    pub lcsc: String,
    /// Manufacturer part number reported by the source.
    pub mpn: String,
    /// Manufacturer as reported by the source.
    pub manufacturer: String,
    /// Provider URL of this exact supplier part.
    pub source_url: String,
    /// Source attribution, retained alongside the original source metadata.
    pub attribution: String,
    /// Official source homepages.
    pub source_links: [&'static str; 2],
    /// SHA-256 of the original response bytes.
    pub sha256: String,
    /// Previously current hash when a refresh changed the source.
    pub changed_from: Option<String>,
    /// Whether the network was avoided.
    pub from_cache: bool,
    /// Content-addressed JSON file for offline conversion.
    pub snapshot: PathBuf,
    /// Original provider result, without dropping fields or license text.
    pub raw: Value,
}

/// Exact-ID source acquisition using the application's HTTP client.
#[derive(Debug, Clone)]
pub struct EasyEdaProvider {
    manager: NetworkAccessManager,
    base: reqwest::Url,
}

impl EasyEdaProvider {
    /// Creates the official provider.
    pub fn new(manager: NetworkAccessManager) -> Result<Self, Error> {
        Self::with_base(manager, "https://easyeda.com/")
    }

    /// Creates a provider with an explicit endpoint root (also used by mock tests).
    pub fn with_base(manager: NetworkAccessManager, base: &str) -> Result<Self, Error> {
        let base = reqwest::Url::parse(base).map_err(|e| Error::Invalid(e.to_string()))?;
        if !matches!(base.scheme(), "https" | "http") {
            return Err(Error::Invalid("Provider URL must use HTTP(S).".into()));
        }
        Ok(Self { manager, base })
    }

    /// Fetches or replays a validated source. No library/project is modified.
    pub async fn acquire(
        &self,
        lcsc: &str,
        cache: &Path,
        policy: CachePolicy,
        cancel: CancellationToken,
    ) -> Result<PartSource, Error> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let lcsc = canonical_lcsc(lcsc)?;
        let directory = cache.join("easyeda-v1").join(&lcsc);
        let cached = read_current(directory.clone()).await?;
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if !matches!(policy, CachePolicy::Refresh) {
            if let Some((hash, bytes)) = cached {
                return source(
                    &lcsc,
                    &self.url(&lcsc)?,
                    &directory,
                    hash,
                    bytes,
                    true,
                    None,
                );
            }
            if matches!(policy, CachePolicy::Offline) {
                return Err(Error::Offline(lcsc));
            }
        }
        let url = self.url(&lcsc)?;
        let request = self
            .manager
            .client()
            .get(url.clone())
            .header("Referer", self.base.as_str())
            .timeout(Duration::from_secs(30));
        let fetch = async {
            let response = request.send().await?.error_for_status()?;
            if response
                .content_length()
                .is_some_and(|n| n > MAX_SOURCE_BYTES as u64)
            {
                return Err(Error::Invalid("Source exceeds the 32 MiB limit.".into()));
            }
            let mut stream = response.bytes_stream();
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                if chunk.len() > MAX_SOURCE_BYTES.saturating_sub(bytes.len()) {
                    return Err(Error::Invalid("Source exceeds the 32 MiB limit.".into()));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok::<_, Error>(bytes)
        };
        let bytes = tokio::select! {
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            result = fetch => result?,
        };
        let hash = hex::encode(Sha256::digest(&bytes));
        let changed_from = cached
            .as_ref()
            .map(|(hash, _)| hash.clone())
            .filter(|old| old != &hash);
        let result = source(
            &lcsc,
            &url,
            &directory,
            hash.clone(),
            bytes.clone(),
            false,
            changed_from,
        )?;
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        tokio::task::spawn_blocking(move || {
            let snapshot = FilePath::new(directory.join("snapshots").join(format!("{hash}.json")))
                .ok_or_else(|| Error::Invalid("Invalid cache snapshot path.".into()))?;
            file_utils::write_file(&snapshot, &bytes)?;
            let current = FilePath::new(directory.join("current"))
                .ok_or_else(|| Error::Invalid("Invalid cache index path.".into()))?;
            file_utils::write_file(&current, hash.as_bytes())?;
            Ok::<_, Error>(())
        })
        .await
        .map_err(|e| Error::Worker(e.to_string()))??;
        Ok(result)
    }

    fn url(&self, lcsc: &str) -> Result<reqwest::Url, Error> {
        self.base
            .join(&format!("api/products/{lcsc}/components?version=6.4.19.5"))
            .map_err(|e| Error::Invalid(e.to_string()))
    }
}

/// Validates and normalizes an LCSC identifier before it is used in URLs/paths.
pub fn canonical_lcsc(value: &str) -> Result<String, Error> {
    let value = value.trim();
    let digits = value
        .strip_prefix('C')
        .or_else(|| value.strip_prefix('c'))
        .ok_or_else(|| Error::Invalid("LCSC ID must be C followed by digits.".into()))?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Invalid(
            "LCSC ID must be C followed by digits.".into(),
        ));
    }
    let number: u64 = digits
        .parse()
        .map_err(|_| Error::Invalid("LCSC ID is too large.".into()))?;
    if number == 0 {
        return Err(Error::Invalid("LCSC ID must not be zero.".into()));
    }
    Ok(format!("C{number}"))
}

async fn read_current(directory: PathBuf) -> Result<Option<(String, Vec<u8>)>, Error> {
    tokio::task::spawn_blocking(move || {
        let hash = match std::fs::read_to_string(directory.join("current")) {
            Ok(hash) => hash,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            return Err(Error::Invalid("Invalid cache snapshot hash.".into()));
        }
        let path = directory.join("snapshots").join(format!("{hash}.json"));
        if std::fs::metadata(&path)?.len() > MAX_SOURCE_BYTES as u64 {
            return Err(Error::Invalid(
                "Cached source exceeds the size limit.".into(),
            ));
        }
        let bytes = std::fs::read(path)?;
        if hex::encode(Sha256::digest(&bytes)) != hash {
            return Err(Error::Invalid(
                "Cached source checksum does not match.".into(),
            ));
        }
        Ok(Some((hash, bytes)))
    })
    .await
    .map_err(|e| Error::Worker(e.to_string()))?
}

fn source(
    lcsc: &str,
    url: &reqwest::Url,
    directory: &Path,
    hash: String,
    bytes: Vec<u8>,
    from_cache: bool,
    changed_from: Option<String>,
) -> Result<PartSource, Error> {
    let envelope: Value = serde_json::from_slice(&bytes)?;
    if envelope["success"] != true {
        return Err(Error::Invalid("Provider did not return success.".into()));
    }
    let raw = envelope
        .get("result")
        .filter(|value| value.is_object())
        .ok_or_else(|| Error::Invalid("Provider result is missing.".into()))?
        .clone();
    let parameters = &raw["dataStr"]["head"]["c_para"];
    let text = |key: &str| {
        parameters[key]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    let mpn = text("Manufacturer Part");
    if mpn.is_empty() {
        return Err(Error::Invalid(
            "Source has no manufacturer part number.".into(),
        ));
    }
    let supplier_id = text("Supplier Part");
    if canonical_lcsc(&supplier_id)? != lcsc {
        return Err(Error::Invalid(
            "Source supplier ID does not match the requested part.".into(),
        ));
    }
    if !raw["dataStr"]["shape"].is_array() || !raw["packageDetail"]["dataStr"]["shape"].is_array() {
        return Err(Error::Invalid(
            "Source is missing symbol or footprint shapes.".into(),
        ));
    }
    Ok(PartSource {
        lcsc: lcsc.to_owned(),
        mpn,
        manufacturer: text("Manufacturer"),
        source_url: url.to_string(),
        attribution: "JLCEDA/EasyEDA Official Library".into(),
        source_links: ["https://lceda.cn/", "https://easyeda.com/"],
        snapshot: directory.join("snapshots").join(format!("{hash}.json")),
        sha256: hash,
        changed_from,
        from_cache,
        raw,
    })
}
