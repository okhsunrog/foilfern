//! Errors of libs/librepcb/core/3d/{occmodel,stepexport}.{h,cpp} adapters.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error(transparent)]
    Metadata(#[from] crate::metadata::Error),
    #[error(transparent)]
    Minify(#[from] crate::minify::Error),
    #[error("STEP geometry: {0}")]
    Geometry(String),
    #[error("Failed to export STEP model of '{component}': {source}")]
    ComponentModel {
        component: String,
        source: Box<Error>,
    },
    #[error("STEP kernel failed while processing the geometry")]
    KernelPanicked,
    #[error("STEP input exceeds the 256 MiB or 256-level nesting limit")]
    InputLimit,
    #[error("Invalid STEP delimiter nesting")]
    InvalidNesting,
    #[error(transparent)]
    Project(Box<foilfern_core::project::Error>),
    #[error(transparent)]
    FileIo(#[from] foilfern_core::fileio::Error),
}
impl From<foilfern_core::project::Error> for Error {
    fn from(error: foilfern_core::project::Error) -> Self {
        Self::Project(Box::new(error))
    }
}
pub type Result<T> = std::result::Result<T, Error>;
