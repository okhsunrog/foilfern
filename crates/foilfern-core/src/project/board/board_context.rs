//! Read-only view of a board with the derived item properties which upstream
//! caches in its `BI_*` objects (`BI_Pad` geometries, `BI_Via` actual
//! size/drill and stop mask diameters, `BI_StrokeText` paths), as needed by
//! the plane fragments builder and the exports.
//!
//! Also creates the attribute lookups of the board and its devices
//! ([`ProjectAttributeLookup`]), to substitute stroke texts and output file
//! paths.
//!
//! Not an upstream class: upstream computes these values in the item
//! objects (see `bi_pad.cpp`, `bi_via.cpp`, `bi_stroketext.cpp`,
//! `bi_device.cpp`); the values and their order are the same.

use std::collections::BTreeMap;

use super::{Board, BoardDevice, BoardStrokeTextData, BoardViaProperties};
use crate::font::StrokeTextPathBuilder;
use crate::geometry::{ComponentSide, Pad, PadGeometry, Path, TraceAnchor, Via};
use crate::library::dev::{Device, Part};
use crate::library::pkg::{Footprint, Package};
use crate::project::Project;
use crate::project::attribute_lookup::ProjectAttributeLookup;
use crate::project::board::export_error::BoardExportResult;
use crate::project::board::pad_data::PadOnBoard;
use crate::project::circuit::ComponentInstance;
use crate::project::error::{EntityKind, Error};
use crate::project::id::{AssemblyVariantId, BoardId, ComponentInstanceId, NetSignalId};
use crate::types::{Angle, Layer, Length, Point, Uuid};
use crate::utils::painter_path;
use crate::utils::transform::Transform;

/// A device of a board with its resolved library elements and component.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ContextDevice<'a> {
    pub device: &'a BoardDevice,
    pub component: &'a ComponentInstance,
    pub lib_device: &'a Device,
    pub package: &'a Package,
    pub footprint: &'a Footprint,
}

impl<'a> ContextDevice<'a> {
    /// Returns the designator (component instance name).
    pub fn designator(&self) -> &'a str {
        self.component.name().as_str()
    }

    /// Returns the transformation of the device.
    pub fn transform(&self) -> Transform {
        self.device.transform()
    }

    /// Returns the parts of the device in an assembly variant (all variants
    /// if `None`), upstream `BI_Device::getParts()`.
    pub fn parts(&self, assembly_variant: Option<AssemblyVariantId>) -> Vec<Part> {
        self.device.parts(self.component, assembly_variant)
    }

    /// Whether the device is assembled in an assembly variant (upstream
    /// `BI_Device::isInAssemblyVariant()`).
    pub fn is_in_assembly_variant(&self, assembly_variant: AssemblyVariantId) -> bool {
        self.device
            .is_in_assembly_variant(self.component, assembly_variant)
    }

    /// Returns the stop mask offsets of the footprint holes (upstream
    /// `BI_Device::getHoleStopMasks()`).
    pub fn hole_stop_mask_offsets(
        &self,
        rules: &super::BoardDesignRules,
    ) -> BTreeMap<Uuid, Option<Length>> {
        BoardDevice::hole_stop_mask_offsets(self.footprint, rules)
    }
}

/// A pad on a board (upstream `BI_Pad`): a footprint pad of a device or a
/// standalone pad of a net segment.
#[derive(Debug, Clone)]
pub(crate) struct ContextPad<'a> {
    /// The pad properties (in pad coordinates).
    pub properties: &'a Pad,
    /// Position on the board.
    pub position: Point,
    /// Rotation on the board.
    pub rotation: Angle,
    /// Whether the pad is mirrored (device on the bottom side).
    pub mirrored: bool,
    /// The net of the pad.
    pub net: Option<NetSignalId>,
    /// The device, for footprint pads.
    pub device: Option<ContextDevice<'a>>,
    /// Name of the package pad (footprint pads connected to a package pad).
    pub package_pad_name: Option<&'a str>,
    /// Name of the component signal (footprint pads connected to a signal).
    pub signal_name: Option<&'a str>,
    /// Geometries on the copper, stop mask and solder paste layers.
    pub geometries: BTreeMap<Layer, Vec<PadGeometry>>,
}

impl ContextPad<'_> {
    fn on_board(&self) -> PadOnBoard<'_> {
        PadOnBoard {
            pad: self.properties,
            mirrored: self.mirrored,
        }
    }

    /// Returns the transformation of the pad (upstream `Transform(BI_Pad)`).
    pub fn transform(&self) -> Transform {
        Transform::new(self.position, self.rotation, self.mirrored)
    }

    /// Upstream `BI_Pad::isOnLayer()`.
    pub fn is_on_layer(&self, layer: Layer) -> bool {
        self.on_board().is_on_layer(layer)
    }

    /// Upstream `BI_Pad::getSolderLayer()`.
    pub fn solder_layer(&self) -> Layer {
        self.on_board().solder_layer()
    }

    /// Upstream `BI_Pad::getComponentSide()`.
    pub fn component_side(&self) -> ComponentSide {
        self.on_board().component_side()
    }

    /// Returns the geometries on a layer.
    pub fn geometries_on(&self, layer: Layer) -> &[PadGeometry] {
        self.geometries.get(&layer).map_or(&[], Vec::as_slice)
    }
}

/// A via with its derived properties (upstream `BI_Via`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ContextVia<'a> {
    pub via: &'a Via,
    pub props: BoardViaProperties,
}

/// A read-only view of a board of a project, see the module documentation.
#[derive(Debug, Clone)]
pub(crate) struct BoardContext<'a> {
    pub project: &'a Project,
    pub board: &'a Board,
    /// Devices in UUID order.
    pub devices: Vec<ContextDevice<'a>>,
    /// Positions of the footprint pads (for trace anchors).
    footprint_pad_positions: BTreeMap<(ComponentInstanceId, Uuid), Point>,
}

impl<'a> BoardContext<'a> {
    /// Resolves the devices of `board`; fails if a library element or
    /// component instance does not exist.
    pub fn new(project: &'a Project, board: &'a Board) -> BoardExportResult<Self> {
        let library = project.library();
        let circuit = project.circuit();
        let mut devices = Vec::with_capacity(board.devices().len());
        let mut footprint_pad_positions = BTreeMap::new();
        for device in board.devices().values() {
            let component =
                circuit
                    .component_instance(device.component())
                    .ok_or(Error::NotFound {
                        kind: EntityKind::Component,
                        uuid: device.component().0,
                    })?;
            let lib_device = library
                .device(&device.lib_device())
                .ok_or(Error::MissingLibraryDevice(device.lib_device()))?;
            let package = library
                .package(&lib_device.package_uuid())
                .ok_or(Error::MissingLibraryPackage(lib_device.package_uuid()))?;
            let footprint = package
                .footprints()
                .required_by_uuid(&device.lib_footprint())
                .map_err(Error::from)?;
            let transform = device.transform();
            for pad in footprint.pads() {
                footprint_pad_positions.insert(
                    (device.component(), pad.uuid()),
                    transform.map(&pad.pad().position()),
                );
            }
            devices.push(ContextDevice {
                device,
                component,
                lib_device,
                package,
                footprint,
            });
        }
        Ok(Self {
            project,
            board,
            devices,
            footprint_pad_positions,
        })
    }

    /// Upstream `Board::calculateBoundingRect()`: the bounding rectangle
    /// `(bottom_left, top_right)` of the board outlines (of the board and of the
    /// footprints), or `None` if there are no outlines.
    pub fn bounding_rect(&self) -> Option<(Point, Point)> {
        let mut outlines: Vec<Path> = self
            .board
            .polygons()
            .values()
            .filter(|p| p.layer() == Layer::BOARD_OUTLINES && !p.path().vertices().is_empty())
            .map(|p| p.path().clone())
            .collect();
        for dev in &self.devices {
            let transform = dev.transform();
            for polygon in dev.footprint.polygons().iter() {
                if polygon.layer() == Layer::BOARD_OUTLINES && !polygon.path().vertices().is_empty()
                {
                    outlines.push(transform.map(polygon.path()));
                }
            }
            for circle in dev.footprint.circles().iter() {
                if circle.layer() == Layer::BOARD_OUTLINES {
                    outlines.push(
                        transform.map(&Path::circle(circle.diameter()).translated(circle.center())),
                    );
                }
            }
        }
        if outlines.is_empty() {
            return None;
        }
        let rect = painter_path::bounding_rect_px(&outlines);
        // The rectangle is built from coordinates of valid points, so the
        // conversion back cannot overflow.
        let bottom_left = Point::from_px(rect.left(), rect.bottom()).unwrap_or_default();
        let top_right = Point::from_px(rect.right(), rect.top()).unwrap_or_default();
        Some((bottom_left, top_right))
    }

    /// Returns the name of a net.
    pub fn net_name(&self, net: Option<NetSignalId>) -> Option<&'a str> {
        net.and_then(|n| self.project.circuit().net_signal(n))
            .map(|n| n.name().as_str())
    }

    /// Returns the footprint pads of a device, sorted by UUID (upstream
    /// `BI_Device::getPads()`).
    pub fn device_pads(
        &self,
        device: &ContextDevice<'a>,
    ) -> BoardExportResult<Vec<ContextPad<'a>>> {
        let copper_layers = self.board.copper_layers();
        let rules = self.board.design_rules();
        let mut views = device
            .device
            .pads(self.project.library(), self.project.circuit())?;
        views.sort_by_key(|v| v.uuid());
        Ok(views
            .into_iter()
            .map(|view| {
                let connected = self.board.anchor_trace_layers(view.trace_anchor());
                ContextPad {
                    properties: view.properties(),
                    position: view.position(),
                    rotation: view.rotation(),
                    mirrored: view.mirrored(),
                    net: view.net(),
                    device: Some(*device),
                    package_pad_name: view.package_pad().map(|p| p.name().as_str()),
                    signal_name: view.component_signal_name(),
                    geometries: view.geometries(&copper_layers, rules, &connected),
                }
            })
            .collect())
    }

    /// Returns the standalone pads of a net segment, sorted by UUID.
    pub fn segment_pads(&self, segment: &'a super::BoardNetSegment) -> Vec<ContextPad<'a>> {
        let copper_layers = self.board.copper_layers();
        let rules = self.board.design_rules();
        segment
            .pads()
            .values()
            .map(|pad| {
                let connected = self.board.anchor_trace_layers(TraceAnchor::Pad(pad.uuid()));
                let on_board = PadOnBoard {
                    pad: pad.pad(),
                    mirrored: false,
                };
                ContextPad {
                    properties: pad.pad(),
                    position: pad.pad().position(),
                    rotation: pad.pad().rotation(),
                    mirrored: false,
                    net: segment.net(),
                    device: None,
                    package_pad_name: None,
                    signal_name: None,
                    geometries: on_board.geometries(&copper_layers, rules, &connected),
                }
            })
            .collect()
    }

    /// Returns the vias of a net segment with their derived properties,
    /// sorted by UUID.
    pub fn segment_vias(&self, segment: &'a super::BoardNetSegment) -> Vec<ContextVia<'a>> {
        let circuit = self.project.circuit();
        segment
            .vias()
            .values()
            .map(|via| ContextVia {
                via,
                props: self.board.via_properties(via, segment.net(), circuit),
            })
            .collect()
    }

    /// Returns the position of a trace anchor of a net segment.
    pub fn anchor_position(
        &self,
        segment: &super::BoardNetSegment,
        anchor: TraceAnchor,
    ) -> Option<Point> {
        match anchor {
            TraceAnchor::FootprintPad { device, pad } => self
                .footprint_pad_positions
                .get(&(ComponentInstanceId(device), pad))
                .copied(),
            TraceAnchor::Junction(uuid) => segment.junctions().get(&uuid).map(|j| j.position()),
            TraceAnchor::Via(uuid) => segment.vias().get(&uuid).map(|v| v.position()),
            TraceAnchor::Pad(uuid) => segment.pads().get(&uuid).map(|p| p.pad().position()),
        }
    }

    /// Returns the paths of a stroke text (relative to its position, not
    /// transformed, upstream `BI_StrokeText::getPaths()`): the text with
    /// substituted attributes of the device (if any) or the board.
    pub fn stroke_text_paths(
        &self,
        text: &BoardStrokeTextData,
        device: Option<&ContextDevice<'a>>,
    ) -> BoardExportResult<Vec<Path>> {
        let font = self
            .project
            .stroke_fonts()
            .font(&self.board.settings().default_font)?;
        let substituted = match device {
            Some(dev) => {
                let part = dev.parts(None).into_iter().next();
                self.device_lookup(dev, part.as_ref())
                    .substitute(text.text())
            }
            None => self.board_lookup().substitute(text.text()),
        };
        Ok(StrokeTextPathBuilder::build(
            font,
            text.letter_spacing(),
            text.line_spacing(),
            text.height(),
            text.stroke_width(),
            text.align(),
            text.rotation(),
            text.auto_rotate(),
            &substituted,
        ))
    }

    /// Returns the transformation of a stroke text.
    pub fn stroke_text_transform(text: &BoardStrokeTextData) -> Transform {
        Transform::new(text.position(), text.rotation(), text.mirrored())
    }

    /// Attribute lookup of the board without assembly variant (upstream
    /// `ProjectAttributeLookup(board, nullptr)`).
    pub fn board_lookup(&self) -> ProjectAttributeLookup<'a> {
        ProjectAttributeLookup::for_board(self.project, self.board, None)
    }

    /// Attribute lookup of a device (upstream
    /// `ProjectAttributeLookup(device, part)`).
    pub fn device_lookup<'p>(
        &self,
        device: &ContextDevice<'a>,
        part: Option<&'p Part>,
    ) -> ProjectAttributeLookup<'p>
    where
        'a: 'p,
    {
        ProjectAttributeLookup::for_device(self.project, self.board, device.device, part)
    }
}

impl Project {
    /// Returns the bounding rectangle `(bottom_left, top_right)` of the
    /// board outlines of a board and of its footprints, or `None` if there
    /// are no outlines (upstream `Board::calculateBoundingRect()`). Fails if
    /// the board or a library element of its devices does not exist.
    pub fn board_bounding_rect(&self, board: BoardId) -> BoardExportResult<Option<(Point, Point)>> {
        let b = self.board(board).ok_or(Error::NotFound {
            kind: EntityKind::Board,
            uuid: board.0,
        })?;
        Ok(BoardContext::new(self, b)?.bounding_rect())
    }
}
