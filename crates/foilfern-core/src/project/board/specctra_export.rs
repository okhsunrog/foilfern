//! Port of libs/librepcb/core/project/board/boardspecctraexport.{h,cpp}.
//!
//! Exports a board as Specctra DSN file (e.g. for autorouting with
//! FreeRouting). The DSN writer is a port of upstream's exporter on top of
//! the core [`SExpression`] writer (permissive mode), so the output is byte
//! identical to upstream's (verified against the upstream test
//! expectation). The `topola_specctra` crate is only used in tests to
//! validate the output independently; it cannot reproduce upstream's
//! formatting.
//!
//! Differences to upstream:
//! - The host CAD name and version written into the `parser` node are
//!   parameters (upstream: `qApp->applicationName()`/`applicationVersion()`),
//!   default "LibrePCB" and the version of this crate.
//! - The pins of a net are ordered by (component UUID, signal UUID, pad
//!   UUID) and (net segment UUID, pad UUID). Upstream iterates its
//!   registration lists, which is the same order for a freshly opened
//!   project but can differ after editing (see COMPAT.md).
//! - Upstream's quirks are reproduced on purpose because they define the
//!   bytes of the file: the padstack ID of blind/buried vias contains a
//!   literal `%2` (a nested `QString::arg()` call), and vertical oblong pads
//!   are exported as a zero-length path.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use super::BoardNetSegment;
use super::board_context::{BoardContext, ContextPad, ContextVia};
use super::export_error::{BoardExportError, BoardExportResult};
use crate::geometry::{NonEmptyPath, PadGeometry, PadGeometryShape, Path, ZoneLayers, ZoneRules};
use crate::library::LibraryBaseElement;
use crate::project::Project;
use crate::project::error::{EntityKind, Error};
use crate::project::id::BoardId;
use crate::serialization::{List, Mode, SExpression};
use crate::types::{Angle, Layer, Length, MaskConfig, Point, PositiveLength, UnsignedLength};
use crate::utils::{clipper_helpers, toolbox};

/// The maximum allowed arc tolerance when flattening arcs (upstream
/// `maxArcTolerance()`).
const MAX_ARC_TOLERANCE: Length = Length::new(5000);

fn max_arc_tolerance() -> PositiveLength {
    PositiveLength::new(MAX_ARC_TOLERANCE).expect("constant is positive")
}

/// Via properties relevant for the export (upstream `ViaData`).
#[derive(Debug, Clone, Copy)]
struct ViaData {
    drill: PositiveLength,
    auto_drill: bool,
    size: PositiveLength,
    auto_size: bool,
    start_layer: Layer,
    end_layer: Layer,
    exposure: MaskConfig,
}

impl ViaData {
    /// Via data of an existing via.
    fn from_via(via: &ContextVia<'_>) -> Self {
        Self {
            drill: via.props.drill_diameter,
            auto_drill: via.via.drill_diameter().is_none(),
            size: via.props.size,
            auto_size: via.via.size().is_none(),
            start_layer: via.via.start_layer(),
            end_layer: via.via.end_layer(),
            exposure: via.via.exposure_config(),
        }
    }

    /// Via data of the default via settings of a board.
    fn from_rules(rules: &super::BoardDesignRules) -> Self {
        let drill = rules.default_via_drill_diameter();
        Self {
            drill,
            auto_drill: true,
            size: crate::geometry::Via::calc_size_from_rules(drill, &rules.via_annular_ring()),
            auto_size: true,
            start_layer: Layer::TOP_COPPER,
            end_layer: Layer::BOT_COPPER,
            exposure: MaskConfig::Off,
        }
    }
}

/// Returns the padstack ID of a via (upstream `getWiringPadStackId()`).
///
/// Keep in sync with the Specctra session import, which parses the drill
/// diameter, size and exposure back from this ID.
pub fn via_padstack_id(
    drill: PositiveLength,
    auto_drill: bool,
    size: PositiveLength,
    auto_size: bool,
    start_layer: Layer,
    end_layer: Layer,
    exposure: MaskConfig,
) -> String {
    let layer_name = |l: Layer| {
        if l == Layer::TOP_COPPER {
            "top".to_owned()
        } else if l == Layer::BOT_COPPER {
            "bot".to_owned()
        } else {
            format!("in{}", l.copper_number())
        }
    };
    let mut s = String::from("via");
    s += &format!("-{}", drill.to_mm_string());
    if auto_drill {
        s += ":auto";
    }
    s += &format!("-{}", size.to_mm_string());
    if auto_size {
        s += ":auto";
    }
    if (start_layer == Layer::TOP_COPPER) && (end_layer == Layer::BOT_COPPER) {
        s += "-tht";
    } else {
        // Upstream: `QString("-%1:%2").arg(getLayerName(start).arg(
        // getLayerName(end)))` -- the inner `arg()` has no placeholder, so
        // the end layer is dropped and `%2` stays in the string.
        s += &format!("-{}:%2", layer_name(start_layer));
    }
    if let Some(offset) = exposure.offset() {
        s += &format!("-exposed:{}", offset.to_mm_string());
    } else if exposure.is_enabled() {
        s += "-exposed";
    }
    s
}

fn via_data_padstack_id(via: &ViaData) -> String {
    via_padstack_id(
        via.drill,
        via.auto_drill,
        via.size,
        via.auto_size,
        via.start_layer,
        via.end_layer,
        via.exposure,
    )
}

/// Returns the net name of a net segment in the DSN file (upstream
/// `getNetName()`): the net name, or `~anonymous~<segment UUID>` for
/// segments without net.
///
/// Keep in sync with the Specctra session import.
pub fn segment_net_name(project: &Project, segment: &BoardNetSegment) -> String {
    match segment.net().and_then(|n| project.circuit().net_signal(n)) {
        Some(net) => net.name().as_str().to_owned(),
        None => format!("{ANONYMOUS_NET_PREFIX}{}", segment.uuid()),
    }
}

/// The default host CAD name written into the DSN file.
pub const SPECCTRA_HOST_CAD: &str = "LibrePCB";

/// The default host CAD version written into the DSN file.
pub const SPECCTRA_HOST_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Prefix of the dummy net names of net segments without net.
pub const ANONYMOUS_NET_PREFIX: &str = "~anonymous~";

/// Exports a board as Specctra DSN (upstream `BoardSpecctraExport`).
#[derive(Debug, Clone)]
pub struct BoardSpecctraExport<'a> {
    ctx: BoardContext<'a>,
    host_cad: String,
    host_version: String,
}

impl<'a> BoardSpecctraExport<'a> {
    /// Creates an exporter for a board of a project.
    ///
    /// Fails if the board or a library element used by it does not exist.
    pub fn new(project: &'a Project, board: BoardId) -> BoardExportResult<Self> {
        let b = project.board(board).ok_or(Error::NotFound {
            kind: EntityKind::Board,
            uuid: board.0,
        })?;
        Ok(Self {
            ctx: BoardContext::new(project, b)?,
            host_cad: SPECCTRA_HOST_CAD.to_owned(),
            host_version: SPECCTRA_HOST_VERSION.to_owned(),
        })
    }

    /// Sets the host CAD name and version written into the file (upstream:
    /// the application name and version).
    pub fn with_host(mut self, cad: impl Into<String>, version: impl Into<String>) -> Self {
        self.host_cad = cad.into();
        self.host_version = version.into();
        self
    }

    /// Generates the DSN file content (upstream `generate()`).
    pub fn generate(&self) -> BoardExportResult<Vec<u8>> {
        let ctx = &self.ctx;
        let board = ctx.board;

        let mut fpt_pad_stacks: Vec<List> = Vec::new();
        let via_pad_stacks = self.via_pad_stacks();

        // Project name must not contain spaces since quotation is not
        // activated until the "parser" node appears.
        let mut name = ctx.project.metadata().name.as_str().to_owned();
        if ctx.project.boards().len() > 1 {
            name += " ";
            name += board.name().as_str();
        }
        let mut name = toolbox::clean_user_input_string(
            &name,
            |c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '.'),
            true,
            false,
            false,
            "-",
            None,
        );
        if name.is_empty() {
            name = "unnamed".to_owned();
        }

        // Build the file.
        let mut root = List::new("pcb");
        root.push(SExpression::token(name));
        root.ensure_line_break();
        root.push(self.gen_parser().into());
        root.ensure_line_break();
        root.push(gen_resolution().into());
        root.ensure_line_break();
        root.append_list("unit").push(SExpression::token("mm"));
        root.ensure_line_break();
        root.push(self.gen_structure(&via_pad_stacks)?.into());
        root.ensure_line_break();
        root.push(self.gen_placement().into());
        root.ensure_line_break();
        root.push(
            self.gen_library(&mut fpt_pad_stacks, via_pad_stacks)?
                .into(),
        );
        root.ensure_line_break();
        root.push(self.gen_network()?.into());
        root.ensure_line_break();
        root.push(self.gen_wiring()?.into());
        root.ensure_line_break();
        Ok(SExpression::from(root).to_byte_array(Mode::Permissive)?)
    }

    /// Returns the IDs of the via padstacks written into the file (those of
    /// the existing vias and of the default via), in file order.
    pub fn via_padstack_ids(&self) -> Vec<String> {
        self.via_pad_stacks().iter().map(pad_stack_name).collect()
    }

    /// The via pad stacks, sorted.
    fn via_pad_stacks(&self) -> Vec<List> {
        let ctx = &self.ctx;
        let board = ctx.board;
        // Add all via pad stacks.
        let mut via_pad_stacks: Vec<List> = Vec::new();
        for segment in board.net_segments().values() {
            for via in ctx.segment_vias(segment) {
                add_to_pad_stacks(
                    &mut via_pad_stacks,
                    self.gen_wiring_pad_stack(&ViaData::from_via(&via)),
                );
            }
        }
        // Important: Always add a via pad stack from the default settings.
        // Without any via in the DSN file, FreeRouting won't route anything
        // at all.
        add_to_pad_stacks(
            &mut via_pad_stacks,
            self.gen_wiring_pad_stack(&ViaData::from_rules(board.design_rules())),
        );
        // Sort for a more natural order of vias.
        via_pad_stacks.sort_by(compare_pad_stacks);
        via_pad_stacks
    }

    fn gen_parser(&self) -> List {
        let mut root = List::new("parser");
        root.ensure_line_break();
        root.append_list("string_quote")
            .push(SExpression::token("\""));
        root.ensure_line_break();
        root.append_list("space_in_quoted_tokens")
            .push(SExpression::token("on"));
        root.ensure_line_break();
        root.append_list("host_cad")
            .push(SExpression::string(self.host_cad.clone()));
        root.ensure_line_break();
        root.append_list("host_version")
            .push(SExpression::string(self.host_version.clone()));
        root.ensure_line_break();
        root
    }

    fn gen_structure(&self, via_pad_stacks: &[List]) -> BoardExportResult<List> {
        let ctx = &self.ctx;
        let board = ctx.board;
        let inner_layer_count = board.settings().inner_layer_count as usize;
        let mut root = List::new("structure");

        // Layers.
        for i in 0..(inner_layer_count + 2) {
            root.ensure_line_break();
            let layer = if i == 0 {
                Layer::TOP_COPPER
            } else if i <= inner_layer_count {
                Layer::inner_copper(i).ok_or(BoardExportError::Logic("invalid inner layer"))?
            } else {
                Layer::BOT_COPPER
            };
            let node = root.append_list("layer");
            node.push(SExpression::token(layer.id()));
            node.append_list("type").push(SExpression::token("signal"));
        }

        // PCB boundary.
        for polygon in board.polygons().values() {
            if polygon.layer() == Layer::BOARD_OUTLINES {
                root.ensure_line_break();
                let node = root.append_list("boundary");
                node.ensure_line_break();
                node.push(to_path("pcb", UnsignedLength::ZERO, polygon.path(), true).into());
                node.ensure_line_break();
            }
        }

        // Planes.
        for plane in board.planes().values() {
            if let Some(net) = ctx.net_name(plane.net()) {
                root.ensure_line_break();
                let node = root.append_list("plane");
                node.push(SExpression::string(net));
                node.ensure_line_break();
                node.push(
                    to_polygon(
                        plane.layer().id(),
                        UnsignedLength::ZERO,
                        plane.outline(),
                        true,
                    )
                    .into(),
                );
                node.ensure_line_break();
            }
        }

        // Keepout areas.
        for polygon in board.polygons().values() {
            if (polygon.layer() == Layer::BOARD_CUTOUTS)
                || (polygon.layer() == Layer::BOARD_PLATED_CUTOUTS)
            {
                root.ensure_line_break();
                root.push(
                    self.to_path_keepout(
                        &format!("cutout:{}", polygon.uuid()),
                        polygon.path(),
                        &BTreeSet::new(),
                    )?
                    .into(),
                );
            }
        }
        for hole in board.holes().values() {
            root.ensure_line_break();
            root.push(
                self.to_hole_keepout(
                    &format!("hole:{}", hole.uuid()),
                    hole.diameter(),
                    hole.path(),
                )
                .into(),
            );
        }
        for zone in board.zones().values() {
            if zone.rules().contains(ZoneRules::NO_COPPER) {
                root.ensure_line_break();
                root.push(
                    self.to_path_keepout(
                        &format!("zone:{}", zone.uuid()),
                        zone.outline(),
                        zone.layers(),
                    )?
                    .into(),
                );
            }
        }

        // Vias.
        root.ensure_line_break();
        {
            let node = root.append_list("via");
            for pad_stack in via_pad_stacks {
                node.ensure_line_break();
                node.push(SExpression::string(pad_stack_name(pad_stack)));
            }
            node.ensure_line_break();
        }

        root.ensure_line_break();
        root.push(self.gen_structure_rule().into());
        root.ensure_line_break();
        Ok(root)
    }

    fn gen_structure_rule(&self) -> List {
        let drc = &self.ctx.board.settings().drc_settings;
        let mut root = List::new("rule");
        root.ensure_line_break();
        root.append_list("width")
            .push(to_token(*drc.min_copper_width()));
        root.ensure_line_break();
        root.append_list("clearance")
            .push(to_token(*drc.min_copper_copper_clearance()));
        root.ensure_line_break();
        for kind in ["smd_via_same_net", "via_via_same_net"] {
            let node = root.append_list("clearance");
            node.push(to_token(Length::ZERO));
            node.append_list("type").push(SExpression::token(kind));
            root.ensure_line_break();
        }
        root
    }

    fn gen_placement(&self) -> List {
        let mut root = List::new("placement");
        let mut add_component =
            |image_id: &str, designator: &str, pos: Point, rotation: Angle, mirrored: bool| {
                root.ensure_line_break();
                let component = root.append_list("component");
                component.push(SExpression::string(image_id));
                component.ensure_line_break();
                let place = component.append_list("place");
                place.push(SExpression::string(designator));
                place.push(to_token(pos.x));
                place.push(to_token(pos.y));
                place.push(SExpression::token(if mirrored { "back" } else { "front" }));
                place.push(SExpression::token(rotation.to_deg_string()));
                component.ensure_line_break();
            };

        // Dummy placement for board (which may contain pads).
        add_component("BOARD", "BOARD", Point::ORIGIN, Angle::DEG0, false);

        // Real devices.
        for dev in &self.ctx.devices {
            add_component(
                &image_id(dev),
                dev.designator(),
                dev.device.position(),
                dev.device.rotation(),
                dev.device.mirrored(),
            );
        }

        root.ensure_line_break();
        root
    }

    fn gen_library(
        &self,
        fpt_pad_stacks: &mut Vec<List>,
        via_pad_stacks: Vec<List>,
    ) -> BoardExportResult<List> {
        let mut root = List::new("library");
        root.ensure_line_break();
        root.push(self.gen_board_image(fpt_pad_stacks)?.into());
        for dev in &self.ctx.devices {
            root.ensure_line_break();
            root.push(self.gen_device_image(dev, fpt_pad_stacks)?.into());
        }
        root.ensure_line_break();
        for (i, mut pad_stack) in std::mem::take(fpt_pad_stacks).into_iter().enumerate() {
            let name = format!("{}{i}", pad_stack_name(&pad_stack));
            pad_stack.children_mut()[0] = SExpression::string(name);
            root.ensure_line_break();
            root.push(pad_stack.into());
        }
        for pad_stack in via_pad_stacks {
            root.ensure_line_break();
            root.push(pad_stack.into());
        }
        root.ensure_line_break();
        Ok(root)
    }

    /// Image of the board-level pads (upstream `genLibraryImage()`).
    fn gen_board_image(&self, fpt_pad_stacks: &mut Vec<List>) -> BoardExportResult<List> {
        let ctx = &self.ctx;
        let mut root = List::new("image");
        root.push(SExpression::string("BOARD"));
        for segment in ctx.board.net_segments().values() {
            for pad in ctx.segment_pads(segment) {
                let index = add_to_pad_stacks(fpt_pad_stacks, self.gen_library_pad_stack(&pad)?);
                root.ensure_line_break();
                let pin = root.append_list("pin");
                pin.push(SExpression::string(format!("pad-{index}")));
                pin.append_list("rotate").push(SExpression::token(
                    pad.properties.rotation().to_deg_string(),
                ));
                pin.push(SExpression::string(
                    format!("{}:{}", segment.uuid(), pad.properties.uuid()).replace('-', ""),
                ));
                pin.push(to_token(pad.properties.position().x));
                pin.push(to_token(pad.properties.position().y));
            }
        }
        root.ensure_line_break();
        Ok(root)
    }

    /// Image of a device (upstream `genLibraryImage(dev)`).
    fn gen_device_image(
        &self,
        dev: &super::board_context::ContextDevice<'a>,
        fpt_pad_stacks: &mut Vec<List>,
    ) -> BoardExportResult<List> {
        let designator = dev.designator();
        let mut root = List::new("image");
        root.push(SExpression::string(image_id(dev)));
        for polygon in dev.footprint.polygons() {
            if polygon.layer() == Layer::TOP_DOCUMENTATION {
                root.ensure_line_break();
                let outline = root.append_list("outline");
                let mut path = polygon.path().clone();
                if polygon.layer().polygons_represent_areas() {
                    path.close();
                }
                outline.push(to_path("signal", polygon.line_width(), &path, true).into());
                outline.ensure_line_break();
            } else if (polygon.layer() == Layer::BOARD_CUTOUTS)
                || (polygon.layer() == Layer::BOARD_PLATED_CUTOUTS)
            {
                root.ensure_line_break();
                root.push(
                    self.to_path_keepout(
                        &format!("{designator}:cutout:{}", polygon.uuid()),
                        polygon.path(),
                        &BTreeSet::new(),
                    )?
                    .into(),
                );
            }
        }
        for pad in self.ctx.device_pads(dev)? {
            let index = add_to_pad_stacks(fpt_pad_stacks, self.gen_library_pad_stack(&pad)?);
            root.ensure_line_break();
            let pin = root.append_list("pin");
            pin.push(SExpression::string(format!("pad-{index}")));
            pin.append_list("rotate").push(SExpression::token(
                pad.properties.rotation().to_deg_string(),
            ));
            pin.push(SExpression::string(
                pad.properties.uuid().to_string().replace('-', ""),
            ));
            pin.push(to_token(pad.properties.position().x));
            pin.push(to_token(pad.properties.position().y));
        }
        for hole in dev.footprint.holes() {
            root.ensure_line_break();
            root.push(
                self.to_hole_keepout(
                    &format!("{designator}:hole:{}", hole.uuid()),
                    hole.diameter(),
                    hole.path(),
                )
                .into(),
            );
        }
        for zone in dev.footprint.zones() {
            if zone.rules().contains(ZoneRules::NO_COPPER) {
                root.ensure_line_break();
                let mut layers = BTreeSet::new();
                if zone.layers().contains(ZoneLayers::TOP) {
                    layers.insert(Layer::TOP_COPPER);
                }
                if zone.layers().contains(ZoneLayers::INNER) {
                    let count = self.ctx.board.settings().inner_layer_count as usize;
                    layers.extend((1..=count).filter_map(Layer::inner_copper));
                }
                if zone.layers().contains(ZoneLayers::BOTTOM) {
                    layers.insert(Layer::BOT_COPPER);
                }
                root.push(
                    self.to_path_keepout(
                        &format!("{designator}:zone:{}", zone.uuid()),
                        zone.outline(),
                        &layers,
                    )?
                    .into(),
                );
            }
        }
        root.ensure_line_break();
        Ok(root)
    }

    /// Upstream `genLibraryPadStack()`.
    fn gen_library_pad_stack(&self, pad: &ContextPad<'_>) -> BoardExportResult<List> {
        let mut root = List::new("padstack");
        root.push(SExpression::string("pad-"));
        root.ensure_line_break();

        // Determine pad shape. The geometry on the solder layer is always the
        // full pad shape.
        let solder_layer = solder_layer(pad);
        let shapes: Vec<PadGeometry> = pad.geometries_on(solder_layer).to_vec();
        let mut geometries: Vec<(Layer, Vec<PadGeometry>)> = Vec::new();
        if pad.properties.is_tht() {
            // Always use full THT pad annular rings because automatic annulars
            // depend on whether a trace is connected or not. But connections
            // might be made in an external software, so we don't know which
            // pads will be connected. It's not a nice solution, but safer
            // than exporting too small annular rings.
            let mut layers: Vec<Layer> = self.ctx.board.copper_layers().into_iter().collect();
            // Sort by layer for reproducibility.
            layers.sort_by_key(|l| l.copper_number());
            geometries.extend(layers.into_iter().map(|l| (l, shapes.clone())));
        } else {
            // The image is defined for the front side, so map the layer back
            // (upstream `Transform(pad).map(solderLayer)`).
            let layer = if pad.mirrored {
                solder_layer.mirrored(None)
            } else {
                solder_layer
            };
            geometries.push((layer, shapes));
        }

        // Convert pad geometries to Specctra.
        for (layer, shapes) in &geometries {
            let layer = layer.id();
            for geometry in shapes {
                let s = geometry.shape();
                let w = geometry.width();
                let h = geometry.height();
                let r = *geometry.corner_radius();
                let rounded = matches!(
                    s,
                    PadGeometryShape::RoundedRect | PadGeometryShape::RoundedOctagon
                );
                let zero = Length::ZERO;
                let vertex_count = geometry.path().vertices().len();
                if (s == PadGeometryShape::RoundedRect) && (w > zero) && (h > zero) && (r == zero) {
                    // Rectangular pad.
                    root.ensure_line_break();
                    let child = root.append_list("shape").append_list("rect");
                    child.push(SExpression::token(layer));
                    child.push(to_token(-w / 2));
                    child.push(to_token(-h / 2));
                    child.push(to_token(w / 2));
                    child.push(to_token(h / 2));
                } else if (rounded && (w == h) && (w > zero) && (r >= (w / 2)))
                    || ((s == PadGeometryShape::Stroke) && (vertex_count == 1) && (w > zero))
                {
                    // Circular pad.
                    root.ensure_line_break();
                    root.append_list("shape")
                        .push(to_circle(layer, positive(w)?, Point::ORIGIN).into());
                } else if rounded && (w > h) && (h > zero) && (r >= (h / 2)) {
                    // Oblong pad (horizontal).
                    root.ensure_line_break();
                    let path = Path::line(
                        Point::new(-(w - h) / 2, zero),
                        Point::new((w - h) / 2, zero),
                        Angle::DEG0,
                    );
                    root.append_list("shape")
                        .push(to_path(layer, unsigned(h)?, &path, false).into());
                } else if rounded && (h > w) && (w > zero) && (r >= (w / 2)) {
                    // Oblong pad (vertical). Upstream uses the same point
                    // twice (kept for identical output).
                    root.ensure_line_break();
                    let path = Path::line(
                        Point::new(zero, (h - w) / 2),
                        Point::new(zero, (h - w) / 2),
                        Angle::DEG0,
                    );
                    root.append_list("shape")
                        .push(to_path(layer, unsigned(w)?, &path, false).into());
                } else if (s == PadGeometryShape::Stroke) && (vertex_count > 1) && (w > zero) {
                    // Circular stroke pad.
                    root.ensure_line_break();
                    root.append_list("shape")
                        .push(to_path(layer, unsigned(w)?, geometry.path(), false).into());
                } else {
                    // Fallback: Arbitrary pads as polygons.
                    for p in geometry.to_outlines()? {
                        root.ensure_line_break();
                        root.append_list("shape")
                            .push(to_polygon(layer, UnsignedLength::ZERO, &p, true).into());
                    }
                }
            }
        }
        root.ensure_line_break();
        root.append_list("attach").push(SExpression::token("off"));
        root.ensure_line_break();
        Ok(root)
    }

    fn gen_network(&self) -> BoardExportResult<List> {
        let ctx = &self.ctx;
        let project = ctx.project;
        let circuit = project.circuit();
        let mut root = List::new("network");

        let mut add_net = |name: &str, pads: &[String]| {
            root.ensure_line_break();
            let net = root.append_list("net");
            net.push(SExpression::string(name));
            if !pads.is_empty() {
                net.ensure_line_break();
                let pins = net.append_list("pins");
                for pad in pads {
                    pins.ensure_line_break();
                    pins.push(SExpression::token(pad.clone()));
                }
                pins.ensure_line_break();
                net.ensure_line_break();
            }
        };

        // The footprint pads per component signal.
        let mut device_pads = Vec::new();
        for dev in &ctx.devices {
            for pad in dev.device.pads(project.library(), circuit)? {
                if let Some(signal) = pad.component_signal() {
                    device_pads.push((signal, pad.uuid(), dev.designator()));
                }
            }
        }
        device_pads.sort();

        for (net_id, net) in circuit.net_signals() {
            let mut pads = Vec::new();
            for (component_id, component) in circuit.component_instances() {
                for (signal, instance) in component.signals() {
                    if instance.net() != Some(*net_id) {
                        continue;
                    }
                    for (sig, pad, designator) in &device_pads {
                        if (sig.component == *component_id) && (sig.signal == *signal) {
                            pads.push(format!("{designator}-{}", pad.to_string().replace('-', "")));
                        }
                    }
                }
            }
            for segment in ctx.board.net_segments().values() {
                if segment.net() == Some(*net_id) {
                    for pad in segment.pads().values() {
                        pads.push(format!(
                            "BOARD-{}",
                            format!("{}:{}", segment.uuid(), pad.uuid()).replace('-', "")
                        ));
                    }
                }
            }
            add_net(net.name().as_str(), &pads);
        }

        // For net segments without a net, add a separate dummy net for each
        // of them.
        for segment in ctx.board.net_segments().values() {
            if segment.net().is_none() {
                let pads: Vec<String> = segment
                    .pads()
                    .values()
                    .map(|pad| {
                        format!(
                            "BOARD-{}",
                            format!("{}:{}", segment.uuid(), pad.uuid()).replace('-', "")
                        )
                    })
                    .collect();
                add_net(&segment_net_name(project, segment), &pads);
            }
        }

        root.ensure_line_break();
        Ok(root)
    }

    fn gen_wiring(&self) -> BoardExportResult<List> {
        let ctx = &self.ctx;
        // Not sure if required, but let's export all wires first, then all
        // vias.
        let mut root = List::new("wiring");
        for segment in ctx.board.net_segments().values() {
            for trace in segment.traces().values() {
                let pos = |anchor| {
                    ctx.anchor_position(segment, anchor)
                        .ok_or(BoardExportError::Logic("trace anchor not found"))
                };
                root.ensure_line_break();
                let wire = root.append_list("wire");
                let path = Path::line(pos(trace.p1())?, pos(trace.p2())?, Angle::DEG0);
                wire.push(
                    to_path(
                        trace.layer().id(),
                        UnsignedLength::from(trace.width()),
                        &path,
                        false,
                    )
                    .into(),
                );
                wire.append_list("net")
                    .push(SExpression::string(segment_net_name(ctx.project, segment)));
                wire.append_list("type").push(SExpression::token("route"));
            }
        }
        for segment in ctx.board.net_segments().values() {
            for via in ctx.segment_vias(segment) {
                root.ensure_line_break();
                let node = root.append_list("via");
                node.push(SExpression::string(via_data_padstack_id(
                    &ViaData::from_via(&via),
                )));
                node.push(to_token(via.via.position().x));
                node.push(to_token(via.via.position().y));
                node.append_list("net")
                    .push(SExpression::string(segment_net_name(ctx.project, segment)));
                node.append_list("type").push(SExpression::token("route"));
            }
        }
        root.ensure_line_break();
        Ok(root)
    }

    /// Upstream `genWiringPadStack()`.
    fn gen_wiring_pad_stack(&self, via: &ViaData) -> List {
        let mut root = List::new("padstack");
        root.push(SExpression::string(via_data_padstack_id(via)));
        let mut layers: Vec<Layer> = self.ctx.board.copper_layers().into_iter().collect();
        layers.sort_by_key(|l| l.copper_number());
        for layer in layers {
            if crate::geometry::Via::is_on_layer_between(layer, via.start_layer, via.end_layer) {
                root.ensure_line_break();
                root.append_list("shape")
                    .push(to_circle(layer.id(), via.size, Point::ORIGIN).into());
            }
        }
        root.ensure_line_break();
        root.append_list("attach").push(SExpression::token("off"));
        root.ensure_line_break();
        root
    }

    /// Keepout of a hole (upstream `toKeepout(id, hole)`).
    fn to_hole_keepout(&self, id: &str, diameter: PositiveLength, path: &NonEmptyPath) -> List {
        let drc = &self.ctx.board.settings().drc_settings;
        let mut root = List::new("keepout");
        root.push(SExpression::string(id));
        root.ensure_line_break();
        let width = PositiveLength::new(*diameter + (*drc.min_copper_npth_clearance() * 2))
            .unwrap_or(diameter);
        // Upstream `Hole::isSlot()`.
        if path.vertices().len() > 1 {
            root.push(to_path("signal", UnsignedLength::from(width), path, true).into());
        } else {
            let pos = path.vertices().first().map_or(Point::ORIGIN, |v| v.pos);
            root.push(to_circle("signal", width, pos).into());
        }
        root.ensure_line_break();
        root
    }

    /// Keepout of an area (upstream `toKeepout(id, path, layers)`).
    fn to_path_keepout(
        &self,
        id: &str,
        path: &Path,
        layers: &BTreeSet<Layer>,
    ) -> BoardExportResult<List> {
        let drc = &self.ctx.board.settings().drc_settings;
        let offset = *drc.min_copper_board_clearance() + MAX_ARC_TOLERANCE;
        let mut paths = vec![clipper_helpers::path_to_clipper(path, max_arc_tolerance())];
        clipper_helpers::offset(
            &mut paths,
            offset,
            max_arc_tolerance(),
            clipper::JoinType::Round,
        )?;
        if paths.len() != 1 {
            return Err(BoardExportError::Logic(
                "keepout offset did not result in exactly one path",
            ));
        }

        // Sort layers for reproducibility; if all layers are selected, use a
        // simplified version.
        let mut layers_sorted: Vec<Option<Layer>> = if *layers == self.ctx.board.copper_layers() {
            Vec::new()
        } else {
            let mut v: Vec<Layer> = layers.iter().copied().collect();
            v.sort_by_key(|l| l.copper_number());
            v.into_iter().map(Some).collect()
        };
        if layers_sorted.is_empty() {
            layers_sorted.push(None); // All layers.
        }

        let outline = clipper_helpers::path_from_clipper(&paths[0]);
        let mut root = List::new("keepout");
        root.push(SExpression::string(id));
        for layer in layers_sorted {
            root.ensure_line_break();
            root.push(
                to_polygon(
                    layer.map_or("signal", Layer::id),
                    UnsignedLength::ZERO,
                    &outline,
                    true,
                )
                .into(),
            );
        }
        root.ensure_line_break();
        Ok(root)
    }
}

/// Upstream `BI_Pad::getSolderLayer()`.
fn solder_layer(pad: &ContextPad<'_>) -> Layer {
    super::pad_data::PadOnBoard {
        pad: pad.properties,
        mirrored: pad.mirrored,
    }
    .solder_layer()
}

/// The image ID of a device: `<designator>:<package name>`.
fn image_id(dev: &super::board_context::ContextDevice<'_>) -> String {
    format!(
        "{}:{}",
        dev.designator(),
        dev.package.metadata().names().default_value()
    )
}

fn gen_resolution() -> List {
    let mut root = List::new("resolution");
    root.push(SExpression::token("mm"));
    root.push(SExpression::token("1000000"));
    root
}

fn to_polygon(layer: &str, width: UnsignedLength, path: &Path, multiline: bool) -> List {
    let mut root = to_path(layer, width, path, multiline);
    root.set_name("polygon");
    root
}

fn to_path(layer: &str, width: UnsignedLength, path: &Path, multiline: bool) -> List {
    let mut root = List::new("path");
    root.push(SExpression::token(layer));
    root.push(to_token(*width));
    let flattened = path.flattened_arcs(max_arc_tolerance());
    for vertex in flattened.vertices() {
        if multiline {
            root.ensure_line_break();
        }
        root.push(to_token(vertex.pos.x));
        root.push(to_token(vertex.pos.y));
    }
    if multiline {
        root.ensure_line_break();
    }
    root
}

fn to_circle(layer: &str, diameter: PositiveLength, pos: Point) -> List {
    let mut root = List::new("circle");
    root.push(SExpression::token(layer));
    root.push(to_token(*diameter));
    if !pos.is_origin() {
        root.push(to_token(pos.x));
        root.push(to_token(pos.y));
    }
    root
}

fn to_token(length: Length) -> SExpression {
    SExpression::token(length.to_mm_string())
}

fn positive(length: Length) -> BoardExportResult<PositiveLength> {
    PositiveLength::new(length).map_err(|_| BoardExportError::Logic("length not positive"))
}

fn unsigned(length: Length) -> BoardExportResult<UnsignedLength> {
    UnsignedLength::new(length).map_err(|_| BoardExportError::Logic("length negative"))
}

/// The name (first child) of a pad stack.
fn pad_stack_name(pad_stack: &List) -> String {
    pad_stack
        .children()
        .first()
        .and_then(|c| c.value().ok())
        .unwrap_or_default()
        .to_owned()
}

fn compare_pad_stacks(a: &List, b: &List) -> Ordering {
    toolbox::compare_numeric(&pad_stack_name(a), &pad_stack_name(b))
}

/// Adds a pad stack if no equal one exists; returns its index (upstream
/// `addToPadStacks()`).
fn add_to_pad_stacks(pad_stacks: &mut Vec<List>, pad_stack: List) -> usize {
    if let Some(i) = pad_stacks.iter().position(|p| *p == pad_stack) {
        return i;
    }
    pad_stacks.push(pad_stack);
    pad_stacks.len() - 1
}
