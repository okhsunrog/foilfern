//! Port of libs/librepcb/core/project/board/drc/boarddesignrulecheckmessages.{h,cpp}.
//!
//! Typed IDs replace source pointers. Approvals preserve the source tokens,
//! field order and line breaks, since projects persist these exact values.

use super::data::Hole;
use crate::geometry::Path;
use crate::project::{BoardId, ComponentInstanceId, NetSegmentId, PlaneId};
use crate::rule_check::{RuleCheckMessage, Severity};
use crate::serialization::List;
use crate::types::{Layer, UnsignedLength, Uuid};
use librepcb_i18n::tr;

/// Identity of a drilled hole, including its owning device or net segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum DrcHoleRef {
    /// Board-level non-plated hole.
    BoardHole(Uuid),
    /// Non-plated hole of a placed footprint.
    DeviceHole {
        device: ComponentInstanceId,
        hole: Uuid,
    },
    /// Plated hole of a standalone pad.
    PadHole {
        segment: NetSegmentId,
        pad: Uuid,
        hole: Uuid,
    },
    /// Plated hole of a footprint pad.
    FootprintPadHole {
        device: ComponentInstanceId,
        pad: Uuid,
        hole: Uuid,
    },
    /// Via drill.
    Via { segment: NetSegmentId, via: Uuid },
}
impl DrcHoleRef {
    /// Whether the hole is plated (a pad or via).
    pub fn is_plated(self) -> bool {
        !matches!(self, Self::BoardHole(_) | Self::DeviceHole { .. })
    }
    pub(super) fn serialize(self, node: &mut List) {
        let fields = match self {
            Self::BoardHole(hole) => vec![("hole", hole)],
            Self::DeviceHole { device, hole } => vec![("device", device.0), ("hole", hole)],
            Self::PadHole { segment, pad, hole } => {
                vec![("netsegment", segment.0), ("pad", pad), ("hole", hole)]
            }
            Self::FootprintPadHole { device, pad, hole } => {
                vec![("device", device.0), ("pad", pad), ("hole", hole)]
            }
            Self::Via { segment, via } => vec![("netsegment", segment.0), ("via", via)],
        };
        for (name, uuid) in fields {
            node.ensure_line_break();
            node.append_child(name, &uuid);
        }
        node.ensure_line_break();
    }
}

/// Entities and configured limit of a board finding.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DrcMessageKind {
    /// Stroke or configured minimum width below a manufacturing limit.
    MinimumWidth {
        object: DrcObjectRef,
        minimum: UnsignedLength,
    },
    /// Board silkscreen text smaller than the configured height.
    MinimumTextHeight {
        object: DrcObjectRef,
        minimum: UnsignedLength,
    },
    /// Round drill smaller than the manufacturing limit.
    MinimumDrillDiameter {
        hole: DrcHoleRef,
        minimum: UnsignedLength,
    },
    /// Slot narrower than the manufacturing limit.
    MinimumSlotWidth {
        hole: DrcHoleRef,
        minimum: UnsignedLength,
    },
    /// Slot shape disallowed by board settings.
    ForbiddenSlot(DrcHoleRef),
    /// Plane spoke width smaller than the plane's minimum width.
    PlaneThermalSpokeWidthIgnored(PlaneId),
    /// Copper objects on a disabled inner layer.
    DisabledLayer(Layer),
    /// Enabled inner layer with no explicitly placed copper.
    UnusedLayer(Layer),
}

/// A board finding with stable navigation IDs and a serializable approval.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardDrcMessage {
    pub(super) board: BoardId,
    pub(super) kind: DrcMessageKind,
    pub(super) rule: RuleCheckMessage,
}
impl BoardDrcMessage {
    /// Board to highlight.
    pub fn board(&self) -> BoardId {
        self.board
    }
    /// Typed finding data.
    pub fn kind(&self) -> &DrcMessageKind {
        &self.kind
    }
    /// Translated text, severity, approval and highlighting geometry.
    pub fn rule_message(&self) -> &RuleCheckMessage {
        &self.rule
    }
}

pub(super) fn net_with_fallback(net: &str) -> String {
    if net.is_empty() {
        tr!("BoardDesignRuleCheckMessages", "(no net)")
    } else {
        net.into()
    }
}

pub(super) fn minimum_drill(
    hole: &Hole,
    minimum: UnsignedLength,
    locations: Vec<Path>,
) -> (DrcMessageKind, RuleCheckMessage) {
    let actual = hole.diameter.to_mm_string();
    let min = minimum.to_mm_string();
    let (message, description) = if matches!(hole.reference, DrcHoleRef::Via { .. }) {
        (
            tr!(
                "DrcMsgMinimumDrillDiameterViolation",
                "Via drill diameter of '{0}': {1} < {2} {3}",
                net_with_fallback(&hole.net_name),
                actual,
                min,
                "mm"
            ),
            tr!(
                "DrcMsgMinimumDrillDiameterViolation",
                "The drill diameter of the via is smaller than the minimum plated drill diameter configured in the DRC settings."
            ),
        )
    } else if hole.reference.is_plated() {
        (
            tr!(
                "DrcMsgMinimumDrillDiameterViolation",
                "Pad drill diameter of '{0}': {1} < {2} {3}",
                hole.pad_name,
                actual,
                min,
                "mm"
            ),
            tr!(
                "DrcMsgMinimumDrillDiameterViolation",
                "The drill diameter of the through-hole pad is smaller than the minimum plated drill diameter configured in the DRC settings."
            ),
        )
    } else {
        (
            tr!(
                "DrcMsgMinimumDrillDiameterViolation",
                "NPTH drill diameter: {0} < {1} {2}",
                actual,
                min,
                "mm"
            ),
            tr!(
                "DrcMsgMinimumDrillDiameterViolation",
                "The drill diameter of the non-plated hole is smaller than the minimum non-plated drill diameter configured in the DRC settings."
            ),
        )
    };
    let description = description
        + "\n\n"
        + &tr!(
            "DrcMsgMinimumDrillDiameterViolation",
            "Check the DRC settings and increase the drill diameter if needed."
        );
    let mut rule = RuleCheckMessage::new(
        Severity::Warning,
        message,
        description,
        "minimum_drill_diameter_violation",
        locations,
    );
    hole.reference.serialize(rule.approval_mut());
    (
        DrcMessageKind::MinimumDrillDiameter {
            hole: hole.reference,
            minimum,
        },
        rule,
    )
}

pub(super) fn minimum_slot(
    hole: &Hole,
    minimum: UnsignedLength,
    locations: Vec<Path>,
) -> (DrcMessageKind, RuleCheckMessage) {
    let actual = hole.diameter.to_mm_string();
    let min = minimum.to_mm_string();
    let (message, description) = if hole.reference.is_plated() {
        (
            tr!(
                "DrcMsgMinimumSlotWidthViolation",
                "Plated slot width: {0} < {1} {2}",
                actual,
                min,
                "mm"
            ),
            tr!(
                "DrcMsgMinimumSlotWidthViolation",
                "The width of the plated slot is smaller than the minimum plated slot width configured in the DRC settings."
            ),
        )
    } else {
        (
            tr!(
                "DrcMsgMinimumSlotWidthViolation",
                "NPTH slot width: {0} < {1} {2}",
                actual,
                min,
                "mm"
            ),
            tr!(
                "DrcMsgMinimumSlotWidthViolation",
                "The width of the non-plated slot is smaller than the minimum non-plated slot width configured in the DRC settings."
            ),
        )
    };
    let description = description
        + "\n\n"
        + &tr!(
            "DrcMsgMinimumSlotWidthViolation",
            "Check the DRC settings and increase the slot width if needed."
        );
    let mut rule = RuleCheckMessage::new(
        Severity::Warning,
        message,
        description,
        "minimum_slot_width_violation",
        locations,
    );
    hole.reference.serialize(rule.approval_mut());
    (
        DrcMessageKind::MinimumSlotWidth {
            hole: hole.reference,
            minimum,
        },
        rule,
    )
}

pub(super) fn forbidden_slot(
    hole: &Hole,
    locations: Vec<Path>,
) -> (DrcMessageKind, RuleCheckMessage) {
    let suggestion = tr!(
        "DrcMsgForbiddenSlot",
        "Either avoid them or check if your PCB manufacturer supports them."
    );
    let slot_mode = tr!(
        "DrcMsgForbiddenSlot",
        "Choose the desired Excellon slot mode when generating the production data (G85 vs. G00..G03)."
    );
    let (message, description, mode) = if hole.path.is_curved() {
        (
            tr!("DrcMsgForbiddenSlot", "Hole is a slot with curves"),
            tr!(
                "DrcMsgForbiddenSlot",
                "Curved slots are a very unusual thing and may cause troubles with many PCB manufacturers."
            ),
            tr!(
                "DrcMsgForbiddenSlot",
                "The drilled slot mode (G85) will not be available when generating production data."
            ),
        )
    } else if hole.path.vertices().len() > 2 {
        (
            tr!("DrcMsgForbiddenSlot", "Hole is a multi-segment slot"),
            tr!(
                "DrcMsgForbiddenSlot",
                "Multi-segment slots are a rather unusual thing and may cause troubles with some PCB manufacturers."
            ),
            slot_mode,
        )
    } else {
        (
            tr!("DrcMsgForbiddenSlot", "Hole is a slot"),
            tr!(
                "DrcMsgForbiddenSlot",
                "Slots may cause troubles with some PCB manufacturers."
            ),
            slot_mode,
        )
    };
    let mut rule = RuleCheckMessage::new(
        Severity::Warning,
        message,
        format!("{description}\n{suggestion}\n{mode}"),
        "forbidden_slot",
        locations,
    );
    hole.reference.serialize(rule.approval_mut());
    (DrcMessageKind::ForbiddenSlot(hole.reference), rule)
}

/// Identity of board graphics, used in findings and persisted approvals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DrcObjectRef {
    /// Board or footprint polygon.
    Polygon {
        device: Option<ComponentInstanceId>,
        polygon: Uuid,
    },
    /// Instance stroke text, whose placement is independent of the device.
    StrokeText {
        device: Option<ComponentInstanceId>,
        text: Uuid,
    },
    /// Footprint circle.
    Circle {
        device: ComponentInstanceId,
        circle: Uuid,
    },
    /// Net segment trace.
    Trace { segment: NetSegmentId, trace: Uuid },
    /// Copper plane.
    Plane(PlaneId),
}
impl DrcObjectRef {
    pub(super) fn serialize(self, node: &mut List) {
        let fields = match self {
            Self::Polygon { device, polygon } => device
                .map(|d| ("device", d.0))
                .into_iter()
                .chain([("polygon", polygon)])
                .collect::<Vec<_>>(),
            Self::StrokeText { device, text } => device
                .map(|d| ("device", d.0))
                .into_iter()
                .chain([("stroke_text", text)])
                .collect(),
            Self::Circle { device, circle } => vec![("device", device.0), ("circle", circle)],
            Self::Trace { segment, trace } => vec![("netsegment", segment.0), ("trace", trace)],
            Self::Plane(id) => vec![("plane", id.0)],
        };
        for (name, id) in fields {
            node.ensure_line_break();
            node.append_child(name, &id);
        }
        node.ensure_line_break();
    }
}
