//! Render meshes from SceneData3D's canonical outlines, drill spans and regions.
//! Unlike mechanical STEP, physical viewing includes via drills. Booleans use
//! the shared 5 um tolerance; earcutr triangulates nested polygon holes.
use crate::{Error, Result};
use clipper::{
    IntPoint,
    PolyFillType::{EvenOdd, NonZero},
    PolyNode,
};
use librepcb_core::{
    export::BoardSide,
    fileio::FileSystem,
    project::{
        AssemblyVariantId, BoardId, Project,
        board::{BoardPhysicalGeometry, PhysicalHoleDepth},
    },
    types::{Color, Layer, Uuid},
    utils::clipper_helpers as ch,
};
use librepcb_step::{StepModel, TriangleSurface};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRole {
    Substrate,
    Copper,
    SolderResist,
    Silkscreen,
    SolderPaste,
    Device,
}

/// Expanded triangles: X/Y along the board, Z up, in millimetres.
#[derive(Debug, Clone)]
pub struct MeshPart {
    pub name: String,
    pub role: SurfaceRole,
    pub color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
}
impl MeshPart {
    fn new(role: SurfaceRole, color: [f32; 4]) -> Self {
        let metal = matches!(role, SurfaceRole::Copper | SurfaceRole::SolderPaste);
        Self {
            name: format!("{role:?}"),
            role,
            color,
            metallic: if metal { 0.75 } else { 0. },
            roughness: if metal { 0.32 } else { 0.55 },
            positions: Vec::new(),
            normals: Vec::new(),
        }
    }
    fn triangle(&mut self, p: [[f32; 3]; 3], normal: [f32; 3]) {
        self.positions.extend(p);
        self.normals.extend([normal; 3]);
    }
}

#[derive(Debug, Clone)]
pub struct BoardMesh {
    pub parts: Vec<MeshPart>,
    pub bounds: [[f32; 3]; 2],
    /// A malformed component must not hide the board and all other components.
    pub warnings: Vec<String>,
}
static_assertions::assert_impl_all!(BoardMesh: Send, Sync);

/// Owned input captured before dispatching CAD/meshing work to another thread.
pub struct BoardSnapshot {
    pub physical: BoardPhysicalGeometry,
    models: std::collections::BTreeMap<(Uuid, Uuid), std::result::Result<Vec<u8>, String>>,
}
static_assertions::assert_impl_all!(BoardSnapshot: Send, Sync);
impl BoardSnapshot {
    pub fn from_project(
        project: &Project,
        board: BoardId,
        variant: Option<AssemblyVariantId>,
    ) -> Result<Self> {
        let board = project
            .board(board)
            .ok_or_else(|| Error::Render("Board no longer exists".into()))?;
        let physical = BoardPhysicalGeometry::from_project(project, board, variant)?;
        let mut models = std::collections::BTreeMap::new();
        for device in &physical.devices {
            models
                .entry((device.package, device.model))
                .or_insert_with(|| {
                    project
                        .directory()
                        .read(&format!(
                            "library/pkg/{}/{}.step",
                            device.package, device.model
                        ))
                        .map_err(|e| e.to_string())
                });
        }
        Ok(Self { physical, models })
    }
}

const COPPER: [f32; 4] = [0.78, 0.56, 0.22, 1.];
// OpenGlSceneBuilder's board body color. Its neutral base also keeps the
// translucent solder resist from taking on an artificial yellow/brown tint.
const SUBSTRATE: [f32; 4] = [70. / 255., 80. / 255., 70. / 255., 1.];
const PASTE: [f32; 4] = [0.65, 0.67, 0.69, 1.];
fn rgba(color: Color) -> [f32; 4] {
    [color.r, color.g, color.b, color.a].map(|v| f32::from(v) / 255.)
}

impl BoardMesh {
    /// Read-only snapshot. The caller supplies rebuilt planes.
    pub fn from_project(
        project: &Project,
        board: BoardId,
        variant: Option<AssemblyVariantId>,
    ) -> Result<Self> {
        Self::from_snapshot(&BoardSnapshot::from_project(project, board, variant)?)
    }
    pub fn from_snapshot(snapshot: &BoardSnapshot) -> Result<Self> {
        let data = &snapshot.physical;
        let read = |package, model| {
            snapshot
                .models
                .get(&(package, model))
                .ok_or_else(|| librepcb_step::Error::Geometry("Missing component model".into()))?
                .as_ref()
                .cloned()
                .map_err(|e| librepcb_step::Error::Geometry(e.clone()))
        };
        let mut output = Self::from_physical(data)?;
        match StepModel::from_component_source(data, read)
            .and_then(|model| model.triangle_surfaces(0.02))
        {
            Ok(surfaces) => output.add_components(surfaces),
            Err(_) => {
                // Usually the assembly-wide import above shares cached package models.
                // Isolate failing devices only when that import cannot complete.
                let mut single = data.clone();
                for device in &data.devices {
                    let name = device.name.clone();
                    single.devices = vec![device.clone()];
                    match StepModel::from_component_source(&single, read)
                        .and_then(|model| model.triangle_surfaces(0.02))
                    {
                        Ok(surfaces) => output.add_components(surfaces),
                        Err(error) => output.warnings.push(format!("{name}: {error}")),
                    }
                }
            }
        }
        output.update_bounds();
        Ok(output)
    }
    fn add_components(&mut self, surfaces: Vec<TriangleSurface>) {
        self.parts.extend(surfaces.into_iter().map(|s| MeshPart {
            name: s.name,
            role: SurfaceRole::Device,
            color: [
                s.material.color[0] as f32,
                s.material.color[1] as f32,
                s.material.color[2] as f32,
                s.material.opacity as f32,
            ],
            metallic: 0.15,
            roughness: 0.45,
            positions: s.positions,
            normals: s.normals,
        }));
    }

    pub fn from_physical(data: &BoardPhysicalGeometry) -> Result<Self> {
        if data.outlines.is_empty() {
            return Err(Error::NoOutline);
        }
        let tol = BoardPhysicalGeometry::arc_tolerance();
        let outlines = ch::paths_to_clipper(&data.outlines, tol);
        let cutouts = ch::paths_to_clipper(&data.cutouts, tol);
        let thickness = data.thickness.to_mm();
        // LibrePCB does not store physical dielectric thicknesses between layers.
        // Distribute enabled copper layers uniformly for blind/buried drill depth.
        let layer_z = |layer: Layer| {
            if layer.is_top() {
                thickness
            } else if layer.is_bottom() {
                0.
            } else {
                thickness
                    * (1. - layer.copper_number() as f64 / f64::from(data.inner_layer_count + 1))
            }
        };
        let mut drills = Vec::new();
        for hole in &data.holes {
            let span = match hole.depth {
                PhysicalHoleDepth::Through => Some((0., thickness)),
                PhysicalHoleDepth::Layers { .. } => hole
                    .drill_layer_span
                    .map(|(start, end)| (layer_z(end), layer_z(start))),
            };
            if let Some((lo, hi)) = span.filter(|(lo, hi)| lo < hi) {
                drills.push((
                    lo,
                    hi,
                    hole.plated,
                    ch::paths_to_clipper(&hole.path.to_outline_strokes(hole.diameter), tol),
                ));
            }
        }
        let mut levels = vec![0., thickness];
        for (lo, hi, _, _) in &drills {
            levels.extend([*lo, *hi]);
        }
        levels.sort_by(f64::total_cmp);
        levels.dedup();
        let mut substrate = MeshPart::new(SurfaceRole::Substrate, SUBSTRATE);
        let mut copper = MeshPart::new(SurfaceRole::Copper, COPPER);
        for slab in levels.windows(2) {
            let (lo, hi) = (slab[0], slab[1]);
            let mut holes = cutouts.clone();
            let mut plated = Vec::new();
            for (a, b, plating, paths) in &drills {
                if *a <= lo && *b >= hi {
                    holes.extend(paths.iter().cloned());
                    if *plating {
                        plated.extend(paths.iter().cloned());
                    }
                }
            }
            ch::unite(&mut holes, NonZero)?;
            let tree = ch::subtract_to_tree(&outlines, &holes, EvenOdd, NonZero, true)?;
            for node in tree.root().children() {
                cap(node, lo as f32, -1., &mut substrate)?;
                cap(node, hi as f32, 1., &mut substrate)?;
                walls(
                    node,
                    lo as f32,
                    hi as f32,
                    &plated,
                    &mut substrate,
                    &mut copper,
                );
            }
        }
        let mut parts = vec![substrate, copper];
        for (side, z, sign, layer) in [
            (BoardSide::Top, thickness as f32, 1., Layer::TOP_COPPER),
            (BoardSide::Bottom, 0., -1., Layer::BOT_COPPER),
        ] {
            let surface = data.surface(side);
            let plane = |paths: &[librepcb_core::geometry::Path],
                         offset,
                         role,
                         color|
             -> Result<MeshPart> {
                let mut part = MeshPart::new(role, color);
                let tree = ch::unite_to_tree(&ch::paths_to_clipper(paths, tol), EvenOdd)?;
                for node in tree.root().children() {
                    cap(node, z + sign * offset, sign, &mut part)?;
                }
                Ok(part)
            };
            if let Some(paths) = surface.copper.get(&layer) {
                parts.push(plane(paths, 0.008, SurfaceRole::Copper, COPPER)?);
            }
            if let Some(color) = data.solder_resist {
                let mut part = plane(
                    &surface.solder_resist,
                    0.016,
                    SurfaceRole::SolderResist,
                    rgba(color.to_solder_resist_color()),
                )?;
                part.roughness = if color.id().ends_with("matte") {
                    0.8
                } else {
                    0.35
                };
                parts.push(part);
            }
            parts.push(plane(
                &surface.silkscreen,
                0.024,
                SurfaceRole::Silkscreen,
                rgba(data.silkscreen.to_silkscreen_color()),
            )?);
            let layer = if side == BoardSide::Top {
                Layer::TOP_SOLDER_PASTE
            } else {
                Layer::BOT_SOLDER_PASTE
            };
            if let Some(paths) = surface.solder_paste.get(&layer) {
                parts.push(plane(paths, 0.032, SurfaceRole::SolderPaste, PASTE)?);
            }
        }
        parts.retain(|part| !part.positions.is_empty());
        if parts.is_empty() {
            return Err(Error::NoOutline);
        }
        let mut output = Self {
            parts,
            bounds: [[0.; 3]; 2],
            warnings: Vec::new(),
        };
        output.update_bounds();
        Ok(output)
    }

    fn update_bounds(&mut self) {
        self.bounds = [[f32::INFINITY; 3], [f32::NEG_INFINITY; 3]];
        for point in self.parts.iter().flat_map(|p| &p.positions) {
            for (axis, value) in point.iter().enumerate() {
                self.bounds[0][axis] = self.bounds[0][axis].min(*value);
                self.bounds[1][axis] = self.bounds[1][axis].max(*value);
            }
        }
    }
}

fn xy(point: IntPoint) -> [f32; 2] {
    [point.x as f32 / 1e6, point.y as f32 / 1e6]
}
fn cap(node: PolyNode<'_>, z: f32, sign: f32, part: &mut MeshPart) -> Result<()> {
    if !node.is_hole() {
        let mut points = Vec::new();
        let mut holes = Vec::new();
        for (index, ring) in std::iter::once(node).chain(node.children()).enumerate() {
            if index > 0 {
                holes.push(points.len() / 2);
            }
            for p in ring.contour() {
                points.extend([p.x as f64 / 1e6, p.y as f64 / 1e6]);
            }
        }
        let indices =
            earcutr::earcut(&points, &holes, 2).map_err(|e| Error::Triangulation(e.to_string()))?;
        for triangle in indices.as_chunks::<3>().0 {
            let mut p = [triangle[0], triangle[1], triangle[2]]
                .map(|i| [points[i * 2] as f32, points[i * 2 + 1] as f32, z]);
            let cross = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1])
                - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0]);
            if cross * sign < 0. {
                p.swap(1, 2);
            }
            part.triangle(p, [0., 0., sign]);
        }
    }
    for child in node.children() {
        cap(child, z, sign, part)?;
    }
    Ok(())
}
fn walls(
    node: PolyNode<'_>,
    lo: f32,
    hi: f32,
    plated: &[clipper::Path],
    substrate: &mut MeshPart,
    copper: &mut MeshPart,
) {
    let contour = node.contour();
    for (a, b) in contour
        .iter()
        .zip(contour.iter().cycle().skip(1))
        .take(contour.len())
    {
        let mid = IntPoint::new(a.x + (b.x - a.x) / 2, a.y + (b.y - a.y) / 2);
        let metal = plated
            .iter()
            .any(|path| clipper::point_in_polygon(mid, path) != 0);
        let part = if metal { &mut *copper } else { &mut *substrate };
        let ([ax, ay], [bx, by]) = (xy(*a), xy(*b));
        let length = libm::sqrtf((bx - ax).powi(2) + (by - ay).powi(2));
        if length <= f32::EPSILON {
            continue;
        }
        // Clipper orients solid boundaries CCW and hole boundaries CW.
        let n = [(by - ay) / length, (ax - bx) / length, 0.];
        part.triangle([[ax, ay, lo], [bx, by, lo], [bx, by, hi]], n);
        part.triangle([[ax, ay, lo], [bx, by, hi], [ax, ay, hi]], n);
    }
    for child in node.children() {
        walls(child, lo, hi, plated, substrate, copper);
    }
}
