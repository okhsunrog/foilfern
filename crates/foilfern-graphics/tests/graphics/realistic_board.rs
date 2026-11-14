//! Physical material colors, compound cutouts and shared raster/vector layout.
use foilfern_core::{
    fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem},
    geometry::Path,
    project::{ProjectLoader, board::BoardPhysicalGeometry},
    types::{Color, Layer, PcbColor, Point},
};
use foilfern_graphics::{
    Drawing, GraphicsExport, GraphicsPage, GraphicsSettings, realistic_board_drawing,
};
use kurbo::{BezPath, Rect, Shape};
use std::{collections::BTreeMap, sync::Arc};

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Path {
    Path::rect(
        Point::from_mm(x0, y0).unwrap(),
        Point::from_mm(x1, y1).unwrap(),
    )
}
fn fixture() -> (BoardPhysicalGeometry, GraphicsSettings) {
    let path = std::path::Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/projects/Empty Project");
    let fs = Arc::new(TransactionalFileSystem::open_ro(&FilePath::new(path).unwrap()).unwrap());
    let file = fs
        .files("")
        .into_iter()
        .find(|name| name.ends_with(".lpp"))
        .unwrap();
    let project = ProjectLoader::new()
        .open(TransactionalDirectory::new(fs, ""), &file)
        .unwrap();
    let mut data =
        BoardPhysicalGeometry::from_project(&project, &project.boards()[0], None).unwrap();
    data.solder_resist = Some(PcbColor::Green);
    data.silkscreen = PcbColor::White;
    for surface in &mut data.surfaces {
        // Same winding intentionally: both encoders must use even-odd fills.
        surface.body = vec![rect(0., 0., 10., 6.), rect(2., 2., 4., 4.)];
        surface.copper = BTreeMap::from([
            (
                Layer::TOP_COPPER,
                vec![rect(0., 0., 5., 6.), rect(2., 2., 4., 4.)],
            ),
            (Layer::BOT_COPPER, vec![rect(0., 0., 1., 6.)]),
        ]);
        surface.solder_resist = vec![rect(8., 0., 10., 6.)];
        surface.silkscreen = vec![rect(8.2, 2., 8.8, 3.)];
        surface.solder_paste =
            BTreeMap::from([(Layer::TOP_SOLDER_PASTE, vec![rect(6., 2., 7., 3.)])]);
    }
    let settings = GraphicsSettings {
        margins: [1.; 4],
        dpi: 254,
        scale: Some(1.),
        colors: BTreeMap::from([
            ("board_outlines".into(), Color::rgb(70, 80, 70)),
            ("board_copper_top".into(), Color::rgb(188, 156, 105)),
            ("board_stop_mask_top".into(), Color::rgba(0, 0, 0, 0)),
            ("board_legend_top".into(), Color::rgba(0, 0, 0, 0)),
            ("board_solder_paste_top".into(), Color::rgb(128, 128, 128)),
        ]),
        paint_order: vec!["board_copper_top".into()],
        ..Default::default()
    };
    (data, settings)
}
fn png(page: GraphicsPage) -> resvg::tiny_skia::Pixmap {
    let file = GraphicsExport::default()
        .generate(&[page], "board.png")
        .unwrap()
        .remove(0);
    resvg::tiny_skia::Pixmap::decode_png(&file.contents).unwrap()
}
fn rgba(image: &resvg::tiny_skia::Pixmap, x: u32, y: u32) -> [u8; 4] {
    let p = image.pixel(x, y).unwrap();
    [p.red(), p.green(), p.blue(), p.alpha()]
}
#[test]
fn vello_renders_materials_holes_and_board_color_fallbacks() {
    let (data, settings) = fixture();
    let drawing = realistic_board_drawing(&data, &settings);
    assert_eq!(drawing.filled_areas().unwrap().len(), 5);
    let image = png(GraphicsPage { drawing, settings });
    assert_eq!((image.width(), image.height()), (120, 80));
    assert_eq!(rgba(&image, 20, 60), [188, 156, 105, 255]); // copper
    assert_eq!(rgba(&image, 40, 40), [0, 0, 0, 0]); // compound hole
    assert_eq!(rgba(&image, 85, 30), [70, 80, 70, 255]); // substrate
    assert_eq!(rgba(&image, 95, 45), [255, 255, 255, 255]); // silk
    assert_eq!(rgba(&image, 75, 45), [128, 128, 128, 255]); // paste
    let mask = rgba(&image, 105, 20);
    assert!(mask[1] > mask[0] && mask[0] < 70 && mask[3] == 255);
    assert_eq!(rgba(&image, 0, 0), [0, 0, 0, 0]);
}
#[test]
fn vello_and_vector_layout_agree_for_both_sides_rotation_and_fixed_paper() {
    let (data, mut settings) = fixture();
    for mirror in [false, true] {
        for rotate in [false, true] {
            settings.mirror = mirror;
            settings.rotate = rotate;
            settings.paper = Some("A6".into());
            settings.scale = None;
            settings.dpi = 96;
            settings.background = Color::rgba(40, 50, 60, 80);
            let page = GraphicsPage {
                drawing: realistic_board_drawing(&data, &settings),
                settings: settings.clone(),
            };
            let svg = GraphicsExport::default().page_svg(&page).unwrap().0;
            let image = png(page);
            let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
            let mut reference =
                resvg::tiny_skia::Pixmap::new(image.width(), image.height()).unwrap();
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::from_scale(
                    image.width() as f32 / tree.size().width(),
                    image.height() as f32 / tree.size().height(),
                ),
                &mut reference.as_mut(),
            );
            // Rasterizers can disagree at edges. Away from boundaries their
            // complete composed RGBA must agree within rounding precision.
            let mut checked = 0;
            for y in (3..image.height() - 3).step_by(7) {
                for x in (3..image.width() - 3).step_by(7) {
                    let expected = rgba(&reference, x, y);
                    if [(x - 2, y), (x + 2, y), (x, y - 2), (x, y + 2)]
                        .into_iter()
                        .all(|(x, y)| rgba(&reference, x, y) == expected)
                    {
                        let actual = rgba(&image, x, y);
                        assert!(
                            actual
                                .into_iter()
                                .zip(expected)
                                .all(|(a, b)| a.abs_diff(b) <= 2),
                            "mirror={mirror}, rotate={rotate}, ({x},{y}): {actual:?} != {expected:?}"
                        );
                        checked += 1;
                    }
                }
            }
            assert!(checked > 100);
        }
    }
}
#[test]
fn missing_materials_are_hidden_explicit_colors_win_and_monochrome_is_opaque() {
    let (mut data, mut settings) = fixture();
    data.solder_resist = None;
    assert_eq!(
        realistic_board_drawing(&data, &settings)
            .filled_areas()
            .unwrap()
            .len(),
        4
    );
    settings
        .colors
        .insert("board_stop_mask_top".into(), Color::rgb(1, 2, 3));
    assert_eq!(
        realistic_board_drawing(&data, &settings)
            .filled_areas()
            .unwrap()[2]
            .color,
        Color::rgb(1, 2, 3)
    );
    settings.colors.remove("board_stop_mask_top");
    settings.colors.remove("board_legend_top");
    assert_eq!(
        realistic_board_drawing(&data, &settings)
            .filled_areas()
            .unwrap()
            .len(),
        3
    );
    settings.monochrome = true;
    assert!(
        realistic_board_drawing(&data, &settings)
            .filled_areas()
            .unwrap()
            .iter()
            .all(|area| area.color == Color::BLACK)
    );
}
#[test]
fn appending_general_drawing_disables_area_rasterization_without_losing_content() {
    let mut areas = Drawing::new();
    areas.area(
        Rect::new(0., 0., 10., 10.).to_path(0.1),
        Some(Color::rgb(255, 0, 0)),
    );
    let mut general = Drawing::new();
    general.path(
        BezPath::from_svg("M0 0 L5 5").unwrap(),
        0.2,
        Some(Color::rgb(0, 0, 255)),
        None,
        &GraphicsSettings::default(),
    );
    assert!(general.filled_areas().is_none());
    areas.append(general);
    assert!(areas.filled_areas().is_none());
    assert!(!areas.is_empty());
}
