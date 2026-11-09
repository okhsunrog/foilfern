//! Network operations of the application (library list, icons, library
//! downloads) on a shared tokio runtime, with `librepcb-network` (upstream:
//! `NetworkAccessManager` on its own thread).
//!
//! Results are delivered on the UI thread with
//! `slint::invoke_from_event_loop()`.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use librepcb_core::fileio::FilePath;
use librepcb_core::types::Uuid;
use librepcb_network::{
    ApiEndpoint, ClientInfo, InstalledLibrary, Library, LibraryDownload, NetworkAccessManager,
    NetworkRequest, Url,
};

/// The runtime of the network operations (two worker threads), `None` if it
/// could not be created.
fn runtime() -> Option<&'static tokio::runtime::Runtime> {
    static RUNTIME: OnceLock<Option<tokio::runtime::Runtime>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("network")
                .enable_all()
                .build()
                .map_err(|e| log::error!("Failed to start the network runtime: {e}"))
                .ok()
        })
        .as_ref()
}

/// The shared network access manager (upstream `NetworkAccessManager`).
fn nam() -> Result<NetworkAccessManager, String> {
    static NAM: OnceLock<Result<NetworkAccessManager, String>> = OnceLock::new();
    NAM.get_or_init(|| {
        NetworkAccessManager::new(ClientInfo {
            app_version: crate::app::APP_VERSION.to_owned(),
            git_revision: String::new(),
            file_format_version: librepcb_core::application::file_format_version(),
            locale: librepcb_i18n::current_language().replace('-', "_"),
        })
        .map_err(|e| e.to_string())
    })
    .clone()
}

/// Runs `task` on the network runtime and delivers its result to `done` on
/// the UI thread.
fn spawn<T: Send + 'static>(
    task: impl Future<Output = T> + Send + 'static,
    done: impl FnOnce(T) + Send + 'static,
) {
    let Some(rt) = runtime() else {
        return;
    };
    rt.spawn(async move {
        let result = task.await;
        if let Err(e) = slint::invoke_from_event_loop(move || done(result)) {
            log::debug!("Network result not delivered: {e}");
        }
    });
}

/// Fetches the library lists of the API endpoints (upstream
/// `LibrariesModel::requestOnlineLibraries()`); `done` gets the libraries
/// and the error messages of failed endpoints.
pub fn fetch_library_lists(
    endpoints: Vec<String>,
    done: impl FnOnce(Vec<Library>, Vec<String>) + Send + 'static,
) {
    spawn(
        async move {
            let mut libraries = Vec::new();
            let mut errors = Vec::new();
            let nam = match nam() {
                Ok(nam) => nam,
                Err(e) => return (libraries, vec![e]),
            };
            for endpoint in endpoints {
                let result = match Url::parse(&endpoint) {
                    Ok(url) => librepcb_network::fetch_library_list(&nam, &ApiEndpoint::new(url))
                        .await
                        .map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                };
                match result {
                    Ok(libs) => libraries.extend(libs),
                    Err(e) => errors.push(librepcb_i18n::tr!(
                        "librepcb::editor::LibrariesModel",
                        "Failed to fetch libraries from '{0}': {1}",
                        endpoint,
                        e
                    )),
                }
            }
            (libraries, errors)
        },
        |(libraries, errors)| done(libraries, errors),
    );
}

/// Downloads a file (e.g. an icon) and passes its content to `done` (not
/// called on errors).
pub fn fetch_data(url: Url, done: impl FnOnce(Vec<u8>) + Send + 'static) {
    spawn(
        async move {
            let nam = nam().ok()?;
            NetworkRequest::get(url).send(&nam).await.ok()
        },
        |reply| {
            if let Some(reply) = reply {
                done(reply.data);
            }
        },
    );
}

/// Installs (or updates) a library from its API server entry into the
/// remote libraries directory, replacing `existing` directories.
pub fn install_library(
    library: Library,
    remote_dir: FilePath,
    existing: HashSet<FilePath>,
    done: impl FnOnce(Result<InstalledLibrary, String>) + Send + 'static,
) {
    spawn(
        async move {
            let nam = nam()?;
            let existing: HashMap<Uuid, HashSet<FilePath>> =
                [(library.uuid, existing)].into_iter().collect();
            librepcb_network::install_libraries(&nam, &[&library], &remote_dir, &existing)
                .await
                .map_err(|e| e.to_string())?
                .pop()
                .ok_or_else(|| "no library installed".to_owned())
        },
        done,
    );
}

/// A running library download (download library tab); dropping it does not
/// abort the download, [`cancel()`](Self::cancel) does.
#[derive(Debug)]
pub struct DownloadHandle {
    token: tokio_util::sync::CancellationToken,
}

impl DownloadHandle {
    /// Aborts the download.
    pub fn cancel(&self) {
        self.token.cancel();
    }
}

/// Downloads a library ZIP and extracts it into `dest_dir` (upstream
/// `LibraryDownload`); `progress` gets the state text and percent on the
/// UI thread, `done` the result.
pub fn download_library(
    url: Url,
    dest_dir: FilePath,
    progress: impl Fn(String, i32) + Send + Sync + 'static,
    done: impl FnOnce(Result<FilePath, String>) + Send + 'static,
) -> Option<DownloadHandle> {
    let token = tokio_util::sync::CancellationToken::new();
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let download = match LibraryDownload::new(url, dest_dir, semaphore) {
        Ok(d) => d.cancellation_token(token.clone()),
        Err(e) => {
            done(Err(e.to_string()));
            return None;
        }
    };
    let mut rx = download.subscribe_progress();
    let progress = std::sync::Arc::new(progress);
    let rt = runtime()?;
    rt.spawn(async move {
        while rx.changed().await.is_ok() {
            let p = rx.borrow_and_update().clone();
            let progress = std::sync::Arc::clone(&progress);
            let _ = slint::invoke_from_event_loop(move || {
                progress(p.state, i32::from(p.percent));
            });
        }
    });
    spawn(
        async move {
            let nam = nam()?;
            download.download(&nam).await.map_err(|e| e.to_string())
        },
        done,
    );
    Some(DownloadHandle { token })
}
