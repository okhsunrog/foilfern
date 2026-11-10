//! Errors at the physical geometry / GPU viewer boundary.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error(transparent)]
    Geometry(#[from] librepcb_core::utils::clipper_helpers::Error),
    #[error(transparent)]
    Project(Box<librepcb_core::project::Error>),
    #[error(transparent)]
    Step(#[from] librepcb_step::Error),
    #[error("Cannot triangulate board material: {0}")]
    Triangulation(String),
    #[error("Board has no closed outline")]
    NoOutline,
    #[error("3D renderer failed: {0}")]
    Render(String),
}
pub type Result<T> = std::result::Result<T, Error>;
impl From<librepcb_core::project::Error> for Error {
    fn from(error: librepcb_core::project::Error) -> Self {
        Self::Project(Box::new(error))
    }
}
