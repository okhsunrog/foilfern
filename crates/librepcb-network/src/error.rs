//! Errors of the network crate.
//!
//! Replaces the error strings passed to the upstream `errored()` /
//! `errorWhileFetching*()` / `*Failed()` signals. Messages are the upstream
//! strings; only those translated upstream go through [`tr!`].

use librepcb_i18n::tr;

/// Result type of the network crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Error returned by network requests.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Transport level error (DNS, connection, TLS, ...).
    #[error("{0}")]
    Transport(String),
    /// The server replied with an HTTP error status.
    #[error(
        "{}",
        tr!(
            "librepcb::NetworkRequestBase",
            "{0} ({1})",
            format!("Error transferring {} - server replied: {}", .url, .reason),
            .status
        )
    )]
    Http {
        /// The requested URL.
        url: String,
        /// HTTP status code.
        status: u16,
        /// HTTP reason phrase.
        reason: String,
    },
    /// A `file://` URL could not be read.
    #[error("Error opening {path}: {message}")]
    FileOpen {
        /// The local path.
        path: String,
        /// Error description.
        message: String,
    },
    /// Unsupported URL scheme.
    #[error("Protocol \"{0}\" is unknown")]
    UnsupportedScheme(String),
    /// Invalid header name or value.
    #[error("Invalid HTTP header: {0}")]
    InvalidHeader(String),
    /// Redirection loop.
    #[error("{}", tr!("librepcb::NetworkRequestBase", "Redirection loop detected."))]
    RedirectLoop,
    /// More than 10 redirections.
    #[error("{}", tr!("librepcb::NetworkRequestBase", "Too many redirects."))]
    TooManyRedirects,
    /// Invalid redirection target.
    #[error("Invalid redirection URL: {0}")]
    InvalidRedirect(String),
    /// The request was aborted with its cancellation token.
    #[error("{}", tr!("librepcb::NetworkRequestBase", "Network request aborted."))]
    Aborted,
    /// A [`NetworkRequest`](crate::NetworkRequest) reply is larger than
    /// 100 MB.
    #[error("{}", tr!("librepcb::NetworkRequest", "The received content exceeds the 100MB size limit."))]
    SizeLimitExceeded,
    /// The checksum of a downloaded file does not match.
    #[error("{}", tr!("librepcb::FileDownload", "Checksum verification of downloaded file failed!"))]
    ChecksumMismatch,
    /// The download destination could not be opened.
    #[error("Could not open file \"{path}\": {message}")]
    OpenDestination {
        /// Destination file (native separators).
        path: String,
        /// Error description.
        message: String,
    },
    /// The downloaded file could not be written.
    #[error("{}", tr!("librepcb::FileDownload", "Error while writing file \"{0}\": {1}", .path, .message))]
    WriteDestination {
        /// Destination file (native separators).
        path: String,
        /// Error description.
        message: String,
    },
    /// File operation error (e.g. while extracting a download).
    #[error(transparent)]
    FileIo(#[from] librepcb_core::fileio::Error),
    /// A received JSON document is not an object (`ApiEndpoint`).
    #[error("{}", tr!("librepcb::ApiEndpoint", "Received JSON object is not valid."))]
    InvalidJson,
    /// A received library list has no results.
    #[error("{}", tr!("librepcb::ApiEndpoint", "Received JSON object does not contain any results."))]
    NoResults,
    /// A received JSON document is not an object (`OrderPcbApiRequest`,
    /// not translated upstream).
    #[error("Received JSON object is not valid.")]
    InvalidOrderJson,
    /// The PCB order upload URL is not known yet.
    #[error("Upload URL not known yet.")]
    UploadUrlUnknown,
    /// The PCB order service is not available.
    #[error(
        "{}",
        tr!(
            "librepcb::OrderPcbApiRequest",
            "This service is currently not available. Please try again later or order the PCB manually either with the Gerber export or the *.lppz export."
        )
    )]
    ServiceNotAvailable,
    /// The project to upload is too large.
    #[error(
        "{}",
        tr!(
            "librepcb::OrderPcbApiRequest",
            "The project is too large ({0}). If you manually added files to the project directory, you might need to move them out of the project directory.",
            .0
        )
    )]
    ProjectTooLarge(String),
    /// The PCB order upload returned an invalid redirection URL.
    #[error("Received an invalid redirection URL.")]
    InvalidOrderRedirect,
    /// A downloaded library ZIP does not contain a library
    /// (`LibraryDownload`).
    #[error(
        "{}",
        tr!(
            "librepcb::editor::LibraryDownload",
            "The downloaded ZIP file does not contain a LibrePCB library."
        )
    )]
    NoLibraryInZip,
    /// No library with this name or UUID is available on the server.
    #[error("Library not found: {0}")]
    LibraryNotFound(String),
    /// The server provides no download URL for a library.
    #[error("No download URL for library {0}")]
    NoDownloadUrl(String),
    /// Installing a library failed.
    #[error("Failed to install library {library}: {source}")]
    LibraryInstall {
        /// Library name.
        library: String,
        /// The underlying error.
        source: Box<Error>,
    },
    /// Workspace library database access failed.
    #[error(transparent)]
    Workspace(#[from] librepcb_core::workspace::Error),
    /// A background task panicked or was cancelled.
    #[error("Background task failed: {0}")]
    Task(String),
    /// A `git` command failed (library installation from its repository).
    #[error("git: {0}")]
    Git(String),
    /// Both the ZIP download and the git fallback of a library failed.
    #[error("{zip}; git fallback: {git}")]
    ZipAndGit {
        /// The error of the ZIP download.
        zip: Box<Error>,
        /// The error of the git fallback.
        git: Box<Error>,
    },
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Self::Transport(e.to_string())
    }
}
