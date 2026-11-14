//! Errors of the CLI commands (upstream: exceptions caught in
//! `CommandLineInterface`, printed as `ERROR: <message>`).

use foilfern_core::project::OutputJobError;
use foilfern_core::project::board::BoardExportError;
use foilfern_core::{export, fileio, library, project, serialization};

/// Error of a CLI command. The messages are the ones of the core errors.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// A file operation failed.
    #[error(transparent)]
    FileIo(#[from] fileio::Error),
    /// Parsing or deserializing a file failed.
    #[error(transparent)]
    Serialization(#[from] serialization::Error),
    /// Loading, modifying or saving a project failed.
    #[error(transparent)]
    Project(Box<project::Error>),
    /// A board export failed.
    #[error(transparent)]
    BoardExport(Box<BoardExportError>),
    /// Running output jobs failed.
    #[error(transparent)]
    OutputJob(Box<OutputJobError>),
    /// An export generator failed.
    #[error(transparent)]
    Export(#[from] export::Error),
    /// Loading, checking or saving a library element failed.
    #[error(transparent)]
    Library(Box<library::Error>),
    #[error(transparent)]
    Step(#[from] foilfern_step::Error),
}

impl From<project::Error> for CliError {
    fn from(e: project::Error) -> Self {
        Self::Project(Box::new(e))
    }
}

impl From<BoardExportError> for CliError {
    fn from(e: BoardExportError) -> Self {
        Self::BoardExport(Box::new(e))
    }
}

impl From<OutputJobError> for CliError {
    fn from(e: OutputJobError) -> Self {
        Self::OutputJob(Box::new(e))
    }
}

impl From<library::Error> for CliError {
    fn from(e: library::Error) -> Self {
        Self::Library(Box::new(e))
    }
}

/// Result type of the CLI commands.
pub type CliResult<T, E = CliError> = std::result::Result<T, E>;
