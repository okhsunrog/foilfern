use librepcb_core::{
    fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem},
    geometry::{NonEmptyPath, Path},
    project::{
        ProjectLoader,
        board::{BoardPhysicalGeometry, PhysicalHole, PhysicalHoleDepth},
    },
    types::{Layer, Length, Point, PositiveLength},
};
use librepcb_render3d::{BoardMesh, SurfaceRole};
use std::sync::Arc;

pub fn snapshot() -> BoardPhysicalGeometry {
    let path = std::path::Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/projects/Empty Project");
    let fs = Arc::new(TransactionalFileSystem::open_ro(&FilePath::new(path).unwrap()).unwrap());
    let name = fs
        .files("")
        .into_iter()
        .find(|f| f.ends_with(".lpp"))
        .unwrap();
    let project = ProjectLoader::new()
        .open(TransactionalDirectory::new(fs, ""), &name)
        .unwrap();
    let mut data =
        BoardPhysicalGeometry::from_project(&project, &project.boards()[0], None).unwrap();
    data.outlines = vec![rect(0., 0., 10., 10.)];
    data.cutouts.clear();
    data.holes.clear();
    data.devices.clear();
    data.surfaces = Default::default();
    data.thickness = PositiveLength::new(Length::from_mm(1.2).unwrap()).unwrap();
    data.inner_layer_count = 2;
    data
}
pub fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Path {
    Path::rect(
        Point::from_mm(x0, y0).unwrap(),
        Point::from_mm(x1, y1).unwrap(),
    )
}
fn volume(mesh: &BoardMesh) -> f64 {
    mesh.parts
        .iter()
        .filter(|p| matches!(p.role, SurfaceRole::Substrate | SurfaceRole::Copper))
        .flat_map(|p| p.positions.as_chunks::<3>().0)
        .map(|t| {
            let [a, b, c] = t.map(|p| p.map(f64::from));
            (a[0] * (b[1] * c[2] - b[2] * c[1])
                + a[1] * (b[2] * c[0] - b[0] * c[2])
                + a[2] * (b[0] * c[1] - b[1] * c[0]))
                / 6.
        })
        .sum()
}

#[test]
fn extrusion_keeps_cutouts_islands_and_orientation() {
    let mut data = snapshot();
    data.cutouts = vec![rect(2., 2., 6., 6.), rect(4., 4., 8., 8.)];
    let mesh = BoardMesh::from_physical(&data).unwrap();
    assert!((volume(&mesh) - 72. * 1.2).abs() < 1e-4);
    for part in &mesh.parts {
        assert_eq!(part.positions.len(), part.normals.len());
        for (triangle, normal) in part
            .positions
            .as_chunks::<3>()
            .0
            .iter()
            .zip(part.normals.iter().step_by(3))
        {
            let [a, b, c] = triangle.map(bevy::math::Vec3::from);
            assert!((b - a).cross(c - a).dot(bevy::math::Vec3::from(*normal)) > 0.);
        }
    }
    data.cutouts = vec![rect(4., -1., 6., 11.)];
    assert!((volume(&BoardMesh::from_physical(&data).unwrap()) - 80. * 1.2).abs() < 1e-4);
}

#[test]
fn vias_have_real_depth_and_invalid_spans_are_omitted() {
    let mut data = snapshot();
    data.holes = vec![PhysicalHole {
        path: NonEmptyPath::from_point(Point::from_mm(5., 5.).unwrap()),
        diameter: PositiveLength::new(Length::from_mm(2.).unwrap()).unwrap(),
        plated: true,
        via: true,
        depth: PhysicalHoleDepth::Layers {
            start: Layer::TOP_COPPER,
            end: Layer::inner_copper(1).unwrap(),
        },
        drill_layer_span: Some((Layer::TOP_COPPER, Layer::inner_copper(1).unwrap())),
    }];
    let blind = BoardMesh::from_physical(&data).unwrap();
    assert!((volume(&blind) - (120. - std::f64::consts::PI * 0.4)).abs() < 0.03);
    let plating = blind
        .parts
        .iter()
        .find(|p| p.role == SurfaceRole::Copper)
        .unwrap();
    assert!(plating.positions.iter().all(|p| p[2] >= 0.8 - 1e-5));
    data.holes[0].drill_layer_span = Some((Layer::TOP_COPPER, Layer::BOT_COPPER));
    let through = BoardMesh::from_physical(&data).unwrap();
    assert!((volume(&through) - (120. - std::f64::consts::PI * 1.2)).abs() < 0.06);
    data.holes[0].drill_layer_span = None;
    assert!((volume(&BoardMesh::from_physical(&data).unwrap()) - 120.).abs() < 1e-4);
}

#[test]
fn surfaces_keep_materials_on_the_correct_side() {
    let mut data = snapshot();
    data.surfaces[0]
        .copper
        .insert(Layer::TOP_COPPER, vec![rect(1., 1., 2., 2.)]);
    data.surfaces[1].silkscreen = vec![rect(3., 3., 4., 4.)];
    let mesh = BoardMesh::from_physical(&data).unwrap();
    let copper = mesh
        .parts
        .iter()
        .find(|p| p.role == SurfaceRole::Copper)
        .unwrap();
    assert!(copper.positions.iter().all(|p| p[2] > 1.2));
    let silk = mesh
        .parts
        .iter()
        .find(|p| p.role == SurfaceRole::Silkscreen)
        .unwrap();
    assert!(silk.positions.iter().all(|p| p[2] < 0.));
    assert!(silk.normals.iter().all(|n| n[2] < 0.));
}
