//! Content-addressed STEP assets referenced by an EasyEDA source snapshot.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

use super::{CachePolicy, EasyEdaProvider, Error, fetch, publish, read_current};

const MAX_MODEL_BYTES: usize = 128 * 1024 * 1024;

/// A verified cached STEP response; geometric validity is checked by the CAD importer.
#[derive(Debug, Clone, Serialize)]
pub struct StepAsset {
    /// Provider model UUID, distinct from native package model UUIDs.
    pub provider_uuid: String,
    /// Download URL.
    pub source_url: String,
    /// Content-addressed local STEP file.
    pub file: PathBuf,
    /// SHA-256 of the raw model bytes.
    pub sha256: String,
    /// Whether a cached model avoided a network request.
    pub from_cache: bool,
    /// Previous current hash if an explicit refresh changed model contents.
    pub changed_from: Option<String>,
}

impl EasyEdaProvider {
    /// Overrides the model endpoint root, for a mirror or local test server.
    pub fn with_model_base(mut self, base: &str) -> Result<Self, Error> {
        let base = reqwest::Url::parse(base).map_err(|e| Error::Invalid(e.to_string()))?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err(Error::Invalid("Model endpoint must use HTTP(S).".into()));
        }
        self.model_base = base;
        Ok(self)
    }

    /// Fetches an exact STEP asset without loading or modifying a native library.
    pub async fn step_asset(
        &self,
        uuid: &str,
        cache: &Path,
        policy: CachePolicy,
        cancel: CancellationToken,
    ) -> Result<StepAsset, Error> {
        let uuid = uuid.to_ascii_lowercase();
        let compact = uuid.replace('-', "");
        if compact.len() != 32
            || !compact.bytes().all(|b| b.is_ascii_hexdigit())
            || !(uuid.len() == 32
                || (uuid.len() == 36
                    && uuid.bytes().enumerate().all(|(i, b)| {
                        if matches!(i, 8 | 13 | 18 | 23) {
                            b == b'-'
                        } else {
                            b.is_ascii_hexdigit()
                        }
                    })))
        {
            return Err(Error::Invalid("Invalid provider model UUID.".into()));
        }
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let directory = cache.join("easyeda-step-v1").join(&uuid);
        let url = self
            .model_base
            .join(&format!("qAxj6KHrDKw4blvCG8QJPs7Y/{uuid}"))
            .map_err(|e| Error::Invalid(e.to_string()))?;
        let cached = read_current(directory.clone(), "step", MAX_MODEL_BYTES).await?;
        let (hash, bytes, from_cache, changed_from) =
            if !matches!(policy, CachePolicy::Refresh) && cached.is_some() {
                let (hash, bytes) =
                    cached.ok_or_else(|| Error::Invalid("Cached model disappeared.".into()))?;
                (hash, bytes, true, None)
            } else {
                if matches!(policy, CachePolicy::Offline) {
                    return Err(Error::Offline(uuid));
                }
                let bytes = fetch(
                    &self.manager,
                    url.clone(),
                    self.base.as_str(),
                    MAX_MODEL_BYTES,
                    cancel.clone(),
                )
                .await?;
                let hash = hex::encode(Sha256::digest(&bytes));
                let changed = cached.map(|(old, _)| old).filter(|old| old != &hash);
                (hash, bytes, false, changed)
            };
        if !bytes.trim_ascii_start().starts_with(b"ISO-10303-21;") {
            return Err(Error::Invalid(
                "Model endpoint did not return a STEP document.".into(),
            ));
        }
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let file = directory.join("snapshots").join(format!("{hash}.step"));
        if !from_cache {
            publish(directory, "step", hash.clone(), bytes).await?;
        }
        Ok(StepAsset {
            provider_uuid: uuid,
            source_url: url.to_string(),
            file,
            sha256: hash,
            from_cache,
            changed_from,
        })
    }
}
