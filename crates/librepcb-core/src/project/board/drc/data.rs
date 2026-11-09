//! Port of libs/librepcb/core/project/board/drc/boarddesignrulecheckdata.{h,cpp}.
//!
//! The input data of the design rule check: a plain, owned snapshot of
//! everything the checks need, extracted from a [`Project`] and one of its
//! boards (upstream copies the data from the board objects for the same
//! reason: the checks run in other threads while the board may be edited).
//!
//! Differences to upstream:
//!
//! - Layers are [`Layer`] values instead of pointers; sets and maps of
//!   layers, net segments, junctions, vias and pads are `BTree*`
//!   collections, so the checks iterate them in a deterministic order
//!   (upstream iterates `QHash`es, see COMPAT.md).
//! - Air wire anchors are an enum instead of a struct of optional UUIDs.
//! - The plane fragments are taken from the board's derived data
//!   ([`BoardDerived::fragments_of()`](super::super::BoardDerived::fragments_of));
//!   the caller is responsible for rebuilding them before (like upstream
//!   `BoardDesignRuleCheck::start()`,
//!   [`Project::run_drc()`](crate::project::Project::run_drc) does it).

use std::collections::{BTreeMap, BTreeSet};

use super::error::{Error, Result};
use crate::font::{StrokeFont, StrokeTextPathBuilder};
use crate::geometry::{NonEmptyPath, PadGeometry, Path, TraceAnchor, ZoneLayers, ZoneRules};
use crate::library::org::BoardDesignRuleCheckSettings;
use crate::project::attribute_lookup::ProjectAttributeLookup;
use crate::project::board::device::ResolvedDevice;
use crate::project::board::pad_data::PadOnBoard;
use crate::project::board::{Board, BoardDesignRules, BoardDevice, BoardNetSegment};
use crate::project::board::{BoardStrokeTextData, FootprintPadView};
use crate::project::circuit::Circuit;
use crate::project::{BoardId, EntityKind, NetSignalId, Project, ProjectLibrary};
use crate::types::{Angle, Layer, Length, Point, PositiveLength, UnsignedLength, Uuid};
use crate::utils::transform::Transform;

/// Design rules of a net class (upstream `Data::NetClass`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrcNetClass {
    /// Minimum copper clearance.
    pub min_copper_copper_clearance: UnsignedLength,
    /// Minimum copper width.
    pub min_copper_width: UnsignedLength,
    /// Minimum via drill diameter.
    pub min_via_drill_diameter: UnsignedLength,
}

/// A junction of a net segment (upstream `Data::Junction`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrcJunction {
    /// UUID.
    pub uuid: Uuid,
    /// Position.
    pub position: Point,
    /// Number of traces connected to the junction.
    pub traces: usize,
}

/// A trace of a net segment (upstream `Data::Trace`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrcTrace {
    /// UUID.
    pub uuid: Uuid,
    /// Start position.
    pub p1: Point,
    /// End position.
    pub p2: Point,
    /// Width.
    pub width: PositiveLength,
    /// Copper layer.
    pub layer: Layer,
}

/// A via of a net segment (upstream `Data::Via`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrcVia {
    /// UUID.
    pub uuid: Uuid,
    /// Position.
    pub position: Point,
    /// Actual drill diameter (automatic values resolved).
    pub drill_diameter: PositiveLength,
    /// Actual size (automatic values resolved).
    pub size: PositiveLength,
    /// The layers of traces directly connected to the via.
    pub connected_layers: BTreeSet<Layer>,
    /// Start layer.
    pub start_layer: Layer,
    /// End layer.
    pub end_layer: Layer,
    /// The drilled layer span on this board (`None` = invalid via).
    pub drill_layer_span: Option<(Layer, Layer)>,
    /// Whether the via is buried.
    pub is_buried: bool,
    /// Whether the via is blind.
    pub is_blind: bool,
    /// Diameter of the stop mask opening on the top side.
    pub stop_mask_diameter_top: Option<PositiveLength>,
    /// Diameter of the stop mask opening on the bottom side.
    pub stop_mask_diameter_bot: Option<PositiveLength>,
}

/// A hole (upstream `Data::Hole`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrcHole {
    /// UUID.
    pub uuid: Uuid,
    /// Diameter.
    pub diameter: PositiveLength,
    /// Path (relative to the pad or device, if any).
    pub path: NonEmptyPath,
    /// Stop mask offset (`None` = no stop mask opening; always `None` for
    /// pad holes).
    pub stop_mask_offset: Option<Length>,
}

/// A pad, either standalone or of a footprint (upstream `Data::Pad`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrcPad {
    /// UUID.
    pub uuid: Uuid,
    /// Name of the package pad (empty if not connected to a package pad).
    pub lib_pkg_pad_name: String,
    /// Absolute position.
    pub position: Point,
    /// Absolute rotation.
    pub rotation: Angle,
    /// Whether the pad is mirrored.
    pub mirror: bool,
    /// The holes (relative to the pad).
    pub holes: Vec<DrcHole>,
    /// The geometries per layer (relative to the pad).
    pub geometries: BTreeMap<Layer, Vec<PadGeometry>>,
    /// Layers where traces are connected.
    pub layers_with_traces: BTreeSet<Layer>,
    /// Copper clearance of the pad.
    pub copper_clearance: UnsignedLength,
    /// Net (UUID).
    pub net: Option<Uuid>,
    /// Net name (empty if no net).
    pub net_name: String,
    /// Net class (UUID).
    pub net_class: Option<Uuid>,
}

/// A net segment (upstream `Data::Segment`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrcSegment {
    /// UUID.
    pub uuid: Uuid,
    /// Net (UUID).
    pub net: Option<Uuid>,
    /// Net name (empty if no net).
    pub net_name: String,
    /// Net class (UUID).
    pub net_class: Option<Uuid>,
    /// Junctions by UUID.
    pub junctions: BTreeMap<Uuid, DrcJunction>,
    /// Traces (in UUID order).
    pub traces: Vec<DrcTrace>,
    /// Vias by UUID.
    pub vias: BTreeMap<Uuid, DrcVia>,
    /// Standalone pads by UUID.
    pub pads: BTreeMap<Uuid, DrcPad>,
}

/// The object an air wire is attached to (upstream `Data::AirWireAnchor`
/// without the position).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrcAirWireObject {
    /// A footprint pad.
    FootprintPad {
        /// Component instance of the device.
        device: Uuid,
        /// Footprint pad.
        pad: Uuid,
    },
    /// A standalone pad of a net segment.
    Pad {
        /// Net segment.
        segment: Uuid,
        /// Pad.
        pad: Uuid,
    },
    /// A junction of a net segment.
    Junction {
        /// Net segment.
        segment: Uuid,
        /// Junction.
        junction: Uuid,
    },
    /// A via of a net segment.
    Via {
        /// Net segment.
        segment: Uuid,
        /// Via.
        via: Uuid,
    },
}

/// An end point of an air wire (upstream `Data::AirWireAnchor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrcAirWireAnchor {
    /// Position.
    pub position: Point,
    /// The anchor object; `None` if it could not be resolved (the check for
    /// missing connections fails then, like upstream).
    pub object: Option<DrcAirWireObject>,
}

/// An air wire (upstream `Data::AirWire`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrcAirWire {
    /// First anchor.
    pub p1: DrcAirWireAnchor,
    /// Second anchor.
    pub p2: DrcAirWireAnchor,
    /// Net name.
    pub net_name: String,
}

/// A plane (upstream `Data::Plane`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrcPlane {
    /// UUID.
    pub uuid: Uuid,
    /// Net (UUID).
    pub net: Option<Uuid>,
    /// Net name (empty if no net).
    pub net_name: String,
    /// Net class (UUID).
    pub net_class: Option<Uuid>,
    /// Copper layer.
    pub layer: Layer,
    /// Minimum width.
    pub min_width: UnsignedLength,
    /// Thermal spoke width.
    pub thermal_spoke_width: PositiveLength,
    /// Outline.
    pub outline: Path,
    /// The calculated fragments.
    pub fragments: Vec<Path>,
}

/// A polygon of the board or of a footprint (upstream `Data::Polygon`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrcPolygon {
    /// UUID.
    pub uuid: Uuid,
    /// Layer (not mirrored for footprint polygons).
    pub layer: Layer,
    /// Line width.
    pub line_width: UnsignedLength,
    /// Whether the polygon is filled.
    pub filled: bool,
    /// Path (relative to the device for footprint polygons).
    pub path: Path,
}

/// A circle of a footprint (upstream `Data::Circle`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrcCircle {
    /// UUID.
    pub uuid: Uuid,
    /// Center (relative to the device).
    pub center: Point,
    /// Diameter.
    pub diameter: PositiveLength,
    /// Layer (not mirrored).
    pub layer: Layer,
    /// Line width.
    pub line_width: UnsignedLength,
    /// Whether the circle is filled.
    pub filled: bool,
}

/// A stroke text of the board or of a device (upstream `Data::StrokeText`,
/// always in board coordinates).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrcStrokeText {
    /// UUID.
    pub uuid: Uuid,
    /// Position.
    pub position: Point,
    /// Rotation.
    pub rotation: Angle,
    /// Whether the text is mirrored.
    pub mirror: bool,
    /// Layer.
    pub layer: Layer,
    /// Stroke width.
    pub stroke_width: UnsignedLength,
    /// Text height.
    pub height: PositiveLength,
    /// The rendered paths (relative to the text position, not rotated).
    pub paths: Vec<Path>,
}

/// A keepout zone of the board or of a footprint (upstream `Data::Zone`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrcZone {
    /// UUID.
    pub uuid: Uuid,
    /// Layers of a board zone (empty for device zones).
    pub board_layers: BTreeSet<Layer>,
    /// Layers of a footprint zone (empty for board zones).
    pub footprint_layers: ZoneLayers,
    /// Rules.
    pub rules: ZoneRules,
    /// Outline (relative to the device for footprint zones).
    pub outline: Path,
}

/// A signal of a component which is connected to a net but has no pad in
/// the footprint (upstream `Data::ImpossibleConnection`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrcImpossibleConnection {
    /// UUID of the component signal.
    pub signal_uuid: Uuid,
    /// Name of the component signal.
    pub signal_name: String,
    /// Name of the net.
    pub net_name: String,
}

/// A device (upstream `Data::Device`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrcDevice {
    /// UUID of the component instance.
    pub uuid: Uuid,
    /// Name of the component instance.
    pub cmp_instance_name: String,
    /// Position.
    pub position: Point,
    /// Rotation.
    pub rotation: Angle,
    /// Whether the device is mirrored.
    pub mirror: bool,
    /// Footprint pads by UUID (absolute transform).
    pub pads: BTreeMap<Uuid, DrcPad>,
    /// Polygons of the library footprint.
    pub polygons: Vec<DrcPolygon>,
    /// Circles of the library footprint.
    pub circles: Vec<DrcCircle>,
    /// Stroke texts (absolute transform).
    pub stroke_texts: Vec<DrcStrokeText>,
    /// Holes of the library footprint.
    pub holes: Vec<DrcHole>,
    /// Zones of the library footprint.
    pub zones: Vec<DrcZone>,
    /// Signals connected to a net without a pad (only filled for a full
    /// check).
    pub impossible_signal_connections: Vec<DrcImpossibleConnection>,
}

impl DrcDevice {
    /// Returns the transformation of the device.
    pub fn transform(&self) -> Transform {
        Transform::new(self.position, self.rotation, self.mirror)
    }
}

/// The input data of the design rule check (upstream
/// `BoardDesignRuleCheckData`).
#[derive(Debug, Clone)]
pub struct BoardDrcData {
    /// The DRC settings.
    pub settings: BoardDesignRuleCheckSettings,
    /// Whether only the quick check is run.
    pub quick: bool,
    /// All copper layers of the board.
    pub copper_layers: BTreeSet<Layer>,
    /// Silkscreen layers of the top side.
    pub silkscreen_layers_top: Vec<Layer>,
    /// Silkscreen layers of the bottom side.
    pub silkscreen_layers_bot: Vec<Layer>,
    /// Net classes by UUID.
    pub net_classes: BTreeMap<Uuid, DrcNetClass>,
    /// Net segments by UUID.
    pub segments: BTreeMap<Uuid, DrcSegment>,
    /// Planes.
    pub planes: Vec<DrcPlane>,
    /// Board polygons.
    pub polygons: Vec<DrcPolygon>,
    /// Board stroke texts.
    pub stroke_texts: Vec<DrcStrokeText>,
    /// Board holes.
    pub holes: Vec<DrcHole>,
    /// Board zones.
    pub zones: Vec<DrcZone>,
    /// Devices by component instance UUID.
    pub devices: BTreeMap<Uuid, DrcDevice>,
    /// Air wires.
    pub air_wires: Vec<DrcAirWire>,
    /// Components without device (and not schematic-only), UUID and name.
    pub unplaced_components: BTreeMap<Uuid, String>,
}

// Moved to and shared between the check threads.
static_assertions::assert_impl_all!(BoardDrcData: Send, Sync);

impl BoardDrcData {
    /// Extracts the input data of the DRC for a board of a project
    /// (upstream constructor).
    ///
    /// The plane fragments and air wires are taken from the board's derived
    /// data as they are, so they should be up to date (see
    /// [`Project::run_drc()`](crate::project::Project::run_drc)).
    pub fn new(
        project: &Project,
        board: BoardId,
        settings: &BoardDesignRuleCheckSettings,
        quick: bool,
    ) -> Result<Self> {
        let b = project
            .board(board)
            .ok_or(crate::project::Error::NotFound {
                kind: EntityKind::Board,
                uuid: board.0,
            })?;
        Extractor::new(project, b).extract(settings, quick)
    }

    /// Returns the minimum copper clearance for a net class (the maximum of
    /// the DRC settings and the net class rule).
    pub fn min_copper_copper_clearance(&self, net_class: Option<Uuid>) -> UnsignedLength {
        let value = self.settings.min_copper_copper_clearance();
        match net_class.and_then(|nc| self.net_classes.get(&nc)) {
            Some(nc) => value.max(nc.min_copper_copper_clearance),
            None => value,
        }
    }

    /// Returns the minimum copper width for a net class.
    pub fn min_copper_width(&self, net_class: Option<Uuid>) -> UnsignedLength {
        let value = self.settings.min_copper_width();
        match net_class.and_then(|nc| self.net_classes.get(&nc)) {
            Some(nc) => value.max(nc.min_copper_width),
            None => value,
        }
    }

    /// Returns the minimum via drill diameter for a net class.
    pub fn min_via_drill_diameter(&self, net_class: Option<Uuid>) -> UnsignedLength {
        let value = self.settings.min_pth_drill_diameter();
        match net_class.and_then(|nc| self.net_classes.get(&nc)) {
            Some(nc) => value.max(nc.min_via_drill_diameter),
            None => value,
        }
    }
}

/// Net information of an item: UUID, name, net class.
type NetInfo = (Option<Uuid>, String, Option<Uuid>);

/// Helper extracting the data of one board.
struct Extractor<'a> {
    project: &'a Project,
    board: &'a Board,
    library: &'a ProjectLibrary,
    circuit: &'a Circuit,
    rules: &'a BoardDesignRules,
    copper_layers: BTreeSet<Layer>,
}

impl<'a> Extractor<'a> {
    fn new(project: &'a Project, board: &'a Board) -> Self {
        Self {
            project,
            board,
            library: project.library(),
            circuit: project.circuit(),
            rules: board.design_rules(),
            copper_layers: board.copper_layers(),
        }
    }

    fn net_info(&self, net: Option<NetSignalId>) -> NetInfo {
        match net.and_then(|id| self.circuit.net_signal(id).map(|n| (id, n))) {
            Some((id, n)) => (Some(id.0), n.name().to_string(), Some(n.net_class().0)),
            None => (None, String::new(), None),
        }
    }

    fn font(&self) -> Result<&'a StrokeFont> {
        Ok(self
            .project
            .stroke_fonts()
            .font(&self.board.settings().default_font)?)
    }

    fn extract(
        &self,
        settings: &BoardDesignRuleCheckSettings,
        quick: bool,
    ) -> Result<BoardDrcData> {
        let s = self.board.settings();
        let net_classes = self
            .circuit
            .net_classes()
            .iter()
            .map(|(id, nc)| {
                (
                    id.0,
                    DrcNetClass {
                        min_copper_copper_clearance: nc.min_copper_copper_clearance(),
                        min_copper_width: nc.min_copper_width(),
                        min_via_drill_diameter: nc.min_via_drill_diameter(),
                    },
                )
            })
            .collect();
        let segments = self
            .board
            .net_segments()
            .values()
            .map(|ns| Ok((ns.uuid(), self.segment(ns)?)))
            .collect::<Result<_>>()?;
        let planes = self
            .board
            .planes()
            .values()
            .map(|plane| {
                let (net, net_name, net_class) = self.net_info(plane.net());
                DrcPlane {
                    uuid: plane.uuid(),
                    net,
                    net_name,
                    net_class,
                    layer: plane.layer(),
                    min_width: plane.min_width(),
                    thermal_spoke_width: plane.thermal_spoke_width(),
                    outline: plane.outline().clone(),
                    fragments: self.board.derived().fragments_of(plane.id()).to_vec(),
                }
            })
            .collect();
        let polygons = self
            .board
            .polygons()
            .values()
            .map(|p| DrcPolygon {
                uuid: p.uuid(),
                layer: p.layer(),
                line_width: p.line_width(),
                filled: p.is_filled(),
                path: p.path().clone(),
            })
            .collect();
        let stroke_texts = self
            .board
            .stroke_texts()
            .values()
            .map(|t| self.stroke_text(t, None))
            .collect::<Result<_>>()?;
        let holes = self
            .board
            .holes()
            .values()
            .map(|h| DrcHole {
                uuid: h.uuid(),
                diameter: h.diameter(),
                path: h.path().clone(),
                stop_mask_offset: h.stop_mask_offset(self.rules),
            })
            .collect();
        let zones = self
            .board
            .zones()
            .values()
            .map(|z| DrcZone {
                uuid: z.uuid(),
                board_layers: z.layers().clone(),
                footprint_layers: ZoneLayers::empty(),
                rules: z.rules(),
                outline: z.outline().clone(),
            })
            .collect();
        let devices = self
            .board
            .devices()
            .values()
            .map(|dev| Ok((dev.component().0, self.device(dev, quick)?)))
            .collect::<Result<_>>()?;
        let air_wires = self.air_wires();
        let unplaced_components = self
            .circuit
            .component_instances()
            .iter()
            .filter(|(id, cmp)| {
                self.board.device(**id).is_none()
                    && !self
                        .library
                        .component(&cmp.lib_component())
                        .is_some_and(|c| c.schematic_only())
            })
            .map(|(id, cmp)| (id.0, cmp.name().to_string()))
            .collect();
        Ok(BoardDrcData {
            settings: settings.clone(),
            quick,
            copper_layers: self.copper_layers.clone(),
            silkscreen_layers_top: s.silkscreen_layers_top.clone(),
            silkscreen_layers_bot: s.silkscreen_layers_bot.clone(),
            net_classes,
            segments,
            planes,
            polygons,
            stroke_texts,
            holes,
            zones,
            devices,
            air_wires,
            unplaced_components,
        })
    }

    /// Upstream `convertPad()` (the transform must already be absolute).
    #[allow(clippy::too_many_arguments)]
    fn pad(
        &self,
        on_board: PadOnBoard<'_>,
        uuid: Uuid,
        lib_pkg_pad_name: String,
        position: Point,
        rotation: Angle,
        anchor: TraceAnchor,
        net: Option<NetSignalId>,
    ) -> DrcPad {
        let layers_with_traces = self.board.anchor_trace_layers(anchor);
        let geometries = on_board.geometries(&self.copper_layers, self.rules, &layers_with_traces);
        let (net, net_name, net_class) = self.net_info(net);
        DrcPad {
            uuid,
            lib_pkg_pad_name,
            position,
            rotation,
            mirror: on_board.mirrored,
            holes: on_board
                .pad
                .holes()
                .iter()
                .map(|h| DrcHole {
                    uuid: h.uuid(),
                    diameter: h.diameter(),
                    path: h.path().clone(),
                    stop_mask_offset: None,
                })
                .collect(),
            geometries,
            layers_with_traces,
            copper_clearance: on_board.pad.copper_clearance(),
            net,
            net_name,
            net_class,
        }
    }

    fn segment(&self, ns: &BoardNetSegment) -> Result<DrcSegment> {
        let (net, net_name, net_class) = self.net_info(ns.net());
        let junctions = ns
            .junctions()
            .values()
            .map(|j| {
                let anchor = TraceAnchor::Junction(j.uuid());
                (
                    j.uuid(),
                    DrcJunction {
                        uuid: j.uuid(),
                        position: j.position(),
                        traces: ns.traces_at(anchor).count(),
                    },
                )
            })
            .collect();
        let traces = ns
            .traces()
            .values()
            .map(|t| {
                let p1 = self.anchor_position(ns, t.p1())?;
                let p2 = self.anchor_position(ns, t.p2())?;
                Ok(DrcTrace {
                    uuid: t.uuid(),
                    p1,
                    p2,
                    width: t.width(),
                    layer: t.layer(),
                })
            })
            .collect::<Result<_>>()?;
        let vias = ns
            .vias()
            .values()
            .map(|via| {
                let anchor = TraceAnchor::Via(via.uuid());
                let connected_layers = ns.traces_at(anchor).map(|t| t.layer()).collect();
                let data = self.via(via, ns.net(), connected_layers)?;
                Ok((via.uuid(), data))
            })
            .collect::<Result<_>>()?;
        let pads = ns
            .pads()
            .values()
            .map(|p| {
                let on_board = PadOnBoard {
                    pad: p.pad(),
                    mirrored: false,
                };
                let data = self.pad(
                    on_board,
                    p.uuid(),
                    String::new(),
                    p.pad().position(),
                    p.pad().rotation(),
                    TraceAnchor::Pad(p.uuid()),
                    ns.net(),
                );
                (p.uuid(), data)
            })
            .collect();
        Ok(DrcSegment {
            uuid: ns.uuid(),
            net,
            net_name,
            net_class,
            junctions,
            traces,
            vias,
            pads,
        })
    }

    fn anchor_position(&self, ns: &BoardNetSegment, anchor: TraceAnchor) -> Result<Point> {
        self.board
            .anchor_position(ns, anchor, self.library, self.circuit)
            .ok_or(Error::InvalidTraceAnchor(anchor_uuid(anchor)))
    }

    /// Upstream `BI_Via` getters (actual drill and size, drill layer span,
    /// stop mask diameters).
    fn via(
        &self,
        via: &crate::geometry::Via,
        net: Option<NetSignalId>,
        connected_layers: BTreeSet<Layer>,
    ) -> Result<DrcVia> {
        let properties = self.board.via_properties(via, net, self.circuit);
        Ok(DrcVia {
            uuid: via.uuid(),
            position: via.position(),
            drill_diameter: properties.drill_diameter,
            size: properties.size,
            connected_layers,
            start_layer: via.start_layer(),
            end_layer: via.end_layer(),
            drill_layer_span: properties.drill_layer_span,
            is_buried: via.is_buried(),
            is_blind: via.is_blind(),
            stop_mask_diameter_top: properties.stop_mask_diameter_top,
            stop_mask_diameter_bot: properties.stop_mask_diameter_bot,
        })
    }

    fn stroke_text(
        &self,
        text: &BoardStrokeTextData,
        device: Option<&BoardDevice>,
    ) -> Result<DrcStrokeText> {
        // Upstream `BI_StrokeText::updateText()`.
        let substituted = match device {
            Some(device) => {
                let component = self.circuit.component_instance(device.component());
                let part = component.and_then(|c| device.parts(c, None).into_iter().next());
                ProjectAttributeLookup::for_device(self.project, self.board, device, part.as_ref())
                    .substitute(text.text())
            }
            None => ProjectAttributeLookup::for_board(self.project, self.board, None)
                .substitute(text.text()),
        };
        let paths = StrokeTextPathBuilder::build(
            self.font()?,
            text.letter_spacing(),
            text.line_spacing(),
            text.height(),
            text.stroke_width(),
            text.align(),
            text.rotation(),
            text.auto_rotate(),
            &substituted,
        );
        Ok(DrcStrokeText {
            uuid: text.uuid(),
            position: text.position(),
            rotation: text.rotation(),
            mirror: text.mirrored(),
            layer: text.layer(),
            stroke_width: text.stroke_width(),
            height: text.height(),
            paths,
        })
    }

    fn device(&self, dev: &BoardDevice, quick: bool) -> Result<DrcDevice> {
        let resolved = ResolvedDevice::resolve(dev, self.library)?;
        let footprint = resolved.footprint;
        let component = self.circuit.component_instance(dev.component());
        let views: Vec<FootprintPadView<'_>> = dev.pads(self.library, self.circuit)?;
        let pads = views
            .iter()
            .map(|view| {
                let on_board = PadOnBoard {
                    pad: view.properties(),
                    mirrored: dev.mirrored(),
                };
                let data = self.pad(
                    on_board,
                    view.uuid(),
                    view.package_pad()
                        .map(|p| p.name().to_string())
                        .unwrap_or_default(),
                    view.position(),
                    view.rotation(),
                    view.trace_anchor(),
                    view.net(),
                );
                (view.uuid(), data)
            })
            .collect();
        let polygons = footprint
            .polygons()
            .iter()
            .map(|p| DrcPolygon {
                uuid: p.uuid(),
                layer: p.layer(),
                line_width: p.line_width(),
                filled: p.is_filled(),
                path: p.path().clone(),
            })
            .collect();
        let circles = footprint
            .circles()
            .iter()
            .map(|c| DrcCircle {
                uuid: c.uuid(),
                center: c.center(),
                diameter: c.diameter(),
                layer: c.layer(),
                line_width: c.line_width(),
                filled: c.is_filled(),
            })
            .collect();
        let stroke_texts = dev
            .stroke_texts()
            .values()
            .map(|t| self.stroke_text(t, Some(dev)))
            .collect::<Result<_>>()?;
        let stop_masks = BoardDevice::hole_stop_mask_offsets(footprint, self.rules);
        let holes = footprint
            .holes()
            .iter()
            .map(|h| DrcHole {
                uuid: h.uuid(),
                diameter: h.diameter(),
                path: h.path().clone(),
                stop_mask_offset: stop_masks.get(&h.uuid()).copied().flatten(),
            })
            .collect();
        let zones = footprint
            .zones()
            .iter()
            .map(|z| DrcZone {
                uuid: z.uuid(),
                board_layers: BTreeSet::new(),
                footprint_layers: z.layers(),
                rules: z.rules(),
                outline: z.outline().clone(),
            })
            .collect();
        let mut impossible_signal_connections = Vec::new();
        if let (false, Some(cmp)) = (quick, component) {
            // A bit unusual, but the actual check is already done here to
            // avoid copying lots of data for such a lightweight check.
            let pad_signals: BTreeSet<Uuid> = views
                .iter()
                .filter_map(|v| v.component_signal().map(|s| s.signal))
                .collect();
            let lib_component = self.library.component(&cmp.lib_component());
            for (signal, instance) in cmp.signals() {
                if let Some(net) = instance.net().and_then(|n| self.circuit.net_signal(n))
                    && !pad_signals.contains(signal)
                {
                    impossible_signal_connections.push(DrcImpossibleConnection {
                        signal_uuid: *signal,
                        signal_name: lib_component
                            .and_then(|c| c.signals().by_uuid(signal))
                            .map(|s| s.name().to_string())
                            .unwrap_or_default(),
                        net_name: net.name().to_string(),
                    });
                }
            }
        }
        Ok(DrcDevice {
            uuid: dev.component().0,
            cmp_instance_name: component.map(|c| c.name().to_string()).unwrap_or_default(),
            position: dev.position(),
            rotation: dev.rotation(),
            mirror: dev.mirrored(),
            pads,
            polygons,
            circles,
            stroke_texts,
            holes,
            zones,
            impossible_signal_connections,
        })
    }

    fn air_wires(&self) -> Vec<DrcAirWire> {
        self.board
            .derived()
            .all_air_wires()
            .map(|aw| DrcAirWire {
                p1: self.air_wire_anchor(aw.p1(), aw.p1_position()),
                p2: self.air_wire_anchor(aw.p2(), aw.p2_position()),
                net_name: self
                    .circuit
                    .net_signal(aw.net())
                    .map(|n| n.name().to_string())
                    .unwrap_or_default(),
            })
            .collect()
    }

    fn air_wire_anchor(&self, anchor: TraceAnchor, position: Point) -> DrcAirWireAnchor {
        let segment = || {
            self.board
                .net_segments()
                .values()
                .find(|s| s.contains_anchor(&anchor))
                .map(|s| s.uuid())
        };
        let object = match anchor {
            TraceAnchor::FootprintPad { device, pad } => {
                Some(DrcAirWireObject::FootprintPad { device, pad })
            }
            TraceAnchor::Pad(pad) => {
                segment().map(|segment| DrcAirWireObject::Pad { segment, pad })
            }
            TraceAnchor::Junction(junction) => {
                segment().map(|segment| DrcAirWireObject::Junction { segment, junction })
            }
            TraceAnchor::Via(via) => {
                segment().map(|segment| DrcAirWireObject::Via { segment, via })
            }
        };
        if object.is_none() {
            log::error!("Unknown air wire anchor type, DRC will fail later.");
        }
        DrcAirWireAnchor { position, object }
    }
}

fn anchor_uuid(anchor: TraceAnchor) -> Uuid {
    match anchor {
        TraceAnchor::Junction(u) | TraceAnchor::Via(u) | TraceAnchor::Pad(u) => u,
        TraceAnchor::FootprintPad { pad, .. } => pad,
    }
}
