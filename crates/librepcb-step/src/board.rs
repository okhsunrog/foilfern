//! Port of libs/librepcb/core/3d/occmodel.cpp (createBoard/addToAssembly)
//! and libs/librepcb/core/3d/stepexport.cpp, as an isolated experiment.
//! Clipper handles planar subtraction; Monstertruck constructs the BRep.
//! Whole surviving contours retain analytic arcs; clipped intersections carry
//! an explicit approximation count at the snapshot's 5 um tolerance.
use anyhow::{Context, Result, ensure};
use clipper::{PolyFillType::NonZero, PolyNode};
use librepcb_core::{
    geometry::Path,
    project::board::{BoardPhysicalGeometry, PhysicalDevice, PhysicalHoleDepth},
    types::Angle,
    utils::{clipper_helpers as ch, toolbox},
};
use monstertruck_modeling::*;
use std::collections::BTreeMap;

/// Upstream passes QColor channel values to Quantity_TOC_RGB (linear), then
/// the STEP writer encodes sRGB. Mechanical PCB bodies export opaque; mask
/// alpha is for the separate surface renderer, not the substrate solid.
pub fn mechanical_material(data: &BoardPhysicalGeometry) -> crate::materials::Material {
    let linear = if let Some(resist) = data.solder_resist {
        let color = resist.to_solder_resist_color();
        [color.r, color.g, color.b].map(|value| f64::from(value) / 510.)
    } else {
        [70., 80., 70.].map(|value| value / 255.)
    };
    let color = linear.map(|value| {
        if value <= 0.0031308 {
            value * 12.92
        } else {
            1.055 * libm::pow(value, 1. / 2.4) - 0.055
        }
    });
    crate::materials::Material { color, opacity: 1. }
}

type ContourKey = Vec<(i64, i64)>;
pub struct BoardBrep {
    pub solids: Vec<Solid>,
    pub approximated_contours: usize,
}

fn key(points: &[clipper::IntPoint]) -> ContourKey {
    let points: Vec<_> = points.iter().map(|p| (p.x, p.y)).collect();
    let rotate = |mut points: ContourKey| {
        if let Some((index, _)) = points.iter().enumerate().min_by_key(|(_, p)| *p) {
            points.rotate_left(index);
        }
        points
    };
    let forward = rotate(points.clone());
    let backward = rotate(points.into_iter().rev().collect());
    forward.min(backward)
}

fn analytic_wire(path: &Path) -> Result<Wire> {
    ensure!(
        path.is_closed() && path.vertices().len() >= 3,
        "invalid closed board contour"
    );
    let source = &path.vertices()[..path.vertices().len() - 1];
    let vertices: Vec<_> = source
        .iter()
        .map(|v| builder::vertex(Point3::new(v.pos.x.to_mm(), v.pos.y.to_mm(), 0.)))
        .collect();
    let mut edges = Vec::new();
    for (index, v0) in source.iter().enumerate() {
        let next = (index + 1) % source.len();
        let v1 = &source[next];
        if v0.angle == Angle::DEG0 {
            edges.push(builder::line(&vertices[index], &vertices[next]));
        } else {
            let center = toolbox::arc_center(v0.pos, v1.pos, v0.angle)
                .context("invalid board arc center")?;
            let radial = v0.pos - center;
            let sign = if v0.angle < Angle::DEG0 { -1. } else { 1. };
            let tangent = Vector3::new(-radial.y.to_mm() * sign, radial.x.to_mm() * sign, 0.);
            edges.push(builder::try_circle_arc(
                &vertices[index],
                &vertices[next],
                tangent,
            )?);
        }
    }
    Ok(edges.into())
}

fn wire(node: PolyNode<'_>) -> Wire {
    let vertices: Vec<_> = node
        .contour()
        .iter()
        .map(|p| builder::vertex(Point3::new(p.x as f64 / 1e6, p.y as f64 / 1e6, 0.)))
        .collect();
    (0..vertices.len())
        .map(|i| builder::line(&vertices[i], &vertices[(i + 1) % vertices.len()]))
        .collect()
}

fn collect(
    node: PolyNode<'_>,
    thickness: f64,
    originals: &BTreeMap<ContourKey, Option<&Path>>,
    output: &mut BoardBrep,
) -> Result<()> {
    if !node.is_hole() {
        let mut wires = Vec::new();
        for contour in std::iter::once(node).chain(node.children()) {
            if let Some(Some(path)) = originals.get(&key(contour.contour())) {
                wires.push(analytic_wire(path)?);
            } else {
                wires.push(wire(contour));
                output.approximated_contours += 1;
            }
        }
        let solid = profile::solid_from_planar_profile(wires, Vector3::new(0., 0., thickness))?;
        output.solids.push(solid);
    }
    for child in node.children() {
        collect(child, thickness, originals, output)?;
    }
    Ok(())
}

/// Mechanical STEP policy follows upstream: via drills are omitted entirely.
/// Non-via drills must be through holes; no blind drill is silently deepened.
#[cfg(any(test, feature = "diagnostics"))]
pub fn board_solids(data: &BoardPhysicalGeometry) -> Result<Vec<Solid>> {
    Ok(board_brep(data)?.solids)
}

pub fn board_brep(data: &BoardPhysicalGeometry) -> Result<BoardBrep> {
    ensure!(!data.outlines.is_empty(), "board has no closed outline");
    let tolerance = BoardPhysicalGeometry::arc_tolerance();
    let outlines = ch::paths_to_clipper(&data.outlines, tolerance);
    let mut holes = data.cutouts.clone();
    for hole in &data.holes {
        if hole.via {
            continue;
        }
        ensure!(
            hole.depth == PhysicalHoleDepth::Through,
            "non-through drill unsupported"
        );
        holes.extend(hole.path.to_outline_strokes(hole.diameter));
    }
    let mut originals = BTreeMap::new();
    for path in data.outlines.iter().chain(&holes) {
        let normalized = ch::unite_to_tree(
            &ch::paths_to_clipper(std::slice::from_ref(path), tolerance),
            NonZero,
        )?;
        if normalized.total() == 1
            && let Some(contour) = normalized.first()
        {
            let key = key(contour.contour());
            if let Some(previous) = originals.get_mut(&key) {
                if *previous != Some(path) {
                    *previous = None;
                }
            } else {
                originals.insert(key, Some(path));
            }
        }
    }
    let holes = ch::paths_to_clipper(&holes, tolerance);
    let tree = ch::subtract_to_tree(&outlines, &holes, NonZero, NonZero, true)?;
    let mut output = BoardBrep {
        solids: Vec::new(),
        approximated_contours: 0,
    };
    for node in tree.root().children() {
        collect(node, data.thickness.to_mm(), &originals, &mut output)?;
    }
    ensure!(
        !output.solids.is_empty(),
        "board has no material after subtraction"
    );
    Ok(output)
}

fn rotation(axis: Vector3, angle: f64) -> Matrix4 {
    cgmath::Quaternion::from_sv(libm::cos(angle / 2.), axis * libm::sin(angle / 2.)).into()
}

/// Upstream OccModel::addToAssembly(): bottom devices rotate 180 deg about Y.
/// This is a proper 3D rotation; using a 2D reflection would invert the solid.
pub fn device_matrix(device: &PhysicalDevice, thickness: f64) -> Matrix4 {
    let transform = device.transform;
    let (x, y, z) = device.model_position;
    let (rx, ry, rz) = device.model_rotation;
    Matrix4::from_translation(Vector3::new(
        transform.position.x.to_mm(),
        transform.position.y.to_mm(),
        0.,
    )) * rotation(Vector3::unit_z(), transform.rotation.to_rad())
        * if transform.mirrored {
            rotation(Vector3::unit_y(), std::f64::consts::PI)
        } else {
            Matrix4::identity()
        }
        * Matrix4::from_translation(Vector3::new(
            x.to_mm(),
            y.to_mm(),
            z.to_mm() + if transform.mirrored { 0. } else { thickness },
        ))
        * rotation(Vector3::unit_z(), rz.to_rad())
        * rotation(Vector3::unit_y(), ry.to_rad())
        * rotation(Vector3::unit_x(), rx.to_rad())
}

#[cfg(test)]
mod tests {
    use super::*;
    use librepcb_core::{
        fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem},
        geometry::{NonEmptyPath, Path},
        project::{ComponentInstanceId, ProjectLoader, board::PhysicalHole},
        types::{Angle, Layer, Length, Point, PositiveLength, Uuid},
        utils::transform::Transform,
    };
    use monstertruck_meshing::prelude::*;
    use monstertruck_modeling::Transform as _;
    use std::sync::Arc;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Path {
        Path::rect(
            Point::from_mm(x0, y0).unwrap(),
            Point::from_mm(x1, y1).unwrap(),
        )
    }
    fn snapshot() -> BoardPhysicalGeometry {
        let path = std::path::Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
            .join("tests/data/projects/Empty Project");
        let fs = Arc::new(TransactionalFileSystem::open_ro(&FilePath::new(path).unwrap()).unwrap());
        let filename = fs
            .files("")
            .into_iter()
            .find(|name| name.ends_with(".lpp"))
            .unwrap();
        let project = ProjectLoader::new()
            .open(TransactionalDirectory::new(fs, ""), &filename)
            .unwrap();
        let mut data =
            BoardPhysicalGeometry::from_project(&project, &project.boards()[0], None).unwrap();
        data.outlines = vec![rect(0., 0., 10., 10.)];
        data.cutouts.clear();
        data.holes.clear();
        data.thickness = PositiveLength::new(Length::from_mm(1.).unwrap()).unwrap();
        // Deliberately invalid flat projections: a 3D body uses canonical data.
        for surface in &mut data.surfaces {
            surface.body.clear();
        }
        data
    }
    fn volume(data: &BoardPhysicalGeometry, expected_solids: usize) -> f64 {
        let solids = board_solids(data).unwrap();
        assert_eq!(solids.len(), expected_solids);
        solids
            .iter()
            .map(|solid| {
                assert!(solid.is_geometric_consistent());
                let mut mesh = PolygonMesh::default();
                for shell in solid.boundaries() {
                    let triangles = shell.robust_triangulation(0.01);
                    assert!(triangles.face_iter().all(|face| face.surface().is_some()));
                    mesh.merge(triangles.to_polygon());
                }
                let stats = crate::kernel::mesh_stats(mesh).unwrap();
                for key in ["boundary_edges", "nonmanifold_edges", "misoriented_edges"] {
                    assert_eq!(stats[key], 0);
                }
                stats["signed_volume"].as_f64().unwrap()
            })
            .sum()
    }
    #[test]
    fn mechanical_pcb_color_is_opaque_and_encoded_as_srgb() {
        let mut data = snapshot();
        data.solder_resist = Some(librepcb_core::types::PcbColor::Green);
        let material = mechanical_material(&data);
        assert_eq!(material.opacity, 1.);
        assert_eq!(material.color[0], 0.);
        assert!((material.color[1] - 0.3458689218895012).abs() < 1e-12);
        assert_eq!(material.color[2], 0.);
    }
    #[test]
    fn circular_holes_keep_two_analytic_arcs_in_both_directions() {
        let mut data = snapshot();
        let diameter = PositiveLength::new(Length::from_mm(4.).unwrap()).unwrap();
        let circle = Path::circle(diameter).translated(Point::from_mm(5., 5.).unwrap());
        for path in [circle.clone(), circle.reversed()] {
            data.cutouts = vec![path];
            let model = board_brep(&data).unwrap();
            assert_eq!(model.approximated_contours, 0);
            assert_eq!(model.solids.len(), 1);
            assert_eq!(model.solids[0].boundaries()[0].len(), 8);
            assert!(model.solids[0].is_geometric_consistent());
            assert!((volume(&data, 1) - (100. - std::f64::consts::PI * 4.)).abs() < 0.2);
        }
        data.cutouts = vec![circle, rect(4., 4., 8., 8.)];
        assert!(board_brep(&data).unwrap().approximated_contours > 0);
    }
    #[test]
    fn canonical_contours_union_overlapping_cutouts() {
        let mut data = snapshot();
        data.cutouts = vec![rect(2., 2., 6., 6.), rect(4., 4., 8., 8.)];
        assert!((volume(&data, 1) - 72.).abs() < 1e-8);
        data.cutouts = vec![rect(4., -1., 6., 11.)];
        assert!((volume(&data, 2) - 80.).abs() < 1e-8);
    }
    #[test]
    fn mechanical_policy_omits_vias_and_rejects_non_through_holes() {
        let mut data = snapshot();
        data.holes.push(PhysicalHole {
            path: NonEmptyPath::from_point(Point::from_mm(5., 5.).unwrap()),
            diameter: PositiveLength::new(Length::from_mm(2.).unwrap()).unwrap(),
            plated: true,
            via: true,
            depth: PhysicalHoleDepth::Layers {
                start: Layer::TOP_COPPER,
                end: Layer::BOT_COPPER,
            },
            drill_layer_span: None,
        });
        assert!((volume(&data, 1) - 100.).abs() < 1e-8);
        data.holes[0].via = false;
        assert!(board_solids(&data).is_err());
        data.holes.clear();
        data.outlines.clear();
        assert!(board_solids(&data).is_err());
    }
    #[test]
    fn top_and_bottom_models_apply_offsets_and_rotations_in_upstream_order() {
        let mut device = PhysicalDevice {
            component: ComponentInstanceId(Uuid::new_random()),
            name: "U1".into(),
            package: Uuid::new_random(),
            model: Uuid::new_random(),
            transform: Transform::new(Point::from_mm(10., 20.).unwrap(), Angle::DEG90, false),
            model_position: (
                Length::from_mm(1.).unwrap(),
                Length::from_mm(2.).unwrap(),
                Length::from_mm(0.3).unwrap(),
            ),
            model_rotation: (Angle::DEG90, Angle::DEG0, Angle::DEG0),
        };
        let top = device_matrix(&device, 1.6).transform_point(Point3::new(0., 1., 0.));
        assert!((top - Point3::new(8., 21., 2.9)).magnitude() < 1e-10);
        device.transform.mirrored = true;
        let matrix = device_matrix(&device, 1.6);
        let bottom = matrix.transform_point(Point3::new(0., 1., 0.));
        assert!((bottom - Point3::new(8., 19., -1.3)).magnitude() < 1e-10);
        assert!((matrix.determinant() - 1.).abs() < 1e-10);
    }
}
