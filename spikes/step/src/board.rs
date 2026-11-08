//! Port of libs/librepcb/core/3d/occmodel.cpp (createBoard/addToAssembly)
//! and libs/librepcb/core/3d/stepexport.cpp, as an isolated experiment.
//! Clipper handles planar subtraction; Monstertruck constructs the BRep.
//! Curved PCB contours are approximated within the snapshot's 5 um tolerance.
use anyhow::{Result, ensure};
use clipper::{PolyFillType::NonZero, PolyNode};
use librepcb_core::{
    project::board::{BoardPhysicalGeometry, PhysicalDevice, PhysicalHoleDepth},
    utils::clipper_helpers as ch,
};
use monstertruck_modeling::*;

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

fn collect(node: PolyNode<'_>, thickness: f64, solids: &mut Vec<Solid>) -> Result<()> {
    if !node.is_hole() {
        let mut wires = vec![wire(node)];
        wires.extend(node.children().map(wire));
        let solid = profile::solid_from_planar_profile(wires, Vector3::new(0., 0., thickness))?;
        solids.push(solid);
    }
    for child in node.children() {
        collect(child, thickness, solids)?;
    }
    Ok(())
}

/// Mechanical STEP policy follows upstream: via drills are omitted entirely.
/// Non-via drills must be through holes; no blind drill is silently deepened.
pub fn board_solids(data: &BoardPhysicalGeometry) -> Result<Vec<Solid>> {
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
    let holes = ch::paths_to_clipper(&holes, tolerance);
    let tree = ch::subtract_to_tree(&outlines, &holes, NonZero, NonZero, true)?;
    let mut solids = Vec::new();
    for node in tree.root().children() {
        collect(node, data.thickness.to_mm(), &mut solids)?;
    }
    ensure!(
        !solids.is_empty(),
        "board has no material after subtraction"
    );
    Ok(solids)
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
                let stats = crate::mesh_stats(mesh).unwrap();
                for key in ["boundary_edges", "nonmanifold_edges", "misoriented_edges"] {
                    assert_eq!(stats[key], 0);
                }
                stats["signed_volume"].as_f64().unwrap()
            })
            .sum()
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
