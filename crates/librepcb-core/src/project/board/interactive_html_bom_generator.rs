//! Port of libs/librepcb/core/project/board/boardinteractivehtmlbomgenerator.{h,cpp}.
//!
//! Also contains the port of upstream `Board::calculateBoundingRect()`,
//! which only this generator uses so far.
//!
//! Differences to upstream: [`BoardInteractiveHtmlBomGenerator::new()`]
//! fails if the board, the assembly variant or a library element does not
//! exist (upstream gets valid objects); the generation date is passed in as
//! [`Timestamp`].

use std::collections::BTreeMap;

use super::board_context::{BoardContext, ContextDevice};
use super::export_error::BoardExportResult;
use crate::export::{
    InteractiveHtmlBom, InteractiveHtmlBomDrawingKind, InteractiveHtmlBomDrawingLayer,
    InteractiveHtmlBomLayer, InteractiveHtmlBomPad, InteractiveHtmlBomSides, Timestamp,
};
use crate::geometry::Path;
use crate::project::Project;
use crate::project::circuit::AssemblyVariant;
use crate::project::error::{EntityKind, Error};
use crate::project::id::{AssemblyVariantId, BoardId};
use crate::types::{Layer, Length, Point, PositiveLength, UnsignedLength};
use crate::utils::toolbox::compare_numeric;

/// A mounted part of the BOM (upstream `BomItem`).
#[derive(Debug, Clone)]
struct BomItem {
    designator: String,
    footprint_id: usize,
    /// Top or bottom (not both).
    side: InteractiveHtmlBomSides,
}

/// Generates the interactive HTML BOM of a board for an assembly variant
/// (upstream `BoardInteractiveHtmlBomGenerator`).
#[derive(Debug, Clone)]
pub struct BoardInteractiveHtmlBomGenerator<'a> {
    ctx: BoardContext<'a>,
    assembly_variant: &'a AssemblyVariant,
    custom_attributes: Vec<String>,
    component_order: Vec<String>,
}

impl<'a> BoardInteractiveHtmlBomGenerator<'a> {
    /// Creates a generator for a board and assembly variant of a project.
    ///
    /// Fails if the board, the assembly variant or a library element used
    /// by the board does not exist.
    pub fn new(
        project: &'a Project,
        board: BoardId,
        assembly_variant: AssemblyVariantId,
    ) -> BoardExportResult<Self> {
        let b = project.board(board).ok_or(Error::NotFound {
            kind: EntityKind::Board,
            uuid: board.0,
        })?;
        let variant =
            project
                .circuit()
                .assembly_variant(assembly_variant)
                .ok_or(Error::NotFound {
                    kind: EntityKind::AssemblyVariant,
                    uuid: assembly_variant.0,
                })?;
        Ok(Self {
            ctx: BoardContext::new(project, b)?,
            assembly_variant: variant,
            custom_attributes: Vec::new(),
            component_order: Vec::new(),
        })
    }

    /// Sets the attributes exported as additional BOM columns.
    pub fn set_custom_attributes(&mut self, attributes: Vec<String>) {
        self.custom_attributes = attributes;
    }

    /// Sets the designator prefixes defining the order of the BOM rows.
    pub fn set_component_order(&mut self, order: Vec<String>) {
        self.component_order = order;
    }

    /// Generates the BOM (upstream `generate()`).
    pub fn generate(&self, date: Timestamp) -> BoardExportResult<InteractiveHtmlBom> {
        let ctx = &self.ctx;
        let project = ctx.project;
        let board = ctx.board;
        let variant_id = AssemblyVariantId(self.assembly_variant.uuid());

        // Create IBOM.
        let mut name = project.metadata().name.to_string();
        if project.circuit().assembly_variants().len() > 1 {
            name.push_str(&format!(" ({})", self.assembly_variant.name()));
        }
        let bbox = ctx.bounding_rect();
        let mut ibom = InteractiveHtmlBom::new(
            &name,
            &project.metadata().author,
            project.metadata().version.as_ref(),
            &date.date_time().format("%Y-%m-%d %H:%M:%S").to_string(),
            bbox.map_or(Point::ORIGIN, |b| b.0),
            bbox.map_or(Point::ORIGIN, |b| b.1),
        );

        // Set configuration.
        let mut fields = vec!["Value / MPN".to_owned(), "Package".to_owned()];
        fields.extend(self.custom_attributes.iter().cloned());
        ibom.set_fields(fields);

        // Add board drawings.
        let settings = board.settings();
        let copper_layer = |layer: Layer| match layer {
            Layer::TOP_COPPER => Some(InteractiveHtmlBomLayer::Top),
            Layer::BOT_COPPER => Some(InteractiveHtmlBomLayer::Bottom),
            _ => None,
        };
        let drawing_kind = |layer: Layer| match layer {
            Layer::TOP_NAMES | Layer::BOT_NAMES => InteractiveHtmlBomDrawingKind::ReferenceText,
            Layer::TOP_VALUES | Layer::BOT_VALUES => InteractiveHtmlBomDrawingKind::ValueText,
            _ => InteractiveHtmlBomDrawingKind::Polygon,
        };
        // Silkscreen layers override the fixed mapping, bottom last.
        let drawing_layer = |layer: Layer| {
            if settings.silkscreen_layers_bot.contains(&layer) {
                Some(InteractiveHtmlBomDrawingLayer::SilkscreenBack)
            } else if settings.silkscreen_layers_top.contains(&layer) {
                Some(InteractiveHtmlBomDrawingLayer::SilkscreenFront)
            } else {
                match layer {
                    Layer::BOARD_OUTLINES | Layer::BOARD_CUTOUTS | Layer::BOARD_PLATED_CUTOUTS => {
                        Some(InteractiveHtmlBomDrawingLayer::Edge)
                    }
                    Layer::TOP_DOCUMENTATION => {
                        Some(InteractiveHtmlBomDrawingLayer::FabricationFront)
                    }
                    Layer::BOT_DOCUMENTATION => {
                        Some(InteractiveHtmlBomDrawingLayer::FabricationBack)
                    }
                    _ => None,
                }
            }
        };
        let add_drawing = |ibom: &mut InteractiveHtmlBom,
                           layer: Layer,
                           path: &Path,
                           mut width: UnsignedLength,
                           filled: bool| {
            if let Some(drawing_layer) = drawing_layer(layer) {
                if drawing_layer == InteractiveHtmlBomDrawingLayer::Edge {
                    width = UnsignedLength::new(Length::new(100_000))
                        .expect("constant is not negative");
                }
                ibom.add_drawing(drawing_kind(layer), drawing_layer, path, width, filled);
            } else if let Some(copper) = copper_layer(layer)
                && let Ok(width) = PositiveLength::new(*width)
            {
                for w in path.vertices().windows(2) {
                    // Note: Arcs not handled yet as we don't use them yet...
                    ibom.add_track(copper, w[0].pos, w[1].pos, width, None);
                }
            }
        };
        let add_hole_drawings = |ibom: &mut InteractiveHtmlBom, paths: Vec<Path>| {
            for p in paths {
                add_drawing(ibom, Layer::BOARD_CUTOUTS, &p, UnsignedLength::ZERO, false);
            }
        };
        for polygon in board.polygons().values() {
            add_drawing(
                &mut ibom,
                polygon.layer(),
                polygon.path(),
                polygon.line_width(),
                polygon.is_filled(),
            );
        }
        for text in board.stroke_texts().values() {
            let transform = BoardContext::stroke_text_transform(text);
            for p in ctx.stroke_text_paths(text, None)? {
                add_drawing(
                    &mut ibom,
                    text.layer(),
                    &transform.map(&p),
                    text.stroke_width(),
                    false,
                );
            }
        }
        for hole in board.holes().values() {
            add_hole_drawings(&mut ibom, hole.path().to_outline_strokes(hole.diameter()));
        }
        // Currently we do not directly support drawing pads which are not part
        // of a footprint. Also I think we cannot draw copper for documentation
        // purpose only? Let's just draw the pad hole for now, and improve it
        // when necessary.
        for segment in board.net_segments().values() {
            for pad in ctx.segment_pads(segment) {
                let transform = pad.transform();
                for hole in pad.properties.holes().iter() {
                    add_hole_drawings(
                        &mut ibom,
                        transform.map(&hole.path().to_outline_strokes(hole.diameter())),
                    );
                }
            }
        }

        // Add tracks & vias.
        for segment in board.net_segments().values() {
            let net_name = ctx.net_name(segment.net());
            for trace in segment.traces().values() {
                if let Some(layer) = copper_layer(trace.layer()) {
                    let (Some(p1), Some(p2)) = (
                        ctx.anchor_position(segment, trace.p1()),
                        ctx.anchor_position(segment, trace.p2()),
                    ) else {
                        continue; // Invalid anchor, rejected by the mutations.
                    };
                    ibom.add_track(layer, p1, p2, trace.width(), net_name);
                }
            }
            for via in ctx.segment_vias(segment) {
                let mut layers = Vec::new();
                if via.via.is_on_layer(Layer::TOP_COPPER) {
                    layers.push(InteractiveHtmlBomLayer::Top);
                }
                if via.via.is_on_layer(Layer::BOT_COPPER) {
                    layers.push(InteractiveHtmlBomLayer::Bottom);
                }
                ibom.add_via(
                    &layers,
                    via.via.position(),
                    via.props.size,
                    via.props.drill_diameter,
                    net_name,
                );
            }
        }

        // Add planes.
        for plane in board.planes().values() {
            let net_name = ctx.net_name(plane.net());
            if let Some(layer) = copper_layer(plane.layer()) {
                for fragment in board.derived().fragments_of(plane.id()) {
                    ibom.add_plane_fragment(layer, fragment, net_name);
                }
            }
        }

        // Add footprints and their drawings.
        let mut bom_items: BTreeMap<Vec<String>, Vec<BomItem>> = BTreeMap::new();
        for dev in &ctx.devices {
            let transform = dev.transform();
            let (bbox_first, bbox_second) = dev.footprint.calculate_bounding_rect(true);
            let context_pads = ctx.device_pads(dev)?;
            let pad_count = context_pads.len();
            let pads: Vec<InteractiveHtmlBomPad> = context_pads
                .iter()
                .map(|p| InteractiveHtmlBomPad {
                    on_top: p.is_on_layer(Layer::TOP_COPPER),
                    on_bottom: p.is_on_layer(Layer::BOT_COPPER),
                    position: p.position,
                    rotation: p.rotation,
                    mirror_geometry: p.mirrored,
                    geometries: p.geometries_on(p.solder_layer()).to_vec(),
                    holes: p.properties.holes().iter().cloned().collect(),
                    net_name: ctx.net_name(p.net).map(str::to_owned),
                    pin1: (pad_count > 1) && matches!(p.package_pad_name, Some("1" | "A")),
                })
                .collect();
            let parts = dev.parts(Some(variant_id));
            let mount = !parts.is_empty();
            let fields = self.device_fields(dev, parts.first());
            let mirrored = dev.device.mirrored();
            let id = ibom.add_footprint(
                if mirrored {
                    InteractiveHtmlBomLayer::Bottom
                } else {
                    InteractiveHtmlBomLayer::Top
                },
                dev.device.position(),
                dev.device.rotation(),
                bbox_first,
                bbox_second,
                mount,
                &fields,
                &pads,
            )?;
            if mount {
                bom_items.entry(fields).or_default().push(BomItem {
                    designator: dev.designator().to_owned(),
                    footprint_id: id,
                    side: if mirrored {
                        InteractiveHtmlBomSides::Bottom
                    } else {
                        InteractiveHtmlBomSides::Top
                    },
                });
            }

            for polygon in dev.footprint.polygons().iter() {
                add_drawing(
                    &mut ibom,
                    transform.map(&polygon.layer()),
                    &transform.map(&polygon.path_for_rendering()),
                    polygon.line_width(),
                    polygon.is_filled(),
                );
            }
            for circle in dev.footprint.circles().iter() {
                add_drawing(
                    &mut ibom,
                    transform.map(&circle.layer()),
                    &transform.map(&Path::circle(circle.diameter()).translated(circle.center())),
                    circle.line_width(),
                    circle.is_filled(),
                );
            }
            for text in dev.device.stroke_texts().values() {
                let text_transform = BoardContext::stroke_text_transform(text);
                for p in ctx.stroke_text_paths(text, Some(dev))? {
                    add_drawing(
                        &mut ibom,
                        text.layer(),
                        &text_transform.map(&p),
                        text.stroke_width(),
                        false,
                    );
                }
            }
            for hole in dev.footprint.holes().iter() {
                add_hole_drawings(
                    &mut ibom,
                    transform.map(&hole.path().to_outline_strokes(hole.diameter())),
                );
            }
        }

        // Sort BOM items.
        let mut sorted_items: Vec<Vec<BomItem>> = bom_items
            .into_values()
            .filter(|items| !items.is_empty())
            .map(|mut items| {
                items.sort_by(|a, b| compare_numeric(&a.designator, &b.designator));
                items
            })
            .collect();
        sorted_items.sort_by(|lhs, rhs| {
            self.priority(lhs)
                .cmp(&self.priority(rhs))
                .then_with(|| compare_numeric(&lhs[0].designator, &rhs[0].designator))
        });

        // Add BOM rows.
        for sides in [
            InteractiveHtmlBomSides::Top,
            InteractiveHtmlBomSides::Bottom,
            InteractiveHtmlBomSides::Both,
        ] {
            for items in &sorted_items {
                let parts: Vec<(String, usize)> = items
                    .iter()
                    .filter(|item| item.side == sides || sides == InteractiveHtmlBomSides::Both)
                    .map(|item| (item.designator.clone(), item.footprint_id))
                    .collect();
                if !parts.is_empty() {
                    ibom.add_bom_row(sides, &parts);
                }
            }
        }

        Ok(ibom)
    }

    /// Returns the BOM field values of a device: value/MPN, package and the
    /// custom attributes.
    fn device_fields(
        &self,
        dev: &ContextDevice<'a>,
        part: Option<&crate::library::dev::Part>,
    ) -> Vec<String> {
        let attributes = self.ctx.device_lookup(dev, part);
        let lookup = |key: &str| attributes.value(key).unwrap_or_default();
        let mpn = lookup("MPN");
        let mut value_mpn = attributes.substitute(&lookup("VALUE"));
        if !value_mpn.contains(&mpn) {
            if value_mpn.trim().is_empty() {
                value_mpn = mpn;
            } else {
                value_mpn.push_str(&format!(" ({mpn})"));
            }
        }
        let mut fields = vec![
            value_mpn
                .split('\n')
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
            lookup("PACKAGE"),
        ];
        for attr in &self.custom_attributes {
            fields.push(attributes.substitute(&lookup(attr)));
        }
        fields
    }

    /// Returns the sort priority of a BOM row: the index of the first
    /// component order entry matching a designator prefix.
    fn priority(&self, items: &[BomItem]) -> usize {
        items
            .iter()
            .filter_map(|item| {
                let prefix = item.designator.trim_end_matches(char::is_numeric);
                self.component_order.iter().position(|p| p == prefix)
            })
            .min()
            .unwrap_or(self.component_order.len())
    }
}
