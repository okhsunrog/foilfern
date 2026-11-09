//! Errors of the KiCad import (upstream `RuntimeError`/`LogicError` of the
//! kicadimport library).

/// Errors of the [`kicad`](super) module.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Invalid S-expression content (e.g. a missing child).
    #[error(transparent)]
    Parse(#[from] librepcb_core::serialization::Error),
    /// A child node is missing.
    #[error("Child not found: @{0}")]
    ChildNotFound(usize),
    /// An invalid bool (neither `yes` nor `no`).
    #[error("Invalid bool: {0}")]
    InvalidBool(String),
    /// A symbol gate name without index and style suffix.
    #[error("Invalid symbol gate name: {0}")]
    InvalidGateName(String),
    /// A symbol gate with an unknown style.
    #[error("Unknown symbol gate style {0}.")]
    UnknownGateStyle(i32),
    /// The file is not a KiCad symbol library.
    #[error("File does not seem to be a KiCad symbol library.")]
    NotASymbolLibrary,
    /// The file is not a KiCad footprint.
    #[error("File does not seem to be a KiCad footprint.")]
    NotAFootprint,
    /// A footprint on the bottom side.
    #[error("Unsupported footprint board side.")]
    UnsupportedFootprintSide,
    /// A geometry on a layer which can't be converted.
    #[error("Unsupported footprint geometry layer {0}.")]
    UnsupportedGeometryLayer(String),
    /// A polygon with less than 2 vertices.
    #[error("Polygon with less than 2 vertices.")]
    TooFewVertices,
    /// A THT pad without drill.
    #[error("Through-hole pad has no valid drill set.")]
    MissingDrill,
    /// A pad with an unsupported combination of layers.
    #[error("Strange/unsupported pad configuration detected.")]
    UnsupportedPadConfiguration,
    /// A pad which could not be converted at all.
    #[error("Could not convert pad '{0}'.")]
    PadNotConverted(String),
    /// A symbol pin missing in the converted symbol.
    #[error("Pin '{0}' not found in symbol.")]
    PinNotFound(String),
    /// A pad referenced by the symbol but missing in the package.
    #[error("Pad '{0}' not found in imported package.")]
    PadNotFound(String),
    /// A dependency is neither converted nor already imported.
    #[error("Dependent {kind} '{reference}' not found.")]
    DependencyNotFound {
        /// Element type (e.g. `"symbol"`).
        kind: &'static str,
        /// Reference (`library:element`).
        reference: String,
    },
    /// The base symbol of an extending symbol was not found.
    #[error("Base symbol '{0}' not found.")]
    BaseSymbolNotFound(String),
    /// An element was converted twice.
    #[error("Duplicate import.")]
    DuplicateImport,
    /// An inconsistency (upstream `LogicError`).
    #[error("Logic error: {0}")]
    Logic(&'static str),
    /// An error of a LibrePCB type (e.g. an invalid length).
    #[error(transparent)]
    Type(#[from] librepcb_core::types::Error),
    /// An error of a LibrePCB library element.
    #[error(transparent)]
    Library(#[from] librepcb_core::library::Error),
    /// A geometry error (e.g. an empty path).
    #[error(transparent)]
    Geometry(#[from] librepcb_core::geometry::Error),
    /// A file system error.
    #[error(transparent)]
    FileIo(#[from] librepcb_core::fileio::Error),
    /// A polygon clipping error.
    #[error(transparent)]
    Clipper(#[from] librepcb_core::utils::clipper_helpers::Error),
    /// A workspace library database error.
    #[error(transparent)]
    Workspace(#[from] librepcb_core::workspace::Error),
}

/// Result type of the [`kicad`](super) module.
pub type Result<T, E = Error> = std::result::Result<T, E>;
