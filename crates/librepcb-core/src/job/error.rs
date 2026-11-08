//! Errors of output job execution (libs/librepcb/core/project/outputjobrunner.cpp).
use crate::types::Uuid;
use librepcb_i18n::tr;
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error(transparent)]
    Project(Box<crate::project::Error>),
    #[error(transparent)]
    Export(#[from] crate::export::Error),
    #[error(transparent)]
    FileIo(#[from] crate::fileio::Error),
    #[error("{} {}",tr!("OutputJobRunner","Unknown output job type '{0}'.",.0),tr!("OutputJobRunner","You may need a more recent LibrePCB version to run this job."))]
    UnknownJob(String),
    #[error("Output job type '{0}' requires a renderer which is not implemented yet.")]
    DeferredJob(String),
    #[error("{0}")]
    Graphics(String),
    #[error("Failed to generate interactive HTML BOM: {0}")]
    InteractiveBom(String),
    #[error("Unsupported {kind} format: '{suffix}'")]
    UnsupportedFormat { kind: &'static str, suffix: String },
    #[error("Board does not exist: {0}")]
    BoardNotFound(Uuid),
    #[error("Assembly variant does not exist: {0}")]
    VariantNotFound(Uuid),
    #[error("{}",tr!("OutputJobRunner","The input file must be located within the project directory, specified by a relative file path."))]
    InvalidCopyInput,
    #[error("{}",tr!("OutputJobRunner","The archive job depends on files from another job which was not run yet. Note that archive jobs can only depend on jobs further ahead in the list so you might need to reorder them."))]
    ArchiveDependencyNotRun,
}

impl From<crate::project::Error> for Error {
    fn from(error: crate::project::Error) -> Self {
        Self::Project(Box::new(error))
    }
}
