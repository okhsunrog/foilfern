//! Errors of the graphics export area (core/export/graphicsexport.cpp).
use librepcb_i18n::tr;
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("{}",tr!("GraphicsExport","No pages to export/print."))]
    NoPages,
    #[error("{}",tr!("GraphicsExport","Failed to export image '{0}' due to unknown file extension. Supported extensions: {1}",.0,"pdf, svg, png"))]
    UnsupportedFormat(String),
    #[error("Unsupported page size: '{0}'")]
    UnsupportedPaper(String),
    #[error("Graphics dimensions are invalid or exceed the configured size limit.")]
    Dimensions,
    #[error("Failed to process SVG: {0}")]
    Svg(String),
    #[error("Failed to encode PDF: {0}")]
    Pdf(String),
    #[error("Failed to encode image: {0}")]
    Image(String),
    #[error("{0}")]
    Model(String),
    #[error(transparent)]
    Clipper(#[from] librepcb_core::utils::clipper_helpers::Error),
    #[error(transparent)]
    FileIo(#[from] librepcb_core::fileio::Error),
    #[error(transparent)]
    Core(Box<librepcb_core::project::Error>),
}
impl From<librepcb_core::project::Error> for Error {
    fn from(e: librepcb_core::project::Error) -> Self {
        Self::Core(Box::new(e))
    }
}
pub type Result<T> = std::result::Result<T, Error>;
