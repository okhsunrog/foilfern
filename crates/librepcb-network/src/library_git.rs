//! Installation of a library from its git repository (no upstream
//! counterpart): the fallback of [`LibraryDownload`](crate::LibraryDownload)
//! for environments where the ZIP download is blocked (e.g. proxies which
//! refuse `codeload.github.com`) but `git` works.
//!
//! The repository is fetched with the `git` executable (shallow, at the
//! commit of the ZIP download URL if it names one, else the default
//! branch) into `<dir>.git-tmp`, the `.git` directory is removed so the
//! result looks like an extracted ZIP, and the library found in it
//! atomically replaces `<dir>` (rename). Other directories of the same
//! library are removed afterwards, like after a ZIP installation.

use std::collections::HashSet;
use std::path::Path;

use librepcb_core::fileio::{FilePath, file_utils};
use reqwest::Url;
use tokio::process::Command;

use crate::error::{Error, Result};
use crate::library_download::find_library_in_zip;

/// Returns whether a `git` executable is available on the `PATH`.
pub async fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .await
        .is_ok_and(|out| out.status.success())
}

/// Returns the commit SHA a ZIP download URL refers to, if its last path
/// segment is one (e.g. `https://codeload.github.com/<org>/<repo>/legacy.zip/<sha>`).
pub fn commit_of_download_url(url: &Url) -> Option<String> {
    let last = url.path_segments()?.rev().find(|s| !s.is_empty())?;
    (last.len() == 40 && last.chars().all(|c| c.is_ascii_hexdigit())).then(|| last.to_lowercase())
}

async fn git(args: &[&str], dir: Option<&Path>) -> Result<()> {
    let mut cmd = Command::new("git");
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    let out = cmd
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| Error::Git(format!("could not run git: {e}")))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(Error::Git(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

fn remove_dir(dir: &FilePath) -> Result<()> {
    if dir.is_existing_dir() {
        file_utils::remove_dir_recursively(dir)?;
    }
    Ok(())
}

/// Fetches the repository into `tmp` (shallow, at `commit` if given and
/// available, else the default branch).
async fn fetch(repository: &Url, commit: Option<&str>, tmp: &FilePath) -> Result<()> {
    let tmp_path = tmp.as_path();
    if let Some(commit) = commit {
        let pinned = async {
            std::fs::create_dir_all(tmp_path)
                .map_err(|e| Error::Git(format!("could not create {}: {e}", tmp.to_native())))?;
            git(&["init", "-q"], Some(tmp_path)).await?;
            git(
                &["fetch", "-q", "--depth", "1", repository.as_str(), commit],
                Some(tmp_path),
            )
            .await?;
            git(&["checkout", "-q", "FETCH_HEAD"], Some(tmp_path)).await
        }
        .await;
        match pinned {
            Ok(()) => return Ok(()),
            Err(e) => {
                log::warn!("Fetching commit {commit} of {repository} failed ({e}), cloning.");
                remove_dir(tmp)?;
            }
        }
    }
    let tmp_native = tmp.to_native();
    git(
        &[
            "clone",
            "-q",
            "--depth",
            "1",
            repository.as_str(),
            tmp_native.as_str(),
        ],
        None,
    )
    .await
}

/// Installs the library of the git repository `repository` into
/// `dest_dir` (replaced if it exists, see the [module docs](self)) and
/// removes the other directories `existing_dirs` of the same library.
/// Returns the library directory.
pub async fn clone_library(
    repository: &Url,
    commit: Option<&str>,
    dest_dir: &FilePath,
    existing_dirs: &HashSet<FilePath>,
) -> Result<FilePath> {
    let tmp = FilePath::new(format!("{dest_dir}.git-tmp"))
        .ok_or_else(|| librepcb_core::fileio::Error::InvalidPath(format!("{dest_dir}.git-tmp")))?;
    remove_dir(&tmp)?;
    if let Some(parent) = tmp.parent_dir()
        && !parent.is_existing_dir()
    {
        file_utils::make_path(&parent)?;
    }
    let result = async {
        fetch(repository, commit, &tmp).await?;
        remove_dir(&tmp.path_to(".git"))?;
        let library_dir = find_library_in_zip(&tmp)?;
        remove_dir(dest_dir)?;
        std::fs::rename(library_dir.as_path(), dest_dir.as_path()).map_err(|e| {
            Error::Git(format!(
                "could not move {} to {}: {e}",
                library_dir.to_native(),
                dest_dir.to_native()
            ))
        })?;
        Ok::<(), Error>(())
    }
    .await;
    // Remove the remains (everything if it failed).
    if let Err(e) = remove_dir(&tmp) {
        log::warn!("Failed to remove {}: {e}", tmp.to_native());
    }
    result?;
    for fp in existing_dirs.iter().filter(|fp| *fp != dest_dir) {
        log::debug!("Removing existing library directory: {}", fp.to_native());
        if let Err(e) = file_utils::remove_dir_recursively(fp) {
            log::warn!("Failed to remove {}: {e}", fp.to_native());
        }
    }
    Ok(dest_dir.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_of_url() {
        let url = Url::parse(
            "https://codeload.github.com/LibrePCB-Libraries/LibrePCB_Base.lplib/legacy.zip/\
             1ba65b6cdf698d82b58cdf0490427d131a1cd0fa",
        )
        .unwrap();
        assert_eq!(
            commit_of_download_url(&url).as_deref(),
            Some("1ba65b6cdf698d82b58cdf0490427d131a1cd0fa")
        );
        let url = Url::parse("https://example.com/lib/archive/master.zip").unwrap();
        assert_eq!(commit_of_download_url(&url), None);
    }
}
