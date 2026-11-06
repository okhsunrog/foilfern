//! Port of libs/librepcb/core/project/board/drc/boarddesignrulecheckdata.{h,cpp}.
//!
//! Owned input for the currently ported manufacturing checks. Library and
//! project references are resolved while extracting, before a caller moves
//! the snapshot to a worker thread.

use super::{DrcHoleRef, DrcObjectRef};
use crate::geometry::{NonEmptyPath, Pad, Path};
use crate::library::org::BoardDesignRuleCheckSettings;
use crate::project::board::{Board, BoardPlane, ResolvedDevice, ViaView};
use crate::project::{BoardId, Error, NetSignalId, Project, Result};
use crate::types::{Layer, Point, PositiveLength, UnsignedLength};
use crate::utils::transform::Transform;
use std::collections::BTreeSet;

/// Immutable, self-contained input to board rule checks.
///
/// Captures drill/slot, width/text, plane-configuration and layer-use inputs.
/// Clearance, connectivity and outline checks are not implemented yet.
#[derive(Debug, Clone)]
pub struct BoardDrcData {
    pub(super) board: BoardId,
    pub(super) settings: BoardDesignRuleCheckSettings,
    pub(super) holes: Vec<Hole>,
    pub(super) planes: Vec<(BoardPlane, String)>,
    pub(super) copper_layers: BTreeSet<Layer>,
    pub(super) used_layers: BTreeSet<Layer>,
    pub(super) widths: Vec<WidthObject>,
    pub(super) outlines: Vec<(DrcObjectRef, Layer, Path)>,
    pub(super) silkscreen_layers: BTreeSet<Layer>,
}
static_assertions::assert_impl_all!(BoardDrcData: Send, Sync);

#[derive(Debug, Clone)]
pub(super) struct Hole {
    pub reference: DrcHoleRef,
    pub diameter: PositiveLength,
    pub path: NonEmptyPath,
    pub pad_position: Option<Point>,
    pub pad_name: String,
    pub net_name: String,
    pub min_via_drill: UnsignedLength,
}
#[derive(Debug, Clone)]
pub(super) struct WidthObject {
    pub reference: DrcObjectRef,
    pub layer: Layer,
    pub width: UnsignedLength,
    pub displayed_width: UnsignedLength,
    pub net_min_width: UnsignedLength,
    pub device_name: String,
    pub locations: Vec<Path>,
    pub board_text_height: Option<PositiveLength>,
}
impl BoardDrcData {
    /// Extracts data with the requested settings, without changing the board
    /// or requiring rebuilt planes/air wires for these checks.
    pub fn extract(
        project: &Project,
        board: &Board,
        settings: &BoardDesignRuleCheckSettings,
    ) -> Result<Self> {
        let mut data = Self {
            board: board.id(),
            settings: settings.clone(),
            holes: Vec::new(),
            planes: Vec::new(),
            copper_layers: board.copper_layers(),
            used_layers: BTreeSet::from([Layer::TOP_COPPER, Layer::BOT_COPPER]),
            widths: Vec::new(),
            outlines: Vec::new(),
            silkscreen_layers: board
                .settings()
                .silkscreen_layers_top
                .iter()
                .chain(&board.settings().silkscreen_layers_bot)
                .copied()
                .collect(),
        };
        let net_name = |net: Option<NetSignalId>| -> Result<String> {
            net.map(|id| {
                project
                    .circuit()
                    .net_signal(id)
                    .map(|n| n.name().to_string())
                    .ok_or(Error::InexistentNetSignal(id.0))
            })
            .transpose()
            .map(Option::unwrap_or_default)
        };
        for hole in board.holes().values() {
            data.holes.push(Hole {
                reference: DrcHoleRef::BoardHole(hole.uuid()),
                diameter: hole.diameter(),
                path: hole.path().clone(),
                pad_position: None,
                pad_name: String::new(),
                net_name: String::new(),
                min_via_drill: UnsignedLength::ZERO,
            });
        }
        for segment in board.net_segments().values() {
            let net = net_name(segment.net())?;
            let min_via_drill = if let Some(id) = segment.net() {
                let signal = project
                    .circuit()
                    .net_signal(id)
                    .ok_or(Error::InexistentNetSignal(id.0))?;
                project
                    .circuit()
                    .net_classes()
                    .get(&signal.net_class())
                    .ok_or(Error::InexistentNetClass(signal.net_class().0))?
                    .min_via_drill_diameter()
            } else {
                UnsignedLength::ZERO
            };
            for pad in segment.pads().values() {
                let p = pad.pad();
                data.pad_holes(
                    p,
                    Transform::new(p.position(), p.rotation(), false),
                    "",
                    |hole| DrcHoleRef::PadHole {
                        segment: segment.id(),
                        pad: pad.uuid(),
                        hole,
                    },
                );
            }
            for via in segment.vias().values() {
                let view = ViaView::new(via, board, segment.net(), project.circuit())?;
                data.holes.push(Hole {
                    reference: DrcHoleRef::Via {
                        segment: segment.id(),
                        via: via.uuid(),
                    },
                    diameter: view.drill_diameter(),
                    path: NonEmptyPath::from_point(via.position()),
                    pad_position: None,
                    pad_name: String::new(),
                    net_name: net.clone(),
                    min_via_drill,
                });
            }
            data.used_layers
                .extend(segment.traces().values().map(|t| t.layer()));
        }
        for device in board.devices().values() {
            let footprint = ResolvedDevice::resolve(device, project.library())?.footprint;
            let transform = device.transform();
            for hole in footprint.holes() {
                data.holes.push(Hole {
                    reference: DrcHoleRef::DeviceHole {
                        device: device.component(),
                        hole: hole.uuid(),
                    },
                    diameter: hole.diameter(),
                    path: transform.map(hole.path()),
                    pad_position: None,
                    pad_name: String::new(),
                    net_name: String::new(),
                    min_via_drill: UnsignedLength::ZERO,
                });
            }
            let mut pads = device.pads(project.library(), project.circuit())?;
            pads.sort_unstable_by_key(|p| p.uuid());
            for pad in pads {
                data.pad_holes(
                    pad.properties(),
                    Transform::new(pad.position(), pad.rotation(), pad.mirrored()),
                    pad.package_pad()
                        .map(|p| p.name().as_str())
                        .unwrap_or_default(),
                    |hole| DrcHoleRef::FootprintPadHole {
                        device: device.component(),
                        pad: pad.uuid(),
                        hole,
                    },
                );
            }
            for layer in footprint
                .polygons()
                .iter()
                .map(|p| p.layer())
                .chain(footprint.circles().iter().map(|c| c.layer()))
            {
                if layer.is_copper() {
                    data.used_layers.insert(transform.map(&layer));
                }
            }
        }
        for plane in board.planes().values() {
            data.used_layers.insert(plane.layer());
            data.planes.push((plane.clone(), net_name(plane.net())?));
        }
        data.used_layers.extend(
            board
                .polygons()
                .values()
                .map(|p| p.layer())
                .chain(board.stroke_texts().values().map(|t| t.layer()))
                .filter(|l| l.is_copper()),
        );
        data.collect_widths(project, board)?;
        for polygon in board
            .polygons()
            .values()
            .filter(|p| p.layer().is_board_edge())
        {
            data.outlines.push((
                DrcObjectRef::Polygon {
                    device: None,
                    polygon: polygon.uuid(),
                },
                polygon.layer(),
                polygon.path().clone(),
            ));
        }
        for device in board.devices().values() {
            let footprint = ResolvedDevice::resolve(device, project.library())?.footprint;
            for polygon in footprint
                .polygons()
                .iter()
                .filter(|p| p.layer().is_board_edge())
            {
                data.outlines.push((
                    DrcObjectRef::Polygon {
                        device: Some(device.component()),
                        polygon: polygon.uuid(),
                    },
                    polygon.layer(),
                    device.transform().map(polygon.path()),
                ));
            }
            for circle in footprint
                .circles()
                .iter()
                .filter(|c| c.layer().is_board_edge())
            {
                data.outlines.push((
                    DrcObjectRef::Circle {
                        device: device.component(),
                        circle: circle.uuid(),
                    },
                    circle.layer(),
                    device
                        .transform()
                        .map(&Path::circle(circle.diameter()).translated(circle.center())),
                ));
            }
        }
        Ok(data)
    }
    /// Board this snapshot belongs to.
    pub fn board(&self) -> BoardId {
        self.board
    }
    /// Settings captured with this snapshot.
    pub fn settings(&self) -> &BoardDesignRuleCheckSettings {
        &self.settings
    }
    fn pad_holes(
        &mut self,
        pad: &Pad,
        transform: Transform,
        name: &str,
        reference: impl Fn(crate::types::Uuid) -> DrcHoleRef,
    ) {
        for hole in pad.holes() {
            self.holes.push(Hole {
                reference: reference(hole.uuid()),
                diameter: hole.diameter(),
                path: transform.map(hole.path()),
                pad_position: Some(transform.position),
                pad_name: name.into(),
                net_name: String::new(),
                min_via_drill: UnsignedLength::ZERO,
            });
        }
    }
}

impl BoardDrcData {
    fn collect_widths(&mut self, project: &Project, board: &Board) -> Result<()> {
        let net_min = |net: Option<NetSignalId>| -> Result<UnsignedLength> {
            if let Some(id) = net {
                let signal = project
                    .circuit()
                    .net_signal(id)
                    .ok_or(Error::InexistentNetSignal(id.0))?;
                Ok(project
                    .circuit()
                    .net_classes()
                    .get(&signal.net_class())
                    .ok_or(Error::InexistentNetClass(signal.net_class().0))?
                    .min_copper_width()
                    .max(self.settings.min_copper_width()))
            } else {
                Ok(self.settings.min_copper_width())
            }
        };
        let stroke = |path: &Path, width: UnsignedLength| {
            path.to_outline_strokes(
                PositiveLength::new((*width).max(crate::types::Length::new(50_000)))
                    .expect("width is at least 50000 nm"),
            )
        };
        for text in board.stroke_texts().values() {
            self.widths.push(WidthObject {
                reference: DrcObjectRef::StrokeText {
                    device: None,
                    text: text.uuid(),
                },
                layer: text.layer(),
                width: text.stroke_width(),
                displayed_width: text.stroke_width(),
                net_min_width: UnsignedLength::ZERO,
                device_name: String::new(),
                locations: text
                    .paths(project, board, None)?
                    .iter()
                    .flat_map(|path| stroke(&text.transform().map(path), text.stroke_width()))
                    .collect(),
                board_text_height: Some(text.height()),
            });
        }
        for polygon in board.polygons().values() {
            if polygon.is_filled()
                && polygon.path().is_closed()
                && polygon.line_width() == UnsignedLength::ZERO
            {
                continue;
            }
            self.widths.push(WidthObject {
                reference: DrcObjectRef::Polygon {
                    device: None,
                    polygon: polygon.uuid(),
                },
                layer: polygon.layer(),
                width: polygon.line_width(),
                displayed_width: polygon.line_width(),
                net_min_width: UnsignedLength::ZERO,
                device_name: String::new(),
                locations: stroke(polygon.path(), polygon.line_width()),
                board_text_height: None,
            });
        }
        for plane in board.planes().values() {
            let mut outline = plane.outline().clone();
            outline.close();
            self.widths.push(WidthObject {
                reference: DrcObjectRef::Plane(plane.id()),
                layer: plane.layer(),
                width: plane.min_width(),
                displayed_width: plane.min_width(),
                net_min_width: net_min(plane.net())?,
                device_name: String::new(),
                locations: outline
                    .to_outline_strokes(PositiveLength::new(crate::types::Length::new(200_000))?),
                board_text_height: None,
            });
        }
        for device in board.devices().values() {
            let footprint = ResolvedDevice::resolve(device, project.library())?.footprint;
            let name = project
                .circuit()
                .component_instance(device.component())
                .ok_or(Error::InexistentComponent(device.component().0))?
                .name()
                .to_string();
            let transform = device.transform();
            for text in device.stroke_texts().values() {
                self.widths.push(WidthObject {
                    reference: DrcObjectRef::StrokeText {
                        device: Some(device.component()),
                        text: text.uuid(),
                    },
                    layer: text.layer(),
                    width: text.stroke_width(),
                    displayed_width: text.stroke_width(),
                    net_min_width: UnsignedLength::ZERO,
                    device_name: name.clone(),
                    locations: text
                        .paths(project, board, Some(device))?
                        .iter()
                        .flat_map(|p| stroke(&text.transform().map(p), text.stroke_width()))
                        .collect(),
                    board_text_height: None,
                });
            }
            for polygon in footprint.polygons() {
                if polygon.is_filled()
                    && polygon.path().is_closed()
                    && polygon.line_width() == UnsignedLength::ZERO
                {
                    continue;
                }
                self.widths.push(WidthObject {
                    reference: DrcObjectRef::Polygon {
                        device: Some(device.component()),
                        polygon: polygon.uuid(),
                    },
                    layer: transform.map(&polygon.layer()),
                    width: polygon.line_width(),
                    displayed_width: polygon.line_width(),
                    net_min_width: UnsignedLength::ZERO,
                    device_name: name.clone(),
                    locations: stroke(&transform.map(polygon.path()), polygon.line_width()),
                    board_text_height: None,
                });
            }
            for circle in footprint.circles() {
                let diameter = circle.diameter() + circle.line_width();
                self.widths.push(WidthObject {
                    reference: DrcObjectRef::Circle {
                        device: device.component(),
                        circle: circle.uuid(),
                    },
                    layer: transform.map(&circle.layer()),
                    width: if circle.is_filled() {
                        diameter.into()
                    } else {
                        circle.line_width()
                    },
                    displayed_width: circle.line_width(),
                    net_min_width: UnsignedLength::ZERO,
                    device_name: name.clone(),
                    locations: vec![
                        transform.map(&Path::circle(diameter).translated(circle.center())),
                    ],
                    board_text_height: None,
                });
            }
        }
        for segment in board.net_segments().values() {
            for trace in segment.traces().values() {
                let position = |anchor| {
                    board
                        .anchor_position(segment, anchor, project.library(), project.circuit())
                        .ok_or(Error::InexistentTraceAnchor {
                            anchor,
                            device_missing: false,
                        })
                };
                self.widths.push(WidthObject {
                    reference: DrcObjectRef::Trace {
                        segment: segment.id(),
                        trace: trace.uuid(),
                    },
                    layer: trace.layer(),
                    width: trace.width().into(),
                    displayed_width: trace.width().into(),
                    net_min_width: net_min(segment.net())?,
                    device_name: String::new(),
                    locations: vec![Path::obround_line(
                        position(trace.p1())?,
                        position(trace.p2())?,
                        trace.width(),
                    )],
                    board_text_height: None,
                });
            }
        }
        Ok(())
    }
}
