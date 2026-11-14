//! Port of Board::buildScene3D() and SceneData3D::preprocess().
use super::{
    BoardPhysicalGeometry, BoardSurfaceGeometry, PhysicalDevice, PhysicalHole, PhysicalHoleDepth,
};
use crate::{
    geometry::{Circle, NonEmptyPath, PadGeometry, Path, Polygon, TraceAnchor, Vertex},
    project::{
        AssemblyVariantId, Project, Result,
        board::{Board, BoardDevice, BoardStrokeTextData, ResolvedDevice, ViaView},
    },
    types::{Angle, Layer, Length, Point, PositiveLength},
    utils::transform::Transform,
};
use std::collections::BTreeMap;

impl BoardPhysicalGeometry {
    /// Extracts from resolved model views without modifying the project,
    /// reading STEP resources or retaining filesystem/model handles.
    /// Plane fragments must be current; exporters rebuild them beforehand.
    /// As upstream, `None` omits component models but keeps their PCB geometry.
    pub fn from_project(
        project: &Project,
        board: &Board,
        variant: Option<AssemblyVariantId>,
    ) -> Result<Self> {
        let settings = board.settings();
        let mut result = Self {
            board: board.id(),
            project_revision: project.revision(),
            thickness: settings.pcb_thickness,
            inner_layer_count: settings.inner_layer_count,
            solder_resist: settings.solder_resist,
            silkscreen: settings.silkscreen_color,
            outlines: Vec::new(),
            cutouts: Vec::new(),
            holes: Vec::new(),
            devices: Vec::new(),
            areas: BTreeMap::new(),
            surfaces: std::array::from_fn(|_| BoardSurfaceGeometry::default()),
            silkscreen_layers: [
                settings.silkscreen_layers_top.iter().copied().collect(),
                settings.silkscreen_layers_bot.iter().copied().collect(),
            ],
        };
        let copper = board.copper_layers();
        let rules = board.design_rules();
        for device in board.devices().values() {
            let resolved = ResolvedDevice::resolve(device, project.library())?;
            let footprint = resolved.footprint;
            let transform = device.transform();
            let component = project
                .circuit()
                .component_instance(device.component())
                .ok_or(crate::project::Error::InexistentComponent(
                    device.component().0,
                ))?;
            if let (Some(variant), Some(model)) = (variant, device.lib_model())
                && !device.parts(component, Some(variant)).is_empty()
                && resolved.package.models().by_uuid(&model).is_some()
            {
                result.devices.push(PhysicalDevice {
                    component: device.component(),
                    name: component.name().to_string(),
                    package: resolved.device.package_uuid(),
                    model,
                    transform,
                    model_position: footprint.model_position(),
                    model_rotation: footprint.model_rotation(),
                });
            }
            for pad in device.pads(project.library(), project.circuit())? {
                result.add_pad(
                    pad.geometries(
                        &copper,
                        rules,
                        &board.anchor_trace_layers(pad.trace_anchor()),
                    ),
                    &Transform::new(pad.position(), pad.rotation(), pad.mirrored()),
                )?;
            }
            for text in device.stroke_texts().values() {
                result.add_text(project, board, Some(device), text)?;
            }
            for polygon in footprint.polygons().iter() {
                result.add_polygon(polygon, &transform);
            }
            for circle in footprint.circles().iter() {
                result.add_circle(circle, &transform);
            }
            let masks = BoardDevice::hole_stop_mask_offsets(footprint, rules);
            for hole in footprint.holes().iter() {
                let path = transform.map(hole.path());
                result.add_hole(
                    path.get().clone(),
                    hole.diameter(),
                    false,
                    false,
                    PhysicalHoleDepth::Through,
                )?;
                result.add_hole_mask(
                    &path,
                    hole.diameter(),
                    masks.get(&hole.uuid()).copied().flatten(),
                )?;
            }
        }
        let identity = Transform::new(Point::ORIGIN, Angle::DEG0, false);
        for (id, plane) in board.planes() {
            for fragment in board
                .derived()
                .plane_fragments()
                .get(id)
                .into_iter()
                .flatten()
            {
                result.add_area(plane.layer(), fragment.clone());
            }
        }
        for polygon in board.polygons().values() {
            result.add_polygon(
                &Polygon::new(
                    polygon.uuid(),
                    polygon.layer(),
                    polygon.line_width(),
                    polygon.is_filled(),
                    polygon.is_grab_area(),
                    polygon.path().clone(),
                ),
                &identity,
            );
        }
        for text in board.stroke_texts().values() {
            result.add_text(project, board, None, text)?;
        }
        for hole in board.holes().values() {
            result.add_hole(
                hole.path().get().clone(),
                hole.diameter(),
                false,
                false,
                PhysicalHoleDepth::Through,
            )?;
            result.add_hole_mask(hole.path(), hole.diameter(), hole.stop_mask_offset(rules))?;
        }
        for segment in board.net_segments().values() {
            for pad in segment.pads().values() {
                let properties = pad.pad();
                result.add_pad(
                    pad.geometries(
                        &copper,
                        rules,
                        &board.anchor_trace_layers(TraceAnchor::Pad(properties.uuid())),
                    ),
                    &Transform::new(properties.position(), properties.rotation(), false),
                )?;
            }
            for via in segment.vias().values() {
                let view = ViaView::new(via, board, segment.net(), project.circuit())?;
                let position = via.position();
                for (layer, on_face) in [
                    (Layer::TOP_COPPER, via.start_layer().is_top()),
                    (Layer::BOT_COPPER, via.end_layer().is_bottom()),
                ] {
                    if on_face {
                        result.add_area(layer, Path::circle(view.size()).translated(position));
                    }
                }
                for (layer, diameter) in [
                    (Layer::TOP_STOP_MASK, view.stop_mask_diameter_top()),
                    (Layer::BOT_STOP_MASK, view.stop_mask_diameter_bottom()),
                ] {
                    if let Some(diameter) = diameter {
                        result.add_area(layer, Path::circle(diameter).translated(position));
                    }
                }
                let depth = if via.start_layer().is_top() && via.end_layer().is_bottom() {
                    PhysicalHoleDepth::Through
                } else {
                    PhysicalHoleDepth::Layers {
                        start: via.start_layer(),
                        end: via.end_layer(),
                    }
                };
                result.add_hole(
                    Path::new(vec![Vertex::at(position)]),
                    view.drill_diameter(),
                    true,
                    true,
                    depth,
                )?;
                // The flat source uses the requested via span; 3D consumers
                // need its actual manufacturing depth on enabled layers.
                if let Some(hole) = result.holes.last_mut() {
                    hole.drill_layer_span = view.drill_layer_span();
                }
            }
            for trace in segment.traces().values() {
                let position = |anchor| {
                    board
                        .anchor_position(segment, anchor, project.library(), project.circuit())
                        .ok_or(crate::project::Error::InexistentTraceAnchor {
                            anchor,
                            device_missing: false,
                        })
                };
                let path = Path::new(vec![
                    Vertex::at(position(trace.p1())?),
                    Vertex::at(position(trace.p2())?),
                ]);
                result.add_strokes(trace.layer(), &[path], *trace.width(), &identity);
            }
        }
        result.holes.sort_unstable();
        result.holes.dedup();
        result.outlines = result
            .areas
            .get(&Layer::BOARD_OUTLINES)
            .cloned()
            .unwrap_or_default();
        for layer in [Layer::BOARD_CUTOUTS, Layer::BOARD_PLATED_CUTOUTS] {
            result
                .cutouts
                .extend(result.areas.get(&layer).into_iter().flatten().cloned());
        }
        result.prepare_surfaces()?;
        Ok(result)
    }
    fn needed(&self, layer: Layer) -> bool {
        matches!(
            layer,
            Layer::BOARD_OUTLINES
                | Layer::BOARD_CUTOUTS
                | Layer::BOARD_PLATED_CUTOUTS
                | Layer::TOP_COPPER
                | Layer::BOT_COPPER
                | Layer::TOP_STOP_MASK
                | Layer::BOT_STOP_MASK
                | Layer::TOP_SOLDER_PASTE
                | Layer::BOT_SOLDER_PASTE
        ) || self
            .silkscreen_layers
            .iter()
            .any(|layers| layers.contains(&layer))
    }
    fn add_area(&mut self, layer: Layer, path: Path) {
        if self.needed(layer) && !path.vertices().is_empty() {
            self.areas.entry(layer).or_default().push(path);
        }
    }
    fn add_strokes(&mut self, layer: Layer, paths: &[Path], width: Length, transform: &Transform) {
        if !self.needed(layer) {
            return;
        }
        if let Ok(width) = PositiveLength::new(width) {
            for path in paths {
                for outline in transform.map(path).to_outline_strokes(width) {
                    self.add_area(layer, outline);
                }
            }
        }
    }
    fn add_polygon(&mut self, polygon: &Polygon, transform: &Transform) {
        let layer = transform.map(&polygon.layer());
        let path = transform.map(polygon.path());
        if !layer.is_board_edge() {
            self.add_strokes(
                layer,
                std::slice::from_ref(&path),
                *polygon.line_width(),
                &Transform::new(Point::ORIGIN, Angle::DEG0, false),
            );
        }
        if (layer.is_board_edge() || polygon.is_filled()) && path.is_closed() {
            self.add_area(layer, path);
        }
    }
    fn add_circle(&mut self, circle: &Circle, transform: &Transform) {
        let layer = transform.map(&circle.layer());
        let path = Path::circle(circle.diameter()).translated(transform.map(&circle.center()));
        if !layer.is_board_edge() {
            self.add_strokes(
                layer,
                std::slice::from_ref(&path),
                *circle.line_width(),
                &Transform::new(Point::ORIGIN, Angle::DEG0, false),
            );
        }
        if layer.is_board_edge() || circle.is_filled() {
            self.add_area(layer, path);
        }
    }
    fn add_text(
        &mut self,
        project: &Project,
        board: &Board,
        device: Option<&BoardDevice>,
        text: &BoardStrokeTextData,
    ) -> Result<()> {
        if self.needed(text.layer()) {
            self.add_strokes(
                text.layer(),
                &text.paths(project, board, device)?,
                *text.stroke_width(),
                &text.transform(),
            );
        }
        Ok(())
    }
    fn add_pad(
        &mut self,
        geometries: BTreeMap<Layer, Vec<PadGeometry>>,
        transform: &Transform,
    ) -> Result<()> {
        for (layer, geometries) in geometries {
            for geometry in geometries {
                for path in geometry.to_outlines()? {
                    self.add_area(layer, transform.map(&path));
                }
                for hole in geometry.holes().iter() {
                    self.add_hole(
                        transform.map(hole.path()),
                        hole.diameter(),
                        true,
                        false,
                        PhysicalHoleDepth::Through,
                    )?;
                }
            }
        }
        Ok(())
    }
    fn add_hole(
        &mut self,
        path: Path,
        diameter: PositiveLength,
        plated: bool,
        via: bool,
        depth: PhysicalHoleDepth,
    ) -> Result<()> {
        self.holes.push(PhysicalHole {
            path: NonEmptyPath::new(path)?,
            diameter,
            plated,
            via,
            depth,
            drill_layer_span: Some((Layer::TOP_COPPER, Layer::BOT_COPPER)),
        });
        Ok(())
    }
    fn add_hole_mask(
        &mut self,
        path: &Path,
        diameter: PositiveLength,
        offset: Option<Length>,
    ) -> Result<()> {
        if let Some(offset) = offset {
            // Widen before addition: invalid/extreme input must not panic.
            let width = i128::from(diameter.to_nm()) + 2 * i128::from(offset.to_nm());
            let width =
                i64::try_from(width)
                    .map(Length::new)
                    .map_err(|_| crate::types::Error::Range {
                        value: width as f64,
                        min: i64::MIN,
                        max: i64::MAX,
                    })?;
            for layer in [Layer::TOP_STOP_MASK, Layer::BOT_STOP_MASK] {
                self.add_strokes(
                    layer,
                    std::slice::from_ref(path),
                    width,
                    &Transform::new(Point::ORIGIN, Angle::DEG0, false),
                );
            }
        }
        Ok(())
    }
}
