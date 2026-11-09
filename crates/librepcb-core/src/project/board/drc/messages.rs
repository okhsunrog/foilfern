//! Port of libs/librepcb/core/project/board/drc/boarddesignrulecheckmessages.{h,cpp}.
//!
//! Upstream has one `RuleCheckMessage` subclass per message kind; here a
//! [`DrcMessage`] is the generic [`RuleCheckMessage`] (severity, texts,
//! approval, locations) plus a [`DrcMessageKind`] naming the upstream
//! class (with the data upstream exposes for automatic fixes). The message
//! texts and the approval S-expressions are identical to upstream, since
//! approvals are stored in `board.lp`.
//!
//! One upstream quirk is kept on purpose: the message of
//! `DrcMsgInvalidPadConnection` only substitutes its first placeholder, so
//! the text ends with a literal `'%2'` (like upstream).

use librepcb_i18n::tr;

use super::data::{
    DrcCircle, DrcDevice, DrcHole, DrcJunction, DrcPad, DrcPlane, DrcPolygon, DrcSegment,
    DrcStrokeText, DrcTrace, DrcVia, DrcZone,
};
use crate::geometry::{NonEmptyPath, Path};
use crate::rule_check::{RuleCheckMessage, Severity};
use crate::serialization::{List, SExpression};
use crate::types::{Layer, Length, PositiveLength, UnsignedLength, Uuid};
use crate::utils::transform::Transform;

/// The kind of a [`DrcMessage`] (the upstream message class), with the
/// data upstream exposes to the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DrcMessageKind {
    /// `DrcMsgMissingDevice`
    MissingDevice,
    /// `DrcMsgMissingConnection`
    MissingConnection,
    /// `DrcMsgImpossibleConnection`
    ImpossibleConnection,
    /// `DrcMsgMissingBoardOutline`
    MissingBoardOutline,
    /// `DrcMsgMultipleBoardOutlines`
    MultipleBoardOutlines,
    /// `DrcMsgOpenBoardOutlinePolygon`
    OpenBoardOutlinePolygon,
    /// `DrcMsgIntersectingBoardOutlines`
    IntersectingBoardOutlines,
    /// `DrcMsgCutoutOutsideBoardArea`
    CutoutOutsideBoardArea,
    /// `DrcMsgMinimumBoardOutlineInnerRadiusViolation`
    MinimumBoardOutlineInnerRadiusViolation,
    /// `DrcMsgPlatedCutouts`
    PlatedCutouts,
    /// `DrcMsgPlatedCutoutWithoutCopper`
    PlatedCutoutWithoutCopper,
    /// `DrcMsgNonPlatedCutoutWithCopper`
    NonPlatedCutoutWithCopper,
    /// `DrcMsgEmptyNetSegment` (can be fixed by removing the segment).
    EmptyNetSegment {
        /// UUID of the empty net segment.
        segment: Uuid,
    },
    /// `DrcMsgUnconnectedJunction`
    UnconnectedJunction,
    /// `DrcMsgMinimumTextHeightViolation`
    MinimumTextHeightViolation,
    /// `DrcMsgMinimumWidthViolation`
    MinimumWidthViolation,
    /// `DrcMsgCopperCopperClearanceViolation`
    CopperCopperClearanceViolation,
    /// `DrcMsgCopperBoardClearanceViolation`
    CopperBoardClearanceViolation,
    /// `DrcMsgCopperHoleClearanceViolation`
    CopperHoleClearanceViolation,
    /// `DrcMsgCopperInKeepoutZone`
    CopperInKeepoutZone,
    /// `DrcMsgDrillDrillClearanceViolation`
    DrillDrillClearanceViolation,
    /// `DrcMsgDrillBoardClearanceViolation`
    DrillBoardClearanceViolation,
    /// `DrcMsgDeviceInCourtyard`
    DeviceInCourtyard,
    /// `DrcMsgOverlappingDevices`
    OverlappingDevices,
    /// `DrcMsgDeviceInKeepoutZone`
    DeviceInKeepoutZone,
    /// `DrcMsgExposureInKeepoutZone`
    ExposureInKeepoutZone,
    /// `DrcMsgMinimumAnnularRingViolation`
    MinimumAnnularRingViolation,
    /// `DrcMsgMinimumDrillDiameterViolation`
    MinimumDrillDiameterViolation,
    /// `DrcMsgMinimumSlotWidthViolation`
    MinimumSlotWidthViolation,
    /// `DrcMsgInvalidPadConnection`
    InvalidPadConnection,
    /// `DrcMsgForbiddenSlot`
    ForbiddenSlot,
    /// `DrcMsgForbiddenVia`
    ForbiddenVia,
    /// `DrcMsgInvalidVia`
    InvalidVia,
    /// `DrcMsgPlaneThermalSpokeWidthIgnored` (can be fixed by adjusting
    /// the plane).
    PlaneThermalSpokeWidthIgnored {
        /// UUID of the plane.
        plane: Uuid,
    },
    /// `DrcMsgSilkscreenClearanceViolation`
    SilkscreenClearanceViolation,
    /// `DrcMsgUselessZone`
    UselessZone,
    /// `DrcMsgUselessVia`
    UselessVia,
    /// `DrcMsgDisabledLayer`
    DisabledLayer,
    /// `DrcMsgUnusedLayer`
    UnusedLayer,
}

/// A message of the design rule check.
#[derive(Debug, Clone)]
pub struct DrcMessage {
    kind: DrcMessageKind,
    message: RuleCheckMessage,
}

impl DrcMessage {
    fn new(
        kind: DrcMessageKind,
        severity: Severity,
        message: String,
        description: String,
        approval_name: &str,
        locations: Vec<Path>,
    ) -> Self {
        Self {
            kind,
            message: RuleCheckMessage::new(
                severity,
                message,
                description,
                approval_name,
                locations,
            ),
        }
    }

    /// Returns the kind of the message.
    pub fn kind(&self) -> DrcMessageKind {
        self.kind
    }

    /// Returns the generic message (severity, texts, approval, locations).
    pub fn message(&self) -> &RuleCheckMessage {
        &self.message
    }

    /// Returns the generic message.
    pub fn into_message(self) -> RuleCheckMessage {
        self.message
    }

    /// Returns the approval node.
    pub fn approval(&self) -> &SExpression {
        self.message.approval()
    }

    fn approval_mut(&mut self) -> &mut List {
        self.message.approval_mut()
    }

    /// Appends `(name uuid)` and a line break.
    fn child_lb(&mut self, name: &str, uuid: &Uuid) {
        let a = self.approval_mut();
        a.append_child(name, uuid);
        a.ensure_line_break();
    }

    fn lb(&mut self) {
        self.approval_mut().ensure_line_break();
    }

    /// Appends two child lists sorted canonically (smaller first), with
    /// line breaks like upstream.
    fn push_sorted_pair(&mut self, mut node1: List, mut node2: List, names: Option<(&str, &str)>) {
        if SExpression::List(node2.clone()) < SExpression::List(node1.clone()) {
            std::mem::swap(&mut node1, &mut node2);
        }
        if let Some((name1, name2)) = names {
            node1.set_name(name1);
            node2.set_name(name2);
        }
        let a = self.approval_mut();
        a.ensure_line_break();
        a.push(SExpression::List(node1));
        a.ensure_line_break();
        a.push(SExpression::List(node2));
        a.ensure_line_break();
    }
}

fn serious_troubles_tr() -> String {
    tr!(
        "BoardDesignRuleCheckMessages",
        "Depending on the capabilities of the PCB manufacturer, this could cause higher costs or even serious troubles during production, leading to a possibly non-functional PCB."
    )
}

fn net_name_with_fallback(net_name: &str) -> String {
    if net_name.is_empty() {
        tr!("BoardDesignRuleCheckMessages", "(no net)")
    } else {
        net_name.to_owned()
    }
}

fn mm(value: Length) -> String {
    value.to_mm_string()
}

/// Joins a text, the "serious troubles" text and a hint like upstream
/// (`text % " " % seriousTroublesTr() % "\n\n" % hint`).
fn serious(text: String, hint: String) -> String {
    format!("{text} {}\n\n{hint}", serious_troubles_tr())
}

/// Joins a text and a hint (`text % "\n\n" % hint`).
fn with_hint(text: String, hint: String) -> String {
    format!("{text}\n\n{hint}")
}

/// A drill (hole or via) referenced by a message (upstream `DrcHoleRef`).
#[derive(Debug, Clone, Copy)]
pub(super) enum DrcHoleRef<'a> {
    /// A board hole.
    BoardHole(&'a DrcHole),
    /// A footprint hole of a device.
    DeviceHole(&'a DrcDevice, &'a DrcHole),
    /// A hole of a footprint pad.
    FootprintPadHole(&'a DrcDevice, &'a DrcPad, &'a DrcHole),
    /// A hole of a standalone pad.
    PadHole(&'a DrcSegment, &'a DrcPad, &'a DrcHole),
    /// A via.
    Via(&'a DrcSegment, &'a DrcVia),
}

impl DrcHoleRef<'_> {
    fn is_pad_hole(&self) -> bool {
        matches!(self, Self::FootprintPadHole(..) | Self::PadHole(..))
    }

    fn is_via_hole(&self) -> bool {
        matches!(self, Self::Via(..))
    }

    fn is_plated(&self) -> bool {
        self.is_pad_hole() || self.is_via_hole()
    }

    fn net_name(&self) -> &str {
        match self {
            Self::FootprintPadHole(_, pad, _) | Self::PadHole(_, pad, _) => &pad.net_name,
            Self::Via(ns, _) => &ns.net_name,
            _ => "",
        }
    }

    fn pad(&self) -> Option<&DrcPad> {
        match self {
            Self::FootprintPadHole(_, pad, _) | Self::PadHole(_, pad, _) => Some(pad),
            _ => None,
        }
    }

    fn diameter(&self) -> PositiveLength {
        match self {
            Self::BoardHole(h)
            | Self::DeviceHole(_, h)
            | Self::FootprintPadHole(_, _, h)
            | Self::PadHole(_, _, h) => h.diameter,
            Self::Via(_, via) => via.drill_diameter,
        }
    }

    fn serialize(&self, node: &mut List) {
        let child = |node: &mut List, name: &str, uuid: &Uuid| {
            node.append_child(name, uuid);
            node.ensure_line_break();
        };
        match self {
            Self::FootprintPadHole(dev, pad, hole) => {
                node.ensure_line_break();
                child(node, "device", &dev.uuid);
                child(node, "pad", &pad.uuid);
                child(node, "hole", &hole.uuid);
            }
            Self::DeviceHole(dev, hole) => {
                node.ensure_line_break();
                child(node, "device", &dev.uuid);
                child(node, "hole", &hole.uuid);
            }
            Self::PadHole(ns, pad, hole) => {
                node.ensure_line_break();
                child(node, "netsegment", &ns.uuid);
                child(node, "pad", &pad.uuid);
                child(node, "hole", &hole.uuid);
            }
            Self::Via(ns, via) => {
                node.ensure_line_break();
                child(node, "netsegment", &ns.uuid);
                child(node, "via", &via.uuid);
            }
            Self::BoardHole(hole) => {
                node.append_child("hole", &hole.uuid);
            }
        }
    }
}

/// An object of a copper clearance violation (upstream
/// `DrcMsgCopperCopperClearanceViolation::Object`).
#[derive(Debug, Clone, Copy)]
pub(super) enum CopperObject<'a> {
    /// A footprint pad.
    FootprintPad(&'a DrcPad, &'a DrcDevice),
    /// A standalone pad.
    Pad(&'a DrcPad, &'a DrcSegment),
    /// A trace.
    Trace(&'a DrcTrace, &'a DrcSegment),
    /// A via.
    Via(&'a DrcVia, &'a DrcSegment),
    /// A plane.
    Plane(&'a DrcPlane),
    /// A polygon of the board or of a device.
    Polygon(&'a DrcPolygon, Option<&'a DrcDevice>),
    /// A circle of a device.
    Circle(&'a DrcCircle, Option<&'a DrcDevice>),
    /// A stroke text of the board or of a device.
    StrokeText(&'a DrcStrokeText, Option<&'a DrcDevice>),
}

impl CopperObject<'_> {
    fn net_name(&self) -> &str {
        match self {
            Self::FootprintPad(pad, _) | Self::Pad(pad, _) => &pad.net_name,
            Self::Trace(_, ns) | Self::Via(_, ns) => &ns.net_name,
            Self::Plane(plane) => &plane.net_name,
            _ => "",
        }
    }

    fn name(&self) -> String {
        const CTX: &str = "DrcMsgCopperCopperClearanceViolation";
        let mut name = String::new();
        let net_name = self.net_name();
        if !net_name.is_empty() {
            name = format!("'{net_name}' ");
        }
        match self {
            Self::FootprintPad(pad, dev) => {
                name = format!("'{}", dev.cmp_instance_name);
                if !pad.lib_pkg_pad_name.is_empty() {
                    name += &format!(":{}", pad.lib_pkg_pad_name);
                }
                name += "'";
            }
            Self::Pad(..) => name += &tr!(CTX, "pad"),
            Self::Trace(..) => name += &tr!(CTX, "trace"),
            Self::Via(..) => name += &tr!(CTX, "via"),
            Self::Plane(..) => name += &tr!(CTX, "plane"),
            Self::Polygon(..) => name += &tr!(CTX, "polygon"),
            Self::Circle(..) => name += &tr!(CTX, "circle"),
            Self::StrokeText(..) => name += &tr!(CTX, "text"),
        }
        name
    }

    /// Serializes the object into an `(object ...)` list.
    pub(super) fn serialize(&self) -> List {
        let mut node = List::new("object");
        let pair = |node: &mut List, n1: &str, u1: &Uuid, n2: &str, u2: &Uuid| {
            node.ensure_line_break();
            node.append_child(n1, u1);
            node.ensure_line_break();
            node.append_child(n2, u2);
            node.ensure_line_break();
        };
        match self {
            Self::FootprintPad(pad, dev) => pair(&mut node, "device", &dev.uuid, "pad", &pad.uuid),
            Self::Pad(pad, ns) => pair(&mut node, "netsegment", &ns.uuid, "pad", &pad.uuid),
            Self::Trace(trace, ns) => {
                pair(&mut node, "netsegment", &ns.uuid, "trace", &trace.uuid);
            }
            Self::Via(via, ns) => pair(&mut node, "netsegment", &ns.uuid, "via", &via.uuid),
            Self::Plane(plane) => {
                node.append_child("plane", &plane.uuid);
            }
            Self::Polygon(p, dev) => match dev {
                Some(dev) => pair(&mut node, "device", &dev.uuid, "polygon", &p.uuid),
                None => {
                    node.append_child("polygon", &p.uuid);
                }
            },
            Self::Circle(c, dev) => match dev {
                Some(dev) => pair(&mut node, "device", &dev.uuid, "circle", &c.uuid),
                None => {
                    node.append_child("circle", &c.uuid);
                }
            },
            Self::StrokeText(st, dev) => match dev {
                Some(dev) => pair(&mut node, "device", &dev.uuid, "stroke_text", &st.uuid),
                None => {
                    node.append_child("stroke_text", &st.uuid);
                }
            },
        }
        node
    }
}

/// An end point of a missing connection (upstream
/// `DrcMsgMissingConnection::Anchor`).
#[derive(Debug, Clone, Copy)]
pub(super) enum ConnectionAnchor<'a> {
    /// A footprint pad.
    FootprintPad(&'a DrcDevice, &'a DrcPad),
    /// A standalone pad of a net segment.
    Pad(&'a DrcSegment),
    /// A via of a net segment.
    Via(&'a DrcSegment),
    /// A junction of a net segment.
    Junction(&'a DrcSegment),
}

impl ConnectionAnchor<'_> {
    fn name(&self) -> String {
        const CTX: &str = "DrcMsgMissingConnection";
        match self {
            Self::FootprintPad(dev, pad) => {
                let mut name = format!("'{}", dev.cmp_instance_name);
                if !pad.lib_pkg_pad_name.is_empty() {
                    name += &format!(":{}", pad.lib_pkg_pad_name);
                }
                name + "'"
            }
            Self::Pad(_) => tr!(CTX, "Pad"),
            Self::Via(_) => tr!(CTX, "Via"),
            Self::Junction(_) => tr!(CTX, "Trace"),
        }
    }

    fn serialize(&self, node: &mut List) {
        match self {
            Self::FootprintPad(dev, pad) => {
                node.ensure_line_break();
                node.append_child("device", &dev.uuid);
                node.ensure_line_break();
                node.append_child("pad", &pad.uuid);
                node.ensure_line_break();
            }
            // I *guess* we don't need to serialize the pad/via/junction
            // since only one connection between a net segment and any other
            // object can be missing(?). (upstream comment)
            Self::Pad(ns) | Self::Via(ns) | Self::Junction(ns) => {
                node.append_child("netsegment", &ns.uuid);
            }
        }
    }
}

/// Name of a pad for keepout zone and pad connection messages: the
/// component and pad name, or the net name.
fn pad_or_net_name(pad: &DrcPad, cmp_inst_name: &str) -> String {
    let mut name = String::new();
    if !cmp_inst_name.is_empty() {
        name = format!("{cmp_inst_name}:{}", pad.lib_pkg_pad_name);
    }
    if name.is_empty() {
        name = net_name_with_fallback(&pad.net_name);
    }
    name
}

/// Object of a keepout zone message (upstream constructor overloads of
/// `DrcMsgCopperInKeepoutZone` and `DrcMsgExposureInKeepoutZone`).
#[derive(Debug, Clone, Copy)]
pub(super) enum ZoneObject<'a> {
    /// A footprint pad.
    FootprintPad(&'a DrcDevice, &'a DrcPad),
    /// A standalone pad.
    Pad(&'a DrcSegment, &'a DrcPad),
    /// A via.
    Via(&'a DrcSegment, &'a DrcVia),
    /// A trace (copper keepout only).
    Trace(&'a DrcSegment, &'a DrcTrace),
    /// A board polygon.
    Polygon(&'a DrcPolygon),
    /// A footprint polygon.
    DevicePolygon(&'a DrcDevice, &'a DrcPolygon),
    /// A footprint circle.
    DeviceCircle(&'a DrcDevice, &'a DrcCircle),
}

impl DrcMessage {
    /// `DrcMsgMissingDevice`
    pub(super) fn missing_device(uuid: &Uuid, name: &str) -> Self {
        const CTX: &str = "DrcMsgMissingDevice";
        let mut m = Self::new(
            DrcMessageKind::MissingDevice,
            Severity::Error,
            tr!(CTX, "Missing device: '{0}'", name),
            tr!(
                CTX,
                "There's a component in the schematics without a corresponding device in the board, so the circuit of the PCB is not complete.\n\nUse the \"Place Devices\" dock to add the device."
            ),
            "missing_device",
            Vec::new(),
        );
        m.lb();
        m.child_lb("device", uuid);
        m
    }

    /// `DrcMsgMissingConnection`
    pub(super) fn missing_connection(
        p1: &ConnectionAnchor<'_>,
        p2: &ConnectionAnchor<'_>,
        net_name: &str,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgMissingConnection";
        let mut m = Self::new(
            DrcMessageKind::MissingConnection,
            Severity::Error,
            tr!(
                CTX,
                "Missing connection in '{0}': {1} ↔ {2}",
                net_name,
                p1.name(),
                p2.name()
            ),
            tr!(
                CTX,
                "There is a missing connection in the net, i.e. not all net items are connected together.\n\nAdd traces and/or planes to create the missing connections.\n\nNote that traces need to be snapped to the origin of footprint pads to make the airwire and this message disappearing."
            ),
            "missing_connection",
            locations,
        );
        let mut from = List::new("tmp");
        p1.serialize(&mut from);
        let mut to = List::new("tmp");
        p2.serialize(&mut to);
        // Sort nodes to make the approval canonical.
        m.push_sorted_pair(from, to, Some(("from", "to")));
        m
    }

    /// `DrcMsgImpossibleConnection`
    pub(super) fn impossible_connection(
        device: &DrcDevice,
        signal_uuid: &Uuid,
        signal_name: &str,
        net_name: &str,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgImpossibleConnection";
        let mut m = Self::new(
            DrcMessageKind::ImpossibleConnection,
            Severity::Error,
            tr!(
                CTX,
                "Impossible connection in '{0}': '{1}:{2}'",
                net_name,
                device.cmp_instance_name,
                signal_name
            ),
            tr!(
                CTX,
                "The pin of this device is connected to a net in the schematics, but the footprint doesn't expose a pad for it. Therefore it is impossible to make the electrical connection in the board. Check if another footprint or another device exposes a corresponding pad."
            ),
            "impossible_connection",
            locations,
        );
        m.lb();
        m.child_lb("device", &device.uuid);
        m.child_lb("signal", signal_uuid);
        m
    }

    /// `DrcMsgMissingBoardOutline`
    pub(super) fn missing_board_outline() -> Self {
        const CTX: &str = "DrcMsgMissingBoardOutline";
        Self::new(
            DrcMessageKind::MissingBoardOutline,
            Severity::Error,
            tr!(CTX, "Missing board outline"),
            with_hint(
                tr!(
                    CTX,
                    "There's no board outline defined at all, so the board cannot be manufactured."
                ),
                tr!(
                    CTX,
                    "Add a closed, zero-width polygon on the layer '{0}' to draw the board outline.",
                    Layer::BOARD_OUTLINES.name_tr()
                ),
            ),
            "missing_board_outline",
            Vec::new(),
        )
    }

    /// `DrcMsgMultipleBoardOutlines`
    pub(super) fn multiple_board_outlines(locations: Vec<Path>) -> Self {
        const CTX: &str = "DrcMsgMultipleBoardOutlines";
        Self::new(
            DrcMessageKind::MultipleBoardOutlines,
            Severity::Warning,
            tr!(CTX, "Multiple board outlines"),
            with_hint(
                tr!(
                    CTX,
                    "There are multiple, independent board outlines defined."
                ),
                tr!(
                    CTX,
                    "Either add only a single board outline or make sure the PCB manufacturer can handle production data containing multiple PCBs."
                ),
            ),
            "multiple_board_outlines",
            locations,
        )
    }

    /// `DrcMsgOpenBoardOutlinePolygon`
    pub(super) fn open_board_outline_polygon(
        polygon: &Uuid,
        device: Option<&Uuid>,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgOpenBoardOutlinePolygon";
        let mut m = Self::new(
            DrcMessageKind::OpenBoardOutlinePolygon,
            Severity::Error,
            tr!(CTX, "Non-closed board outline"),
            serious(
                tr!(
                    CTX,
                    "The board outline polygon is not closed, i.e. the last vertex is not at the same coordinate as the first vertex."
                ),
                tr!(
                    CTX,
                    "Replace multiple coincident polygons with a single, connected polygon and append an explicit last vertex to make the polygon closed."
                ),
            ),
            "open_board_outline",
            locations,
        );
        m.lb();
        if let Some(device) = device {
            m.child_lb("device", device);
        }
        m.child_lb("polygon", polygon);
        m
    }

    /// `DrcMsgIntersectingBoardOutlines`
    pub(super) fn intersecting_board_outlines(
        polygon1: &DrcPolygon,
        device1: Option<&DrcDevice>,
        polygon2: &DrcPolygon,
        device2: Option<&DrcDevice>,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgIntersectingBoardOutlines";
        let mut m = Self::new(
            DrcMessageKind::IntersectingBoardOutlines,
            Severity::Error,
            tr!(CTX, "Intersecting board outlines"),
            with_hint(
                tr!(
                    CTX,
                    "Two board outline polygons are intersecting each other, which will lead to invalid production data."
                ),
                tr!(
                    CTX,
                    "Make sure there is exactly one board outline polygon. Cutouts at the board edge need to be part of the board outline polygon, not separate polygons. Cutouts inside the board need to be drawn on the '{0}' layer.",
                    Layer::BOARD_CUTOUTS.name_tr()
                ),
            ),
            "intersecting_board_outlines",
            locations,
        );
        let serialize = |polygon: &DrcPolygon, device: Option<&DrcDevice>| {
            let mut node = List::new("object");
            node.ensure_line_break();
            if let Some(device) = device {
                node.append_child("device", &device.uuid);
                node.ensure_line_break();
            }
            node.append_child("polygon", &polygon.uuid);
            node.ensure_line_break();
            node
        };
        // Sort nodes to make the approval canonical.
        m.push_sorted_pair(
            serialize(polygon1, device1),
            serialize(polygon2, device2),
            None,
        );
        m
    }

    /// `DrcMsgCutoutOutsideBoardArea`
    pub(super) fn cutout_outside_board_area(
        polygon: &Uuid,
        device: Option<&Uuid>,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgCutoutOutsideBoardArea";
        let mut m = Self::new(
            DrcMessageKind::CutoutOutsideBoardArea,
            Severity::Error,
            tr!(CTX, "Cutout outside of board area"),
            with_hint(
                tr!(
                    CTX,
                    "A cutout polygon is outside the board area (either partially or fully), which will lead to invalid production data."
                ),
                tr!(
                    CTX,
                    "Make sure all cutouts are fully inside the board area. Cutouts at the board edge need to be part of the board outlines polygon on the '{0}' layer, not separate polygons.",
                    Layer::BOARD_OUTLINES.name_tr()
                ),
            ),
            "cutout_outside_board_area",
            locations,
        );
        m.lb();
        if let Some(device) = device {
            m.child_lb("device", device);
        }
        m.child_lb("polygon", polygon);
        m
    }

    /// `DrcMsgMinimumBoardOutlineInnerRadiusViolation`
    pub(super) fn minimum_board_outline_inner_radius_violation(
        min_radius: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgMinimumBoardOutlineInnerRadiusViolation";
        Self::new(
            DrcMessageKind::MinimumBoardOutlineInnerRadiusViolation,
            Severity::Warning,
            tr!(
                CTX,
                "Board outline inner radius < {0} {1}",
                mm(*min_radius),
                "mm"
            ),
            with_hint(
                tr!(
                    CTX,
                    "The board outline polygon is not manufacturable with the minimum tool diameter configured in the DRC settings due to edges with a smaller radius. Thus the actually produced board outline might contain larger edge radii and too small cutouts might even be missing completely."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and add/increase the radius of inner board edges if needed."
                ),
            ),
            "minimum_board_inner_radius_violation",
            locations,
        )
    }

    /// `DrcMsgPlatedCutouts`
    pub(super) fn plated_cutouts(locations: Vec<Path>) -> Self {
        // Upstream declares the translation context of the class above
        // (`Q_DECLARE_TR_FUNCTIONS(DrcMsgMultipleBoardOutlines)`).
        const CTX: &str = "DrcMsgMultipleBoardOutlines";
        Self::new(
            DrcMessageKind::PlatedCutouts,
            Severity::Hint,
            tr!(CTX, "Plated cutouts detected"),
            tr!(
                CTX,
                "The board contains plated cutouts with arbitrary shape (polygons or circles). Those are currently considered EXPERIMENTAL because of the issues described below. In future releases, they might behave differently.\n\n1. Unfortunately, there is no standardized way to let PCB manufacturers know that those cutouts need to be plated. The Gerber export just includes those cutouts in the normal board outlines layer (together with the non-plated cutouts), therefore you need to ensure that the manufacturer will recognize them correctly. Some manufacturers automatically detect plated vs. non-plated cutouts by the existence of copper on the top & bottom layers along the cutout path. The DRC will raise a separate warning if this is not the case, but you should check if your manufacturer will follow this convention.\n\n2. The DRC does not yet detect every possible problem with such cutouts, you have to be careful not to accidentally create short-circuits with plated cutouts as the DRC won't warn you about this."
            ),
            "plated_cutouts",
            locations,
        )
    }

    fn cutout_approval(&mut self, device: Option<&DrcDevice>, name: &str, uuid: &Uuid) {
        self.lb();
        if let Some(device) = device {
            self.child_lb("device", &device.uuid);
        }
        self.child_lb(name, uuid);
    }

    /// `DrcMsgPlatedCutoutWithoutCopper` (`polygon` or `circle`).
    pub(super) fn plated_cutout_without_copper(
        object: (&str, &Uuid),
        device: Option<&DrcDevice>,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgPlatedCutoutWithoutCopper";
        let mut m = Self::new(
            DrcMessageKind::PlatedCutoutWithoutCopper,
            Severity::Warning,
            tr!(CTX, "Plated cutout not surrounded by copper"),
            tr!(
                CTX,
                "Plated- and non-plated cutouts are exported to the same Gerber file because there is no standardized way to send plated cutouts to manufacturers. Some manufacturers just detect plated vs. non-plated cutouts by the existence of copper along their outline.\n\nThis cutout is on the \"{0}\" layer but does not have copper on both top & bottom layers along its complete outline, therefore this detection might fail, possibly leading to wrong manufacturing. It is recommended to draw copper along the complete cutout outline.",
                Layer::BOARD_PLATED_CUTOUTS.name_tr()
            ),
            "plated_cutout_without_copper",
            locations,
        );
        m.cutout_approval(device, object.0, object.1);
        m
    }

    /// `DrcMsgNonPlatedCutoutWithCopper` (`polygon` or `circle`).
    pub(super) fn non_plated_cutout_with_copper(
        object: (&str, &Uuid),
        device: Option<&DrcDevice>,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgNonPlatedCutoutWithCopper";
        let mut m = Self::new(
            DrcMessageKind::NonPlatedCutoutWithCopper,
            Severity::Warning,
            tr!(CTX, "Non-plated cutout intersects with copper"),
            tr!(
                CTX,
                "Plated- and non-plated cutouts are exported to the same Gerber file because there is no standardized way to send plated cutouts to manufacturers. Some manufacturers just detect plated vs. non-plated cutouts by the existence of copper along their outline.\n\nThis cutout is on the \"{0}\" layer but intersects with copper layers along its outline, therefore this detection might fail, possibly leading to wrong manufacturing. It is recommended to remove any copper along the cutouts outline.",
                Layer::BOARD_CUTOUTS.name_tr()
            ),
            "nonplated_cutout_with_copper",
            locations,
        );
        m.cutout_approval(device, object.0, object.1);
        m
    }

    /// `DrcMsgEmptyNetSegment`
    pub(super) fn empty_net_segment(ns: &DrcSegment) -> Self {
        let mut m = Self::new(
            DrcMessageKind::EmptyNetSegment { segment: ns.uuid },
            Severity::Hint,
            tr!(
                "DrcMsgEmptyNetSegment",
                "Empty segment of net '{0}': '{1}'",
                net_name_with_fallback(&ns.net_name),
                ns.uuid
            ),
            // Not translated upstream.
            "There's a net segment in the board without any pad, via or trace. This should not happen, please report it as a bug. But no worries, this issue is not harmful at all and you can just trigger the automatic fix to remove the empty net segment.".to_owned(),
            "empty_netsegment",
            Vec::new(),
        );
        m.lb();
        m.child_lb("netsegment", &ns.uuid);
        m
    }

    /// `DrcMsgUnconnectedJunction`
    pub(super) fn unconnected_junction(
        junction: &DrcJunction,
        ns: &DrcSegment,
        locations: Vec<Path>,
    ) -> Self {
        let mut m = Self::new(
            DrcMessageKind::UnconnectedJunction,
            Severity::Hint,
            tr!(
                "DrcMsgUnconnectedJunction",
                "Unconnected junction in net: '{0}'",
                net_name_with_fallback(&ns.net_name)
            ),
            // Not translated upstream.
            "There's an invisible junction in the board without any trace attached. This should not happen, please report it as a bug. But no worries, this issue is not harmful at all so you can safely ignore this message.".to_owned(),
            "unconnected_junction",
            locations,
        );
        m.lb();
        m.child_lb("netsegment", &ns.uuid);
        m.child_lb("junction", &junction.uuid);
        m
    }

    /// `DrcMsgMinimumTextHeightViolation`
    pub(super) fn minimum_text_height_violation(
        st: &DrcStrokeText,
        device: Option<&DrcDevice>,
        min_height: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgMinimumTextHeightViolation";
        let mut m = Self::new(
            DrcMessageKind::MinimumTextHeightViolation,
            Severity::Warning,
            tr!(
                CTX,
                "Text height on '{0}': {1} < {2} {3}",
                st.layer.name_tr(),
                mm(*st.height),
                mm(*min_height),
                "mm"
            ),
            with_hint(
                tr!(
                    CTX,
                    "The text height is smaller than the minimum height configured in the DRC settings. If the text is smaller than the minimum height specified by the PCB manufacturer, it may not be readable after production."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and increase the text height if needed."
                ),
            ),
            "minimum_text_height_violation",
            locations,
        );
        m.lb();
        if let Some(device) = device {
            m.child_lb("device", &device.uuid);
        }
        m.child_lb("stroke_text", &st.uuid);
        m
    }

    fn minimum_width(
        message: String,
        description: String,
        severity: Severity,
        locations: Vec<Path>,
    ) -> Self {
        Self::new(
            DrcMessageKind::MinimumWidthViolation,
            severity,
            message,
            description,
            "minimum_width_violation",
            locations,
        )
    }

    /// `DrcMsgMinimumWidthViolation` of a trace.
    pub(super) fn minimum_width_trace(
        ns: &DrcSegment,
        trace: &DrcTrace,
        min_width: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgMinimumWidthViolation";
        let mut m = Self::minimum_width(
            tr!(
                CTX,
                "Trace width on '{0}': {1} < {2} {3}",
                trace.layer.name_tr(),
                mm(*trace.width),
                mm(*min_width),
                "mm"
            ),
            serious(
                tr!(
                    CTX,
                    "The trace is thinner than the minimum copper width configured in the DRC settings."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and increase the trace width if needed."
                ),
            ),
            Severity::Error,
            locations,
        );
        m.lb();
        m.child_lb("netsegment", &ns.uuid);
        m.child_lb("trace", &trace.uuid);
        m
    }

    /// `DrcMsgMinimumWidthViolation` of a plane.
    pub(super) fn minimum_width_plane(
        plane: &DrcPlane,
        min_width: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgMinimumWidthViolation";
        let mut m = Self::minimum_width(
            tr!(
                CTX,
                "Min. plane width on '{0}': {1} < {2} {3}",
                plane.layer.name_tr(),
                mm(*plane.min_width),
                mm(*min_width),
                "mm"
            ),
            serious(
                tr!(
                    CTX,
                    "The configured minimum width of the plane is smaller than the minimum copper width configured in the DRC settings."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and increase the minimum plane width in its properties if needed."
                ),
            ),
            Severity::Error,
            locations,
        );
        m.lb();
        m.child_lb("plane", &plane.uuid);
        m
    }

    /// `DrcMsgMinimumWidthViolation` of a board polygon.
    pub(super) fn minimum_width_polygon(
        polygon: &DrcPolygon,
        min_width: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgMinimumWidthViolation";
        let mut m = Self::minimum_width(
            tr!(
                CTX,
                "Polygon width on '{0}': {1} < {2} {3}",
                polygon.layer.name_tr(),
                mm(*polygon.line_width),
                mm(*min_width),
                "mm"
            ),
            with_hint(
                tr!(
                    CTX,
                    "The polygon line width is smaller than the minimum width configured in the DRC settings."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and increase the polygon line width if needed."
                ),
            ),
            Severity::Warning,
            locations,
        );
        m.lb();
        m.child_lb("polygon", &polygon.uuid);
        m
    }

    /// `DrcMsgMinimumWidthViolation` of a stroke text.
    pub(super) fn minimum_width_stroke_text(
        text: &DrcStrokeText,
        device: Option<&DrcDevice>,
        min_width: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgMinimumWidthViolation";
        let mut m = Self::minimum_width(
            tr!(
                CTX,
                "Stroke width on '{0}': {1} < {2} {3}",
                text.layer.name_tr(),
                mm(*text.stroke_width),
                mm(*min_width),
                "mm"
            ),
            with_hint(
                tr!(
                    CTX,
                    "The text stroke width is smaller than the minimum width configured in the DRC settings."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and increase the text stroke width if needed."
                ),
            ),
            Severity::Warning,
            locations,
        );
        m.lb();
        if let Some(device) = device {
            m.child_lb("device", &device.uuid);
        }
        m.child_lb("stroke_text", &text.uuid);
        m
    }

    /// `DrcMsgMinimumWidthViolation` of a footprint polygon.
    pub(super) fn minimum_width_device_polygon(
        device: &DrcDevice,
        polygon: &DrcPolygon,
        min_width: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgMinimumWidthViolation";
        let mut m = Self::minimum_width(
            tr!(
                CTX,
                "Polygon width of '{0}' on '{1}': {2} < {3} {4}",
                device.cmp_instance_name,
                device.transform().map(&polygon.layer).name_tr(),
                mm(*polygon.line_width),
                mm(*min_width),
                "mm"
            ),
            with_hint(
                tr!(
                    CTX,
                    "The polygon line width is smaller than the minimum width configured in the DRC settings."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and increase the polygon line width if needed."
                ),
            ),
            Severity::Warning,
            locations,
        );
        m.lb();
        m.child_lb("device", &device.uuid);
        m.child_lb("polygon", &polygon.uuid);
        m
    }

    /// `DrcMsgMinimumWidthViolation` of a footprint circle.
    pub(super) fn minimum_width_device_circle(
        device: &DrcDevice,
        circle: &DrcCircle,
        min_width: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgMinimumWidthViolation";
        let mut m = Self::minimum_width(
            tr!(
                CTX,
                "Circle width of '{0}' on '{1}': {2} < {3} {4}",
                device.cmp_instance_name,
                device.transform().map(&circle.layer).name_tr(),
                mm(*circle.line_width),
                mm(*min_width),
                "mm"
            ),
            with_hint(
                tr!(
                    CTX,
                    "The circle line width is smaller than the minimum width configured in the DRC settings."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and increase the circle line width if needed."
                ),
            ),
            Severity::Warning,
            locations,
        );
        m.lb();
        m.child_lb("device", &device.uuid);
        m.child_lb("circle", &circle.uuid);
        m
    }

    /// `DrcMsgCopperCopperClearanceViolation`
    pub(super) fn copper_copper_clearance_violation(
        obj1: &CopperObject<'_>,
        obj2: &CopperObject<'_>,
        layer_count: usize,
        single_layer: Option<Layer>,
        min_clearance: Length,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgCopperCopperClearanceViolation";
        let layer_name = match (layer_count, single_layer) {
            (1, Some(layer)) => format!("'{}'", layer.name_tr()),
            _ => tr!(CTX, "{0} layers", layer_count),
        };
        let mut m = Self::new(
            DrcMessageKind::CopperCopperClearanceViolation,
            Severity::Error,
            tr!(
                CTX,
                "Clearance on {0}: {1} ↔ {2} < {3} {4}",
                layer_name,
                obj1.name(),
                obj2.name(),
                mm(min_clearance),
                "mm"
            ),
            serious(
                tr!(
                    CTX,
                    "The clearance between two copper objects of different nets is smaller than the minimum copper clearance configured in the DRC settings."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and move the objects to increase their clearance if needed."
                ),
            ),
            "copper_clearance_violation",
            locations,
        );
        // Sort nodes to make the approval canonical.
        m.push_sorted_pair(obj1.serialize(), obj2.serialize(), None);
        m
    }

    fn copper_board(
        severity: Severity,
        message: String,
        description: String,
        locations: Vec<Path>,
    ) -> Self {
        Self::new(
            DrcMessageKind::CopperBoardClearanceViolation,
            severity,
            message,
            description,
            "copper_board_clearance_violation",
            locations,
        )
    }

    /// `DrcMsgCopperBoardClearanceViolation` of a pad (`parent` is
    /// `("netsegment", uuid)` or `("device", uuid)`).
    pub(super) fn copper_board_clearance_pad(
        parent: (&str, &Uuid),
        pad: &DrcPad,
        min_clearance: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgCopperBoardClearanceViolation";
        let mut m = Self::copper_board(
            Severity::Error,
            tr!(
                CTX,
                "Clearance pad ↔ board outline < {0} {1}",
                mm(*min_clearance),
                "mm"
            ),
            serious(
                tr!(
                    CTX,
                    "The clearance between a pad and the board outline is smaller than the board outline clearance configured in the DRC settings."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and move the pad away from the board outline if needed."
                ),
            ),
            locations,
        );
        m.lb();
        m.child_lb(parent.0, parent.1);
        m.child_lb("pad", &pad.uuid);
        m
    }

    /// `DrcMsgCopperBoardClearanceViolation` of a via.
    pub(super) fn copper_board_clearance_via(
        ns: &DrcSegment,
        via: &DrcVia,
        min_clearance: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgCopperBoardClearanceViolation";
        let mut m = Self::copper_board(
            Severity::Error,
            tr!(
                CTX,
                "Clearance via ↔ board outline < {0} {1}",
                mm(*min_clearance),
                "mm"
            ),
            serious(
                tr!(
                    CTX,
                    "The clearance between a via and the board outline is smaller than the board outline clearance configured in the DRC settings."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and move the via away from the board outline if needed."
                ),
            ),
            locations,
        );
        m.lb();
        m.child_lb("netsegment", &ns.uuid);
        m.child_lb("via", &via.uuid);
        m
    }

    /// `DrcMsgCopperBoardClearanceViolation` of a trace.
    pub(super) fn copper_board_clearance_trace(
        ns: &DrcSegment,
        trace: &DrcTrace,
        min_clearance: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgCopperBoardClearanceViolation";
        let mut m = Self::copper_board(
            Severity::Error,
            tr!(
                CTX,
                "Clearance trace ↔ board outline < {0} {1}",
                mm(*min_clearance),
                "mm"
            ),
            serious(
                tr!(
                    CTX,
                    "The clearance between a trace and the board outline is smaller than the board outline clearance configured in the DRC settings."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and move the trace away from the board outline if needed."
                ),
            ),
            locations,
        );
        m.lb();
        m.child_lb("netsegment", &ns.uuid);
        m.child_lb("trace", &trace.uuid);
        m
    }

    /// `DrcMsgCopperBoardClearanceViolation` of a plane.
    pub(super) fn copper_board_clearance_plane(
        plane: &DrcPlane,
        min_clearance: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgCopperBoardClearanceViolation";
        let mut m = Self::copper_board(
            Severity::Error,
            tr!(
                CTX,
                "Clearance plane ↔ board outline < {0} {1}",
                mm(*min_clearance),
                "mm"
            ),
            serious(
                tr!(
                    CTX,
                    "The clearance between a plane and the board outline is smaller than the board outline clearance configured in the DRC settings."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and increase the configured plane clearance if needed."
                ),
            ),
            locations,
        );
        m.lb();
        m.child_lb("plane", &plane.uuid);
        m
    }

    /// `DrcMsgCopperBoardClearanceViolation` of a polygon.
    pub(super) fn copper_board_clearance_polygon(
        polygon: &DrcPolygon,
        device: Option<&DrcDevice>,
        min_clearance: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgCopperBoardClearanceViolation";
        let mut m = Self::copper_board(
            Severity::Warning,
            tr!(
                CTX,
                "Clearance copper polygon ↔ board outline < {0} {1}",
                mm(*min_clearance),
                "mm"
            ),
            with_hint(
                tr!(
                    CTX,
                    "The clearance between a polygon and the board outline is smaller than the board outline clearance configured in the DRC settings."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and move the polygon away from the board outline if needed."
                ),
            ),
            locations,
        );
        m.lb();
        if let Some(device) = device {
            m.child_lb("device", &device.uuid);
        }
        m.child_lb("polygon", &polygon.uuid);
        m
    }

    /// `DrcMsgCopperBoardClearanceViolation` of a footprint circle.
    pub(super) fn copper_board_clearance_circle(
        device: &DrcDevice,
        circle: &DrcCircle,
        min_clearance: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgCopperBoardClearanceViolation";
        let mut m = Self::copper_board(
            Severity::Warning,
            tr!(
                CTX,
                "Clearance copper circle ↔ board outline < {0} {1}",
                mm(*min_clearance),
                "mm"
            ),
            with_hint(
                tr!(
                    CTX,
                    "The clearance between a circle and the board outline is smaller than the board outline clearance configured in the DRC settings."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and move the circle away from the board outline if needed."
                ),
            ),
            locations,
        );
        m.lb();
        m.child_lb("device", &device.uuid);
        m.child_lb("circle", &circle.uuid);
        m
    }

    /// `DrcMsgCopperBoardClearanceViolation` of a stroke text.
    pub(super) fn copper_board_clearance_stroke_text(
        st: &DrcStrokeText,
        device: Option<&DrcDevice>,
        min_clearance: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgCopperBoardClearanceViolation";
        let mut m = Self::copper_board(
            Severity::Warning,
            tr!(
                CTX,
                "Clearance copper text ↔ board outline < {0} {1}",
                mm(*min_clearance),
                "mm"
            ),
            with_hint(
                tr!(
                    CTX,
                    "The clearance between a stroke text and the board outline is smaller than the board outline clearance configured in the DRC settings."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and move the stroke text away from the board outline if needed."
                ),
            ),
            locations,
        );
        m.lb();
        if let Some(device) = device {
            m.child_lb("device", &device.uuid);
        }
        m.child_lb("stroke_text", &st.uuid);
        m
    }

    /// `DrcMsgCopperHoleClearanceViolation`
    pub(super) fn copper_hole_clearance_violation(
        hole: &DrcHole,
        device: Option<&DrcDevice>,
        min_clearance: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgCopperHoleClearanceViolation";
        let mut m = Self::new(
            DrcMessageKind::CopperHoleClearanceViolation,
            Severity::Error,
            tr!(
                CTX,
                "Clearance copper ↔ hole < {0} {1}",
                mm(*min_clearance),
                "mm"
            ),
            serious(
                tr!(
                    CTX,
                    "The clearance between a non-plated hole and copper objects is smaller than the hole clearance configured in the DRC settings."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and move the copper objects away from the hole if needed."
                ),
            ),
            "copper_hole_clearance_violation",
            locations,
        );
        m.lb();
        if let Some(device) = device {
            m.child_lb("device", &device.uuid);
        }
        m.child_lb("hole", &hole.uuid);
        m
    }

    /// Appends the object and zone nodes of a keepout zone message.
    fn zone_approval(
        &mut self,
        object: &ZoneObject<'_>,
        zone: &DrcZone,
        zone_device: Option<&DrcDevice>,
    ) {
        match object {
            ZoneObject::FootprintPad(dev, pad) => {
                self.child_lb("device", &dev.uuid);
                self.child_lb("pad", &pad.uuid);
            }
            ZoneObject::Pad(ns, pad) => {
                self.child_lb("netsegment", &ns.uuid);
                self.child_lb("pad", &pad.uuid);
            }
            ZoneObject::Via(ns, via) => {
                self.child_lb("netsegment", &ns.uuid);
                self.child_lb("via", &via.uuid);
            }
            ZoneObject::Trace(ns, trace) => {
                self.child_lb("netsegment", &ns.uuid);
                self.child_lb("trace", &trace.uuid);
            }
            ZoneObject::Polygon(polygon) => {
                self.child_lb("polygon", &polygon.uuid);
            }
            ZoneObject::DevicePolygon(dev, polygon) => {
                self.child_lb("device", &dev.uuid);
                self.child_lb("polygon", &polygon.uuid);
            }
            ZoneObject::DeviceCircle(dev, circle) => {
                self.child_lb("device", &dev.uuid);
                self.child_lb("circle", &circle.uuid);
            }
        }
        self.add_zone_nodes(zone, zone_device);
        self.lb();
    }

    /// Upstream `addZoneApprovalNodes()`.
    fn add_zone_nodes(&mut self, zone: &DrcZone, zone_device: Option<&DrcDevice>) {
        let a = self.approval_mut();
        a.ensure_line_break();
        a.append_child("zone", &zone.uuid);
        if let Some(dev) = zone_device {
            a.ensure_line_break();
            a.append_child("from_device", &dev.uuid);
        }
    }

    /// `DrcMsgCopperInKeepoutZone`
    pub(super) fn copper_in_keepout_zone(
        zone: &DrcZone,
        zone_device: Option<&DrcDevice>,
        object: &ZoneObject<'_>,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgCopperInKeepoutZone";
        let message = match object {
            ZoneObject::FootprintPad(dev, pad) => tr!(
                CTX,
                "Pad in copper keepout zone: '{0}'",
                pad_or_net_name(pad, &dev.cmp_instance_name)
            ),
            ZoneObject::Pad(_, pad) => tr!(
                CTX,
                "Pad in copper keepout zone: '{0}'",
                pad_or_net_name(pad, "")
            ),
            ZoneObject::Via(ns, _) => tr!(
                CTX,
                "Via in copper keepout zone: '{0}'",
                net_name_with_fallback(&ns.net_name)
            ),
            ZoneObject::Trace(ns, _) => tr!(
                CTX,
                "Trace in copper keepout zone: '{0}'",
                net_name_with_fallback(&ns.net_name)
            ),
            ZoneObject::Polygon(_) => tr!(CTX, "Polygon in copper keepout zone"),
            ZoneObject::DevicePolygon(dev, _) => tr!(
                CTX,
                "Polygon in copper keepout zone: '{0}'",
                dev.cmp_instance_name
            ),
            ZoneObject::DeviceCircle(dev, _) => tr!(
                CTX,
                "Circle in copper keepout zone: '{0}'",
                dev.cmp_instance_name
            ),
        };
        let mut m = Self::new(
            DrcMessageKind::CopperInKeepoutZone,
            Severity::Error,
            message,
            with_hint(
                tr!(
                    CTX,
                    "There is a copper object within a copper keepout zone."
                ),
                tr!(CTX, "Move the object to outside the keepout zone."),
            ),
            "copper_in_keepout_zone",
            locations,
        );
        m.zone_approval(object, zone, zone_device);
        m
    }

    /// `DrcMsgExposureInKeepoutZone` (traces are not possible).
    pub(super) fn exposure_in_keepout_zone(
        zone: &DrcZone,
        zone_device: Option<&DrcDevice>,
        object: &ZoneObject<'_>,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgExposureInKeepoutZone";
        let message = match object {
            ZoneObject::FootprintPad(dev, pad) => tr!(
                CTX,
                "Pad in exposure keepout zone: '{0}'",
                pad_or_net_name(pad, &dev.cmp_instance_name)
            ),
            ZoneObject::Pad(_, pad) => tr!(
                CTX,
                "Pad in exposure keepout zone: '{0}'",
                pad_or_net_name(pad, "")
            ),
            ZoneObject::Via(ns, _) | ZoneObject::Trace(ns, _) => tr!(
                CTX,
                "Via in exposure keepout zone: '{0}'",
                net_name_with_fallback(&ns.net_name)
            ),
            ZoneObject::Polygon(_) => tr!(CTX, "Polygon in exposure keepout zone"),
            ZoneObject::DevicePolygon(dev, _) => tr!(
                CTX,
                "Polygon in exposure keepout zone: '{0}'",
                dev.cmp_instance_name
            ),
            ZoneObject::DeviceCircle(dev, _) => tr!(
                CTX,
                "Circle in exposure keepout zone: '{0}'",
                dev.cmp_instance_name
            ),
        };
        let mut m = Self::new(
            DrcMessageKind::ExposureInKeepoutZone,
            Severity::Error,
            message,
            with_hint(
                tr!(
                    CTX,
                    "There is a solder resist opening within an exposure keepout zone."
                ),
                tr!(CTX, "Move the object to outside the keepout zone."),
            ),
            "exposure_in_keepout_zone",
            locations,
        );
        m.zone_approval(object, zone, zone_device);
        m
    }

    /// `DrcMsgDeviceInKeepoutZone`
    pub(super) fn device_in_keepout_zone(
        zone: &DrcZone,
        zone_device: Option<&DrcDevice>,
        device: &DrcDevice,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgDeviceInKeepoutZone";
        let mut m = Self::new(
            DrcMessageKind::DeviceInKeepoutZone,
            Severity::Error,
            tr!(
                CTX,
                "Device in keepout zone: '{0}'",
                device.cmp_instance_name
            ),
            with_hint(
                tr!(CTX, "There is a device within a keepout zone."),
                tr!(CTX, "Move the device to outside the keepout zone."),
            ),
            "device_in_keepout_zone",
            locations,
        );
        m.child_lb("device", &device.uuid);
        m.add_zone_nodes(zone, zone_device);
        m.lb();
        m
    }

    /// `DrcMsgDrillDrillClearanceViolation`
    pub(super) fn drill_drill_clearance_violation(
        hole1: &DrcHoleRef<'_>,
        hole2: &DrcHoleRef<'_>,
        min_clearance: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgDrillDrillClearanceViolation";
        let mut m = Self::new(
            DrcMessageKind::DrillDrillClearanceViolation,
            Severity::Error,
            tr!(
                CTX,
                "Clearance drill ↔ drill < {0} {1}",
                mm(*min_clearance),
                "mm"
            ),
            serious(
                tr!(
                    CTX,
                    "The clearance between two drills is smaller than the drill clearance configured in the DRC settings."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and move the drills to increase their distance if needed."
                ),
            ),
            "drill_clearance_violation",
            locations,
        );
        let mut node1 = List::new("drill");
        hole1.serialize(&mut node1);
        let mut node2 = List::new("drill");
        hole2.serialize(&mut node2);
        // Sort nodes to make the approval canonical.
        m.push_sorted_pair(node1, node2, None);
        m
    }

    /// `DrcMsgDrillBoardClearanceViolation`
    pub(super) fn drill_board_clearance_violation(
        hole: &DrcHoleRef<'_>,
        min_clearance: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgDrillBoardClearanceViolation";
        let mut m = Self::new(
            DrcMessageKind::DrillBoardClearanceViolation,
            Severity::Error,
            tr!(
                CTX,
                "Clearance drill ↔ board outline < {0} {1}",
                mm(*min_clearance),
                "mm"
            ),
            serious(
                tr!(
                    CTX,
                    "The clearance between a drill and the board outline is smaller than the drill clearance configured in the DRC settings."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and move the drill away from the board outline if needed."
                ),
            ),
            "drill_board_clearance_violation",
            locations,
        );
        m.lb();
        hole.serialize(m.approval_mut());
        m.lb();
        m
    }

    fn device_pair(&mut self, device1: &DrcDevice, device2: &DrcDevice) {
        self.lb();
        self.child_lb("device", &device1.uuid.min(device2.uuid));
        self.child_lb("device", &device1.uuid.max(device2.uuid));
    }

    fn device_names(device1: &DrcDevice, device2: &DrcDevice) -> (String, String) {
        let (a, b) = (&device1.cmp_instance_name, &device2.cmp_instance_name);
        (a.min(b).clone(), a.max(b).clone())
    }

    /// `DrcMsgDeviceInCourtyard`
    pub(super) fn device_in_courtyard(
        device1: &DrcDevice,
        device2: &DrcDevice,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgDeviceInCourtyard";
        let (name1, name2) = Self::device_names(device1, device2);
        let mut m = Self::new(
            DrcMessageKind::DeviceInCourtyard,
            Severity::Warning,
            tr!(CTX, "Device in courtyard: '{0}' ↔ '{1}'", name1, name2),
            with_hint(
                tr!(
                    CTX,
                    "A device is placed within the courtyard of another device, which might cause troubles during assembly of these parts."
                ),
                tr!(
                    CTX,
                    "Either move the devices to increase their clearance or approve this message if you're sure they can be assembled without problems."
                ),
            ),
            "device_in_courtyard",
            locations,
        );
        m.device_pair(device1, device2);
        m
    }

    /// `DrcMsgOverlappingDevices`
    pub(super) fn overlapping_devices(
        device1: &DrcDevice,
        device2: &DrcDevice,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgOverlappingDevices";
        let (name1, name2) = Self::device_names(device1, device2);
        let mut m = Self::new(
            DrcMessageKind::OverlappingDevices,
            Severity::Error,
            tr!(CTX, "Device overlap: '{0}' ↔ '{1}'", name1, name2),
            with_hint(
                tr!(
                    CTX,
                    "Two devices are overlapping and thus probably cannot be assembled both at the same time."
                ),
                tr!(
                    CTX,
                    "Either move the devices to increase their clearance or approve this message if you're sure they can be assembled without problems (or only one of them gets assembled)."
                ),
            ),
            "overlapping_devices",
            locations,
        );
        m.device_pair(device1, device2);
        m
    }

    fn annular_ring(message: String, description: String, locations: Vec<Path>) -> Self {
        Self::new(
            DrcMessageKind::MinimumAnnularRingViolation,
            Severity::Error,
            message,
            description,
            "minimum_annular_ring_violation",
            locations,
        )
    }

    /// `DrcMsgMinimumAnnularRingViolation` of a via.
    pub(super) fn minimum_annular_ring_via(
        ns: &DrcSegment,
        via: &DrcVia,
        min_annular_width: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgMinimumAnnularRingViolation";
        let mut m = Self::annular_ring(
            tr!(
                CTX,
                "Via annular ring of '{0}' < {1} {2}",
                net_name_with_fallback(&ns.net_name),
                mm(*min_annular_width),
                "mm"
            ),
            serious(
                tr!(
                    CTX,
                    "The via annular ring width (i.e. the copper around the hole) is smaller than the minimum annular width configured in the DRC settings."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and increase the via size if needed."
                ),
            ),
            locations,
        );
        m.lb();
        m.child_lb("netsegment", &ns.uuid);
        m.child_lb("via", &via.uuid);
        m
    }

    /// `DrcMsgMinimumAnnularRingViolation` of a pad (`parent` is
    /// `("netsegment", uuid, "")` or `("device", uuid, component name)`).
    pub(super) fn minimum_annular_ring_pad(
        parent: (&str, &Uuid, &str),
        pad: &DrcPad,
        min_annular_width: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgMinimumAnnularRingViolation";
        let mut m = Self::annular_ring(
            tr!(
                CTX,
                "Pad annular ring of '{0}' < {1} {2}",
                pad_or_net_name(pad, parent.2),
                mm(*min_annular_width),
                "mm"
            ),
            serious(
                tr!(
                    CTX,
                    "The through-hole pad annular ring width (i.e. the copper around the hole) is smaller than the minimum annular width configured in the DRC settings."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and increase the pad size if needed."
                ),
            ),
            locations,
        );
        m.lb();
        m.child_lb(parent.0, parent.1);
        m.child_lb("pad", &pad.uuid);
        m
    }

    /// `DrcMsgMinimumDrillDiameterViolation`
    pub(super) fn minimum_drill_diameter_violation(
        hole: &DrcHoleRef<'_>,
        min_diameter: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgMinimumDrillDiameterViolation";
        let message = if hole.is_via_hole() {
            tr!(
                CTX,
                "Via drill diameter of '{0}': {1} < {2} {3}",
                net_name_with_fallback(hole.net_name()),
                mm(*hole.diameter()),
                mm(*min_diameter),
                "mm"
            )
        } else if let Some(pad) = hole.pad() {
            tr!(
                CTX,
                "Pad drill diameter of '{0}': {1} < {2} {3}",
                pad.lib_pkg_pad_name,
                mm(*hole.diameter()),
                mm(*min_diameter),
                "mm"
            )
        } else {
            tr!(
                CTX,
                "NPTH drill diameter: {0} < {1} {2}",
                mm(*hole.diameter()),
                mm(*min_diameter),
                "mm"
            )
        };
        let text = if hole.is_via_hole() {
            tr!(
                CTX,
                "The drill diameter of the via is smaller than the minimum plated drill diameter configured in the DRC settings."
            )
        } else if hole.is_pad_hole() {
            tr!(
                CTX,
                "The drill diameter of the through-hole pad is smaller than the minimum plated drill diameter configured in the DRC settings."
            )
        } else {
            tr!(
                CTX,
                "The drill diameter of the non-plated hole is smaller than the minimum non-plated drill diameter configured in the DRC settings."
            )
        };
        let mut m = Self::new(
            DrcMessageKind::MinimumDrillDiameterViolation,
            Severity::Warning,
            message,
            with_hint(
                text,
                tr!(
                    CTX,
                    "Check the DRC settings and increase the drill diameter if needed."
                ),
            ),
            "minimum_drill_diameter_violation",
            locations,
        );
        m.lb();
        hole.serialize(m.approval_mut());
        m.lb();
        m
    }

    /// `DrcMsgMinimumSlotWidthViolation`
    pub(super) fn minimum_slot_width_violation(
        hole: &DrcHoleRef<'_>,
        min_width: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgMinimumSlotWidthViolation";
        let actual = mm(*hole.diameter());
        let (message, text) = if hole.is_plated() {
            (
                tr!(
                    CTX,
                    "Plated slot width: {0} < {1} {2}",
                    actual,
                    mm(*min_width),
                    "mm"
                ),
                tr!(
                    CTX,
                    "The width of the plated slot is smaller than the minimum plated slot width configured in the DRC settings."
                ),
            )
        } else {
            (
                tr!(
                    CTX,
                    "NPTH slot width: {0} < {1} {2}",
                    actual,
                    mm(*min_width),
                    "mm"
                ),
                tr!(
                    CTX,
                    "The width of the non-plated slot is smaller than the minimum non-plated slot width configured in the DRC settings."
                ),
            )
        };
        let mut m = Self::new(
            DrcMessageKind::MinimumSlotWidthViolation,
            Severity::Warning,
            message,
            with_hint(
                text,
                tr!(
                    CTX,
                    "Check the DRC settings and increase the slot width if needed."
                ),
            ),
            "minimum_slot_width_violation",
            locations,
        );
        m.lb();
        hole.serialize(m.approval_mut());
        m.lb();
        m
    }

    /// `DrcMsgInvalidPadConnection` (`parent` is `("netsegment", uuid, "")`
    /// or `("device", uuid, component name)`).
    pub(super) fn invalid_pad_connection(
        parent: (&str, &Uuid, &str),
        pad: &DrcPad,
        layer: Layer,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgInvalidPadConnection";
        let mut m = Self::new(
            DrcMessageKind::InvalidPadConnection,
            Severity::Error,
            // Upstream substitutes only the first placeholder.
            tr!(
                CTX,
                "Invalid connection of pad '{0}' on '{1}'",
                pad_or_net_name(pad, parent.2),
                "%2"
            ),
            tr!(
                CTX,
                "The pad origin must be located within the pads copper area, or for THT pads within a hole. Otherwise traces might not beconnected fully. This issue needs to be fixed in the library."
            ),
            "invalid_pad_connection",
            locations,
        );
        m.approval_mut().append_child("layer", &layer);
        m.lb();
        m.child_lb(parent.0, parent.1);
        m.child_lb("pad", &pad.uuid);
        m
    }

    /// `DrcMsgForbiddenSlot` (`parent` is `None` for board holes, the
    /// device for device holes, or the device/segment and pad for pad
    /// holes).
    pub(super) fn forbidden_slot(
        hole: &DrcHole,
        parent: Option<(&str, &Uuid)>,
        pad: Option<&DrcPad>,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgForbiddenSlot";
        let path: &NonEmptyPath = &hole.path;
        let suggestion = format!(
            "\n{}",
            tr!(
                CTX,
                "Either avoid them or check if your PCB manufacturer supports them."
            )
        );
        let check_slot_mode = format!(
            "\n{}",
            tr!(
                CTX,
                "Choose the desired Excellon slot mode when generating the production data (G85 vs. G00..G03)."
            )
        );
        let g85_not_available = format!(
            "\n{}",
            tr!(
                CTX,
                "The drilled slot mode (G85) will not be available when generating production data."
            )
        );
        let (message, description) = if path.is_curved() {
            (
                tr!(CTX, "Hole is a slot with curves"),
                tr!(
                    CTX,
                    "Curved slots are a very unusual thing and may cause troubles with many PCB manufacturers."
                ) + &suggestion
                    + &g85_not_available,
            )
        } else if path.vertices().len() > 2 {
            (
                tr!(CTX, "Hole is a multi-segment slot"),
                tr!(
                    CTX,
                    "Multi-segment slots are a rather unusual thing and may cause troubles with some PCB manufacturers."
                ) + &suggestion
                    + &check_slot_mode,
            )
        } else {
            (
                tr!(CTX, "Hole is a slot"),
                tr!(CTX, "Slots may cause troubles with some PCB manufacturers.")
                    + &suggestion
                    + &check_slot_mode,
            )
        };
        let mut m = Self::new(
            DrcMessageKind::ForbiddenSlot,
            Severity::Warning,
            message,
            description,
            "forbidden_slot",
            locations,
        );
        m.lb();
        if let Some((name, uuid)) = parent {
            m.child_lb(name, uuid);
        }
        if let Some(pad) = pad {
            m.child_lb("pad", &pad.uuid);
        }
        m.child_lb("hole", &hole.uuid);
        m
    }

    fn via_approval(&mut self, ns: &DrcSegment, via: &DrcVia) {
        self.lb();
        self.child_lb("netsegment", &ns.uuid);
        self.child_lb("via", &via.uuid);
    }

    /// `DrcMsgForbiddenVia`
    pub(super) fn forbidden_via(ns: &DrcSegment, via: &DrcVia, locations: Vec<Path>) -> Self {
        const CTX: &str = "DrcMsgForbiddenVia";
        let net = net_name_with_fallback(&ns.net_name);
        let suggestion = format!(
            "\n{}",
            tr!(
                CTX,
                "Either avoid them or check if your PCB manufacturer supports them and adjust the DRC settings accordingly."
            )
        );
        let (message, description) = if via.is_blind {
            (
                tr!(CTX, "Blind via in net '{0}'", net),
                tr!(
                    CTX,
                    "Blind vias are expensive to manufacture and not every PCB manufacturer is able to create them."
                ) + &suggestion,
            )
        } else {
            (
                tr!(CTX, "Buried via in net '{0}'", net),
                tr!(
                    CTX,
                    "Buried vias are expensive to manufacture and not every PCB manufacturer is able to create them."
                ) + &suggestion,
            )
        };
        let mut m = Self::new(
            DrcMessageKind::ForbiddenVia,
            Severity::Error,
            message,
            description,
            "forbidden_via",
            locations,
        );
        m.via_approval(ns, via);
        m
    }

    /// `DrcMsgInvalidVia`
    pub(super) fn invalid_via(ns: &DrcSegment, via: &DrcVia, locations: Vec<Path>) -> Self {
        const CTX: &str = "DrcMsgInvalidVia";
        let mut m = Self::new(
            DrcMessageKind::InvalidVia,
            Severity::Warning,
            tr!(
                CTX,
                "Invalid via in net '{0}'",
                net_name_with_fallback(&ns.net_name)
            ),
            tr!(
                CTX,
                "The via is only drilled between one layer and is therefore invalid."
            ),
            "invalid_via",
            locations,
        );
        m.via_approval(ns, via);
        m
    }

    /// `DrcMsgPlaneThermalSpokeWidthIgnored`
    pub(super) fn plane_thermal_spoke_width_ignored(
        plane: &DrcPlane,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgPlaneThermalSpokeWidthIgnored";
        let mut m = Self::new(
            DrcMessageKind::PlaneThermalSpokeWidthIgnored { plane: plane.uuid },
            Severity::Warning,
            tr!(
                CTX,
                "Ignored thermal spoke width of plane in '{0}'",
                net_name_with_fallback(&plane.net_name)
            ),
            with_hint(
                tr!(
                    CTX,
                    "The thermal spoke width of the plane is set to a value smaller than the planes minimum width, which is not allowed. Therefore the plane filling algorithm ignores the specified spoke width and uses the specified minimum width instead."
                ),
                tr!(
                    CTX,
                    "It is recommended to explicitly set the thermal spoke width equal to or higher than the minimum width of the plane."
                ),
            ),
            "plane_thermal_spoke_width_ignored",
            locations,
        );
        m.lb();
        m.child_lb("plane", &plane.uuid);
        m
    }

    /// `DrcMsgSilkscreenClearanceViolation`
    pub(super) fn silkscreen_clearance_violation(
        st: &DrcStrokeText,
        device: Option<&DrcDevice>,
        min_clearance: UnsignedLength,
        locations: Vec<Path>,
    ) -> Self {
        const CTX: &str = "DrcMsgSilkscreenClearanceViolation";
        let mut m = Self::new(
            DrcMessageKind::SilkscreenClearanceViolation,
            Severity::Warning,
            tr!(
                CTX,
                "Clearance silkscreen text ↔ stop mask < {0} {1}",
                mm(*min_clearance),
                "mm"
            ),
            with_hint(
                tr!(
                    CTX,
                    "The clearance between a silkscreen text and a solder resist opening is smaller than the minimum clearance configured in the DRC settings. This could lead to clipped silkscreen during production."
                ),
                tr!(
                    CTX,
                    "Check the DRC settings and move the text away from the solder resist opening if needed."
                ),
            ),
            "silkscreen_clearance_violation",
            locations,
        );
        m.lb();
        if let Some(device) = device {
            m.child_lb("device", &device.uuid);
        }
        m.child_lb("stroke_text", &st.uuid);
        m
    }

    /// `DrcMsgUselessZone`
    pub(super) fn useless_zone(zone: &DrcZone, locations: Vec<Path>) -> Self {
        const CTX: &str = "DrcMsgUselessZone";
        let mut m = Self::new(
            DrcMessageKind::UselessZone,
            Severity::Warning,
            tr!(CTX, "Useless zone"),
            tr!(
                CTX,
                "The zone has no layer or rule enabled so it is useless."
            ),
            "useless_zone",
            locations,
        );
        m.lb();
        m.child_lb("zone", &zone.uuid);
        m
    }

    /// `DrcMsgUselessVia`
    pub(super) fn useless_via(ns: &DrcSegment, via: &DrcVia, locations: Vec<Path>) -> Self {
        const CTX: &str = "DrcMsgUselessVia";
        let mut m = Self::new(
            DrcMessageKind::UselessVia,
            Severity::Warning,
            tr!(
                CTX,
                "Useless via in net '{0}'",
                net_name_with_fallback(&ns.net_name)
            ),
            tr!(
                CTX,
                "The via is connected on less than two layers, thus it seems to be useless."
            ),
            "useless_via",
            locations,
        );
        m.via_approval(ns, via);
        m
    }

    /// `DrcMsgDisabledLayer`
    pub(super) fn disabled_layer(layer: Layer) -> Self {
        const CTX: &str = "DrcMsgDisabledLayer";
        let mut m = Self::new(
            DrcMessageKind::DisabledLayer,
            Severity::Warning,
            tr!(CTX, "Objects on disabled layer: '{0}'", layer.name_tr()),
            tr!(
                CTX,
                "The layer contains copper objects, but it is disabled in the board setup dialog and thus will be ignored in any production data exports. Either increase the layer count to get this layer exported, or remove all objects on this layer (by temporarily enabling this layer to see them)."
            ),
            "disabled_layer",
            Vec::new(),
        );
        m.lb();
        m.child_lb_str("layer", layer.id());
        m
    }

    /// `DrcMsgUnusedLayer`
    pub(super) fn unused_layer(layer: Layer) -> Self {
        const CTX: &str = "DrcMsgUnusedLayer";
        let mut m = Self::new(
            DrcMessageKind::UnusedLayer,
            Severity::Hint,
            tr!(CTX, "Unused layer: '{0}'", layer.name_tr()),
            tr!(
                CTX,
                "The layer contains no copper objects (except the automatically generated through-hole annular rings, if any) so it is useless. This is not critical, but if your intention is to flood it with copper, you need to add a plane manually. Or if you don't need this layer, you might want to reduce the layer count in the board setup dialog to avoid unnecessary production costs. Also some PCB manufacturers might be confused by empty layers."
            ),
            "unused_layer",
            Vec::new(),
        );
        m.lb();
        m.child_lb_str("layer", layer.id());
        m
    }

    /// Appends `(name "value")` (a string, like upstream's `QString`
    /// serialization) and a line break.
    fn child_lb_str(&mut self, name: &str, value: &str) {
        let a = self.approval_mut();
        a.append_child(name, value);
        a.ensure_line_break();
    }
}

/// Returns the transform of a pad.
pub(super) fn pad_transform(pad: &DrcPad) -> Transform {
    Transform::new(pad.position, pad.rotation, pad.mirror)
}
