//! Shared physical snapshot extraction over upstream project fixtures.
use foilfern_core::{
    fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem, ZipArchive},
    project::{AssemblyVariantId, ProjectLoader, board::BoardPhysicalGeometry},
};
use std::{collections::BTreeMap, path::Path, sync::Arc};
fn contents(bytes: Vec<u8>) -> BTreeMap<String, Vec<u8>> {
    let mut zip = ZipArchive::from_bytes(bytes).unwrap();
    (0..zip.len())
        .map(|i| (zip.file_name(i).unwrap(), zip.read_file(i).unwrap()))
        .collect()
}

#[test]
fn physical_snapshots_are_owned_read_only_and_resolve_assembly_models() {
    let root = Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data/projects");
    let mut count = 0;
    let mut models = 0;
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if !path.join(".librepcb-project").is_file() {
            continue;
        }
        let fs = Arc::new(TransactionalFileSystem::open_ro(&FilePath::new(path).unwrap()).unwrap());
        let filename = fs
            .files("")
            .into_iter()
            .find(|f| f.ends_with(".lpp"))
            .unwrap();
        let project = ProjectLoader::new()
            .open(TransactionalDirectory::new(fs.clone(), ""), &filename)
            .unwrap();
        let before = contents(fs.export_to_zip(None).unwrap());
        let revision = project.revision();
        for board in project.boards() {
            let data = BoardPhysicalGeometry::from_project(&project, board, None).unwrap();
            assert_eq!(data.board, board.id());
            assert_eq!(data.thickness, board.settings().pcb_thickness);
            assert!(data.devices.is_empty());
            assert_eq!(data.project_revision, revision);
            if board.polygons().values().any(|polygon| {
                polygon.layer() == foilfern_core::types::Layer::BOARD_OUTLINES
                    && polygon.path().is_closed()
            }) {
                assert!(!data.outlines.is_empty(), "{filename} / {}", board.name());
            }
            for variant in project.circuit().assembly_variants().iter() {
                let data = BoardPhysicalGeometry::from_project(
                    &project,
                    board,
                    Some(AssemblyVariantId(variant.uuid())),
                )
                .unwrap();
                for device in &data.devices {
                    let actual = board.device(device.component).unwrap();
                    assert_eq!(Some(device.model), actual.lib_model());
                    assert!(
                        !actual
                            .parts(
                                project
                                    .circuit()
                                    .component_instance(actual.component())
                                    .unwrap(),
                                Some(AssemblyVariantId(variant.uuid()))
                            )
                            .is_empty()
                    );
                }
                models += data.devices.len();
            }
            count += 1;
        }
        assert_eq!(project.revision(), revision);
        let after = contents(fs.export_to_zip(None).unwrap());
        assert_eq!(
            before.keys().collect::<Vec<_>>(),
            after.keys().collect::<Vec<_>>()
        );
        for (name, bytes) in before {
            assert!(bytes == after[&name], "Project file changed: {name}");
        }
    }
    assert!(count >= 6);
    assert!(
        models > 0,
        "Fixture set should exercise 3D model references"
    );
}
