//! The "download library" tab.
//!
//! Port of libs/librepcb/editor/library/downloadlibrarytab.{h,cpp}: the
//! URL of a library ZIP (with suggestions for GitHub and GitLab
//! repositories) and the directory (below
//! `<workspace>/data/libraries/local/`) are validated while typing;
//! "Download" downloads and extracts the library in the background
//! ([`crate::network::download_library()`]) with progress, "Cancel" aborts
//! it.

use librepcb_app_ui as ui;
use librepcb_core::fileio::FilePath;
use librepcb_i18n::tr;
use librepcb_network::Url;

use super::create_library::directory_for;
use super::{TabId, TabRequest, TabUpdate};
use crate::network::DownloadHandle;
use crate::validation;

/// The "download library" tab (upstream `DownloadLibraryTab`).
pub struct DownloadLibraryTab {
    id: TabId,
    local_libraries: FilePath,
    data: ui::DownloadLibraryTabData,
    url: Option<Url>,
    directory: Option<FilePath>,
    download: Option<DownloadHandle>,
}

/// What the application does with a finished download.
#[derive(Debug, Clone, PartialEq)]
pub enum DownloadResult {
    /// The library was extracted to the directory.
    Done(FilePath),
    /// The download failed.
    Failed(String),
}

impl DownloadLibraryTab {
    /// A new tab downloading libraries into `local_libraries`.
    pub fn new(local_libraries: FilePath) -> Self {
        let mut tab = Self {
            id: TabId::new(),
            local_libraries,
            data: ui::DownloadLibraryTabData::default(),
            url: None,
            directory: None,
            download: None,
        };
        tab.validate();
        tab
    }

    /// The unique tab identifier.
    pub fn id(&self) -> TabId {
        self.id
    }

    /// The `TabData`.
    pub fn ui_data(&self) -> ui::TabData {
        ui::TabData {
            r#type: ui::TabType::DownloadLibrary,
            title: tr!("librepcb::editor::DownloadLibraryTab", "Download Library").into(),
            ..Default::default()
        }
    }

    /// The `DownloadLibraryTabData`.
    pub fn derived_ui_data(&self) -> ui::DownloadLibraryTabData {
        self.data.clone()
    }

    /// The URL and destination directory, if valid.
    pub fn target(&self) -> Option<(Url, FilePath)> {
        Some((self.url.clone()?, self.directory.clone()?))
    }

    /// Applies the data written by the UI.
    pub fn set_derived_ui_data(&mut self, data: &ui::DownloadLibraryTabData) -> TabUpdate {
        let running = self.data.download_running;
        let progress = self.data.download_progress;
        let status = self.data.download_status.clone();
        self.data = data.clone();
        // Backend state.
        self.data.download_running = running;
        self.data.download_progress = progress;
        self.data.download_status = status;
        self.validate();
        TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        }
    }

    fn validate(&mut self) {
        let d = &mut self.data;
        let url_str = d.url.to_string();
        let mut error = String::new();
        self.url = validation::url(&url_str, &mut error, false);
        d.url_error = error.as_str().into();
        d.url_suggestion = self
            .url
            .as_ref()
            .and_then(url_suggestion)
            .unwrap_or_default()
            .into();
        let name = library_name_of_url(&url_str, self.url.as_ref());
        let (default, directory) =
            directory_for(&self.local_libraries, &name, &d.directory, &mut error);
        d.directory_default = default.into();
        self.directory = directory;
        d.directory_error = error.as_str().into();
        d.valid = self.url.is_some() && self.directory.is_some();
    }

    /// Handles a tab action (upstream `DownloadLibraryTab::trigger()`).
    pub fn trigger(&mut self, action: ui::TabAction) -> TabUpdate {
        match action {
            ui::TabAction::Cancel => {
                if let Some(download) = self.download.take() {
                    download.cancel();
                    self.data.download_running = false;
                    self.data.download_progress = 0;
                    self.data.download_status = Default::default();
                    TabUpdate {
                        data_changed: true,
                        ..TabUpdate::default()
                    }
                } else {
                    TabUpdate {
                        close: true,
                        ..TabUpdate::default()
                    }
                }
            }
            ui::TabAction::Accept => match self.target() {
                Some((url, dir)) if self.download.is_none() && !dir.is_existing_dir() => {
                    self.data.download_running = true;
                    self.data.download_progress = 0;
                    TabUpdate {
                        data_changed: true,
                        requests: vec![TabRequest::DownloadLibrary { url, dir }],
                        ..TabUpdate::default()
                    }
                }
                _ => TabUpdate::default(),
            },
            _ => TabUpdate::default(),
        }
    }

    /// The download was started by the application.
    pub fn set_download(&mut self, handle: Option<DownloadHandle>) {
        self.download = handle;
    }

    /// Progress of the download.
    pub fn download_progress(&mut self, status: String, percent: i32) -> TabUpdate {
        if self.download.is_none() {
            return TabUpdate::default();
        }
        self.data.download_status = status.into();
        self.data.download_progress = percent;
        TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        }
    }

    /// The download finished (upstream `downloadFinished()`): on success,
    /// the tab closes and the library is highlighted in the libraries
    /// panel after the rescan.
    pub fn download_finished(&mut self, result: DownloadResult) -> TabUpdate {
        if self.download.take().is_none() {
            return TabUpdate::default();
        }
        match result {
            DownloadResult::Done(dir) => TabUpdate {
                close: true,
                requests: vec![TabRequest::LibraryDownloaded(dir)],
                ..TabUpdate::default()
            },
            DownloadResult::Failed(error) => {
                self.data.download_status = error.into();
                self.data.download_running = false;
                self.data.download_progress = 0;
                TabUpdate {
                    data_changed: true,
                    ..TabUpdate::default()
                }
            }
        }
    }
}

/// Upstream: a suggestion for the ZIP URL of GitHub and GitLab
/// repositories.
fn url_suggestion(url: &Url) -> Option<String> {
    let s = url.as_str();
    if s.ends_with(".zip") {
        return None;
    }
    let base = s.trim_end_matches('/');
    let host = url.host_str()?.to_lowercase();
    if host.contains("github") {
        Some(format!("{base}/archive/refs/heads/master.zip"))
    } else if host.contains("gitlab") {
        let repo = url.path().split('/').rfind(|p| !p.is_empty())?;
        Some(format!("{base}/-/archive/master/{repo}-master.zip"))
    } else {
        None
    }
}

/// Upstream: the library name derived from the URL (for the default
/// directory).
fn library_name_of_url(url_str: &str, url: Option<&Url>) -> String {
    let lower = url_str.to_lowercase();
    let left = match lower.find(".lplib") {
        Some(i) => &url_str[..i],
        None => url_str,
    };
    let mut name = match left.rfind('/') {
        Some(i) => left[i..].to_owned(),
        None => left.to_owned(),
    };
    if name == url_str {
        name = url
            .and_then(|u| u.path_segments()?.next_back().map(str::to_owned))
            .unwrap_or_default();
    }
    for s in ["-master", "-main", ".zip", ".lplib"] {
        name = name.replace(s, "");
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions_and_names() {
        let url = Url::parse("https://github.com/LibrePCB-Libraries/LibrePCB_Base.lplib").unwrap();
        assert_eq!(
            url_suggestion(&url).as_deref(),
            Some(
                "https://github.com/LibrePCB-Libraries/LibrePCB_Base.lplib/archive/refs/heads/master.zip"
            )
        );
        let url = Url::parse("https://gitlab.com/foo/bar").unwrap();
        assert_eq!(
            url_suggestion(&url).as_deref(),
            Some("https://gitlab.com/foo/bar/-/archive/master/bar-master.zip")
        );
        assert_eq!(
            library_name_of_url(
                "https://github.com/LibrePCB-Libraries/LibrePCB_Base.lplib/archive/master.zip",
                None
            ),
            "/LibrePCB_Base"
        );
        assert_eq!(
            library_name_of_url("https://example.com/foo-main.zip", None),
            "/foo"
        );
    }
}
