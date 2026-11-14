//! Port of libs/librepcb/core/library/sym/symbolcheckmessages.{h,cpp}.

use foilfern_i18n::tr;

use super::symbol_pin::SymbolPin;
use crate::geometry::Text;
use crate::library::natural_cmp_case_insensitive;
use crate::rule_check::{RuleCheckMessage, Severity};
use crate::types::{CircuitIdentifier, Layer, Point, PositiveLength};

/// Reason of a [`SymbolCheckMessage::InvalidImageFile`] (upstream
/// `MsgInvalidImageFile::Error`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageFileError {
    /// The file does not exist.
    FileMissing,
    /// The file could not be read.
    FileReadError,
    /// The file extension is not supported.
    UnsupportedFormat,
    /// The file could not be loaded as image.
    ImageLoadError,
}

/// Messages of the symbol check (upstream `Msg*` classes in
/// symbolcheckmessages.h).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum SymbolCheckMessage {
    /// Multiple pins have the same name (upstream `MsgDuplicatePinName`).
    DuplicatePinName {
        /// The pin name.
        name: CircuitIdentifier,
    },
    /// An image file is missing or invalid (upstream `MsgInvalidImageFile`).
    InvalidImageFile {
        /// File name of the image.
        file_name: String,
        /// What's wrong.
        error: ImageFileError,
        /// Error details (may be empty).
        details: String,
    },
    /// No `{{NAME}}` text (upstream `MsgMissingSymbolName`).
    MissingSymbolName,
    /// No `{{VALUE}}` text (upstream `MsgMissingSymbolValue`).
    MissingSymbolValue,
    /// A pin name starts with an inversion sign which has no function
    /// (upstream `MsgNonFunctionalSymbolPinInversionSign`).
    NonFunctionalPinInversionSign {
        /// The pin.
        pin: SymbolPin,
    },
    /// The origin is not in the center of the symbol (upstream
    /// `MsgSymbolOriginNotInCenter`).
    OriginNotInCenter {
        /// The actual center of the symbol.
        center: Point,
    },
    /// Multiple pins at the same position (upstream
    /// `MsgOverlappingSymbolPins`).
    OverlappingPins {
        /// The overlapping pins.
        pins: Vec<SymbolPin>,
    },
    /// A pin is not on the grid (upstream `MsgSymbolPinNotOnGrid`).
    PinNotOnGrid {
        /// The pin.
        pin: SymbolPin,
        /// The grid interval.
        grid_interval: PositiveLength,
    },
    /// A `{{NAME}}`/`{{VALUE}}` text is on an unusual layer (upstream
    /// `MsgWrongSymbolTextLayer`).
    WrongTextLayer {
        /// The text.
        text: Text,
        /// The layer the text should be on.
        expected_layer: Layer,
    },
}

impl SymbolCheckMessage {
    /// Returns the generic rule check message.
    pub fn to_message(&self) -> RuleCheckMessage {
        match self {
            Self::DuplicatePinName { name } => {
                let mut msg = RuleCheckMessage::new(
                    Severity::Error,
                    tr!("MsgDuplicatePinName", "Duplicate pin name: '{0}'", name),
                    tr!(
                        "MsgDuplicatePinName",
                        "All symbol pins must have unique names, otherwise they cannot be \
                         distinguished later in the component editor. If your part has \
                         several pins with same functionality (e.g. multiple GND pins), you \
                         should add only one of these pins to the symbol. The assignment to \
                         multiple leads should be done in the device editor instead."
                    ),
                    "duplicate_pin_name",
                    Vec::new(),
                );
                msg.approval_mut().append_child("name", name.as_str());
                msg
            }
            Self::InvalidImageFile {
                file_name,
                error,
                details,
            } => {
                let message = match error {
                    ImageFileError::FileMissing => {
                        tr!(
                            "MsgInvalidImageFile",
                            "Missing image file: '{0}'",
                            file_name
                        )
                    }
                    ImageFileError::FileReadError => {
                        format!("Failed to read image file: '{file_name}'")
                    }
                    ImageFileError::UnsupportedFormat => {
                        tr!(
                            "MsgInvalidImageFile",
                            "Unsupported image format: '{0}'",
                            file_name
                        )
                    }
                    ImageFileError::ImageLoadError => {
                        tr!(
                            "MsgInvalidImageFile",
                            "Invalid image file: '{0}'",
                            file_name
                        )
                    }
                };
                let mut description = tr!(
                    "MsgInvalidImageFile",
                    "The referenced file of an image does either not exist in the symbol \
                     or is not a valid image file. Try removing and re-adding the image \
                     from the symbol."
                );
                if !details.is_empty() {
                    description.push_str("\n\n");
                    description.push_str(&tr!("MsgInvalidImageFile", "Details:"));
                    description.push(' ');
                    description.push_str(details);
                }
                let mut msg = RuleCheckMessage::new(
                    Severity::Error,
                    message,
                    description,
                    "invalid_image_file",
                    Vec::new(),
                );
                msg.approval_mut().append_child("file", file_name.as_str());
                msg
            }
            Self::MissingSymbolName => RuleCheckMessage::new(
                Severity::Warning,
                tr!("MsgMissingSymbolName", "Missing text: '{0}'", "{{NAME}}"),
                tr!(
                    "MsgMissingSymbolName",
                    "Most symbols should have a text element for the component's name, \
                     otherwise you won't see that name in the schematics. There are \
                     only a few exceptions (e.g. a schematic frame) which don't need a \
                     name, for those you can ignore this message."
                ),
                "missing_name_text",
                Vec::new(),
            ),
            Self::MissingSymbolValue => RuleCheckMessage::new(
                Severity::Warning,
                tr!("MsgMissingSymbolValue", "Missing text: '{0}'", "{{VALUE}}"),
                tr!(
                    "MsgMissingSymbolValue",
                    "Most symbols should have a text element for the component's value, \
                     otherwise you won't see that value in the schematics. There are \
                     only a few exceptions (e.g. a schematic frame) which don't need a \
                     value, for those you can ignore this message."
                ),
                "missing_value_text",
                Vec::new(),
            ),
            Self::NonFunctionalPinInversionSign { pin } => {
                let mut msg = RuleCheckMessage::new(
                    Severity::Hint,
                    tr!(
                        "MsgNonFunctionalSymbolPinInversionSign",
                        "Non-functional inversion sign: '{0}'",
                        pin.name()
                    ),
                    tr!(
                        "MsgNonFunctionalSymbolPinInversionSign",
                        "The pin name seems to start with an inversion sign, but LibrePCB \
                         uses a different sign to indicate inversion.\n\nIt's recommended \
                         to prefix inverted pin names with '{0}', regardless of the \
                         inversion sign used in the parts datasheet.",
                        "!"
                    ),
                    "nonfunctional_inversion_sign",
                    Vec::new(),
                );
                let approval = msg.approval_mut();
                approval.ensure_line_break();
                approval.append_child("pin", &pin.uuid());
                approval.ensure_line_break();
                msg
            }
            Self::OriginNotInCenter { .. } => RuleCheckMessage::new(
                Severity::Hint,
                tr!("MsgSymbolOriginNotInCenter", "Origin not in center"),
                tr!(
                    "MsgSymbolOriginNotInCenter",
                    "Generally the origin (0, 0) should be in the center of the symbol \
                     body (roughly, mapped to grid). It's not recommended to have it at \
                     pin-1 coordinate, top-left or something like that.\n\nIt looks \
                     like this rule is not followed in this symbol. However, for \
                     irregular symbol shapes this warning may not be justified. In such \
                     cases, just approve it."
                ),
                "origin_not_in_center",
                Vec::new(),
            ),
            Self::OverlappingPins { pins } => {
                let mut names: Vec<String> =
                    pins.iter().map(|pin| format!("'{}'", pin.name())).collect();
                names.sort_by(|a, b| natural_cmp_case_insensitive(a, b));
                let mut msg = RuleCheckMessage::new(
                    Severity::Error,
                    tr!(
                        "MsgOverlappingSymbolPins",
                        "Overlapping pins: {0}",
                        names.join(", ")
                    ),
                    tr!(
                        "MsgOverlappingSymbolPins",
                        "There are multiple pins at the same position. This is not allowed \
                         because you cannot connect wires to these pins in the schematic \
                         editor."
                    ),
                    "overlapping_pins",
                    Vec::new(),
                );
                // Note: Upstream intends to sort the pins by UUID here, but
                // actually appends them in their original order.
                let approval = msg.approval_mut();
                for pin in pins {
                    approval.ensure_line_break();
                    approval.append_child("pin", &pin.uuid());
                }
                approval.ensure_line_break();
                msg
            }
            Self::PinNotOnGrid { pin, grid_interval } => {
                let grid = grid_interval.to_mm_string();
                let mut msg = RuleCheckMessage::new(
                    Severity::Error,
                    tr!(
                        "MsgSymbolPinNotOnGrid",
                        "Pin not on {0}mm grid: '{1}'",
                        grid,
                        pin.name()
                    ),
                    tr!(
                        "MsgSymbolPinNotOnGrid",
                        "Every pin must be placed exactly on the {0}mm grid, otherwise it \
                         cannot be connected in the schematic editor.",
                        grid
                    ),
                    "pin_not_on_grid",
                    Vec::new(),
                );
                let approval = msg.approval_mut();
                approval.ensure_line_break();
                approval.append_child("pin", &pin.uuid());
                approval.ensure_line_break();
                msg
            }
            Self::WrongTextLayer {
                text,
                expected_layer,
            } => {
                let layer = expected_layer.name_tr();
                let mut msg = RuleCheckMessage::new(
                    Severity::Warning,
                    tr!(
                        "MsgWrongSymbolTextLayer",
                        "Layer of '{0}' is not '{1}'",
                        text.text(),
                        layer
                    ),
                    tr!(
                        "MsgWrongSymbolTextLayer",
                        "The text element '{0}' should normally be on layer '{1}'.",
                        text.text(),
                        layer
                    ),
                    "unusual_text_layer",
                    Vec::new(),
                );
                let approval = msg.approval_mut();
                approval.ensure_line_break();
                approval.append_child("text", &text.uuid());
                approval.ensure_line_break();
                msg
            }
        }
    }
}
