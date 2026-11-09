//! Port of libs/librepcb/core/network (HTTP requests, file downloads and the
//! LibrePCB API).
//!
//! Unlike `librepcb-core`, this crate is asynchronous: all requests are
//! `async fn`s running on the caller's tokio runtime, based on [`reqwest`]
//! (rustls, streaming bodies). Progress is published through
//! [`tokio::sync::watch`] channels, cancellation works by dropping the
//! future or with a [`tokio_util::sync::CancellationToken`], and blocking
//! file operations (ZIP extraction) run in `spawn_blocking` using the
//! synchronous [`librepcb_core::fileio`] code.
//!
//! - [`NetworkAccessManager`]: shared HTTP client and client information.
//! - [`NetworkRequest`]: general purpose request (up to 100 MB).
//! - [`FileDownload`]: download to a file with checksum verification and
//!   optional ZIP extraction.
//! - [`ApiEndpoint`], [`OrderPcbApiRequest`]: LibrePCB API server access.
//! - [`LibraryDownload`]: download and atomic installation of a library
//!   ZIP.
//! - [`library_git`]: library installation from its git repository (fallback
//!   when the ZIP download is blocked).
//! - [`library_installer`]: installation of libraries from an API server
//!   into a workspace ([`install_official_libraries()`]).

mod api_endpoint;
mod error;
mod file_download;
mod library_download;
pub mod library_git;
pub mod library_installer;
mod network_access_manager;
mod network_request;
mod order_pcb_api_request;

pub use api_endpoint::{ApiEndpoint, Library, Part};
pub use error::{Error, Result};
pub use file_download::{
    ChecksumAlgorithm, Downloaded, FileDownload, ZipCleanupCallback, ZipDiscoveryCallback,
};
pub use library_download::{LibraryDownload, find_library_in_zip};
pub use library_installer::{
    InstallMethod, InstalledLibrary, fetch_library_list, install_libraries,
    install_official_libraries, installed_library_dirs, select_libraries,
};
pub use network_access_manager::{ClientInfo, NetworkAccessManager};
pub use network_request::{NetworkRequest, Progress, Reply};
pub use order_pcb_api_request::{OrderInfo, OrderPcbApiRequest};
pub use reqwest::Url;
