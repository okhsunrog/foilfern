//! Port of BoardPlaneFragmentsBuilder::createJob()/run() preprocessing in
//! libs/librepcb/core/project/board/boardplanefragmentsbuilder.cpp.

use super::tolerance;
use crate::geometry::{NonEmptyPath, PadGeometry, Path, Vertex, ZoneLayers, ZoneRules};
use crate::project::board::{Board, BoardPlane, PadOnBoard, ResolvedDevice, ViaView};
use crate::project::{NetSignalId, Project, Result};
use crate::types::{Layer, PositiveLength, UnsignedLength};
use crate::utils::clipper_helpers::{self as ch, ClipperPaths};
use crate::utils::transform::Transform;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct Polygon {
    pub layer: Layer,
    pub net: Option<NetSignalId>,
    pub path: Path,
    pub width: UnsignedLength,
    pub filled: bool,
}
pub(super) struct Via {
    pub net: Option<NetSignalId>,
    pub path: Path,
    pub diameter: PositiveLength,
    pub position: crate::types::Point,
    pub start: Layer,
    pub end: Layer,
}
pub(super) struct Pad {
    pub transform: Transform,
    pub net: Option<NetSignalId>,
    pub clearance: UnsignedLength,
    pub geometries: BTreeMap<Layer, Vec<PadGeometry>>,
}
pub(super) struct Data {
    pub planes: Vec<BoardPlane>,
    pub polygons: Vec<Polygon>,
    pub pads: Vec<Pad>,
    pub vias: Vec<Via>,
    pub holes: Vec<(PositiveLength, NonEmptyPath)>,
    pub zones: Vec<(BTreeSet<Layer>, Path)>,
    pub board_area: ClipperPaths,
}
impl Data {
    pub fn collect(project: &Project, board: &Board, layers: &BTreeSet<Layer>) -> Result<Self> {
        let mut data = Self {
            planes: board
                .planes()
                .values()
                .filter(|p| layers.contains(&p.layer()))
                .cloned()
                .collect(),
            polygons: Vec::new(),
            pads: Vec::new(),
            vias: Vec::new(),
            holes: Vec::new(),
            zones: Vec::new(),
            board_area: Vec::new(),
        };
        data.planes.sort_by(|a, b| {
            b.priority()
                .cmp(&a.priority())
                .then_with(|| b.uuid().cmp(&a.uuid()))
        });
        let copper = board.copper_layers();
        let mut used_layers = layers.clone();
        used_layers.extend([Layer::BOARD_OUTLINES, Layer::BOARD_CUTOUTS]);
        for device in board.devices().values() {
            let transform = device.transform();
            let footprint = ResolvedDevice::resolve(device, project.library())?.footprint;
            let mut pads = device.pads(project.library(), project.circuit())?;
            pads.sort_unstable_by_key(|pad| pad.uuid());
            for pad in pads {
                data.pads.push(Pad {
                    transform: Transform::new(pad.position(), pad.rotation(), pad.mirrored()),
                    net: pad.net(),
                    clearance: pad.properties().copper_clearance(),
                    geometries: pad.geometries(
                        &copper,
                        board.design_rules(),
                        &board.anchor_trace_layers(pad.trace_anchor()),
                    ),
                });
            }
            for polygon in footprint.polygons() {
                let layer = transform.map(&polygon.layer());
                if used_layers.contains(&layer) {
                    data.polygons.push(Polygon {
                        layer,
                        net: None,
                        path: transform.map(polygon.path()),
                        width: polygon.line_width(),
                        filled: polygon.is_filled(),
                    });
                }
            }
            for circle in footprint.circles() {
                let layer = transform.map(&circle.layer());
                if used_layers.contains(&layer) {
                    data.polygons.push(Polygon {
                        layer,
                        net: None,
                        path: transform
                            .map(&Path::circle(circle.diameter()).translated(circle.center())),
                        width: circle.line_width(),
                        filled: circle.is_filled(),
                    });
                }
            }
            for zone in footprint.zones() {
                if !zone.rules().contains(ZoneRules::NO_PLANES) {
                    continue;
                }
                let mut zone_layers = BTreeSet::new();
                if zone.layers().contains(ZoneLayers::TOP) {
                    zone_layers.insert(transform.map(&Layer::TOP_COPPER));
                }
                if zone.layers().contains(ZoneLayers::BOTTOM) {
                    zone_layers.insert(transform.map(&Layer::BOT_COPPER));
                }
                if zone.layers().contains(ZoneLayers::INNER) {
                    zone_layers.extend(layers.iter().copied().filter(|l| l.is_inner()));
                }
                let mut outline = zone.outline().clone();
                outline.close();
                data.zones.push((zone_layers, transform.map(&outline)));
            }
            for hole in footprint.holes() {
                data.holes
                    .push((hole.diameter(), transform.map(hole.path())));
            }
            for text in device.stroke_texts().values() {
                if used_layers.contains(&text.layer()) {
                    for path in text.paths(project, board, Some(device))? {
                        data.polygons.push(Polygon {
                            layer: text.layer(),
                            net: None,
                            path: text.transform().map(&path),
                            width: text.stroke_width(),
                            filled: false,
                        });
                    }
                }
            }
        }
        for zone in board.zones().values() {
            if zone.rules().contains(ZoneRules::NO_PLANES) {
                let mut outline = zone.outline().clone();
                outline.close();
                data.zones.push((zone.layers().clone(), outline));
            }
        }
        for polygon in board.polygons().values() {
            if used_layers.contains(&polygon.layer()) {
                data.polygons.push(Polygon {
                    layer: polygon.layer(),
                    net: None,
                    path: polygon.path().clone(),
                    width: polygon.line_width(),
                    filled: polygon.is_filled(),
                });
            }
        }
        for text in board.stroke_texts().values() {
            if used_layers.contains(&text.layer()) {
                for path in text.paths(project, board, None)? {
                    data.polygons.push(Polygon {
                        layer: text.layer(),
                        net: None,
                        path: text.transform().map(&path),
                        width: text.stroke_width(),
                        filled: false,
                    });
                }
            }
        }
        for hole in board.holes().values() {
            data.holes.push((hole.diameter(), hole.path().clone()));
        }
        // Upstream preprocesses traces after all other polygons; retain this
        // order because Clipper contours can depend on input order.
        let mut traces = Vec::new();
        for segment in board.net_segments().values() {
            for pad in segment.pads().values() {
                let properties = pad.pad();
                let view = PadOnBoard {
                    pad: properties,
                    mirrored: false,
                };
                data.pads.push(Pad {
                    transform: Transform::new(properties.position(), properties.rotation(), false),
                    net: segment.net(),
                    clearance: properties.copper_clearance(),
                    geometries: view.geometries(
                        &copper,
                        board.design_rules(),
                        &board.anchor_trace_layers(crate::geometry::TraceAnchor::Pad(pad.uuid())),
                    ),
                });
            }
            for via in segment.vias().values() {
                let geometry = ViaView::new(via, board, segment.net(), project.circuit())?;
                data.vias.push(Via {
                    position: via.position(),
                    net: segment.net(),
                    path: Path::circle(geometry.size()).translated(via.position()),
                    diameter: geometry.size(),
                    start: via.start_layer(),
                    end: via.end_layer(),
                });
            }
            for trace in segment.traces().values() {
                if !used_layers.contains(&trace.layer()) {
                    continue;
                }
                let position = |anchor| {
                    board
                        .anchor_position(segment, anchor, project.library(), project.circuit())
                        .ok_or(crate::project::Error::InexistentTraceAnchor {
                            anchor,
                            device_missing: false,
                        })
                };
                traces.push(Polygon {
                    layer: trace.layer(),
                    net: segment.net(),
                    path: Path::new(vec![
                        Vertex::new(position(trace.p1())?, crate::types::Angle::DEG0),
                        Vertex::new(position(trace.p2())?, crate::types::Angle::DEG0),
                    ]),
                    width: UnsignedLength::new(*trace.width())?,
                    filled: false,
                });
            }
        }
        data.polygons.extend(traces);
        let outlines: Vec<_> = data
            .polygons
            .iter()
            .filter(|p| p.layer == Layer::BOARD_OUTLINES && p.path.is_closed())
            .map(|p| p.path.clone())
            .collect();
        let cutouts: Vec<_> = data
            .polygons
            .iter()
            .filter(|p| p.layer == Layer::BOARD_CUTOUTS && p.path.is_closed())
            .map(|p| p.path.clone())
            .collect();
        data.board_area = ch::paths_to_clipper(&outlines, tolerance());
        ch::subtract(
            &mut data.board_area,
            &ch::paths_to_clipper(&cutouts, tolerance()),
            clipper::PolyFillType::NonZero,
            clipper::PolyFillType::NonZero,
        )?;
        Ok(data)
    }
}
