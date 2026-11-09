//! Errors of the EAGLE import (upstream `std::runtime_error` of parseagle
//! and `RuntimeError`/`LogicError` of the eagleimport library).

use librepcb_i18n::tr;

/// Errors of the [`eagle`](super) module.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The XML is malformed.
    #[error("Failed to parse XML: {0}")]
    Xml(String),
    /// The file is not an XML file (e.g. a binary file of EAGLE v5).
    #[error(
        "File does not seem to be from EAGLE v6 or later (no XML node found). Note that files \
         from EAGLE v5 or older are not supported."
    )]
    NotXml,
    /// A required attribute is missing.
    #[error("Attribute '{attribute}' not found in XML element '{element}'.")]
    MissingAttribute {
        /// Attribute name.
        attribute: String,
        /// Element tag name.
        element: String,
    },
    /// An attribute is not a valid bool.
    #[error("Invalid bool in attribute {0}")]
    InvalidBool(String),
    /// An attribute is not a valid integer.
    #[error("Invalid integer in attribute {0}")]
    InvalidInt(String),
    /// An attribute is not a valid floating point number.
    #[error("Invalid double in attribute {0}")]
    InvalidDouble(String),
    /// A required child element is missing.
    #[error("Child not found: {0}")]
    MissingChild(String),
    /// The file does not exist.
    #[error("File does not exist: {0}")]
    FileNotFound(String),
    /// Reading a file failed.
    #[error("Cannot open file {path}: {message}")]
    Io {
        /// File path.
        path: String,
        /// Error message.
        message: String,
    },
    /// Unsupported layer setup of a board.
    #[error("Unsupported layer setup: {0}")]
    UnsupportedLayerSetup(String),
    /// A design rule parameter is not a valid length.
    #[error("Invalid length parameter value: '{0}'")]
    InvalidLengthParam(String),
    /// A design rule parameter is not a valid ratio.
    #[error("Invalid ratio parameter value: '{0}'")]
    InvalidRatioParam(String),
    /// A wire with flat caps could not be converted.
    #[error("Unsupported geometry with flat caps.")]
    UnsupportedFlatCapGeometry,
    /// A THT pad has an unknown shape.
    #[error("Unknown pad shape: {0}")]
    UnknownPadShape(String),
    /// An SMT pad is on an invalid layer.
    #[error("Invalid pad layer: {0}")]
    InvalidPadLayer(i32),
    /// An element was converted twice.
    #[error("Duplicate import.")]
    DuplicateImport,
    /// The symbol of a gate was not converted before.
    #[error("{}", tr!("librepcb::eagleimport::EagleLibraryConverter", "Dependent symbol \"{0}\" not imported.", .0))]
    DependentSymbolNotImported(String),
    /// The component of a device was not converted before.
    #[error("{}", tr!("librepcb::eagleimport::EagleLibraryConverter", "Dependent component \"{0}\" not imported.", .0))]
    DependentComponentNotImported(String),
    /// The package of a device was not converted before.
    #[error("{}", tr!("librepcb::eagleimport::EagleLibraryConverter", "Dependent package \"{0}\" not imported.", .0))]
    DependentPackageNotImported(String),
    /// A pin has no corresponding component signal.
    #[error("Could not find component signal from pin name: {0}")]
    ComponentSignalNotFound(String),
    /// An error of a LibrePCB type (e.g. an invalid length).
    #[error(transparent)]
    Type(#[from] librepcb_core::types::Error),
    /// An error of a LibrePCB library element.
    #[error(transparent)]
    Library(#[from] librepcb_core::library::Error),
    /// A file system error.
    #[error(transparent)]
    FileIo(#[from] librepcb_core::fileio::Error),
    /// An error of the project model.
    #[error(transparent)]
    Project(Box<librepcb_core::project::Error>),
    /// A geometry error.
    #[error(transparent)]
    Geometry(#[from] librepcb_core::geometry::Error),
    /// An attribute error.
    #[error(transparent)]
    Attribute(#[from] librepcb_core::attribute::Error),
    /// A polygon clipping error.
    #[error(transparent)]
    Clipper(#[from] librepcb_core::utils::clipper_helpers::Error),
    /// Any other error (upstream `LogicError`/`RuntimeError`).
    #[error("{0}")]
    Other(String),
}

/// Result type of the [`eagle`](super) module.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<librepcb_core::project::Error> for Error {
    fn from(e: librepcb_core::project::Error) -> Self {
        Self::Project(Box::new(e))
    }
}
