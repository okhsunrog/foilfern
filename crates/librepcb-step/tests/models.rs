//! Upstream OccModelTest inputs through the guarded application API.
use librepcb_step::StepModel;
use std::path::Path;

#[test]
fn board_only_export_is_read_only() {
    use librepcb_core::{
        fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem, ZipArchive},
        project::ProjectLoader,
    };
    use std::{collections::BTreeMap, sync::Arc};
    fn contents(bytes: Vec<u8>) -> BTreeMap<String, Vec<u8>> {
        let mut archive = ZipArchive::from_bytes(bytes).unwrap();
        (0..archive.len())
            .map(|i| (archive.file_name(i).unwrap(), archive.read_file(i).unwrap()))
            .collect()
    }
    let root = Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/projects/Project With Two Boards");
    let fs = Arc::new(TransactionalFileSystem::open_ro(&FilePath::new(root).unwrap()).unwrap());
    let name = fs
        .files("")
        .into_iter()
        .find(|name| name.ends_with(".lpp"))
        .unwrap();
    let project = ProjectLoader::new()
        .open(TransactionalDirectory::new(fs.clone(), ""), &name)
        .unwrap();
    let before = contents(fs.export_to_zip(None).unwrap());
    let revision = project.revision();
    let model = StepModel::from_board(&project, &project.boards()[0], None).unwrap();
    assert!(model.solid_count() > 0);
    assert_eq!(
        StepModel::from_step(&model.to_step().unwrap())
            .unwrap()
            .solid_count(),
        model.solid_count()
    );
    assert_eq!(project.revision(), revision);
    assert_eq!(contents(fs.export_to_zip(None).unwrap()), before);
}

#[test]
fn upstream_models_export_and_mesh_without_a_display() {
    let root = Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/unittests/librepcbcommon/OccModelTest");
    for (name, solids) in [("colors.step", 5), ("model.step", 1)] {
        let model = StepModel::from_step(&std::fs::read(root.join(name)).unwrap()).unwrap();
        assert_eq!(model.solid_count(), solids);
        let bytes = model.to_step().unwrap();
        let again = StepModel::from_step(&bytes).unwrap();
        assert_eq!(again.solid_count(), solids);
        let (obj, mtl) = again.to_obj(0.01).unwrap();
        let (meshes, materials) = tobj::load_obj_buf(
            &mut std::io::Cursor::new(obj),
            &tobj::LoadOptions::default(),
            |_| tobj::load_mtl_buf(&mut std::io::Cursor::new(mtl.as_bytes())),
        )
        .unwrap();
        assert!(meshes.iter().any(|mesh| !mesh.mesh.indices.is_empty()));
        assert!(materials.unwrap().len() >= 2);
        assert!(again.to_obj(f64::NAN).is_err());
    }
}
