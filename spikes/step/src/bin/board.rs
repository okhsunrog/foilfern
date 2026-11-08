//! Build a mechanical 3D board prototype from the shared physical snapshot.
use anyhow::{Context, Result, ensure};
use librepcb_core::{
    fileio::{FilePath, FileSystem, TransactionalDirectory, TransactionalFileSystem},
    project::{AssemblyVariantId, ProjectLoader, board::BoardPhysicalGeometry},
};
use monstertruck_io::step::save;
use monstertruck_meshing::prelude::*;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

fn checked_mesh(mesh: &PolygonMesh) -> Result<Value> {
    let stats = mesh_stats(mesh.clone())?;
    ensure!(
        ["boundary_edges", "nonmanifold_edges", "misoriented_edges"]
            .iter()
            .all(|key| stats[key] == 0),
        "invalid solid mesh: {stats}"
    );
    Ok(stats)
}
use step_spike::{
    board::{board_solids, device_matrix},
    materials::{Material, export_obj, export_step},
    mesh_stats,
    metadata::StepDocument,
    placed_solids, tessellate_solid, transform_solid,
};

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let input = PathBuf::from(
        args.next()
            .context("usage: board PROJECT.lpp[z] OUTPUT_DIR [BOARD_INDEX] [VARIANT_INDEX]")?,
    );
    let output = PathBuf::from(args.next().context("missing output directory")?);
    let board_index = args
        .next()
        .map(|v| v.to_string_lossy().parse::<usize>())
        .transpose()?
        .unwrap_or(0);
    let variant_index = args
        .next()
        .map(|v| v.to_string_lossy().parse::<usize>())
        .transpose()?
        .unwrap_or(0);
    let fs = if input.extension().is_some_and(|v| v == "lppz") {
        let fs = TransactionalFileSystem::open_ro(
            &FilePath::new(output.join("virtual-project")).context("invalid output path")?,
        )?;
        fs.load_from_zip(&FilePath::new(&input).context("invalid input path")?)?;
        fs
    } else {
        TransactionalFileSystem::open_ro(
            &FilePath::new(input.parent().context("missing project parent")?)
                .context("invalid project directory")?,
        )?
    };
    let fs = Arc::new(fs);
    let filename = if input.extension().is_some_and(|v| v == "lppz") {
        fs.files("")
            .into_iter()
            .find(|name| name.ends_with(".lpp"))
            .context("archive has no project")?
    } else {
        input
            .file_name()
            .context("missing filename")?
            .to_string_lossy()
            .into_owned()
    };
    let project = ProjectLoader::new().open(TransactionalDirectory::new(fs, ""), &filename)?;
    let board = project
        .boards()
        .get(board_index)
        .context("invalid board index")?;
    let variant = project
        .circuit()
        .assembly_variants()
        .iter()
        .nth(variant_index)
        .context("invalid assembly variant index")?;
    let data = BoardPhysicalGeometry::from_project(
        &project,
        board,
        Some(AssemblyVariantId(variant.uuid())),
    )?;
    let bodies = board_solids(&data)?;
    std::fs::create_dir_all(&output)?;
    let pcb_solids: Vec<_> = bodies.iter().map(|solid| solid.compress()).collect();
    let pcb_models: save::StepModels<_, _, _> = pcb_solids.iter().collect();
    let pcb_text =
        save::CompleteStepDisplay::new(pcb_models, save::StepHeaderDescriptor::default())
            .to_string();
    std::fs::write(output.join("pcb.step"), &pcb_text)?;
    // The modeling and STEP-import geometry enums differ. Reload the generated
    // BRep to share one save-side type, without a lossy mesh conversion.
    let pcb_document = StepDocument::parse(pcb_text.as_bytes())?;
    pcb_document.table.entity_report.require_empty()?;
    let mut pcb_solids = placed_solids(&pcb_document)?;
    let pcb_material = if let Some(resist) = data.solder_resist {
        let color = resist.to_solder_resist_color();
        Material {
            color: [color.r, color.g, color.b].map(|value| f64::from(value) / 510.),
            opacity: f64::from(color.a) / 255.,
        }
    } else {
        Material {
            color: [70., 80., 70.].map(|value| value / 255.),
            opacity: 1.,
        }
    };
    for item in &mut pcb_solids {
        for shell in &mut item.appearance {
            shell.fill(Some(pcb_material));
        }
    }
    std::fs::write(output.join("pcb.step"), export_step(&pcb_solids)?)?;
    let mut combined = PolygonMesh::default();
    let mut material_groups = Vec::new();
    let mut meshes = Vec::new();
    for solid in &pcb_solids {
        let (mesh, groups) = tessellate_solid(solid, 0.01)?;
        material_groups.extend(groups);
        let stats = checked_mesh(&mesh)?;
        meshes.push(json!({"name":"PCB","mesh":stats,"appearance":solid.appearance}));
        combined.merge(mesh);
    }
    let mut solids = Vec::new();
    let mut cache = BTreeMap::new();
    for device in &data.devices {
        let key = (device.package, device.model);
        if let std::collections::btree_map::Entry::Vacant(entry) = cache.entry(key) {
            let path = format!("library/pkg/{}/{}.step", device.package, device.model);
            let bytes = project
                .directory()
                .read(&path)
                .with_context(|| format!("model for {}: {path}", device.name))?;
            let document = StepDocument::parse(&bytes)?;
            document.table.entity_report.require_empty()?;
            entry.insert(placed_solids(&document)?);
        }
        let matrix = device_matrix(device, data.thickness.to_mm());
        for item in &cache[&key] {
            let mut solid = item.clone();
            transform_solid(&mut solid.solid, matrix);
            let (mesh, groups) = tessellate_solid(&solid, 0.01)?;
            material_groups.extend(groups);
            let stats = checked_mesh(&mesh)?;
            meshes.push(json!({"name":device.name,"mesh":stats,"appearance":solid.appearance}));
            combined.merge(mesh);
            solids.push(solid);
        }
    }
    let (obj, mtl) = export_obj(&combined, &material_groups)?;
    std::fs::write(output.join("assembly.obj"), obj)?;
    std::fs::write(output.join("assembly.mtl"), mtl)?;
    let component_solids = solids.len();
    solids.extend(pcb_solids.clone());
    std::fs::write(output.join("assembly.step"), export_step(&solids)?)?;
    let report = json!({"input":input,"board":board.name().to_string(),"variant":variant.name().to_string(),"devices":data.devices.len(),"pcb_solids":pcb_solids.len(),"component_solids":component_solids,"meshes":meshes,"contour_tolerance_mm":0.005,"tessellation_tolerance_mm":0.01,"surface_materials_preserved":true,"via_drills_omitted":true,"model_units_assumed_mm":false});
    std::fs::write(
        output.join("board.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    println!(
        "{} devices, {} component solids, {} PCB solids -> {}",
        data.devices.len(),
        component_solids,
        pcb_solids.len(),
        output.display()
    );
    Ok(())
}
