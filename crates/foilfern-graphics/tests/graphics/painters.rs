//! Library/project painter contracts against upstream model fixtures.
use foilfern_core::{
    fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem, ZipArchive},
    library::{LibraryBaseElement, pkg::Package, sym::Symbol},
    project::{Mutation, ProjectLoader, SchematicMutation, SymbolRef},
    types::{Layer, Point},
};
use foilfern_graphics::{
    GraphicsExport, GraphicsPage, GraphicsSettings, footprint_drawing, schematic_drawing,
    schematic_field_geometry, symbol_drawing,
};
use std::{path::Path, sync::Arc};

fn directory(relative: &str) -> TransactionalDirectory {
    let path = Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data")
        .join(relative);
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(path).unwrap()).unwrap();
    TransactionalDirectory::new(Arc::new(fs), "")
}
fn contents(bytes: Vec<u8>) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut zip = ZipArchive::from_bytes(bytes).unwrap();
    (0..zip.len())
        .map(|i| (zip.file_name(i).unwrap(), zip.read_file(i).unwrap()))
        .collect()
}

#[test]
fn field_placement_accounts_for_export_text_inside_an_outline() {
    use foilfern_core::geometry::{Path, Polygon};
    use foilfern_core::types::{Length, Uuid};

    let directory = directory("projects/Gerber Test");
    let project_file = directory
        .file_system()
        .files("")
        .into_iter()
        .find(|n| n.ends_with(".lpp"))
        .unwrap();
    let mut project = ProjectLoader::new().open(directory, &project_file).unwrap();
    let sheet = &project.schematics()[0];
    let (symbol, text) = sheet
        .symbols()
        .values()
        .find_map(|symbol| {
            symbol
                .texts()
                .values()
                .find(|text| text.layer() == Layer::SYMBOL_NAMES)
                .map(|text| (symbol.id(), text.clone()))
        })
        .unwrap();
    let reference = SymbolRef {
        schematic: sheet.id(),
        symbol,
    };
    let p = |x: i64, y: i64| Point::from_nm(x * 1_000_000, y * 1_000_000);
    let outline = Polygon::new(
        Uuid::new_random(),
        Layer::SCHEMATIC_DOCUMENTATION,
        Length::new(200_000).try_into().unwrap(),
        false,
        false,
        Path::new(
            [(400, 400), (450, 400), (450, 450), (400, 450), (400, 400)]
                .map(|(x, y)| p(x, y).into())
                .to_vec(),
        ),
    );
    project
        .apply(Mutation::Schematic(SchematicMutation::AddPolygon {
            schematic: reference.schematic,
            polygon: outline,
        }))
        .unwrap();
    project
        .apply(Mutation::Schematic(SchematicMutation::AddText {
            schematic: reference.schematic,
            text: foilfern_core::geometry::Text::new(
                Uuid::new_random(),
                Layer::SCHEMATIC_DOCUMENTATION,
                "Exported obstacle",
                p(425, 425),
                text.rotation(),
                text.height(),
                text.align(),
                false,
            ),
        }))
        .unwrap();
    let sheet = project.schematic(reference.schematic).unwrap();
    let geometry = schematic_field_geometry(
        &project,
        sheet,
        reference,
        text.uuid(),
        &GraphicsSettings::default(),
    )
    .unwrap()
    .unwrap();
    assert!(!geometry.fits(text.position(), p(425, 425), p(401, 401), p(449, 449), 1.));
    assert!(geometry.fits(text.position(), p(420, 415), p(401, 401), p(449, 449), 1.));
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
    let before = contents(fs.export_to_zip(None).unwrap());
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
    // Archive creation times can differ; compare every uncompressed file.
    let after = contents(fs.export_to_zip(None).unwrap());
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>()
    );
    for (name, bytes) in before {
        assert!(bytes == after[&name], "Project file changed: {name}");
    }
}
