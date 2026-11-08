//! GraphicsExport contracts: decoded geometry/content, not merely existence.
use librepcb_core::{
    job::PageOrientation,
    types::{Alignment, Angle, Color, Point},
};
use librepcb_graphics::{Drawing, Error, GraphicsExport, GraphicsPage, GraphicsSettings};
fn page() -> GraphicsPage {
    let settings = GraphicsSettings {
        margins: [0.; 4],
        min_line_width: 0.1,
        dpi: 100,
        ..Default::default()
    };
    let mut drawing = Drawing::new();
    drawing.path(
        kurbo::Rect::new(0., 0., 20., 10.).to_path(0.01),
        0.,
        None,
        Some(Color::rgb(220, 0, 0)),
        &settings,
    );
    GraphicsPage { drawing, settings }
}
use kurbo::Shape;
#[test]
fn vector_and_raster_have_matching_extent_and_visible_content() {
    let page = page();
    let exporter = GraphicsExport::default();
    let (svg, w, h) = exporter.page_svg(&page).unwrap();
    assert!((w - 20.).abs() < 1e-5);
    assert!((h - 10.).abs() < 1e-5);
    assert!(svg.contains("<path"));
    let file = exporter.generate(&[page], "red.png").unwrap().remove(0);
    let pixmap = resvg::tiny_skia::Pixmap::decode_png(&file.contents).unwrap();
    assert_eq!((pixmap.width(), pixmap.height()), (79, 39));
    let center = pixmap.pixel(40, 20).unwrap();
    assert_eq!(
        (center.red(), center.green(), center.blue(), center.alpha()),
        (220, 0, 0, 255)
    );
}
#[test]
fn rotation_mirroring_margins_and_fixed_paper() {
    let mut page = page();
    let exporter = GraphicsExport::default();
    page.settings.rotate = true;
    page.settings.mirror = true;
    page.settings.scale = Some(2.);
    page.settings.margins = [1., 2., 3., 4.];
    let (_, w, h) = exporter.page_svg(&page).unwrap();
    assert!((w - 24.).abs() < 1e-4);
    assert!((h - 46.).abs() < 1e-4);
    page.settings.paper = Some("A4".into());
    page.settings.orientation = PageOrientation::Landscape;
    let (_, w, h) = exporter.page_svg(&page).unwrap();
    assert_eq!((w, h), (297., 210.));
    page.settings.paper = Some("unsupported".into());
    assert!(matches!(
        exporter.page_svg(&page),
        Err(Error::UnsupportedPaper(_))
    ));
}
#[test]
fn multiple_images_get_numbered_names_and_pdf_keeps_pages_and_text() {
    let mut first = page();
    first.drawing.label(
        Point::from_nm(1_000_000, 2_000_000),
        Angle::DEG0,
        1.,
        Alignment::default(),
        "First page",
        Some(Color::BLACK),
        false,
        false,
        false,
    );
    let mut second = page();
    second.drawing.label(
        Point::ORIGIN,
        Angle::DEG0,
        1.,
        Alignment::default(),
        "Second page",
        Some(Color::BLACK),
        false,
        false,
        false,
    );
    let export = GraphicsExport {
        title: "Ω test".into(),
        ..Default::default()
    };
    let images = export
        .generate(&[first.clone(), second.clone()], "path/test.SVG")
        .unwrap();
    assert_eq!(
        images.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        ["path/test1.SVG", "path/test2.SVG"]
    );
    let files = export.generate(&[first, second], "test.pdf").unwrap();
    assert_eq!(files.len(), 1);
    let pdf = lopdf::Document::load_mem(&files[0].contents).unwrap();
    assert_eq!(pdf.get_pages().len(), 2);
    assert!(
        pdf.extract_text(&[1])
            .unwrap()
            .replace('\n', "")
            .contains("First page"),
        "{:?}",
        pdf.extract_text(&[1])
    );
    assert!(
        pdf.extract_text(&[2])
            .unwrap()
            .replace('\n', "")
            .contains("Second page")
    );
    for id in pdf.get_pages().into_values() {
        assert!(pdf.get_dictionary(id).unwrap().get(b"MediaBox").is_ok());
    }
}
#[test]
fn errors_are_typed_and_raster_allocation_is_bounded() {
    let exporter = GraphicsExport {
        max_pixels: 10,
        ..Default::default()
    };
    assert!(matches!(
        exporter.generate(&[page()], "test.png"),
        Err(Error::Dimensions)
    ));
    assert!(matches!(
        exporter.generate(&[], "test.pdf"),
        Err(Error::NoPages)
    ));
    assert!(matches!(
        exporter.generate(&[page()], "test.foo"),
        Err(Error::UnsupportedFormat(_))
    ));
    let mut invalid = page();
    invalid.settings.scale = Some(f64::NAN);
    assert!(matches!(
        exporter.generate(&[invalid], "test.svg"),
        Err(Error::Dimensions)
    ));
}
#[test]
fn layer_filter_and_monochrome_preserve_grab_area_alpha() {
    let mut s = GraphicsSettings::default();
    assert_eq!(s.color("schematic_wires"), Some(Color::rgb(0, 128, 0)));
    assert_eq!(
        s.fill("schematic_outlines", false, true),
        Some(Color::rgb(255, 255, 225))
    );
    assert!(s.fill("schematic_comments", false, true).is_none());
    s.monochrome = true;
    assert_eq!(s.color("schematic_wires"), Some(Color::BLACK));
    s.background = Color::BLACK;
    assert_eq!(s.color("schematic_wires"), Some(Color::WHITE));
    assert!(s.color("unknown").is_none());
}
