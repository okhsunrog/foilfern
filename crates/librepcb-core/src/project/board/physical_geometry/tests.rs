//! Analytic material-region tests for the shared physical geometry.
use super::*;
use crate::{
    geometry::Vertex,
    types::{Length, Point},
};

fn point(x: f64, y: f64) -> Point {
    Point::from_mm(x, y).unwrap()
}
fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Path {
    Path::rect(point(x0, y0), point(x1, y1))
}
fn fixture() -> BoardPhysicalGeometry {
    let outline = rect(0., 0., 10., 10.);
    BoardPhysicalGeometry {
        board: BoardId(Uuid::new_random()),
        project_revision: 0,
        thickness: PositiveLength::new(Length::new(1_600_000)).unwrap(),
        inner_layer_count: 2,
        solder_resist: Some(PcbColor::Green),
        silkscreen: PcbColor::White,
        outlines: vec![outline.clone()],
        cutouts: Vec::new(),
        holes: Vec::new(),
        devices: Vec::new(),
        areas: BTreeMap::from([
            (Layer::TOP_COPPER, vec![outline.clone()]),
            (Layer::BOT_COPPER, vec![outline]),
        ]),
        surfaces: Default::default(),
        silkscreen_layers: [
            BTreeSet::from([Layer::TOP_LEGEND]),
            BTreeSet::from([Layer::BOT_LEGEND]),
        ],
    }
}
fn contains(paths: &[Path], x: f64, y: f64) -> bool {
    let pt = point(x, y);
    paths
        .iter()
        .filter(|path| {
            clipper::point_in_polygon(
                clipper::IntPoint::new(pt.x.to_nm(), pt.y.to_nm()),
                &crate::utils::clipper_helpers::path_to_clipper(
                    path,
                    BoardPhysicalGeometry::arc_tolerance(),
                ),
            ) != 0
        })
        .count()
        % 2
        == 1
}
fn hole(x: f64, y: f64, depth: PhysicalHoleDepth) -> PhysicalHole {
    PhysicalHole {
        path: NonEmptyPath::new(Path::new(vec![Vertex::at(point(x, y))])).unwrap(),
        diameter: PositiveLength::new(Length::new(1_000_000)).unwrap(),
        plated: true,
        via: true,
        depth,
        drill_layer_span: match depth {
            PhysicalHoleDepth::Through => Some((Layer::TOP_COPPER, Layer::BOT_COPPER)),
            PhysicalHoleDepth::Layers { start, end } => Some((start, end)),
        },
    }
}
#[test]
fn body_and_copper_subtract_drills_and_cutouts_and_clip_to_outline() {
    let mut data = fixture();
    data.cutouts.push(rect(4., 4., 6., 6.));
    data.holes.push(hole(2., 2., PhysicalHoleDepth::Through));
    data.areas
        .insert(Layer::TOP_COPPER, vec![rect(-1., -1., 11., 11.)]);
    data.prepare_surfaces().unwrap();
    for side in [BoardSide::Top, BoardSide::Bottom] {
        let surface = data.surface(side);
        for paths in [&surface.body, &surface.copper] {
            assert!(contains(paths, 1., 1.));
            assert!(!contains(paths, 2., 2.));
            assert!(!contains(paths, 5., 5.));
            assert!(!contains(paths, -0.5, 5.));
        }
    }
}
#[test]
fn silkscreen_is_clipped_to_mask_and_paste_to_body() {
    let mut data = fixture();
    data.areas
        .insert(Layer::TOP_STOP_MASK, vec![rect(4., 0., 6., 10.)]);
    data.areas
        .insert(Layer::TOP_LEGEND, vec![rect(4.5, 1., 6.5, 9.)]);
    data.areas
        .insert(Layer::TOP_SOLDER_PASTE, vec![rect(-2., 1., 2., 2.)]);
    data.prepare_surfaces().unwrap();
    let surface = data.surface(BoardSide::Top);
    assert!(!contains(&surface.solder_resist, 5., 3.));
    assert!(!contains(&surface.silkscreen, 5., 3.));
    assert!(contains(&surface.silkscreen, 6.25, 3.));
    assert!(contains(&surface.solder_paste, 1., 1.5));
    assert!(!contains(&surface.solder_paste, -1., 1.5));
    assert!(data.surface(BoardSide::Bottom).silkscreen.is_empty());
}
#[test]
fn drill_depth_is_retained_independently_of_the_upstream_flat_projection() {
    let mut data = fixture();
    let top = PhysicalHoleDepth::Layers {
        start: Layer::TOP_COPPER,
        end: Layer::inner_copper(1).unwrap(),
    };
    data.holes.push(hole(2., 2., top));
    data.holes.push(hole(
        7.,
        7.,
        PhysicalHoleDepth::Layers {
            start: Layer::inner_copper(1).unwrap(),
            end: Layer::inner_copper(2).unwrap(),
        },
    ));
    data.prepare_surfaces().unwrap();
    assert_eq!(data.holes[0].depth, top);
    // Preserve RealisticBoardPainter's face-dependent blind-hole projection.
    // Mesh builders use the original depth, not an extrusion of this body.
    assert!(contains(&data.surface(BoardSide::Top).body, 2., 2.));
    assert!(!contains(&data.surface(BoardSide::Top).copper, 2., 2.));
    assert!(!contains(&data.surface(BoardSide::Bottom).body, 2., 2.));
    for side in [BoardSide::Top, BoardSide::Bottom] {
        assert!(contains(&data.surface(side).body, 7., 7.));
    }
}
#[test]
fn no_board_outline_does_not_invent_a_physical_body() {
    let mut data = fixture();
    data.outlines.clear();
    data.prepare_surfaces().unwrap();
    for surface in &data.surfaces {
        assert_eq!(surface, &BoardSurfaceGeometry::default());
    }
}
