//! Errors of the [`types`](super) module.
//!
//! Replaces the `RuntimeError`/`RangeError` exceptions thrown by the upstream
//! type constructors (libs/librepcb/core/exceptions.{h,cpp}). The messages
//! are the upstream (translated, where upstream translates them) strings.

use librepcb_i18n::tr;

/// Error returned when constructing or parsing a basic type fails.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A floating point value is out of range (upstream `RangeError`).
    #[error("Range error: {value} not in [{min}..{max}]")]
    Range {
        /// The rejected value.
        value: f64,
        /// Minimum allowed value.
        min: i64,
        /// Maximum allowed value.
        max: i64,
    },
    /// A fixed point decimal number string is malformed or out of range.
    #[error("{}", tr!("Toolbox", "Invalid fixed point number string: \"{0}\"", .0))]
    InvalidFixedPointNumber(String),
    /// An `UnsignedLength` was constructed from a negative length.
    #[error("{}", tr!("Length", "Value must be >= 0!"))]
    NegativeLength,
    /// A `PositiveLength` was constructed from a length <= 0.
    #[error("{}", tr!("Length", "Value must be > 0!"))]
    NonPositiveLength,
    /// An `UnsignedRatio` was constructed from a negative ratio.
    #[error("{}", tr!("Ratio", "Value must be >= 0!"))]
    NegativeRatio,
    /// An `UnsignedLimitedRatio` was constructed from a ratio outside 0..1.
    #[error("{}", tr!("Ratio", "Value must be 0..1!"))]
    RatioNotInUnitRange,
    /// A `BoundedUnsignedRatio` has min > max.
    #[error(
        "{}",
        tr!("BoundedUnsignedRatio", "Minimum value must not be greater than maximum value.")
    )]
    BoundsMinGreaterThanMax,
    /// Invalid UUID string.
    #[error("{}", tr!("Uuid", "String is not a valid UUID: \"{0}\"", .0))]
    InvalidUuid(String),
    /// Invalid version number string.
    #[error("{}", tr!("Version", "Invalid version number: \"{0}\"", .0))]
    InvalidVersion(String),
    /// Invalid length unit string.
    #[error("{}", tr!("LengthUnit", "Invalid length unit: \"{0}\"", .0))]
    InvalidLengthUnit(String),
    /// Invalid `ElementName`.
    #[error("{}", tr!("ElementName", "Invalid name: '{0}'", .0))]
    InvalidElementName(String),
    /// Invalid `CircuitIdentifier`.
    #[error("{}", tr!("CircuitIdentifier", "Invalid identifier: '{0}'", .0))]
    InvalidCircuitIdentifier(String),
    /// Invalid `BusName`.
    #[error("{}", tr!("BusName", "Invalid bus name: '{0}'", .0))]
    InvalidBusName(String),
    /// Invalid `FileProofName`.
    #[error("{}", tr!("FileProofName", "Invalid name: '{0}'", .0))]
    InvalidFileProofName(String),
    /// Invalid `SimpleString`.
    #[error("{}", tr!("SimpleString", "Invalid name: '{0}'", .0))]
    InvalidSimpleString(String),
    /// Invalid `Tag`.
    #[error("{}", tr!("Tag", "Invalid tag: '{0}'", .0))]
    InvalidTag(String),
    /// Invalid `AttributeKey` (see [`crate::attribute::AttributeKey`]).
    #[error("{}", tr!("AttributeKey", "Invalid attribute key: '{0}'", .0))]
    InvalidAttributeKey(String),
    /// Unknown layer identifier.
    #[error("{}", tr!("Layer", "Unknown layer: '{0}'", .0))]
    UnknownLayer(String),
    /// Unknown PCB color identifier (not translated upstream).
    #[error("Unknown color: '{0}'")]
    UnknownPcbColor(String),
    /// Unknown signal role (not translated upstream).
    #[error("Unknown signal role: '{0}'")]
    UnknownSignalRole(String),
    /// Invalid horizontal alignment (not translated upstream).
    #[error("Invalid horizontal alignment: '{0}'")]
    InvalidHAlign(String),
    /// Invalid vertical alignment (not translated upstream).
    #[error("Invalid vertical alignment: '{0}'")]
    InvalidVAlign(String),
    /// Unknown auto update mode (not translated upstream).
    #[error("Unknown auto update mode: '{0}'")]
    UnknownAutoUpdateMode(String),
    /// Unknown grid style (not translated upstream).
    #[error("Unknown grid style: '{0}'")]
    UnknownGridStyle(String),
    /// Invalid color string (not translated upstream).
    #[error("Invalid color: '{0}'")]
    InvalidColor(String),
    /// Invalid `ComponentPrefix` (see
    /// [`crate::library::cmp::ComponentPrefix`]).
    #[error("{}", tr!("ComponentPrefix", "Invalid component prefix: '{0}'", .0))]
    InvalidComponentPrefix(String),
    /// Invalid `ComponentSymbolVariantItemSuffix` (see
    /// [`crate::library::cmp::ComponentSymbolVariantItemSuffix`]).
    #[error(
        "{}",
        tr!(
            "ComponentSymbolVariantItemSuffix",
            "Invalid component symbol suffix: '{0}'",
            .0
        )
    )]
    InvalidComponentSymbolVariantItemSuffix(String),
    /// Unknown component signal pin display type (not translated upstream,
    /// see [`crate::library::cmp::CmpSigPinDisplayType`]).
    #[error("Invalid component signal pin display type: '{0}'")]
    InvalidCmpSigPinDisplayType(String),
    /// Unknown allowed slots value of DRC settings (not translated upstream,
    /// see [`crate::library::org::AllowedSlots`]).
    #[error("Unknown allowed slots value: '{0}'")]
    UnknownAllowedSlots(String),
    /// Unknown connect style of a board plane (not translated upstream,
    /// see [`crate::project::board::PlaneConnectStyle`]).
    #[error("Unknown plane connect style: '{0}'")]
    UnknownPlaneConnectStyle(String),
    /// Unknown pad annular ring shape in the board design rules (not
    /// translated upstream, see
    /// [`crate::project::board::BoardDesignRules`]).
    #[error("Invalid pad annular shape: '{0}'")]
    InvalidPadAnnularShape(String),
    /// A board zone on a non-copper layer (not translated upstream, see
    /// [`crate::project::board::BoardZoneData`]).
    #[error("Invalid zone layer: {0}")]
    InvalidZoneLayer(String),
    /// Unknown page orientation of graphics exports (not translated
    /// upstream, see [`crate::export::PageOrientation`]).
    #[error("Invalid page orientation: '{0}'")]
    InvalidPageOrientation(String),
    /// Unknown content type of a graphics output job (not translated
    /// upstream, see [`crate::job::GraphicsContentType`]).
    #[error("Invalid content type: '{0}'")]
    InvalidGraphicsContentType(String),
    /// Unknown view mode of the interactive HTML BOM (not translated
    /// upstream, see [`crate::export::InteractiveHtmlBomViewMode`]).
    #[error("Unknown interactive BOM view mode: '{0}'")]
    UnknownInteractiveHtmlBomViewMode(String),
    /// Unknown pin 1 highlight mode of the interactive HTML BOM (not
    /// translated upstream, see
    /// [`crate::export::InteractiveHtmlBomHighlightPin1Mode`]).
    #[error("Unknown interactive BOM pin1 mode: '{0}'")]
    UnknownInteractiveHtmlBomHighlightPin1Mode(String),
}
