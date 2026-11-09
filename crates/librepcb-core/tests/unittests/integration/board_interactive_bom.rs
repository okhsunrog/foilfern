//! Port of BoardInteractiveHtmlBomGeneratorTest, plus read-only assembly cases.
use librepcb_core::{
    export::Timestamp,
    fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem, ZipArchive},
    project::{AssemblyVariantId, Project, ProjectLoader, board::BoardInteractiveHtmlBomGenerator},
    types::Uuid,
};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path, sync::Arc};

fn load(name: &str) -> Project {
    let path = Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/projects")
        .join(name);
    let fs = Arc::new(TransactionalFileSystem::open_ro(&FilePath::new(path).unwrap()).unwrap());
    let file = fs
        .files("")
        .into_iter()
        .find(|name| name.ends_with(".lpp"))
        .unwrap();
    ProjectLoader::new()
        .open(TransactionalDirectory::new(fs, ""), &file)
        .unwrap()
}
fn timestamp() -> Timestamp {
    Timestamp::Utc(chrono::DateTime::from_timestamp(9, 0).unwrap().naive_utc())
}
fn decode(html: &str) -> Value {
    let encoded = html
        .split("var pcbdata = JSON.parse(LZString.decompressFromBase64(\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let decoded = String::from_utf16(&lz_str::decompress_from_base64(encoded).unwrap()).unwrap();
    serde_json::from_str(&decoded).unwrap()
}
fn same(left: &Value, right: &Value, path: &str) {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) if left.is_f64() && right.is_f64() => {
            // Qt's golden JSON pretty-printer and serde_json can differ by
            // one f64 ULP. The generator's geometry is explicitly f32.
            assert_eq!(
                (left.as_f64().unwrap() as f32).to_bits(),
                (right.as_f64().unwrap() as f32).to_bits(),
                "{path}"
            );
        }
        (Value::Object(left), Value::Object(right)) => {
            assert_eq!(
                left.keys().collect::<Vec<_>>(),
                right.keys().collect::<Vec<_>>(),
                "{path}"
            );
            for (key, value) in left {
                same(value, &right[key], &format!("{path}/{key}"));
            }
        }
        (Value::Array(left), Value::Array(right)) => {
            assert_eq!(left.len(), right.len(), "{path}");
            for (i, (a, b)) in left.iter().zip(right).enumerate() {
                same(a, b, &format!("{path}/{i}"));
            }
        }
        _ => assert_eq!(left, right, "{path}"),
    }
}
#[test]
fn upstream_golden_interactive_bom() {
    let project = load("Gerber Test");
    let board = &project.boards()[0];
    let variant = AssemblyVariantId(
        project
            .circuit()
            .assembly_variants()
            .iter()
            .next()
            .unwrap()
            .uuid(),
    );
    let html = BoardInteractiveHtmlBomGenerator::new(&project, board.id(), variant)
        .unwrap()
        .generate(timestamp())
        .unwrap()
        .generate_html()
        .unwrap();
    let golden = Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/unittests/librepcbproject/BoardInteractiveHtmlBomGeneratorTest");
    let expected: Value =
        serde_json::from_slice(&std::fs::read(golden.join("expected.json")).unwrap()).unwrap();
    same(&decode(&html), &expected, "pcbdata");
    assert!(
        html == std::fs::read_to_string(golden.join("expected.html")).unwrap(),
        "HTML differs from the upstream golden file"
    );
}
fn contents(project: &Project) -> BTreeMap<String, Vec<u8>> {
    let mut zip = ZipArchive::from_bytes(
        project
            .directory()
            .file_system()
            .export_to_zip(None)
            .unwrap(),
    )
    .unwrap();
    (0..zip.len())
        .map(|i| (zip.file_name(i).unwrap(), zip.read_file(i).unwrap()))
        .collect()
}
#[test]
fn assembly_variants_custom_columns_and_extraction_are_read_only() {
    let mut boards = 0;
    for entry in
        std::fs::read_dir(Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data/projects"))
            .unwrap()
    {
        let path = entry.unwrap().path();
        if !path.join(".librepcb-project").is_file() {
            continue;
        }
        let project = load(path.file_name().unwrap().to_str().unwrap());
        let revision = project.revision();
        let before = contents(&project);
        for board in project.boards() {
            for variant in project.circuit().assembly_variants().iter() {
                let variant = AssemblyVariantId(variant.uuid());
                let mut generator =
                    BoardInteractiveHtmlBomGenerator::new(&project, board.id(), variant).unwrap();
                generator.set_custom_attributes(vec!["MPN".into(), "MANUFACTURER".into()]);
                generator.set_component_order(vec!["R".into(), "C".into(), "D".into()]);
                let data = generator.generate(timestamp()).unwrap();
                let html = data.generate_html().unwrap();
                assert!(!html.contains("<script src="));
                let decoded = decode(&html);
                assert_eq!(
                    super::interactive_bom_jobs::settings(&html)["fields"],
                    serde_json::json!(["Value / MPN", "Package", "MPN", "MANUFACTURER"])
                );
                assert_eq!(
                    decoded["footprints"].as_array().unwrap().len(),
                    board.devices().len()
                );
                let expected_dnp = board
                    .devices()
                    .values()
                    .filter(|d| {
                        d.parts(
                            project.circuit().component_instance(d.component()).unwrap(),
                            Some(variant),
                        )
                        .is_empty()
                    })
                    .count();
                assert_eq!(
                    decoded["bom"]["skipped"].as_array().unwrap().len(),
                    expected_dnp
                );
                let refs = decoded["bom"]["both"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| row.as_array().unwrap().len())
                    .sum::<usize>();
                assert_eq!(refs + expected_dnp, board.devices().len());
            }
            assert!(generator_with_invalid_variant(&project, board));
            boards += 1;
        }
        assert_eq!(project.revision(), revision);
        let after = contents(&project);
        assert_eq!(
            before.keys().collect::<Vec<_>>(),
            after.keys().collect::<Vec<_>>()
        );
        for (name, bytes) in before {
            assert!(bytes == after[&name], "Project file changed: {name}");
        }
    }
    assert!(boards >= 6);
}
fn generator_with_invalid_variant(
    project: &Project,
    board: &librepcb_core::project::board::Board,
) -> bool {
    BoardInteractiveHtmlBomGenerator::new(
        project,
        board.id(),
        AssemblyVariantId(Uuid::new_random()),
    )
    .is_err()
}
