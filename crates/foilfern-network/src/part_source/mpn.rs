//! Exact MPN resolution through the tscircuit JLC index; raw EasyEDA source is
//! still authoritative. No fuzzy result, stock or price selects a component.

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::{
    CachePolicy, EasyEdaProvider, Error, PartSource, canonical_lcsc, fetch, publish, read_current,
};

const MAX_SEARCH_BYTES: usize = 4 * 1024 * 1024;
const SEARCH_LIMIT: usize = 100;

/// Unique exact supplier selection, with original index snapshot retained.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedMpn {
    /// Requested exact manufacturer part number.
    pub mpn: String,
    /// Canonical supplier identity.
    pub lcsc: String,
    /// Search URL (index evidence, not the authoritative CAD source).
    pub source_url: String,
    /// SHA-256 of the original index response.
    pub sha256: String,
    /// Whether resolution made no HTTP request.
    pub from_cache: bool,
}

fn selection(bytes: &[u8], mpn: &str) -> Result<String, Error> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let components = value["components"]
        .as_array()
        .ok_or_else(|| Error::Invalid("Supplier index has no components list.".into()))?;
    if components.len() >= SEARCH_LIMIT {
        return Err(Error::Invalid(
            "Supplier index results may be truncated; pass an explicit LCSC ID.".into(),
        ));
    }
    let mut matches = BTreeSet::new();
    for component in components {
        // The index's `mfr` field means manufacturer part number, not maker.
        if component["mfr"]
            .as_str()
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(mpn))
        {
            let id = component["lcsc"].as_u64().ok_or_else(|| {
                Error::Invalid("Exact MPN result has no numeric LCSC identity.".into())
            })?;
            matches.insert(canonical_lcsc(&format!("C{id}"))?);
        }
    }
    match matches.len() {
        0 => Err(Error::NoMatch(mpn.into())),
        1 => matches
            .into_iter()
            .next()
            .ok_or_else(|| Error::NoMatch(mpn.into())),
        _ => Err(Error::Ambiguous {
            mpn: mpn.into(),
            candidates: matches.into_iter().collect(),
        }),
    }
}

impl EasyEdaProvider {
    /// Overrides the supplier index root for a mirror or local test server.
    pub fn with_search_base(mut self, base: &str) -> Result<Self, Error> {
        let base = reqwest::Url::parse(base).map_err(|e| Error::Invalid(e.to_string()))?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err(Error::Invalid("Index endpoint must use HTTP(S).".into()));
        }
        self.search_base = base;
        Ok(self)
    }

    /// Resolves only an exact MPN. Duplicate exact supplier IDs are ambiguous;
    /// fuzzy names and possibly truncated responses never pick the first item.
    pub async fn resolve_mpn(
        &self,
        mpn: &str,
        cache: &Path,
        policy: CachePolicy,
        cancel: CancellationToken,
    ) -> Result<ResolvedMpn, Error> {
        let mpn = mpn.trim();
        if mpn.is_empty() || mpn.len() > 200 || mpn.chars().any(char::is_control) {
            return Err(Error::Invalid(
                "MPN must contain 1–200 bytes without control characters.".into(),
            ));
        }
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let key = hex::encode(Sha256::digest(mpn.to_ascii_uppercase().as_bytes()));
        let directory = cache.join("jlcsearch-mpn-v1").join(key);
        let mut url = self
            .search_base
            .join("api/search")
            .map_err(|e| Error::Invalid(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("q", mpn)
            .append_pair("limit", &SEARCH_LIMIT.to_string());
        let cached = read_current(directory.clone(), "json", MAX_SEARCH_BYTES).await?;
        let (hash, bytes, from_cache) =
            if !matches!(policy, CachePolicy::Refresh) && cached.is_some() {
                let (hash, bytes) =
                    cached.ok_or_else(|| Error::Invalid("Cached index disappeared.".into()))?;
                (hash, bytes, true)
            } else {
                if matches!(policy, CachePolicy::Offline) {
                    return Err(Error::Offline(mpn.into()));
                }
                let bytes = fetch(
                    &self.manager,
                    url.clone(),
                    self.search_base.as_str(),
                    MAX_SEARCH_BYTES,
                    cancel.clone(),
                )
                .await?;
                (hex::encode(Sha256::digest(&bytes)), bytes, false)
            };
        let lcsc = selection(&bytes, mpn)?;
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if !from_cache {
            publish(directory, "json", hash.clone(), bytes).await?;
        }
        Ok(ResolvedMpn {
            mpn: mpn.into(),
            lcsc,
            source_url: url.into(),
            sha256: hash,
            from_cache,
        })
    }

    /// Acquires a unique exact MPN and independently verifies the returned CAD
    /// source's MPN. A stale/mistaken index cannot substitute a different part.
    pub async fn acquire_mpn(
        &self,
        mpn: &str,
        cache: &Path,
        policy: CachePolicy,
        cancel: CancellationToken,
    ) -> Result<PartSource, Error> {
        let resolved = self.resolve_mpn(mpn, cache, policy, cancel.clone()).await?;
        let source = self.acquire(&resolved.lcsc, cache, policy, cancel).await?;
        if !source.mpn.eq_ignore_ascii_case(&resolved.mpn) {
            return Err(Error::Invalid(format!(
                "Supplier index resolved {:?} to {}, but CAD source reports {:?}.",
                resolved.mpn, resolved.lcsc, source.mpn
            )));
        }
        Ok(source)
    }
}
