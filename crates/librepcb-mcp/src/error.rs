//! Tool errors with stable kinds (no upstream counterpart).
//!
//! Every error a tool returns carries an [`ErrorKind`] (a stable,
//! machine-readable string such as `not_found` or `stale_revision`) and the
//! human readable message, which for errors of the core is the upstream
//! message. The kinds are part of the MCP interface; add new ones instead
//! of changing existing ones.

use librepcb_core::fileio;
use librepcb_core::project::{self, board::BoardExportError};
use librepcb_core::workspace;

/// Stable error kinds of tool failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The addressed entity (component, net, pin, board, library element,
    /// file, ...) does not exist.
    NotFound,
    /// An argument is malformed or out of range.
    InvalidArgument,
    /// The operation conflicts with the current state (duplicate name,
    /// entity still in use, directory already locked, ...).
    Conflict,
    /// `expected_revision` does not match the project revision; nothing was
    /// changed.
    StaleRevision,
    /// The tool needs an open project.
    NoProject,
    /// The tool needs an open workspace.
    NoWorkspace,
    /// File system access failed.
    Io,
    /// A network request failed.
    Network,
    /// Unexpected internal failure.
    Internal,
    /// The requested feature or backend is not available in this build
    /// (e.g. an external router which is not integrated).
    NotAvailable,
    /// The host application embedding the server refused the operation
    /// (e.g. closing a project the user is editing).
    Refused,
}

impl ErrorKind {
    /// The wire name, e.g. `"not_found"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::InvalidArgument => "invalid_argument",
            Self::Conflict => "conflict",
            Self::StaleRevision => "stale_revision",
            Self::NoProject => "no_project",
            Self::NoWorkspace => "no_workspace",
            Self::Io => "io",
            Self::Network => "network",
            Self::Internal => "internal",
            Self::NotAvailable => "not_available",
            Self::Refused => "refused",
        }
    }
}

impl std::fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A failed tool call: stable kind plus message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{kind}: {message}")]
pub struct ToolError {
    /// The stable kind.
    pub kind: ErrorKind,
    /// The message (upstream message for core errors).
    pub message: String,
}

/// Result type of the tool implementations.
pub type ToolResult<T> = Result<T, ToolError>;

impl ToolError {
    /// Creates an error.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Shorthand for [`ErrorKind::NotFound`].
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotFound, message)
    }

    /// Shorthand for [`ErrorKind::InvalidArgument`].
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidArgument, message)
    }

    /// Shorthand for [`ErrorKind::Internal`].
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    /// Shorthand for [`ErrorKind::Refused`].
    pub fn refused(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Refused, message)
    }

    /// The error of tools which need an open project.
    pub fn no_project() -> Self {
        Self::new(
            ErrorKind::NoProject,
            "No project is open (use project_open or project_create first).",
        )
    }

    /// The error of tools which need an open workspace.
    pub fn no_workspace() -> Self {
        Self::new(
            ErrorKind::NoWorkspace,
            "No workspace is open (start the server with --workspace, or use workspace_open \
             or workspace_create first).",
        )
    }
}

fn file_io_kind(e: &fileio::Error) -> ErrorKind {
    use fileio::Error as E;
    match e {
        E::FileNotFound(..)
        | E::DirectoryNotFound(..)
        | E::FileOrDirectoryNotFound(..)
        | E::TransactionalFileNotFound(..)
        | E::LockDirectoryNotFound(..) => ErrorKind::NotFound,
        E::AlreadyExists(..) | E::AlreadyLocked { .. } | E::AutosaveDetected(..) => {
            ErrorKind::Conflict
        }
        E::InvalidPath(..)
        | E::SandboxBreakout(..)
        | E::InvalidOutputPath(..)
        | E::OutputPathOutsideDirectory { .. }
        | E::AbsoluteOutputPath(..)
        | E::PipeInOutputPath => ErrorKind::InvalidArgument,
        _ => ErrorKind::Io,
    }
}

fn project_kind(e: &project::Error) -> ErrorKind {
    use project::Error as E;
    match e {
        E::FileIo(e) => file_io_kind(e),
        E::ProjectFileNotFound(..)
        | E::NotAProjectDirectory(..)
        | E::NotFound { .. }
        | E::InexistentNetClass(..)
        | E::InexistentNetSignal(..)
        | E::InexistentBus(..)
        | E::MissingLibraryComponent(..)
        | E::InexistentComponentSignal(..)
        | E::InexistentDeviceComponent(..)
        | E::MissingLibraryDevice(..)
        | E::MissingLibraryPackage(..)
        | E::MissingPackagePad { .. }
        | E::InexistentTraceAnchor { .. }
        | E::InexistentComponent(..)
        | E::MissingLibrarySymbol(..)
        | E::InexistentNetPoint(..)
        | E::InexistentBusSegment(..)
        | E::InexistentBusJunction { .. }
        | E::InexistentSymbol(..)
        | E::InexistentSymbolPin { .. } => ErrorKind::NotFound,
        E::ProjectDirectoryNotEmpty(..)
        | E::DuplicateComponentSignal(..)
        | E::DuplicateUuid { .. }
        | E::DuplicateName { .. }
        | E::DuplicateDirectoryName { .. }
        | E::DuplicateLibraryElement(..)
        | E::SchematicNotEmpty(..)
        | E::LastAssemblyVariant
        | E::NetClassInUse(..)
        | E::NetSignalInUse(..)
        | E::BusInUse(..)
        | E::ComponentInUse(..)
        | E::ComponentSignalInUse { .. }
        | E::DuplicateDevice(..)
        | E::SymbolItemAlreadyPlaced { .. }
        | E::PadInMultipleSegments { .. }
        | E::AnchorInMultipleSegments { .. }
        | E::ViaLayersInUse(..)
        | E::ItemInUse { .. } => ErrorKind::Conflict,
        _ => ErrorKind::InvalidArgument,
    }
}

impl From<project::Error> for ToolError {
    fn from(e: project::Error) -> Self {
        Self::new(project_kind(&e), e.to_string())
    }
}

impl From<librepcb_editor::Error> for ToolError {
    fn from(e: librepcb_editor::Error) -> Self {
        use librepcb_editor::Error as E;
        let kind = match &e {
            E::Project(p) => project_kind(p),
            E::FileIo(f) => file_io_kind(f),
            E::NotFound { .. } | E::NotInLibrarySource { .. } | E::NotInProjectLibrary { .. } => {
                ErrorKind::NotFound
            }
            E::GroupActive | E::NoGroupActive => ErrorKind::Conflict,
            E::DeviceNotCompatible | E::AllGatesPlaced(..) | E::NetMismatch(..) => {
                ErrorKind::Conflict
            }
            _ => ErrorKind::InvalidArgument,
        };
        Self::new(kind, e.to_string())
    }
}

impl From<fileio::Error> for ToolError {
    fn from(e: fileio::Error) -> Self {
        Self::new(file_io_kind(&e), e.to_string())
    }
}

impl From<workspace::Error> for ToolError {
    fn from(e: workspace::Error) -> Self {
        let kind = match &e {
            workspace::Error::FileIo(e) => file_io_kind(e),
            workspace::Error::InvalidWorkspace(..)
            | workspace::Error::IncompatibleWorkspace { .. }
            | workspace::Error::DataDirectoryTooNew(..) => ErrorKind::InvalidArgument,
            workspace::Error::Database(..) => ErrorKind::Io,
            _ => ErrorKind::Internal,
        };
        Self::new(kind, e.to_string())
    }
}

impl From<librepcb_core::library::Error> for ToolError {
    fn from(e: librepcb_core::library::Error) -> Self {
        let kind = match &e {
            librepcb_core::library::Error::FileIo(e) => file_io_kind(e),
            _ => ErrorKind::InvalidArgument,
        };
        Self::new(kind, e.to_string())
    }
}

impl From<BoardExportError> for ToolError {
    fn from(e: BoardExportError) -> Self {
        let kind = match &e {
            BoardExportError::Project(e) => project_kind(e),
            BoardExportError::FileIo(e) => file_io_kind(e),
            _ => ErrorKind::Internal,
        };
        Self::new(kind, e.to_string())
    }
}

impl From<librepcb_core::types::Error> for ToolError {
    fn from(e: librepcb_core::types::Error) -> Self {
        Self::invalid(e.to_string())
    }
}

impl From<librepcb_scene::Error> for ToolError {
    fn from(e: librepcb_scene::Error) -> Self {
        let kind = match &e {
            librepcb_scene::Error::SchematicNotFound(..) => ErrorKind::NotFound,
            _ => ErrorKind::Internal,
        };
        Self::new(kind, e.to_string())
    }
}

impl From<librepcb_network::Error> for ToolError {
    fn from(e: librepcb_network::Error) -> Self {
        let kind = match &e {
            librepcb_network::Error::LibraryNotFound(..) => ErrorKind::NotFound,
            _ => ErrorKind::Network,
        };
        Self::new(kind, e.to_string())
    }
}

impl From<librepcb_core::export::Error> for ToolError {
    fn from(e: librepcb_core::export::Error) -> Self {
        Self::internal(e.to_string())
    }
}

impl From<serde_json::Error> for ToolError {
    fn from(e: serde_json::Error) -> Self {
        Self::invalid(e.to_string())
    }
}

impl From<librepcb_import::eagle::Error> for ToolError {
    fn from(e: librepcb_import::eagle::Error) -> Self {
        use librepcb_import::eagle::Error;
        let kind = match &e {
            Error::FileNotFound(..) => ErrorKind::NotFound,
            Error::Io { .. } => ErrorKind::Io,
            Error::FileIo(e) => file_io_kind(e),
            Error::Project(e) => project_kind(e),
            _ => ErrorKind::InvalidArgument,
        };
        Self::new(kind, e.to_string())
    }
}
