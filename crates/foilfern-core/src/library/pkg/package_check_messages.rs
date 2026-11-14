//! Port of libs/librepcb/core/library/pkg/packagecheckmessages.{h,cpp}.
//!
//! Upstream messages keep shared pointers to the affected footprint, pad,
//! polygon etc. (for highlighting them in the editor); here they carry
//! the UUIDs (and the names needed for the texts) instead.

use foilfern_i18n::tr;

use crate::rule_check::{RuleCheckMessage, Severity};
use crate::types::{CircuitIdentifier, ElementName, Layer, Length, Point, PositiveLength, Uuid};

/// A footprint referenced by a [`PackageCheckMessage`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FootprintRef {
    /// UUID of the footprint.
    pub uuid: Uuid,
    /// Default name of the footprint.
    pub name: ElementName,
}

/// A footprint pad referenced by a [`PackageCheckMessage`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PadRef {
    /// The footprint containing the pad.
    pub footprint: FootprintRef,
    /// UUID of the footprint pad.
    pub pad: Uuid,
    /// Name of the connected package pad (empty if unconnected or if the
    /// package pad doesn't exist).
    pub package_pad_name: String,
}

/// Kind of object of a [`PackageCheckMessage::MinimumWidthViolation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WidthObject {
    /// A polygon.
    Polygon(Uuid),
    /// A circle.
    Circle(Uuid),
    /// A stroke text.
    StrokeText(Uuid),
}

/// Messages of the package check (upstream `Msg*` classes in
/// packagecheckmessages.h).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum PackageCheckMessage {
    /// Footprints are not uniquely identifiable by their tags (upstream
    /// `MsgAmbiguousFootprintTags`).
    AmbiguousFootprintTags,
    /// Assembly type "auto" (upstream `MsgDeprecatedAssemblyType`).
    DeprecatedAssemblyType,
    /// Assembly type differs from the detected one (upstream
    /// `MsgSuspiciousAssemblyType`).
    SuspiciousAssemblyType,
    /// Multiple package pads have the same name (upstream
    /// `MsgDuplicatePadName`).
    DuplicatePadName {
        /// The pad name.
        name: CircuitIdentifier,
    },
    /// Copper clearance of a fiducial is less than its stop mask expansion
    /// (upstream `MsgFiducialClearanceLessThanStopMask`).
    FiducialClearanceLessThanStopMask(PadRef),
    /// Automatic stop mask on a fiducial (upstream
    /// `MsgFiducialStopMaskNotSet`).
    FiducialStopMaskNotSet(PadRef),
    /// Hole without stop mask opening (upstream `MsgHoleWithoutStopMask`).
    HoleWithoutStopMask {
        /// The footprint.
        footprint: FootprintRef,
        /// UUID of the hole.
        hole: Uuid,
        /// Diameter of the hole.
        diameter: PositiveLength,
    },
    /// Custom pad outline without valid area (upstream
    /// `MsgInvalidCustomPadOutline`).
    InvalidCustomPadOutline(PadRef),
    /// A footprint pad is connected to a non-existent package pad
    /// (upstream `MsgInvalidPadConnection`).
    InvalidPadConnection(PadRef),
    /// A silkscreen line is too thin (upstream `MsgMinimumWidthViolation`).
    MinimumWidthViolation {
        /// The footprint.
        footprint: FootprintRef,
        /// The polygon, circle or stroke text.
        object: WidthObject,
        /// Layer of the object.
        layer: Layer,
        /// The recommended minimum width.
        min_width: Length,
    },
    /// No courtyard (upstream `MsgMissingCourtyard`).
    MissingCourtyard(FootprintRef),
    /// No footprint (upstream `MsgMissingFootprint`).
    MissingFootprint,
    /// No 3D model (upstream `MsgMissingFootprintModel`).
    MissingFootprintModel(FootprintRef),
    /// No `{{NAME}}` text (upstream `MsgMissingFootprintName`).
    MissingFootprintName(FootprintRef),
    /// No `{{VALUE}}` text (upstream `MsgMissingFootprintValue`).
    MissingFootprintValue(FootprintRef),
    /// No package outline (upstream `MsgMissingPackageOutline`).
    MissingPackageOutline(FootprintRef),
    /// The origin is not in the center of the footprint (upstream
    /// `MsgFootprintOriginNotInCenter`).
    FootprintOriginNotInCenter {
        /// The footprint.
        footprint: FootprintRef,
        /// The actual center of the footprint.
        center: Point,
    },
    /// The copper areas of two pads overlap (upstream
    /// `MsgOverlappingPads`).
    OverlappingPads {
        /// The first pad.
        pad1: PadRef,
        /// The second pad (in the same footprint).
        pad2: PadRef,
    },
    /// Annular ring of a THT pad too small (upstream
    /// `MsgPadAnnularRingViolation`).
    PadAnnularRingViolation {
        /// The pad.
        pad: PadRef,
        /// The recommended minimum annular ring.
        annular_ring: Length,
    },
    /// Clearance between two pads too small (upstream
    /// `MsgPadClearanceViolation`).
    PadClearanceViolation {
        /// The first pad.
        pad1: PadRef,
        /// The second pad (in the same footprint).
        pad2: PadRef,
        /// The minimum clearance of the package.
        clearance: Length,
    },
    /// A pad hole is not fully surrounded by copper (upstream
    /// `MsgPadHoleOutsideCopper`).
    PadHoleOutsideCopper(PadRef),
    /// The origin of a pad is outside its copper (or hole) area (upstream
    /// `MsgPadOriginOutsideCopper`).
    PadOriginOutsideCopper(PadRef),
    /// Legend too close to a pad (upstream `MsgPadOverlapsWithLegend`).
    PadOverlapsWithLegend {
        /// The pad.
        pad: PadRef,
        /// The recommended clearance.
        clearance: Length,
    },
    /// No stop mask opening on a pad (upstream `MsgPadStopMaskOff`).
    PadStopMaskOff(PadRef),
    /// Copper clearance on a non-fiducial pad (upstream
    /// `MsgPadWithCopperClearance`).
    PadWithCopperClearance(PadRef),
    /// Solder paste on an SMT pad which is not soldered (upstream
    /// `MsgSmtPadWithSolderPaste`).
    SmtPadWithSolderPaste(PadRef),
    /// No solder paste on an SMT pad which is soldered (upstream
    /// `MsgSmtPadWithoutSolderPaste`).
    SmtPadWithoutSolderPaste(PadRef),
    /// Pad function doesn't fit to the pad (upstream
    /// `MsgSuspiciousPadFunction`).
    SuspiciousPadFunction(PadRef),
    /// Solder paste on a THT pad (upstream `MsgThtPadWithSolderPaste`).
    ThtPadWithSolderPaste(PadRef),
    /// Pad function not specified (upstream `MsgUnspecifiedPadFunction`).
    UnspecifiedPadFunction(PadRef),
    /// Custom outline on a pad with another shape (upstream
    /// `MsgUnusedCustomPadOutline`).
    UnusedCustomPadOutline(PadRef),
    /// Keepout zone without effect (upstream `MsgUselessZone`).
    UselessZone {
        /// The footprint.
        footprint: FootprintRef,
        /// UUID of the zone.
        zone: Uuid,
    },
    /// A `{{NAME}}`/`{{VALUE}}` text is on an unusual layer (upstream
    /// `MsgWrongFootprintTextLayer`).
    WrongFootprintTextLayer {
        /// The footprint.
        footprint: FootprintRef,
        /// UUID of the stroke text.
        text: Uuid,
        /// Text of the stroke text.
        text_value: String,
        /// The layer the text should be on.
        expected_layer: Layer,
    },
}

/// Formats a length in micrometers like upstream
/// `QString::number(length.toMm() * 1000) % "μm"`.
fn format_um(length: Length) -> String {
    format!("{}μm", format_number_g6(length.to_mm() * 1000.0))
}

/// Formats a number like `QString::number(double)`, i.e. `printf("%g")`
/// with 6 significant digits.
fn format_number_g6(value: f64) -> String {
    if value == 0.0 || !value.is_finite() {
        return format!("{value}");
    }
    // Determine the decimal exponent after rounding to 6 digits.
    let sci = format!("{value:.5e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let trim = |s: &str| -> String {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            s.to_owned()
        }
    };
    if !(-4..6).contains(&exp) {
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", trim(mantissa), exp.abs())
    } else {
        let decimals = usize::try_from(5 - exp).unwrap_or(0);
        trim(&format!("{value:.decimals$}"))
    }
}

/// Appends the footprint (and optionally other objects) to the approval.
fn approve(msg: &mut RuleCheckMessage, footprint: &FootprintRef, children: &[(&str, Uuid)]) {
    let approval = msg.approval_mut();
    approval.ensure_line_break();
    approval.append_child("footprint", &footprint.uuid);
    approval.ensure_line_break();
    for (name, uuid) in children {
        approval.append_child(*name, uuid);
        approval.ensure_line_break();
    }
}

/// Creates a message about a pad, with footprint and pad in the approval.
fn pad_message(
    pad: &PadRef,
    severity: Severity,
    message: String,
    description: String,
    approval_name: &str,
) -> RuleCheckMessage {
    let mut msg = RuleCheckMessage::new(severity, message, description, approval_name, Vec::new());
    approve(&mut msg, &pad.footprint, &[("pad", pad.pad)]);
    msg
}

/// Creates a message about a footprint, with the footprint in the approval.
fn footprint_message(
    footprint: &FootprintRef,
    severity: Severity,
    message: String,
    description: String,
    approval_name: &str,
) -> RuleCheckMessage {
    let mut msg = RuleCheckMessage::new(severity, message, description, approval_name, Vec::new());
    approve(&mut msg, footprint, &[]);
    msg
}

impl PackageCheckMessage {
    /// Returns the generic rule check message.
    pub fn to_message(&self) -> RuleCheckMessage {
        match self {
            Self::AmbiguousFootprintTags => RuleCheckMessage::new(
                Severity::Hint,
                tr!("MsgAmbiguousFootprintTags", "Ambiguous footprint tags"),
                tr!(
                    "MsgAmbiguousFootprintTags",
                    "The package provides multiple footprints, but they don't specify \
                     (enough) tags to make them uniquely identifiable just by their tags. \
                     This is not a problem at all, but adding unique tags to the \
                     footprints would make the automatic footprint selection of the board \
                     editor more sophisticated."
                ),
                "ambiguous_footprint_tags",
                Vec::new(),
            ),
            Self::DeprecatedAssemblyType => RuleCheckMessage::new(
                Severity::Hint,
                tr!("MsgDeprecatedAssemblyType", "Non-recommended assembly type"),
                tr!(
                    "MsgDeprecatedAssemblyType",
                    "The assembly type 'Auto-detect' is not recommended as the detection \
                     might not be correct in every case. It's safer to specify the \
                     assembly type manually."
                ),
                "auto_assembly_type",
                Vec::new(),
            ),
            Self::SuspiciousAssemblyType => RuleCheckMessage::new(
                Severity::Warning,
                tr!("MsgSuspiciousAssemblyType", "Suspicious assembly type"),
                tr!(
                    "MsgSuspiciousAssemblyType",
                    "The specified assembly type differs from the assembly type which is \
                     auto-detected from the footprint contents. Double-check if the \
                     specified assembly type is really correct."
                ),
                "suspicious_assembly_type",
                Vec::new(),
            ),
            Self::DuplicatePadName { name } => {
                let mut msg = RuleCheckMessage::new(
                    Severity::Error,
                    tr!("MsgDuplicatePadName", "Duplicate pad name: '{0}'", name),
                    tr!(
                        "MsgDuplicatePadName",
                        "All package pads must have unique names, otherwise they cannot be \
                         distinguished later in the device editor. If your part has several \
                         leads with same functionality (e.g. multiple GND leads), you can \
                         assign all these pads to the same component signal later in the \
                         device editor.\n\nFor neutral packages (e.g. SOT23), pads should be \
                         named only by numbers anyway, not by functionality (e.g. name them \
                         '1', '2', '3' instead of 'D', 'G', 'S')."
                    ),
                    "duplicate_pad_name",
                    Vec::new(),
                );
                msg.approval_mut().append_child("name", name.as_str());
                msg
            }
            Self::FiducialClearanceLessThanStopMask(pad) => pad_message(
                pad,
                Severity::Warning,
                tr!(
                    "MsgFiducialClearanceLessThanStopMask",
                    "Small copper clearance on fiducial in '{0}'",
                    pad.footprint.name
                ),
                tr!(
                    "MsgFiducialClearanceLessThanStopMask",
                    "The copper clearance of the fiducial pad is less than its stop mask \
                     expansion, which is unusual. Typically the copper clearance should be \
                     equal to or greater than the stop mask expansion to avoid copper \
                     located within the stop mask opening."
                ),
                "fiducial_copper_clearance_less_than_stop_mask",
            ),
            Self::FiducialStopMaskNotSet(pad) => pad_message(
                pad,
                Severity::Warning,
                tr!(
                    "MsgFiducialStopMaskNotSet",
                    "Stop mask not set on fiducial in '{0}'",
                    pad.footprint.name
                ),
                tr!(
                    "MsgFiducialStopMaskNotSet",
                    "The stop mask expansion of the fiducial pad is set to automatic, \
                     which is unusual. Typically the stop mask expansion of fiducials need \
                     to be manually set to a much larger value."
                ),
                "fiducial_stop_mask_not_set",
            ),
            Self::HoleWithoutStopMask {
                footprint,
                hole,
                diameter,
            } => {
                let mut msg = RuleCheckMessage::new(
                    Severity::Warning,
                    tr!(
                        "MsgHoleWithoutStopMask",
                        "No stop mask on {0} hole in '{1}'",
                        format!("{}mm", diameter.to_mm_string()),
                        footprint.name
                    ),
                    tr!(
                        "MsgHoleWithoutStopMask",
                        "Non-plated holes should have a stop mask opening to avoid solder \
                         resist flowing into the hole. An automatic stop mask opening can be \
                         enabled in the hole properties."
                    ),
                    "hole_without_stop_mask",
                    Vec::new(),
                );
                approve(&mut msg, footprint, &[("hole", *hole)]);
                msg
            }
            Self::InvalidCustomPadOutline(pad) => pad_message(
                pad,
                Severity::Error,
                tr!(
                    "MsgInvalidCustomPadOutline",
                    "Invalid custom outline of pad '{0}' in '{1}'",
                    pad.package_pad_name,
                    pad.footprint.name
                ),
                tr!(
                    "MsgInvalidCustomPadOutline",
                    "The pad has set a custom outline which does not represent a valid \
                     area. Either choose a different pad shape or specify a valid custom \
                     outline."
                ),
                "invalid_custom_pad_outline",
            ),
            Self::InvalidPadConnection(pad) => pad_message(
                pad,
                Severity::Error,
                tr!(
                    "MsgInvalidPadConnection",
                    "Invalid pad connection in '{0}'",
                    pad.footprint.name
                ),
                tr!(
                    "MsgInvalidPadConnection",
                    "A footprint pad is connected to a package pad which doesn't exist. \
                     Check all pads for proper connections."
                ),
                "invalid_pad_connection",
            ),
            Self::MinimumWidthViolation {
                footprint,
                object,
                layer,
                min_width,
            } => {
                let layer_name = layer.name_tr();
                let width = format_um(*min_width);
                let (description, child) = match object {
                    WidthObject::Polygon(uuid) => (
                        tr!(
                            "MsgMinimumWidthViolation",
                            "It is recommended that polygons on layer '{0}' have a line width \
                             of at least {1}.",
                            layer_name,
                            width
                        ),
                        ("polygon", *uuid),
                    ),
                    WidthObject::Circle(uuid) => (
                        tr!(
                            "MsgMinimumWidthViolation",
                            "It is recommended that circles on layer '{0}' have a line width \
                             of at least {1}.",
                            layer_name,
                            width
                        ),
                        ("circle", *uuid),
                    ),
                    WidthObject::StrokeText(uuid) => (
                        tr!(
                            "MsgMinimumWidthViolation",
                            "It is recommended that stroke texts on layer '{0}' have a stroke \
                             width of at least {1}.",
                            layer_name,
                            width
                        ),
                        ("text", *uuid),
                    ),
                };
                let description = format!(
                    "{description} {}",
                    tr!(
                        "MsgMinimumWidthViolation",
                        "Otherwise it could lead to manufacturing problems in some cases \
                         (depending on board settings and/or the capabilities of the PCB \
                         manufacturer)."
                    )
                );
                let mut msg = RuleCheckMessage::new(
                    Severity::Warning,
                    tr!(
                        "MsgMinimumWidthViolation",
                        "Minimum width of '{0}' in '{1}'",
                        layer_name,
                        footprint.name
                    ),
                    description,
                    "thin_line",
                    Vec::new(),
                );
                approve(&mut msg, footprint, &[child]);
                msg
            }
            Self::MissingCourtyard(footprint) => footprint_message(
                footprint,
                Severity::Warning,
                tr!(
                    "MsgMissingCourtyard",
                    "Missing courtyard in footprint '{0}'",
                    footprint.name
                ),
                format!(
                    "{}\n\n{}",
                    tr!(
                        "MsgMissingCourtyard",
                        "It is recommended to draw the package courtyard with a single, \
                         closed, zero-width polygon or circle on layer '{0}'. This allows \
                         the DRC to warn if another device is placed within the courtyard of \
                         this device (i.e. too close).",
                        Layer::TOP_COURTYARD.name_tr()
                    ),
                    tr!(
                        "MsgMissingCourtyard",
                        "Often this is identical to the package outline but with a small \
                         offset. If you're unsure, just ignore this message."
                    )
                ),
                "missing_courtyard",
            ),
            Self::MissingFootprint => RuleCheckMessage::new(
                Severity::Error,
                tr!("MsgMissingFootprint", "No footprint defined"),
                tr!(
                    "MsgMissingFootprint",
                    "Every package must have at least one footprint, otherwise it can't be \
                     added to a board."
                ),
                "missing_footprint",
                Vec::new(),
            ),
            Self::MissingFootprintModel(footprint) => footprint_message(
                footprint,
                Severity::Hint,
                tr!(
                    "MsgMissingFootprintModel",
                    "No 3D model defined for '{0}'",
                    footprint.name
                ),
                tr!(
                    "MsgMissingFootprintModel",
                    "The footprint has no 3D model specified, so the package will be \
                     missing in the 3D viewer and in 3D data exports. However, this has no \
                     impact on the PCB production data."
                ),
                "missing_footprint_3d_model",
            ),
            Self::MissingFootprintName(footprint) => footprint_message(
                footprint,
                Severity::Warning,
                tr!(
                    "MsgMissingFootprintName",
                    "Missing text '{0}' in footprint '{1}'",
                    "{{NAME}}",
                    footprint.name
                ),
                tr!(
                    "MsgMissingFootprintName",
                    "Most footprints should have a text element for the component's name, \
                     otherwise you won't see that name on the PCB (e.g. on silkscreen). \
                     There are only a few exceptions which don't need a name (e.g. if the \
                     footprint is only a drawing), for those you can ignore this message."
                ),
                "missing_name_text",
            ),
            Self::MissingFootprintValue(footprint) => footprint_message(
                footprint,
                Severity::Warning,
                tr!(
                    "MsgMissingFootprintValue",
                    "Missing text '{0}' in footprint '{1}'",
                    "{{VALUE}}",
                    footprint.name
                ),
                tr!(
                    "MsgMissingFootprintValue",
                    "Most footprints should have a text element for the component's value, \
                     otherwise you won't see that value on the PCB (e.g. on silkscreen). \
                     There are only a few exceptions which don't need a value (e.g. if the \
                     footprint is only a drawing), for those you can ignore this message."
                ),
                "missing_value_text",
            ),
            Self::MissingPackageOutline(footprint) => footprint_message(
                footprint,
                Severity::Warning,
                tr!(
                    "MsgMissingPackageOutline",
                    "Missing outline in footprint '{0}'",
                    footprint.name
                ),
                tr!(
                    "MsgMissingPackageOutline",
                    "It is recommended to draw the package outline with a single, closed, \
                     zero-width polygon or circle on layer '{0}'. This allows the DRC to \
                     warn if this device is placed within the courtyard of another device \
                     (i.e. too close).",
                    Layer::TOP_PACKAGE_OUTLINES.name_tr()
                ),
                "missing_package_outline",
            ),
            // Note: Upstream doesn't add the footprint to the approval.
            Self::FootprintOriginNotInCenter { footprint, .. } => RuleCheckMessage::new(
                Severity::Hint,
                tr!(
                    "MsgFootprintOriginNotInCenter",
                    "Origin of '{0}' not in center",
                    footprint.name
                ),
                tr!(
                    "MsgFootprintOriginNotInCenter",
                    "Generally the origin (0, 0) should be at the coordinate used for \
                     pick&place which is typically in the center of the package body. It \
                     should even be (more or less) <b>exactly</b> in the center, not \
                     aligned to a grid (off-grid pads are fine).\n\nIt looks like this rule \
                     is not followed in this footprint. However, for irregular package \
                     shapes or other special cases this warning may not be justified. In \
                     such cases, just approve it."
                ),
                "origin_not_in_center",
                Vec::new(),
            ),
            // Note: Upstream doesn't add the footprint and pads to the approval.
            Self::OverlappingPads { pad1, pad2 } => RuleCheckMessage::new(
                Severity::Error,
                tr!(
                    "MsgOverlappingPads",
                    "Overlapping pads '{0}' and '{1}' in '{2}'",
                    pad1.package_pad_name,
                    pad2.package_pad_name,
                    pad1.footprint.name
                ),
                tr!(
                    "MsgOverlappingPads",
                    "The copper area of two pads overlap. This can lead to serious issues \
                     with the design rule check and probably leads to a short circuit in the \
                     board so this really needs to be fixed."
                ),
                "overlapping_pads",
                Vec::new(),
            ),
            Self::PadAnnularRingViolation { pad, annular_ring } => pad_message(
                pad,
                Severity::Warning,
                tr!(
                    "MsgPadAnnularRingViolation",
                    "Annular ring of pad '{0}' in '{1}'",
                    pad.package_pad_name,
                    pad.footprint.name
                ),
                tr!(
                    "MsgPadAnnularRingViolation",
                    "Pads should have at least {0} annular ring (copper around each pad \
                     hole). Note that this value is just a general recommendation, the \
                     exact value depends on the capabilities of the PCB manufacturer.",
                    format_um(*annular_ring)
                ),
                "small_pad_annular_ring",
            ),
            Self::PadClearanceViolation {
                pad1,
                pad2,
                clearance,
            } => {
                let mut msg = RuleCheckMessage::new(
                    Severity::Warning,
                    tr!(
                        "MsgPadClearanceViolation",
                        "Clearance of pad '{0}' to pad '{1}' in '{2}'",
                        pad1.package_pad_name,
                        pad2.package_pad_name,
                        pad1.footprint.name
                    ),
                    tr!(
                        "MsgPadClearanceViolation",
                        "Pads must have at least {0} clearance between each other, as \
                         configured in the package. Either increase the clearance between \
                         those pads, or reduce the configured minimum clearance value if you \
                         are sure the PCB manufacturer can reliably handle it.",
                        format_um(*clearance)
                    ),
                    "small_pad_clearance",
                    Vec::new(),
                );
                approve(
                    &mut msg,
                    &pad1.footprint,
                    &[
                        ("pad", pad1.pad.min(pad2.pad)),
                        ("pad", pad1.pad.max(pad2.pad)),
                    ],
                );
                msg
            }
            Self::PadHoleOutsideCopper(pad) => pad_message(
                pad,
                Severity::Error,
                tr!(
                    "MsgPadHoleOutsideCopper",
                    "Hole outside copper of pad '{0}' in '{1}'",
                    pad.package_pad_name,
                    pad.footprint.name
                ),
                tr!(
                    "MsgPadHoleOutsideCopper",
                    "All THT pad holes must be fully surrounded by copper, otherwise they \
                     could lead to serious issues during the design rule check or \
                     manufacturing process."
                ),
                "pad_hole_outside_copper",
            ),
            Self::PadOriginOutsideCopper(pad) => pad_message(
                pad,
                Severity::Error,
                tr!(
                    "MsgPadOriginOutsideCopper",
                    "Invalid origin of pad '{0}' in '{1}'",
                    pad.package_pad_name,
                    pad.footprint.name
                ),
                tr!(
                    "MsgPadOriginOutsideCopper",
                    "The origin of each pad must be located within its copper area, \
                     otherwise traces won't be connected properly.\n\nFor THT pads, the \
                     origin must be located within a drill hole since on some layers the \
                     pad might only have a small annular ring instead of the full pad \
                     shape."
                ),
                "pad_origin_outside_copper",
            ),
            Self::PadOverlapsWithLegend { pad, clearance } => pad_message(
                pad,
                Severity::Warning,
                tr!(
                    "MsgPadOverlapsWithLegend",
                    "Clearance of pad '{0}' in '{1}' to legend",
                    pad.package_pad_name,
                    pad.footprint.name
                ),
                tr!(
                    "MsgPadOverlapsWithLegend",
                    "Pads should have at least {0} clearance to drawings on the legend \
                     because these drawings would be cropped during the Gerber export when \
                     used as silkscreen.",
                    format_um(*clearance)
                ),
                "pad_overlaps_legend",
            ),
            Self::PadStopMaskOff(pad) => pad_message(
                pad,
                Severity::Error,
                tr!(
                    "MsgPadStopMaskOff",
                    "Solder resist on pad '{0}' in '{1}'",
                    pad.package_pad_name,
                    pad.footprint.name
                ),
                tr!(
                    "MsgPadStopMaskOff",
                    "There's no stop mask opening enabled on the pad, so the copper pad \
                     will be covered by solder resist and is thus not functional. This is \
                     very unusual, you should double-check if this is really what you \
                     want."
                ),
                "pad_stop_mask_off",
            ),
            Self::PadWithCopperClearance(pad) => pad_message(
                pad,
                Severity::Hint,
                tr!(
                    "MsgPadWithCopperClearance",
                    "Copper clearance >0 on pad '{0}' in '{1}'",
                    pad.package_pad_name,
                    pad.footprint.name
                ),
                tr!(
                    "MsgPadWithCopperClearance",
                    "There is a custom copper clearance enabled on the pad, which is \
                     unusual for pads which do not represent a fiducial. Note that the \
                     clearance value from the board design rules is applied to all pads \
                     anyway, thus manual clearance values are usually not needed. If this \
                     pad is a fiducial, make sure to set its function to the corresponding \
                     value."
                ),
                "pad_with_copper_clearance",
            ),
            Self::SmtPadWithSolderPaste(pad) => pad_message(
                pad,
                Severity::Warning,
                tr!(
                    "MsgSmtPadWithSolderPaste",
                    "Solder paste on SMT pad '{0}' in '{1}'",
                    pad.package_pad_name,
                    pad.footprint.name
                ),
                tr!(
                    "MsgSmtPadWithSolderPaste",
                    "The SMT pad has solder paste enabled, but its function indicates that \
                     there's no lead to be soldered on it (e.g. a fiducial). Usually solder \
                     paste is not desired on such special pads which won't be soldered."
                ),
                "smt_pad_with_solder_paste",
            ),
            Self::SmtPadWithoutSolderPaste(pad) => pad_message(
                pad,
                Severity::Warning,
                tr!(
                    "MsgSmtPadWithoutSolderPaste",
                    "No solder paste on SMT pad '{0}' in '{1}'",
                    pad.package_pad_name,
                    pad.footprint.name
                ),
                tr!(
                    "MsgSmtPadWithoutSolderPaste",
                    "The SMT pad has no solder paste enabled, which is unusual since \
                     without solder paste the pad cannot be reflow soldered. Only use this \
                     if there's no lead to be soldered on that pad, or if you have drawn a \
                     manual solder paste area."
                ),
                "smt_pad_without_solder_paste",
            ),
            Self::SuspiciousPadFunction(pad) => pad_message(
                pad,
                Severity::Warning,
                tr!(
                    "MsgSuspiciousPadFunction",
                    "Suspicious function of pad '{0}' in '{1}'",
                    pad.package_pad_name,
                    pad.footprint.name
                ),
                tr!(
                    "MsgSuspiciousPadFunction",
                    "The configured pad function does not match other properties of the \
                     pad and thus looks suspicious. Possible reasons:\n\n - Function is \
                     intended for THT pads but pad is SMT\n - Function is intended for SMT \
                     pads but pad is THT\n - Function is electrical but pad is not \
                     connected\n - Function is fiducial but pad is connected"
                ),
                "suspicious_pad_function",
            ),
            Self::ThtPadWithSolderPaste(pad) => pad_message(
                pad,
                Severity::Warning,
                tr!(
                    "MsgThtPadWithSolderPaste",
                    "Solder paste on THT pad '{0}' in '{1}'",
                    pad.package_pad_name,
                    pad.footprint.name
                ),
                tr!(
                    "MsgThtPadWithSolderPaste",
                    "The THT pad has solder paste enabled, which is very unusual since \
                     through-hole components are usually not reflow soldered. Also the \
                     solder paste could flow into the pads hole, possibly causing troubles \
                     during THT assembly. Double-check if this is really what you want."
                ),
                "tht_pad_with_solder_paste",
            ),
            Self::UnspecifiedPadFunction(pad) => pad_message(
                pad,
                Severity::Hint,
                tr!(
                    "MsgUnspecifiedPadFunction",
                    "Unspecified function of pad '{0}' in '{1}'",
                    pad.package_pad_name,
                    pad.footprint.name
                ),
                format!(
                    "{}\n\n{}",
                    tr!(
                        "MsgUnspecifiedPadFunction",
                        "The function of the pad is not specified, which could lead to \
                         inaccurate or wrong data in exports (e.g. pick&place files). Also \
                         the automatic checks can detect more potential issues if the \
                         function is specified. Thus it's recommended to explicitly specify \
                         the function of each pad."
                    ),
                    tr!(
                        "MsgUnspecifiedPadFunction",
                        "However, the image data of a PCB is not affected by the pad \
                         function."
                    )
                ),
                "pad_function_unspecified",
            ),
            Self::UnusedCustomPadOutline(pad) => pad_message(
                pad,
                Severity::Warning,
                tr!(
                    "MsgUnusedCustomPadOutline",
                    "Unused custom outline of pad '{0}' in '{1}'",
                    pad.package_pad_name,
                    pad.footprint.name
                ),
                tr!(
                    "MsgUnusedCustomPadOutline",
                    "The pad has set a custom outline but it isn't used as the shape. So it \
                     has no effect and should be removed to avoid confusion."
                ),
                "unused_custom_pad_outline",
            ),
            Self::UselessZone { footprint, zone } => {
                let mut msg = RuleCheckMessage::new(
                    Severity::Warning,
                    tr!(
                        "MsgUselessZone",
                        "Useless keepout zone in '{0}'",
                        footprint.name
                    ),
                    tr!(
                        "MsgUselessZone",
                        "The keepout zone has no layer or rule enabled so it has no effect. \
                         Either correct its properties or remove it from the footprint."
                    ),
                    "useless_zone",
                    Vec::new(),
                );
                approve(&mut msg, footprint, &[("zone", *zone)]);
                msg
            }
            Self::WrongFootprintTextLayer {
                footprint,
                text,
                text_value,
                expected_layer,
            } => {
                let layer = expected_layer.name_tr();
                let mut msg = RuleCheckMessage::new(
                    Severity::Warning,
                    tr!(
                        "MsgWrongFootprintTextLayer",
                        "Layer of '{0}' in '{1}' is not '{2}'",
                        text_value,
                        footprint.name,
                        layer
                    ),
                    tr!(
                        "MsgWrongFootprintTextLayer",
                        "The text element '{0}' should normally be on layer '{1}'.",
                        text_value,
                        layer
                    ),
                    "unusual_text_layer",
                    Vec::new(),
                );
                approve(&mut msg, footprint, &[("text", *text)]);
                msg
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_formatting() {
        assert_eq!(format_number_g6(150.0), "150");
        assert_eq!(format_number_g6(0.2 * 1000.0), "200");
        assert_eq!(format_number_g6(0.175 * 1000.0), "175");
        assert_eq!(format_number_g6(12.5), "12.5");
        assert_eq!(format_number_g6(0.001), "0.001");
        assert_eq!(format_number_g6(0.0001), "0.0001");
        assert_eq!(format_number_g6(0.00001), "1e-05");
        assert_eq!(format_number_g6(123456.0), "123456");
        assert_eq!(format_number_g6(1234567.0), "1.23457e+06");
        assert_eq!(format_number_g6(999999.5), "1e+06");
        assert_eq!(format_number_g6(0.0), "0");
        assert_eq!(format_um(Length::new(150_000)), "150μm");
    }
}
