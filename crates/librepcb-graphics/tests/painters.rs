//! Library/project painter contracts against upstream model fixtures.
use librepcb_core::{
    fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem},
    library::{LibraryBaseElement, pkg::Package, sym::Symbol},
    project::ProjectLoader,
};
use librepcb_graphics::{
    GraphicsExport, GraphicsPage, GraphicsSettings, footprint_drawing, schematic_drawing,
    symbol_drawing,
};
use std::{path::Path, sync::Arc};

fn directory(relative: &str) -> TransactionalDirectory {
    let path = Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data")
        .join(relative);
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(path).unwrap()).unwrap();
    TransactionalDirectory::new(Arc::new(fs), "")
}

#[test]
fn symbol_keeps_embedded_images_pin_positions_and_overline_markup() {
    let symbol = Symbol::open(directory(
        "libraries/Populated Library.lplib/sym/35aad2af-5cd4-42ae-8576-fe7febad0d8a",
    ))
    .unwrap();
    let settings = GraphicsSettings::default();
    let drawing = symbol_drawing(&symbol, &settings).unwrap();
    let (svg, _, _) = GraphicsExport::default()
        .page_svg(&GraphicsPage { drawing, settings })
        .unwrap();
    assert!(svg.contains("FooBar"));
    assert!(svg.contains("data:image/png;base64,"));
    assert!(svg.contains("data:image/svg+xml;base64,"));
    assert!(svg.contains("M-10.16,-7.62 L-7.62,-7.62"), "{svg}");
    assert!(svg.contains("text-decoration=\"overline\""));
    assert!(!svg.contains("!8"));
}

#[test]
fn footprints_have_distinct_placed_geometry_and_compound_pad_holes() {
    let package = Package::open(directory(
        "libraries/Populated Library.lplib/pkg/0eaf289c-166d-4bd9-a4ba-dbf6bbc76ef1",
    ))
    .unwrap();
    let settings = GraphicsSettings::default();
    let svgs: Vec<_> = package
        .footprints()
        .iter()
        .map(|fp| {
            let drawing = footprint_drawing(fp, &settings).unwrap();
            GraphicsExport::default()
                .page_svg(&GraphicsPage {
                    drawing,
                    settings: settings.clone(),
                })
                .unwrap()
                .0
        })
        .collect();
    assert_eq!(svgs.len(), 2);
    assert_ne!(svgs[0], svgs[1]);
    assert!(svgs.iter().all(|s| s.contains("fill-rule=\"evenodd\"")));
}

#[test]
fn schematic_resolves_attributes_and_export_is_read_only() {
    let directory = directory("projects/Gerber Test");
    let fs = Arc::clone(directory.file_system());
    let file = fs
        .files("")
        .into_iter()
        .find(|n| n.ends_with(".lpp"))
        .unwrap();
    let project = ProjectLoader::new().open(directory, &file).unwrap();
    let before = fs.export_to_zip(None).unwrap();
    let revision = project.revision();
    let settings = GraphicsSettings::default();
    let pages: Vec<_> = project
        .schematics()
        .iter()
        .map(|schematic| GraphicsPage {
            drawing: schematic_drawing(&project, schematic, &settings).unwrap(),
            settings: settings.clone(),
        })
        .collect();
    let export = GraphicsExport::default();
    let (svg, width, height) = export.page_svg(&pages[0]).unwrap();
    assert!(width > 20. && height > 20.);
    assert!(svg.contains("FooBar"));
    assert!(!svg.contains("{{NAME}}"));
    assert!(!svg.contains("{{VALUE}}"));
    let pdf = export.generate(&pages, "schematic.pdf").unwrap();
    let pdf = lopdf::Document::load_mem(&pdf[0].contents).unwrap();
    assert_eq!(pdf.get_pages().len(), project.schematics().len());
    assert_eq!(project.revision(), revision);
    // Transactional ZIP metadata is fixed, so this also compares every file.
    assert_eq!(fs.export_to_zip(None).unwrap(), before);
}
