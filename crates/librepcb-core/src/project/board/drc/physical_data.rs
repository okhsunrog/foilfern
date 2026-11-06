//! Port of geometric/connectivity snapshot extraction in
//! libs/librepcb/core/project/board/drc/boarddesignrulecheckdata.cpp.

use super::{DrcCopperRef, DrcMode, DrcObjectRef, DrcPadRef};
use crate::geometry::{PadGeometry, Path, TraceAnchor, Vertex, Via, ZoneLayers, ZoneRules};
use crate::project::board::{
    Board, BoardAirWiresBuilder, BoardPlaneFragmentsBuilder, PadOnBoard, ResolvedDevice, ViaView,
};
use crate::project::{ComponentInstanceId, Error, NetSegmentId, NetSignalId, Project, Result};
use crate::types::{Angle, Layer, Length, Point, PositiveLength, UnsignedLength, Uuid};
use crate::utils::transform::Transform;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default)]
pub(super) struct PhysicalData {
    pub clearance_enabled: bool,
    pub drawings: Vec<Drawing>,
    pub texts: Vec<Text>,
    pub pads: Vec<Pad>,
    pub vias: Vec<OwnedVia>,
    pub traces: Vec<Trace>,
    pub planes: Vec<Plane>,
    pub zones: Vec<Zone>,
    pub devices: BTreeMap<ComponentInstanceId, Device>,
    /// Raw mask diameters are validated only when a rule evaluates mask geometry.
    pub masks: Vec<(Path, Length)>,
    pub unplaced: Vec<(ComponentInstanceId, String)>,
    pub stale: Vec<Stale>,
    pub air_wires: Vec<AirWire>,
}
#[derive(Debug, Clone)]
pub(super) struct Drawing {
    pub reference: DrcObjectRef,
    pub layer: Layer,
    pub path: Path,
    pub width: UnsignedLength,
    pub filled: bool,
    pub circle: Option<(Point, PositiveLength)>,
}
#[derive(Debug, Clone)]
pub(super) struct Text {
    pub reference: DrcObjectRef,
    pub layer: Layer,
    pub paths: Vec<Path>,
    pub width: UnsignedLength,
}
#[derive(Debug, Clone)]
pub(super) struct Pad {
    pub reference: DrcPadRef,
    pub name: String,
    pub transform: Transform,
    pub geometries: BTreeMap<Layer, Vec<PadGeometry>>,
    pub connected: BTreeSet<Layer>,
    pub net: Option<NetSignalId>,
    pub net_name: String,
    pub clearance: UnsignedLength,
}
#[derive(Debug, Clone)]
pub(super) struct OwnedVia {
    pub reference: DrcCopperRef,
    pub via: Via,
    pub size: PositiveLength,
    pub drill: PositiveLength,
    pub span: Option<(Layer, Layer)>,
    pub connected: BTreeSet<Layer>,
    pub net: Option<NetSignalId>,
    pub net_name: String,
    pub clearance: UnsignedLength,
    pub mask_top: Option<PositiveLength>,
    pub mask_bottom: Option<PositiveLength>,
}
#[derive(Debug, Clone)]
pub(super) struct Trace {
    pub reference: DrcObjectRef,
    pub p1: Point,
    pub p2: Point,
    pub width: PositiveLength,
    pub layer: Layer,
    pub net: Option<NetSignalId>,
    pub net_name: String,
    pub clearance: UnsignedLength,
}
#[derive(Debug, Clone)]
pub(super) struct Plane {
    pub reference: DrcObjectRef,
    pub layer: Layer,
    pub net: Option<NetSignalId>,
    pub net_name: String,
    pub clearance: UnsignedLength,
    pub outline: Path,
    pub fragments: Vec<Path>,
}
#[derive(Debug, Clone)]
pub(super) struct Zone {
    pub uuid: Uuid,
    pub device: Option<ComponentInstanceId>,
    pub layers: BTreeSet<Layer>,
    pub rules: ZoneRules,
    pub outline: Path,
}
#[derive(Debug, Clone)]
pub(super) struct Device {
    pub name: String,
    pub position: Point,
    pub locations: Vec<Path>,
    pub impossible: Vec<(Uuid, String, String)>,
}
#[derive(Debug, Clone)]
pub(super) enum Stale {
    Segment {
        segment: NetSegmentId,
        net: String,
    },
    Junction {
        segment: NetSegmentId,
        junction: Uuid,
        position: Point,
        net: String,
    },
}
#[derive(Debug, Clone)]
pub(super) struct Anchor {
    pub device: Option<ComponentInstanceId>,
    pub segment: Option<NetSegmentId>,
    pub anchor: TraceAnchor,
    pub name: String,
}
#[derive(Debug, Clone)]
pub(super) struct AirWire {
    pub from: Anchor,
    pub to: Anchor,
    pub net: String,
    pub locations: Vec<Path>,
}

impl PhysicalData {
    pub fn collect(
        project: &Project,
        board: &Board,
        settings: &crate::library::org::BoardDesignRuleCheckSettings,
        mode: DrcMode,
    ) -> Result<Self> {
        let mut data = Self {
            clearance_enabled: settings.min_copper_copper_clearance() > UnsignedLength::ZERO
                || project
                    .circuit()
                    .net_classes()
                    .values()
                    .any(|c| c.min_copper_copper_clearance() > UnsignedLength::ZERO),
            ..Self::default()
        };
        let copper = board.copper_layers();
        let net_info = |net: Option<NetSignalId>| -> Result<(String, UnsignedLength)> {
            if let Some(id) = net {
                let signal = project
                    .circuit()
                    .net_signal(id)
                    .ok_or(Error::InexistentNetSignal(id.0))?;
                let class = project
                    .circuit()
                    .net_classes()
                    .get(&signal.net_class())
                    .ok_or(Error::InexistentNetClass(signal.net_class().0))?;
                Ok((
                    signal.name().to_string(),
                    class
                        .min_copper_copper_clearance()
                        .max(settings.min_copper_copper_clearance()),
                ))
            } else {
                Ok((String::new(), settings.min_copper_copper_clearance()))
            }
        };
        for polygon in board.polygons().values() {
            data.drawings.push(Drawing {
                reference: DrcObjectRef::Polygon {
                    device: None,
                    polygon: polygon.uuid(),
                },
                layer: polygon.layer(),
                path: polygon.path().clone(),
                width: polygon.line_width(),
                filled: polygon.is_filled(),
                circle: None,
            });
        }
        for text in board.stroke_texts().values() {
            data.texts.push(Text {
                reference: DrcObjectRef::StrokeText {
                    device: None,
                    text: text.uuid(),
                },
                layer: text.layer(),
                paths: text
                    .paths(project, board, None)?
                    .iter()
                    .map(|p| text.transform().map(p))
                    .collect(),
                width: text.stroke_width(),
            });
        }
        for hole in board.holes().values() {
            if let Some(offset) = hole.stop_mask_offset(board.design_rules()) {
                let diameter = *hole.diameter() + offset * 2;
                data.masks
                    .push((hole.path().clone().into_inner(), diameter));
            }
        }
        for segment in board.net_segments().values() {
            let (net_name, clearance) = net_info(segment.net())?;
            for pad in segment.pads().values() {
                let p = pad.pad();
                data.pads.push(Pad {
                    reference: DrcPadRef::Board {
                        segment: segment.id(),
                        pad: pad.uuid(),
                    },
                    name: String::new(),
                    transform: Transform::new(p.position(), p.rotation(), false),
                    geometries: PadOnBoard {
                        pad: p,
                        mirrored: false,
                    }
                    .geometries(
                        &copper,
                        board.design_rules(),
                        &board.anchor_trace_layers(TraceAnchor::Pad(pad.uuid())),
                    ),
                    connected: board.anchor_trace_layers(TraceAnchor::Pad(pad.uuid())),
                    net: segment.net(),
                    net_name: net_name.clone(),
                    clearance: clearance.max(p.copper_clearance()),
                });
            }
            for via in segment.vias().values() {
                let view = ViaView::new(via, board, segment.net(), project.circuit())?;
                data.vias.push(OwnedVia {
                    reference: DrcCopperRef::Via {
                        segment: segment.id(),
                        via: via.uuid(),
                    },
                    via: via.clone(),
                    size: view.size(),
                    drill: view.drill_diameter(),
                    span: view.drill_layer_span(),
                    connected: segment
                        .traces()
                        .values()
                        .filter(|t| {
                            t.p1() == TraceAnchor::Via(via.uuid())
                                || t.p2() == TraceAnchor::Via(via.uuid())
                        })
                        .map(|t| t.layer())
                        .collect(),
                    net: segment.net(),
                    net_name: net_name.clone(),
                    clearance,
                    mask_top: view.stop_mask_diameter_top(),
                    mask_bottom: view.stop_mask_diameter_bottom(),
                });
            }
            for trace in segment.traces().values() {
                let position = |anchor| {
                    board
                        .anchor_position(segment, anchor, project.library(), project.circuit())
                        .ok_or(Error::InexistentTraceAnchor {
                            anchor,
                            device_missing: false,
                        })
                };
                data.traces.push(Trace {
                    reference: DrcObjectRef::Trace {
                        segment: segment.id(),
                        trace: trace.uuid(),
                    },
                    p1: position(trace.p1())?,
                    p2: position(trace.p2())?,
                    width: trace.width(),
                    layer: trace.layer(),
                    net: segment.net(),
                    net_name: net_name.clone(),
                    clearance,
                });
            }
            if segment.junctions().is_empty()
                && segment.traces().is_empty()
                && segment.vias().is_empty()
                && segment.pads().is_empty()
            {
                data.stale.push(Stale::Segment {
                    segment: segment.id(),
                    net: net_name.clone(),
                });
            }
            for junction in segment.junctions().values() {
                if !segment.traces().values().any(|t| {
                    t.p1() == TraceAnchor::Junction(junction.uuid())
                        || t.p2() == TraceAnchor::Junction(junction.uuid())
                }) {
                    data.stale.push(Stale::Junction {
                        segment: segment.id(),
                        junction: junction.uuid(),
                        position: junction.position(),
                        net: net_name.clone(),
                    });
                }
            }
        }
        for device in board.devices().values() {
            let footprint = ResolvedDevice::resolve(device, project.library())?.footprint;
            let transform = device.transform();
            let component = project
                .circuit()
                .component_instance(device.component())
                .ok_or(Error::InexistentComponent(device.component().0))?;
            let library = project
                .library()
                .component(&component.lib_component())
                .ok_or(Error::MissingLibraryComponent(component.lib_component()))?;
            let mut signals = BTreeSet::new();
            for pad in device.pads(project.library(), project.circuit())? {
                let (net_name, clearance) = net_info(pad.net())?;
                if let Some(s) = pad.component_signal() {
                    signals.insert(s.signal);
                }
                data.pads.push(Pad {
                    reference: DrcPadRef::Footprint {
                        device: device.component(),
                        pad: pad.uuid(),
                    },
                    name: pad
                        .package_pad()
                        .map(|p| p.name().to_string())
                        .unwrap_or_default(),
                    transform: Transform::new(pad.position(), pad.rotation(), pad.mirrored()),
                    geometries: pad.geometries(
                        &copper,
                        board.design_rules(),
                        &board.anchor_trace_layers(pad.trace_anchor()),
                    ),
                    connected: board.anchor_trace_layers(pad.trace_anchor()),
                    net: pad.net(),
                    net_name,
                    clearance: clearance.max(pad.properties().copper_clearance()),
                });
            }
            let mut impossible = Vec::new();
            for (id, signal) in component.signals() {
                if let Some(net) = signal.net()
                    && !signals.contains(id)
                {
                    let name = library
                        .signals()
                        .by_uuid(id)
                        .ok_or(Error::InexistentComponentSignal(*id))?
                        .name()
                        .to_string();
                    impossible.push((*id, name, net_info(Some(net))?.0));
                }
            }
            for polygon in footprint.polygons() {
                data.drawings.push(Drawing {
                    reference: DrcObjectRef::Polygon {
                        device: Some(device.component()),
                        polygon: polygon.uuid(),
                    },
                    layer: transform.map(&polygon.layer()),
                    path: transform.map(polygon.path()),
                    width: polygon.line_width(),
                    filled: polygon.is_filled(),
                    circle: None,
                });
            }
            for circle in footprint.circles() {
                let center = transform.map(&circle.center());
                data.drawings.push(Drawing {
                    reference: DrcObjectRef::Circle {
                        device: device.component(),
                        circle: circle.uuid(),
                    },
                    layer: transform.map(&circle.layer()),
                    path: Path::circle(circle.diameter()).translated(center),
                    width: circle.line_width(),
                    filled: circle.is_filled(),
                    circle: Some((center, circle.diameter())),
                });
            }
            for text in device.stroke_texts().values() {
                data.texts.push(Text {
                    reference: DrcObjectRef::StrokeText {
                        device: Some(device.component()),
                        text: text.uuid(),
                    },
                    layer: text.layer(),
                    paths: text
                        .paths(project, board, Some(device))?
                        .iter()
                        .map(|p| text.transform().map(p))
                        .collect(),
                    width: text.stroke_width(),
                });
            }
            let offsets = crate::project::board::BoardDevice::hole_stop_mask_offsets(
                footprint,
                board.design_rules(),
            );
            for hole in footprint.holes() {
                if let Some(Some(offset)) = offsets.get(&hole.uuid()) {
                    let diameter = *hole.diameter() + *offset * 2;
                    data.masks
                        .push((transform.map(hole.path()).into_inner(), diameter));
                }
            }
            for zone in footprint.zones() {
                let mut layers = BTreeSet::new();
                if zone.layers().contains(ZoneLayers::TOP) {
                    layers.insert(transform.map(&Layer::TOP_COPPER));
                }
                if zone.layers().contains(ZoneLayers::BOTTOM) {
                    layers.insert(transform.map(&Layer::BOT_COPPER));
                }
                if zone.layers().contains(ZoneLayers::INNER) {
                    layers.extend(copper.iter().copied().filter(|l| l.is_inner()));
                }
                data.zones.push(Zone {
                    uuid: zone.uuid(),
                    device: Some(device.component()),
                    layers,
                    rules: zone.rules(),
                    outline: transform.map(zone.outline()),
                });
            }
            let mut locations = Vec::new();
            for layers in [
                [Layer::TOP_DOCUMENTATION, Layer::BOT_DOCUMENTATION],
                [Layer::TOP_LEGEND, Layer::BOT_LEGEND],
            ] {
                for drawing in data.drawings.iter().filter(|d| {
                    d.reference.device() == Some(device.component()) && layers.contains(&d.layer)
                }) {
                    if drawing.width > UnsignedLength::ZERO {
                        locations.extend(
                            drawing
                                .path
                                .to_outline_strokes(PositiveLength::new(*drawing.width)?),
                        );
                    }
                    if drawing.path.is_closed() && drawing.filled {
                        locations.push(drawing.path.clone());
                    }
                }
                if !locations.is_empty() {
                    break;
                }
            }
            let cross = Path::new(vec![
                Vertex::at(Point::from_nm(-500_000, 0)),
                Vertex::at(Point::from_nm(500_000, 0)),
            ]);
            let width = PositiveLength::new(Length::new(50_000))?;
            locations.extend(
                cross
                    .translated(device.position())
                    .to_outline_strokes(width),
            );
            locations.extend(
                cross
                    .rotated(Angle::DEG90, Point::ORIGIN)
                    .translated(device.position())
                    .to_outline_strokes(width),
            );
            data.devices.insert(
                device.component(),
                Device {
                    name: component.name().to_string(),
                    position: device.position(),
                    locations,
                    impossible,
                },
            );
        }
        for zone in board.zones().values() {
            data.zones.push(Zone {
                uuid: zone.uuid(),
                device: None,
                layers: zone.layers().clone(),
                rules: zone.rules(),
                outline: zone.outline().clone(),
            });
        }
        let fragments = if mode == DrcMode::Full {
            BoardPlaneFragmentsBuilder::build(project, board, None)?
                .fragments()
                .clone()
        } else {
            board.derived().plane_fragments().clone()
        };
        let mut rebuilt = board.clone();
        rebuilt.derived.plane_fragments = fragments.clone();
        for plane in board.planes().values() {
            let (net_name, clearance) = net_info(plane.net())?;
            data.planes.push(Plane {
                reference: DrcObjectRef::Plane(plane.id()),
                layer: plane.layer(),
                net: plane.net(),
                net_name,
                clearance,
                outline: plane.outline().clone(),
                fragments: fragments.get(&plane.id()).cloned().unwrap_or_default(),
            });
        }
        for (id, component) in project.circuit().component_instances() {
            if !board.devices().contains_key(id)
                && !project
                    .library()
                    .component(&component.lib_component())
                    .ok_or(Error::MissingLibraryComponent(component.lib_component()))?
                    .schematic_only()
            {
                data.unplaced.push((*id, component.name().to_string()));
            }
        }
        if mode == DrcMode::Full {
            let builder = BoardAirWiresBuilder::new(&rebuilt, project.library(), project.circuit());
            for net in project.circuit().net_signals().keys() {
                for wire in builder.build_air_wires(*net) {
                    data.air_wires.push(AirWire {
                        from: data.anchor(board, wire.p1())?,
                        to: data.anchor(board, wire.p2())?,
                        net: net_info(Some(*net))?.0,
                        locations: vec![Path::obround_line(
                            wire.p1_position(),
                            wire.p2_position(),
                            PositiveLength::new(Length::new(50_000))?,
                        )],
                    });
                }
            }
        }
        Ok(data)
    }
    fn anchor(&self, board: &Board, anchor: TraceAnchor) -> Result<Anchor> {
        if let TraceAnchor::FootprintPad { device, pad } = anchor {
            let id = crate::project::ComponentInstanceId(device);
            let d = self
                .devices
                .get(&id)
                .ok_or(Error::InexistentComponent(device))?;
            let p = self
                .pads
                .iter()
                .find(|p| p.reference == DrcPadRef::Footprint { device: id, pad })
                .ok_or(Error::InexistentTraceAnchor {
                    anchor,
                    device_missing: false,
                })?;
            let name = if p.name.is_empty() {
                format!("'{}'", d.name)
            } else {
                format!("'{}:{}'", d.name, p.name)
            };
            return Ok(Anchor {
                device: Some(id),
                segment: None,
                anchor,
                name,
            });
        }
        let segment = board
            .net_segments()
            .values()
            .find(|s| match anchor {
                TraceAnchor::Pad(id) => s.pads().contains_key(&id),
                TraceAnchor::Via(id) => s.vias().contains_key(&id),
                TraceAnchor::Junction(id) => s.junctions().contains_key(&id),
                TraceAnchor::FootprintPad { .. } => false,
            })
            .ok_or(Error::InexistentTraceAnchor {
                anchor,
                device_missing: false,
            })?;
        let name = match anchor {
            TraceAnchor::Pad(_) => librepcb_i18n::tr!("DrcMsgMissingConnection", "Pad"),
            TraceAnchor::Via(_) => librepcb_i18n::tr!("DrcMsgMissingConnection", "Via"),
            _ => librepcb_i18n::tr!("DrcMsgMissingConnection", "Trace"),
        };
        Ok(Anchor {
            device: None,
            segment: Some(segment.id()),
            anchor,
            name,
        })
    }
}
