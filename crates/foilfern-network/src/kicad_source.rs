//! Pinned files from the official KiCad GitLab repositories (no upstream
//! LibrePCB counterpart). Shares bounded HTTP and verified snapshot storage
//! with supplier acquisition; parsing/conversion stays outside networking.

use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::NetworkAccessManager;
use crate::part_source::{CachePolicy, Error, fetch, publish, read_current};

/// Official repository, never a caller-provided repository URL.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Repository {
    /// Schematic symbols.
    Symbols,
    /// Native KiCad footprint files.
    Footprints,
    /// STEP models and their license documents.
    Models,
}

impl Repository {
    /// Official GitLab repository name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Symbols => "kicad-symbols",
            Self::Footprints => "kicad-footprints",
            Self::Models => "kicad-packages3D",
        }
    }
}

/// A resolved tag or commit; subsequent file fetches use only the exact commit.
#[derive(Debug, Clone, Serialize)]
pub struct Revision {
    /// Official repository identity.
    pub repository: Repository,
    /// Requested tag/commit (not silently replaced by another branch).
    pub reference: String,
    /// Exact resolved commit ID.
    pub commit: String,
    /// Metadata response URL.
    pub source_url: String,
    /// Whether revision resolution avoided HTTP.
    pub from_cache: bool,
}

/// Original file bytes in a verified content-addressed snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct SourceFile {
    /// Exact repository revision.
    pub revision: Revision,
    /// Original repository-relative file path.
    pub path: String,
    /// Exact download URL.
    pub source_url: String,
    /// SHA-256 of the raw bytes.
    pub sha256: String,
    /// Cached original bytes, not a normalized/converted file.
    pub file: PathBuf,
    /// Whether fetching this file avoided HTTP.
    pub from_cache: bool,
}

/// Official GitLab source acquisition, also supporting explicit mirrors in tests.
#[derive(Debug, Clone)]
pub struct OfficialKiCad {
    manager: NetworkAccessManager,
    base: reqwest::Url,
}

fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn valid_commit(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

impl OfficialKiCad {
    /// Uses gitlab.com, where the official libraries are hosted.
    pub fn new(manager: NetworkAccessManager) -> Result<Self, Error> {
        Self::with_base(manager, "https://gitlab.com/")
    }

    /// Explicit GitLab-compatible mirror root, useful for local HTTP tests.
    pub fn with_base(manager: NetworkAccessManager, base: &str) -> Result<Self, Error> {
        let base = reqwest::Url::parse(base).map_err(|e| Error::Invalid(e.to_string()))?;
        if !matches!(base.scheme(), "https" | "http") {
            return Err(Error::Invalid(
                "KiCad source endpoint must use HTTP(S).".into(),
            ));
        }
        Ok(Self { manager, base })
    }

    fn endpoint(&self, repo: Repository, tail: &[&str]) -> Result<reqwest::Url, Error> {
        let mut url = self
            .base
            .join("api/v4/projects/")
            .map_err(|e| Error::Invalid(e.to_string()))?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|()| Error::Invalid("Invalid GitLab API root.".into()))?;
            segments
                .pop_if_empty()
                .push(&format!("kicad/libraries/{}", repo.name()))
                .push("repository");
            for segment in tail {
                segments.push(segment);
            }
        }
        Ok(url)
    }

    /// Resolves a requested tag/commit and retains the raw response for offline
    /// replay. A failed resolution never falls back to a default branch.
    pub async fn resolve(
        &self,
        repo: Repository,
        reference: &str,
        cache: &Path,
        policy: CachePolicy,
        cancel: CancellationToken,
    ) -> Result<Revision, Error> {
        let reference = reference.trim();
        if reference.is_empty()
            || reference.len() > 100
            || !reference
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
            || matches!(reference, "." | "..")
        {
            return Err(Error::Invalid(
                "KiCad revision must be an exact tag or commit name.".into(),
            ));
        }
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let directory = cache
            .join("kicad-revisions-v1")
            .join(repo.name())
            .join(hash(reference.as_bytes()));
        let url = self.endpoint(repo, &["commits", reference])?;
        let cached = read_current(directory.clone(), "json", 4 * 1024 * 1024).await?;
        let (digest, bytes, from_cache) =
            if !matches!(policy, CachePolicy::Refresh) && cached.is_some() {
                let (digest, bytes) = cached
                    .ok_or_else(|| Error::Invalid("Cached KiCad revision disappeared.".into()))?;
                (digest, bytes, true)
            } else {
                if matches!(policy, CachePolicy::Offline) {
                    return Err(Error::Offline(format!("{}@{reference}", repo.name())));
                }
                let bytes = fetch(
                    &self.manager,
                    url.clone(),
                    self.base.as_str(),
                    4 * 1024 * 1024,
                    cancel.clone(),
                )
                .await?;
                (hash(&bytes), bytes, false)
            };
        let metadata: serde_json::Value = serde_json::from_slice(&bytes)?;
        let commit = metadata["id"]
            .as_str()
            .filter(|id| valid_commit(id))
            .ok_or_else(|| Error::Invalid("GitLab returned no exact commit ID.".into()))?
            .to_ascii_lowercase();
        if valid_commit(reference) && !reference.eq_ignore_ascii_case(&commit) {
            return Err(Error::Invalid(
                "GitLab commit does not match requested revision.".into(),
            ));
        }
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if !from_cache {
            publish(directory, "json", digest, bytes).await?;
        }
        Ok(Revision {
            repository: repo,
            reference: reference.into(),
            commit,
            source_url: url.into(),
            from_cache,
        })
    }

    /// Fetches one original repository file at the resolved commit. STEP assets
    /// are bounded to 128 MiB, other files to 32 MiB; all requests are cancellable.
    pub async fn file(
        &self,
        revision: &Revision,
        path: &str,
        cache: &Path,
        policy: CachePolicy,
        cancel: CancellationToken,
    ) -> Result<SourceFile, Error> {
        if !valid_commit(&revision.commit)
            || path.len() > 500
            || path.contains('\\')
            || path.chars().any(char::is_control)
            || path.split('/').any(|part| matches!(part, "" | "." | ".."))
        {
            return Err(Error::Invalid(
                "Invalid pinned KiCad repository file path.".into(),
            ));
        }
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let limit = if matches!(revision.repository, Repository::Models) {
            128 * 1024 * 1024
        } else {
            32 * 1024 * 1024
        };
        let directory = cache
            .join("kicad-files-v1")
            .join(revision.repository.name())
            .join(&revision.commit)
            .join(hash(path.as_bytes()));
        let mut url = self.endpoint(revision.repository, &["files", path, "raw"])?;
        url.query_pairs_mut().append_pair("ref", &revision.commit);
        let cached = read_current(directory.clone(), "bin", limit).await?;
        let (digest, bytes, from_cache) =
            if !matches!(policy, CachePolicy::Refresh) && cached.is_some() {
                let (digest, bytes) = cached
                    .ok_or_else(|| Error::Invalid("Cached KiCad file disappeared.".into()))?;
                (digest, bytes, true)
            } else {
                if matches!(policy, CachePolicy::Offline) {
                    return Err(Error::Offline(format!(
                        "{}@{}:{path}",
                        revision.repository.name(),
                        revision.commit
                    )));
                }
                let bytes = fetch(
                    &self.manager,
                    url.clone(),
                    self.base.as_str(),
                    limit,
                    cancel.clone(),
                )
                .await?;
                (hash(&bytes), bytes, false)
            };
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if !from_cache {
            publish(directory.clone(), "bin", digest.clone(), bytes).await?;
        }
        Ok(SourceFile {
            revision: revision.clone(),
            path: path.into(),
            source_url: url.into(),
            file: directory.join("snapshots").join(format!("{digest}.bin")),
            sha256: digest,
            from_cache,
        })
    }
}
